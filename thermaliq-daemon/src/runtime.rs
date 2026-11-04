//! Daemon wiring (SPEC §5): the engine thread, the shared snapshot cache the API reads,
//! and the history thread. Only `thermaliq-engine` writes to fans.

use crate::history::{recorded_sensor_ids, HistoryDb};
use std::collections::HashMap;
use std::fs;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thermaliq_common::{bundled_profiles, FanReading, HardwareSnapshot, Profile, SensorReading};
use thermaliq_engine::{Engine, Source};
use thermaliq_hal::HardwareLayer;
use tokio::sync::{oneshot, watch};

/// Ticks queued for the history thread before new ones are dropped (the engine never waits).
const HISTORY_QUEUE: usize = 64;
/// SPEC §4: a manual override is time-limited.
const MAX_OVERRIDE: Duration = Duration::from_secs(300);
/// How long a caller waits for the engine thread to answer a command.
const COMMAND_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The engine thread is gone or did not answer; nothing was done that we know of.
    Unavailable,
    /// The target (fan, override) does not exist.
    NotFound(String),
    /// The engine refused the request.
    Rejected(String),
}

/// `Ok(Some(reason))`: done, but a safety rule (S2–S4) currently overrides it. Only overrides use it.
type Done = Result<Option<&'static str>, CommandError>;
type Reply = oneshot::Sender<Done>;

enum Cmd {
    SetProfile(Profile, Reply),
    Override(String, u8, Duration, Reply),
    Cancel(String, Reply),
}

enum Msg {
    Stop,
    Cmd(Cmd),
}

/// Duty clamped to 0–100, seconds capped at 300. `None`: 0 seconds is not an override.
pub fn clamp_override(duty: i64, seconds: u64) -> Option<(u8, Duration)> {
    (seconds > 0).then(|| {
        let duty = u8::try_from(duty.clamp(0, 100)).unwrap_or(100);
        (duty, Duration::from_secs(seconds).min(MAX_OVERRIDE))
    })
}

fn apply(engine: &mut Engine, began: Instant, cache: &RwLock<Cache>, cmd: Cmd) {
    let (result, reply) = match cmd {
        Cmd::SetProfile(p, reply) => {
            engine.set_profile(&p);
            cache
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .active_profile = p.name;
            (Ok(None), reply)
        }
        Cmd::Override(fan, duty, dur, reply) => {
            let r = if engine.can_control(&fan) {
                engine
                    .submit(&fan, Source::Override, duty, Some(began.elapsed() + dur))
                    .map_err(CommandError::Rejected)
                    .map(|()| engine.override_blocked_by(&fan))
            } else {
                Err(CommandError::Rejected(format!(
                    "fan '{fan}' is not controlled by the active profile, or is read-only"
                )))
            };
            (r, reply)
        }
        Cmd::Cancel(fan, reply) => {
            let r = match engine.cancel_override(&fan) {
                Ok(true) => Ok(None),
                Ok(false) => Err(CommandError::NotFound(format!(
                    "fan '{fan}' has no active override"
                ))),
                Err(e) => Err(CommandError::NotFound(e)),
            };
            (r, reply)
        }
    };
    let _ = reply.send(result);
}

/// A cheap, cloneable way for the API to read the cache and talk to the engine thread.
#[derive(Clone)]
pub struct Handle {
    cache: Arc<RwLock<Cache>>,
    tx: mpsc::Sender<Msg>,
    ticks: watch::Receiver<u64>,
}

impl Handle {
    /// Changes after every tick's cache update; closes when the engine thread ends.
    pub fn ticks(&self) -> watch::Receiver<u64> {
        self.ticks.clone()
    }

    pub fn cache(&self) -> Cache {
        self.cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn ask(&self, make: impl FnOnce(Reply) -> Cmd) -> Done {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(Msg::Cmd(make(reply)))
            .map_err(|_| CommandError::Unavailable)?;
        match tokio::time::timeout(COMMAND_WAIT, answer).await {
            Ok(Ok(result)) => result,
            _ => Err(CommandError::Unavailable),
        }
    }

    pub async fn set_profile(&self, profile: Profile) -> Result<(), CommandError> {
        self.ask(|r| Cmd::SetProfile(profile, r)).await.map(drop)
    }

    /// `duty` is clamped to 0–100 and `seconds` capped at 300; 0 seconds is rejected.
    /// `Ok(Some(reason))`: stored, but safety holds the fan right now ("latched", "degraded", "sensor_lost").
    pub async fn set_override(&self, fan: &str, duty: i64, seconds: u64) -> Done {
        let (duty, dur) = clamp_override(duty, seconds)
            .ok_or_else(|| CommandError::Rejected("an override needs seconds > 0".into()))?;
        self.ask(|r| Cmd::Override(fan.into(), duty, dur, r)).await
    }

    pub async fn cancel_override(&self, fan: &str) -> Result<(), CommandError> {
        self.ask(|r| Cmd::Cancel(fan.into(), r)).await.map(drop)
    }
}

pub struct Options {
    pub tick: Duration,
    /// SPEC §6: hourly.
    pub prune_every: Duration,
}

/// What `/status` reports (SPEC S2, S4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// False once the engine thread has stopped or panicked; everything else is then stale.
    pub engine_running: bool,
    pub latched: bool,
    pub degraded: bool,
    /// CPU/GPU sensors the latch is waiting on to cool down or come back.
    pub waiting_on: Vec<String>,
    /// Optional sensor sources (e.g. "hwinfo") that failed on the last tick and are missing
    /// from the snapshot. Not a safety state: required hardware is fine.
    pub unavailable_sources: Vec<String>,
}

/// The one snapshot per tick, shared. API reads use this, never the hardware (SPEC §5).
#[derive(Debug, Clone, Default)]
pub struct Cache {
    pub snapshot: HardwareSnapshot,
    pub status: Status,
    /// Ticks run so far; 0 means nothing has been read yet.
    pub ticks: u64,
    /// Ticks since `snapshot` was read from the hardware; 0 = fresh (S4 starts at 3).
    pub snapshot_age: u64,
    pub active_profile: String,
}

type HistoryTick = (i64, Vec<SensorReading>, Vec<FanReading>);

pub struct Daemon {
    cache: Arc<RwLock<Cache>>,
    tx: mpsc::Sender<Msg>,
    ticks: watch::Receiver<u64>,
    engine: Option<JoinHandle<()>>,
    history: Option<JoinHandle<()>>,
}

impl Daemon {
    /// Runs `active` on its own thread. `saved` is every saved profile, so history also
    /// records the sensors profiles other than the active one reference (SPEC §6).
    pub fn start(
        hal: Arc<dyn HardwareLayer>,
        active: &Profile,
        mut saved: Vec<Profile>,
        history: Arc<Mutex<HistoryDb>>,
        opts: Options,
    ) -> Self {
        saved.push(active.clone());
        let cache = Arc::new(RwLock::new(Cache {
            status: Status {
                engine_running: true,
                ..Status::default()
            },
            active_profile: active.name.clone(),
            ..Cache::default()
        }));
        let (tx, commands) = mpsc::channel::<Msg>();
        let (tick_tx, ticks) = watch::channel(0u64);
        let (history_tx, rx) = mpsc::sync_channel::<HistoryTick>(HISTORY_QUEUE);
        let history = std::thread::spawn(move || history_loop(&rx, &history, opts.prune_every));
        let engine = {
            let (cache, active, tick) = (cache.clone(), active.clone(), opts.tick);
            std::thread::spawn(move || {
                let run = catch_unwind(AssertUnwindSafe(|| {
                    // The engine is dropped when this closure ends or unwinds, which hands the fans back (S5).
                    let sources = hal.clone();
                    let mut engine = Engine::new(hal, &active);
                    let began = Instant::now();
                    let (mut ticks, mut snapshot_age) = (0u64, 0u64);
                    'run: loop {
                        let t0 = Instant::now();
                        engine.tick(t0 - began);
                        ticks += 1;
                        let snapshot = engine.last_snapshot().cloned().unwrap_or_default();
                        snapshot_age = if engine.snapshot_fresh() {
                            0
                        } else {
                            snapshot_age + 1
                        };
                        if engine.snapshot_fresh() {
                            let sensors = recorded_sensor_ids(&saved, &snapshot);
                            let sensors = snapshot
                                .sensors
                                .iter()
                                .filter(|s| sensors.contains(&s.id))
                                .cloned()
                                .collect();
                            // A full queue drops this tick's history rather than stall the engine.
                            let _ =
                                history_tx.try_send((unix_now(), sensors, snapshot.fans.clone()));
                        }
                        let status = Status {
                            engine_running: true,
                            latched: engine.latched(),
                            degraded: engine.degraded(),
                            waiting_on: engine.latch_waiting_on().to_vec(),
                            unavailable_sources: sources.unavailable_sources(),
                        };
                        {
                            let mut c = cache.write().unwrap_or_else(PoisonError::into_inner);
                            (c.snapshot, c.status) = (snapshot, status);
                            (c.ticks, c.snapshot_age) = (ticks, snapshot_age);
                        }
                        tick_tx.send_replace(ticks);
                        // Commands run between ticks; past the deadline the next tick comes first.
                        let deadline = t0 + tick;
                        loop {
                            match commands
                                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            {
                                Ok(Msg::Cmd(cmd)) => {
                                    apply(&mut engine, began, &cache, cmd);
                                    if Instant::now() >= deadline {
                                        break;
                                    }
                                }
                                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break 'run,
                                Err(RecvTimeoutError::Timeout) => break,
                            }
                        }
                    }
                }));
                // The engine is gone and the fans are handed back; don't keep claiming it runs.
                cache
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .status
                    .engine_running = false;
                if let Err(panic) = run {
                    resume_unwind(panic); // so shutdown() reports it
                }
            })
        };
        Self {
            cache,
            tx,
            ticks,
            engine: Some(engine),
            history: Some(history),
        }
    }

    pub fn cache(&self) -> Cache {
        self.handle().cache()
    }

    pub fn handle(&self) -> Handle {
        Handle {
            cache: self.cache.clone(),
            tx: self.tx.clone(),
            ticks: self.ticks.clone(),
        }
    }

    /// Stops the engine (dropping it hands every touched fan back, S5), then the history
    /// thread. Errors if either thread panicked.
    pub fn shutdown(mut self) -> Result<(), String> {
        self.join()
    }

    fn join(&mut self) -> Result<(), String> {
        let _ = self.tx.send(Msg::Stop);
        let engine = self.engine.take().map(JoinHandle::join);
        let history = self.history.take().map(JoinHandle::join);
        match (engine, history) {
            (Some(Err(_)), _) => Err("engine thread panicked".into()),
            (_, Some(Err(_))) => Err("history thread panicked".into()),
            _ => Ok(()),
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

/// Records each tick, and prunes every `prune_every`, so a slow prune only delays history.
/// shortcut: DB errors are not logged yet; add logging with the daemon's log setup.
fn history_loop(rx: &mpsc::Receiver<HistoryTick>, db: &Mutex<HistoryDb>, prune_every: Duration) {
    let mut last_prune = Instant::now();
    loop {
        match rx.recv_timeout(prune_every.saturating_sub(last_prune.elapsed())) {
            Ok((now, sensors, fans)) => {
                let _ = db
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .record_tick(now, &sensors, &fans);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if last_prune.elapsed() >= prune_every {
            let _ = db
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .prune(unix_now());
            last_prune = Instant::now();
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Bundled profiles plus every `*.toml` in `<dir>/profiles` (a saved profile replaces a
/// bundled one of the same name). Returns the files it could not use, with the reason.
pub fn load_profiles(dir: &Path) -> (Vec<Profile>, Vec<String>) {
    let mut skipped = Vec::new();
    let mut profiles: Vec<Profile> = bundled_profiles()
        .iter()
        .filter_map(|(name, s)| {
            Profile::from_toml(s)
                .map_err(|e| skipped.push(format!("bundled {name}: {e}")))
                .ok()
        })
        .collect();
    let mut files: Vec<_> = match fs::read_dir(dir.join("profiles")) {
        Ok(rd) => rd.filter_map(Result::ok).map(|e| e.path()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            skipped.push(format!("profiles dir: {e}"));
            Vec::new()
        }
    };
    files.sort();
    let mut saved_from: HashMap<String, String> = HashMap::new();
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
    {
        let shown = path.file_name().unwrap_or_default().to_string_lossy();
        match fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|s| Profile::from_toml(&s).map_err(|e| e.to_string()))
        {
            Ok(p) => {
                if let Some(earlier) = saved_from.insert(p.name.clone(), shown.to_string()) {
                    skipped.push(format!(
                        "{earlier}: overridden by {shown} (both are named '{}')",
                        p.name
                    ));
                }
                profiles.retain(|b| b.name != p.name);
                profiles.push(p);
            }
            Err(e) => skipped.push(format!("{shown}: {e}")),
        }
    }
    (profiles, skipped)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use thermaliq_common::{HardwareSnapshot, Profile, SensorKind};
    use thermaliq_hal::{Capabilities, HalError, HardwareLayer, MockHardwareLayer, Tick};

    const FLAT: &str = "name = \"flat\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n";
    const ON_X: &str = "name = \"on_x\"\n[[curves]]\ntype = \"linear\"\nid = \"c\"\nsource = \"x\"\nmin_temp = 30.0\nmax_temp = 90.0\nmin_pwm = 20\nmax_pwm = 100\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n";

    fn fast() -> Options {
        Options {
            tick: Duration::from_millis(5),
            prune_every: Duration::from_secs(3600),
        }
    }

    fn at(cpu: f32) -> Tick {
        Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, cpu)])
    }

    fn db() -> Arc<Mutex<HistoryDb>> {
        Arc::new(Mutex::new(HistoryDb::in_memory().unwrap()))
    }

    fn start(
        script: Vec<Tick>,
        saved: Vec<Profile>,
        db: &Arc<Mutex<HistoryDb>>,
        opts: Options,
    ) -> (Arc<MockHardwareLayer>, Daemon) {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], script));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal.clone(), &active, saved, db.clone(), opts);
        (hal, d)
    }

    fn wait_until(mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn engine_runs_on_its_own_thread_and_shutdown_hands_every_touched_fan_back() {
        let (hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        wait_until(|| !hal.writes().is_empty());
        assert!(
            hal.restores().is_empty(),
            "nothing handed back while running"
        );
        d.shutdown().unwrap();
        assert_eq!(hal.restores(), ["f1"]);
    }

    #[test]
    fn dropping_the_daemon_also_hands_fans_back() {
        let (hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        wait_until(|| !hal.writes().is_empty());
        drop(d);
        assert_eq!(hal.restores(), ["f1"]);
    }

    #[test]
    fn engine_panic_still_hands_back_and_shutdown_reports_it() {
        struct PanicsOnThird(Arc<MockHardwareLayer>, AtomicU8);
        impl HardwareLayer for PanicsOnThird {
            fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
                assert!(self.1.fetch_add(1, Ordering::SeqCst) < 2, "boom");
                self.0.snapshot()
            }
            fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
                self.0.set_fan_duty(fan, duty)
            }
            fn restore_auto(&self, fan: &str) -> Result<(), HalError> {
                self.0.restore_auto(fan)
            }
            fn capabilities(&self) -> Capabilities {
                self.0.capabilities()
            }
        }
        let mock = Arc::new(MockHardwareLayer::new(&["f1"], vec![at(40.0)]));
        let hal = Arc::new(PanicsOnThird(mock.clone(), AtomicU8::new(0)));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal, &active, vec![], db(), fast());
        wait_until(|| !mock.restores().is_empty());
        assert_eq!(mock.restores(), ["f1"]);
        wait_until(|| !d.cache().status.engine_running);
        let c = d.cache();
        assert!(
            !c.status.latched && !c.status.degraded,
            "not a health claim"
        );
        assert!(d.shutdown().is_err());
    }

    #[test]
    fn status_names_the_optional_sources_that_are_down() {
        struct Down(Arc<MockHardwareLayer>);
        impl HardwareLayer for Down {
            fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
                self.0.snapshot()
            }
            fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
                self.0.set_fan_duty(fan, duty)
            }
            fn restore_auto(&self, fan: &str) -> Result<(), HalError> {
                self.0.restore_auto(fan)
            }
            fn capabilities(&self) -> Capabilities {
                self.0.capabilities()
            }
            fn unavailable_sources(&self) -> Vec<String> {
                vec!["hwinfo".into()]
            }
        }
        let mock = Arc::new(MockHardwareLayer::new(&["f1"], vec![at(40.0)]));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(Arc::new(Down(mock)), &active, vec![], db(), fast());
        wait_until(|| d.cache().ticks > 0);
        let c = d.cache();
        assert_eq!(c.status.unavailable_sources, ["hwinfo"]);
        assert!(
            !c.status.degraded,
            "an optional source is not a safety state"
        );
    }

    #[test]
    fn reads_come_from_the_cache() {
        let (_hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        wait_until(|| d.cache().ticks > 0);
        let c = d.cache();
        assert_eq!(c.snapshot.sensors[0].id, "cpu");
        assert_eq!(c.snapshot.fans.len(), 2);
        let running = Status {
            engine_running: true,
            ..Status::default()
        };
        assert_eq!(c.status, running);
        assert_eq!(c.snapshot_age, 0);
        d.shutdown().unwrap();
    }

    #[test]
    fn engine_running_is_true_from_start_and_false_after_a_clean_stop() {
        let (_hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        assert!(d.cache().status.engine_running);
        let cache = d.cache.clone();
        d.shutdown().unwrap();
        assert!(!cache.read().unwrap().status.engine_running);
    }

    #[test]
    fn a_failing_snapshot_ages_the_cache_but_keeps_the_old_values() {
        let (_hal, d) = start(vec![at(40.0), Tick::Fail], vec![], &db(), fast());
        wait_until(|| d.cache().snapshot_age >= 2);
        let c = d.cache();
        assert_eq!(c.snapshot.sensors[0].temp_celsius, 40.0);
        let age = c.snapshot_age;
        wait_until(|| d.cache().snapshot_age > age);
        d.shutdown().unwrap();
    }

    #[test]
    fn status_reports_latched_and_the_sensors_it_waits_on() {
        let gone = Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, 40.0)]);
        let (_hal, d) = start(vec![at(97.0), gone], vec![], &db(), fast());
        wait_until(|| d.cache().status.waiting_on == ["cpu"]);
        let s = d.cache().status;
        assert!(s.latched && !s.degraded);
        d.shutdown().unwrap();
    }

    #[test]
    fn status_reports_degraded_when_the_hal_fails() {
        let (_hal, d) = start(vec![at(40.0), Tick::Fail], vec![], &db(), fast());
        wait_until(|| d.cache().status.degraded);
        assert!(!d.cache().status.latched);
        d.shutdown().unwrap();
    }

    #[test]
    fn history_records_fans_cpu_gpu_and_sensors_any_saved_profile_references() {
        let script = vec![Tick::Temps(vec![
            ("cpu".into(), SensorKind::Cpu, 40.0),
            ("x".into(), SensorKind::Other, 33.0),
            ("y".into(), SensorKind::Other, 34.0),
        ])];
        let db = db();
        let saved = vec![Profile::from_toml(ON_X).unwrap()];
        let (_hal, d) = start(script, saved, &db, fast());
        wait_until(|| !db.lock().unwrap().sensor_rows("x").unwrap().is_empty());
        d.shutdown().unwrap();
        let db = db.lock().unwrap();
        assert!(!db.sensor_rows("cpu").unwrap().is_empty());
        assert!(db.sensor_rows("y").unwrap().is_empty(), "unreferenced");
        assert!(!db.fan_rows("f1").unwrap().is_empty());
        assert!(!db.fan_rows("f2").unwrap().is_empty());
    }

    #[test]
    fn failed_ticks_are_not_recorded_again() {
        let db = db();
        let (_hal, d) = start(vec![at(40.0), Tick::Fail], vec![], &db, fast());
        wait_until(|| d.cache().status.degraded && d.cache().ticks >= 10);
        d.shutdown().unwrap();
        assert_eq!(db.lock().unwrap().sensor_rows("cpu").unwrap().len(), 1);
    }

    #[test]
    fn a_slow_or_locked_history_db_never_stalls_the_engine() {
        let db = db();
        let (_hal, d) = start(vec![at(40.0)], vec![], &db, fast());
        let held = db.lock().unwrap();
        let before = d.cache().ticks;
        wait_until(|| d.cache().ticks >= before + 20);
        drop(held);
        d.shutdown().unwrap();
    }

    #[test]
    fn prune_runs_periodically_off_the_engine_thread() {
        let db = db();
        db.lock()
            .unwrap()
            .record_tick(
                1_000,
                &[thermaliq_common::SensorReading {
                    id: "old".into(),
                    label: "old".into(),
                    kind: SensorKind::Cpu,
                    temp_celsius: 30.0,
                }],
                &[],
            )
            .unwrap();
        let opts = Options {
            prune_every: Duration::from_millis(10),
            ..fast()
        };
        let (_hal, d) = start(vec![at(40.0)], vec![], &db, opts);
        wait_until(|| db.lock().unwrap().sensor_rows("old").unwrap().is_empty());
        d.shutdown().unwrap();
    }

    fn last_write(hal: &MockHardwareLayer) -> Option<(String, u8)> {
        hal.writes().last().cloned()
    }

    #[test]
    fn override_request_is_clamped_to_duty_0_100_and_300_seconds() {
        assert_eq!(
            clamp_override(250, 10_000),
            Some((100, Duration::from_secs(300)))
        );
        assert_eq!(clamp_override(-5, 5), Some((0, Duration::from_secs(5))));
        assert_eq!(clamp_override(50, 0), None, "an override must expire");
    }

    #[tokio::test]
    async fn an_override_reaches_the_hal_clamped() {
        let (hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        d.handle().set_override("f1", 250, 60).await.unwrap();
        wait_until(|| last_write(&hal) == Some(("f1".into(), 100)));
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn an_override_expires_back_to_the_profile() {
        let (hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        d.handle().set_override("f1", 90, 1).await.unwrap();
        wait_until(|| last_write(&hal) == Some(("f1".into(), 90)));
        wait_until(|| last_write(&hal) == Some(("f1".into(), 30)));
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn cancelling_an_override_returns_to_the_profile_and_a_second_cancel_is_not_found() {
        let (hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        let h = d.handle();
        h.set_override("f1", 90, 60).await.unwrap();
        wait_until(|| last_write(&hal) == Some(("f1".into(), 90)));
        h.cancel_override("f1").await.unwrap();
        wait_until(|| last_write(&hal) == Some(("f1".into(), 30)));
        assert!(matches!(
            h.cancel_override("f1").await,
            Err(CommandError::NotFound(_))
        ));
        assert!(matches!(
            h.cancel_override("nope").await,
            Err(CommandError::NotFound(_))
        ));
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn an_override_the_engine_cannot_apply_is_an_error_and_writes_nothing() {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], vec![at(40.0)]).read_only("f1"));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal.clone(), &active, vec![], db(), fast());
        let h = d.handle();
        for fan in [
            "f1", /* read-only */
            "f2", /* no control */
            "nope",
        ] {
            assert!(
                matches!(
                    h.set_override(fan, 50, 60).await,
                    Err(CommandError::Rejected(_))
                ),
                "{fan}"
            );
        }
        assert!(matches!(
            h.set_override("f1", 50, 0).await,
            Err(CommandError::Rejected(_))
        ));
        d.shutdown().unwrap();
        assert!(hal.writes().is_empty());
    }

    #[tokio::test]
    async fn set_profile_switches_the_running_engine_and_the_reported_name() {
        let (hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        assert_eq!(d.cache().active_profile, "flat");
        let sixty = FLAT
            .replace("pwm = 30", "pwm = 60")
            .replace("name = \"flat\"", "name = \"sixty\"");
        d.handle()
            .set_profile(Profile::from_toml(&sixty).unwrap())
            .await
            .unwrap();
        assert_eq!(d.cache().active_profile, "sixty");
        wait_until(|| last_write(&hal) == Some(("f1".into(), 60)));
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn commands_after_the_engine_stopped_fail_instead_of_pretending() {
        let (_hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        let h = d.handle();
        d.shutdown().unwrap();
        assert!(matches!(
            h.set_override("f1", 50, 60).await,
            Err(CommandError::Unavailable)
        ));
        assert!(matches!(
            h.cancel_override("f1").await,
            Err(CommandError::Unavailable)
        ));
        let p = Profile::from_toml(FLAT).unwrap();
        assert!(matches!(
            h.set_profile(p).await,
            Err(CommandError::Unavailable)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_continuous_flood_of_commands_does_not_starve_ticks() {
        let (_hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        let h = d.handle();
        wait_until(|| d.cache().ticks > 0);
        let before = d.cache().ticks;
        // Flood for 20 tick periods (the tick is 5 ms), so the queue never drains in one window.
        let end = Instant::now() + 20 * fast().tick;
        // Producers don't wait for replies, so the queue is never empty while they run.
        let flood: Vec<_> = (0..4)
            .map(|_| {
                let tx = h.tx.clone();
                std::thread::spawn(move || {
                    while Instant::now() < end {
                        let (reply, _) = oneshot::channel();
                        let _ = tx.send(Msg::Cmd(Cmd::Cancel("f1".into(), reply)));
                    }
                })
            })
            .collect();
        for f in flood {
            f.join().unwrap();
        }
        let rose = d.cache().ticks - before;
        assert!(rose >= 3, "only {rose} ticks ran during the flood");
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn an_override_reports_whether_it_is_in_effect() {
        let (_hal, d) = start(vec![at(40.0)], vec![], &db(), fast());
        wait_until(|| d.cache().ticks > 0);
        assert_eq!(d.handle().set_override("f1", 50, 60).await, Ok(None));
        d.shutdown().unwrap();
        let (_hal, d) = start(vec![at(97.0)], vec![], &db(), fast());
        wait_until(|| d.cache().status.latched);
        assert_eq!(
            d.handle().set_override("f1", 50, 60).await,
            Ok(Some("latched"))
        );
        d.shutdown().unwrap();
    }

    #[test]
    fn load_profiles_reads_bundled_and_saved_and_reports_the_bad_ones() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("profiles");
        fs::create_dir(&p).unwrap();
        fs::write(
            p.join("mine.toml"),
            FLAT.replacen("\"flat\"", "\"mine\"", 1),
        )
        .unwrap();
        fs::write(p.join("bad.toml"), "name = 3\n").unwrap();
        fs::write(p.join("notes.txt"), "ignored").unwrap();
        let (profiles, skipped) = load_profiles(dir.path());
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"mine"));
        assert!(names.contains(&"balanced"), "bundled profiles are included");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("bad.toml"), "{skipped:?}");
    }

    #[test]
    fn load_profiles_reports_a_saved_profile_overridden_by_a_later_same_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("profiles");
        fs::create_dir(&p).unwrap();
        fs::write(p.join("a.toml"), FLAT).unwrap();
        fs::write(p.join("b.toml"), FLAT).unwrap();
        let (profiles, skipped) = load_profiles(dir.path());
        assert_eq!(profiles.iter().filter(|p| p.name == "flat").count(), 1);
        assert_eq!(skipped.len(), 1);
        assert!(
            skipped[0].contains("a.toml") && skipped[0].contains("b.toml"),
            "{skipped:?}"
        );
    }

    #[test]
    fn a_saved_profile_replacing_a_bundled_one_is_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("profiles");
        fs::create_dir(&p).unwrap();
        fs::write(
            p.join("x.toml"),
            FLAT.replacen("\"flat\"", "\"balanced\"", 1),
        )
        .unwrap();
        let (_, skipped) = load_profiles(dir.path());
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    #[test]
    fn load_profiles_without_a_profiles_dir_gives_just_the_bundled_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (profiles, skipped) = load_profiles(dir.path());
        assert_eq!(profiles.len(), thermaliq_common::bundled_profiles().len());
        assert!(skipped.is_empty());
    }
}

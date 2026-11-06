//! Daemon wiring (SPEC §5): the engine thread, the shared snapshot cache the API reads,
//! and the history thread. Only `thermaliq-engine` writes to fans.

use crate::agent::{Agent, AgentTick, Brain, DEFAULT_SLIDER};
use crate::history::{recorded_sensor_ids, HistoryDb};
use std::collections::HashMap;
use std::fs;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thermaliq_agent::optimiser::REQUEST_TTL;
use thermaliq_common::{
    bundled_profiles, FanReading, HardwareSnapshot, MetricReading, Profile, SensorReading,
};
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
    /// The agent's wish for this tick: replaces its previous requests (S1: it only submits).
    Agent(Vec<(String, u8)>),
    /// Pause and agent crash: drop every Agent request now.
    WithdrawAgent(Reply),
}

/// What `/status` says about the agent thread. A crashed agent stays crashed until the
/// service restarts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentState {
    #[default]
    Running,
    Paused,
    Crashed,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Crashed => "crashed",
        }
    }
}

#[derive(Default)]
struct AgentFlags {
    paused: AtomicBool,
    crashed: AtomicBool,
    /// Reasons set from outside the engine thread (the identity check); shown with every cache read.
    notes: Mutex<Vec<String>>,
}

impl AgentFlags {
    fn state(&self) -> AgentState {
        if self.crashed.load(Ordering::SeqCst) {
            AgentState::Crashed
        } else if self.paused.load(Ordering::SeqCst) {
            AgentState::Paused
        } else {
            AgentState::Running
        }
    }
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

fn apply(
    engine: &mut Engine,
    began: Instant,
    cache: &RwLock<Cache>,
    agent: &AgentShared,
    cmd: Cmd,
) {
    let (result, reply) = match cmd {
        Cmd::Agent(asks) => {
            // A paused agent's late asks are dropped here, so pause can't be raced.
            if !agent.flags.paused.load(Ordering::SeqCst) {
                engine.withdraw_all(Source::Agent);
                let until = began.elapsed() + REQUEST_TTL;
                for (fan, duty) in asks {
                    // The engine refuses pumps and unknown fans, and clamps the duty.
                    let _ = engine.submit(&fan, Source::Agent, duty, Some(until));
                }
            }
            return;
        }
        Cmd::WithdrawAgent(reply) => {
            engine.withdraw_all(Source::Agent);
            (Ok(None), reply)
        }
        Cmd::SetProfile(p, reply) => match engine.check_profile(&p) {
            Ok(()) => {
                engine.set_profile(&p);
                *agent
                    .profile
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = p.clone();
                cache
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .active_profile = p.name;
                (Ok(None), reply)
            }
            Err(e) => (Err(CommandError::Rejected(e)), reply),
        },
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

/// What the engine thread shares with the agent: the pause/crash flags and the active profile.
#[derive(Clone)]
struct AgentShared {
    flags: Arc<AgentFlags>,
    profile: Arc<RwLock<Profile>>,
}

/// A cheap, cloneable way for the API to read the cache and talk to the engine thread.
#[derive(Clone)]
pub struct Handle {
    cache: Arc<RwLock<Cache>>,
    tx: mpsc::Sender<Msg>,
    ticks: watch::Receiver<u64>,
    flags: Arc<AgentFlags>,
}

impl Handle {
    /// Changes after every tick's cache update; closes when the engine thread ends.
    pub fn ticks(&self) -> watch::Receiver<u64> {
        self.ticks.clone()
    }

    pub fn cache(&self) -> Cache {
        let mut c = self
            .cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        c.status.agent = self.flags.state(); // read live, so pause shows at once
        let notes = self
            .flags
            .notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !notes.is_empty() {
            c.status.degraded = true;
            c.status.degraded_reasons.extend(notes.iter().cloned());
        }
        c
    }

    /// Adds a reason that stays in `/status` `degraded_reasons` until the service restarts.
    pub fn note_degraded(&self, why: String) {
        self.flags
            .notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(why);
    }

    /// Withdraws the agent's requests and ignores it until `resume_agent`.
    pub async fn pause_agent(&self) -> Result<(), CommandError> {
        self.agent_alive()?;
        self.flags.paused.store(true, Ordering::SeqCst);
        self.ask(Cmd::WithdrawAgent).await.map(drop)
    }

    pub async fn resume_agent(&self) -> Result<(), CommandError> {
        self.agent_alive()?;
        if !self.cache().status.engine_running {
            return Err(CommandError::Unavailable);
        }
        self.flags.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// A crashed agent thread is gone: saying "resumed" would be a lie (S8).
    fn agent_alive(&self) -> Result<(), CommandError> {
        if self.flags.crashed.load(Ordering::SeqCst) {
            return Err(CommandError::Rejected(
                "the agent stopped after an error; restart the service".into(),
            ));
        }
        Ok(())
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
    /// Hardware faults behind `degraded` that aren't S4, such as a fan header that would not go
    /// back to the BIOS (SPEC §5.1). Empty while `degraded` comes from S4 alone.
    pub degraded_reasons: Vec<String>,
    /// CPU/GPU sensors the latch is waiting on to cool down or come back.
    pub waiting_on: Vec<String>,
    /// Optional sensor sources (e.g. "hwinfo") that failed on the last tick and are missing
    /// from the snapshot. Not a safety state: required hardware is fine.
    pub unavailable_sources: Vec<String>,
    /// Filled in when read (`Handle::cache`), not by the engine thread.
    pub agent: AgentState,
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
    /// Per fan in the snapshot: what the curve, the agent and the engine's last write say.
    pub fan_info: HashMap<String, FanInfo>,
}

/// Why a fan is where it is. `None` = no such thing right now (never 0). `commanded` is what
/// the engine last wrote, not NVML's readback, which lags a slewing fan by seconds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FanInfo {
    pub curve: Option<u8>,
    pub agent: Option<u8>,
    pub commanded: Option<u8>,
}

type HistoryTick = (i64, Vec<SensorReading>, Vec<FanReading>, Vec<MetricReading>);

pub struct Daemon {
    cache: Arc<RwLock<Cache>>,
    tx: mpsc::Sender<Msg>,
    ticks: watch::Receiver<u64>,
    flags: Arc<AgentFlags>,
    engine: Option<JoinHandle<()>>,
    history: Option<JoinHandle<()>>,
    agent: Option<JoinHandle<()>>,
}

/// Ticks queued for the agent before new ones are dropped (the engine never waits).
const AGENT_QUEUE: usize = 4;

/// The agent thread (SPEC §7: separate from the engine, only submits). A panic withdraws
/// its requests so the profile takes over, and is reported in `/status`.
fn agent_loop(
    mut brain: Box<dyn Brain>,
    ticks: &mpsc::Receiver<AgentTick>,
    tx: &mpsc::Sender<Msg>,
    flags: &AgentFlags,
) {
    let began = Instant::now();
    let run = catch_unwind(AssertUnwindSafe(|| {
        while let Ok(tick) = ticks.recv() {
            if flags.paused.load(Ordering::SeqCst) {
                continue;
            }
            let asks = brain.step(&tick, began.elapsed());
            if tx.send(Msg::Cmd(Cmd::Agent(asks))).is_err() {
                return;
            }
        }
    }));
    if run.is_err() {
        flags.crashed.store(true, Ordering::SeqCst);
        let (reply, _) = oneshot::channel();
        let _ = tx.send(Msg::Cmd(Cmd::WithdrawAgent(reply)));
    }
}

impl Daemon {
    /// Runs `active` on its own thread. `saved` is every saved profile, so history also
    /// records the sensors profiles other than the active one reference (SPEC §6).
    pub fn start(
        hal: Arc<dyn HardwareLayer>,
        active: &Profile,
        saved: Vec<Profile>,
        history: Arc<Mutex<HistoryDb>>,
        opts: Options,
    ) -> Self {
        let for_agent = history.clone();
        Self::start_with(hal, active, saved, history, opts, move |profile| {
            Box::new(Agent::new(for_agent, profile, DEFAULT_SLIDER))
        })
    }

    /// `start` with the agent's brain supplied (tests script or break it). `make_brain`
    /// gets the shared active profile, which the engine thread keeps current.
    pub(crate) fn start_with(
        hal: Arc<dyn HardwareLayer>,
        active: &Profile,
        mut saved: Vec<Profile>,
        history: Arc<Mutex<HistoryDb>>,
        opts: Options,
        make_brain: impl FnOnce(Arc<RwLock<Profile>>) -> Box<dyn Brain>,
    ) -> Self {
        saved.push(active.clone());
        let shared = AgentShared {
            flags: Arc::default(),
            profile: Arc::new(RwLock::new(active.clone())),
        };
        let flags = shared.flags.clone();
        let brain = make_brain(shared.profile.clone());
        let (agent_tx, agent_rx) = mpsc::sync_channel::<AgentTick>(AGENT_QUEUE);
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
        let agent = {
            let (tx, flags) = (tx.clone(), flags.clone());
            std::thread::Builder::new()
                .name("thermaliq-agent".into())
                .spawn(move || agent_loop(brain, &agent_rx, &tx, &flags))
                .expect("spawn agent thread")
        };
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
                            let _ = history_tx.try_send((
                                unix_now(),
                                sensors,
                                snapshot.fans.clone(),
                                snapshot.metrics.clone(),
                            ));
                        }
                        let mut degraded_reasons = sources.degraded_reasons();
                        // Inert controls: the profile names fans that no backend lists (SPEC §5.2).
                        degraded_reasons.extend(engine.missing_fans().into_iter().map(|f| {
                            format!("the active profile controls fan '{f}', which no backend lists")
                        }));
                        // A profile whose roles all went unmapped leaves every fan to the BIOS (SPEC §5.2).
                        if engine.controls_nothing() {
                            degraded_reasons.push(format!(
                                "profile '{}' controls no fans: run thermaliq-daemon identify",
                                shared
                                    .profile
                                    .read()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .name
                            ));
                        }
                        // A fresh snapshot with no fan at all means every fan backend failed to
                        // start or lost its fans. An empty engine must not look healthy (S8).
                        if engine.snapshot_fresh() && snapshot.fans.is_empty() {
                            degraded_reasons
                                .push("no fans found: no hardware backend is providing any".into());
                        }
                        let status = Status {
                            engine_running: true,
                            latched: engine.latched(),
                            degraded: engine.degraded() || !degraded_reasons.is_empty(),
                            degraded_reasons,
                            waiting_on: engine.latch_waiting_on().to_vec(),
                            unavailable_sources: sources.unavailable_sources(),
                            agent: AgentState::default(),
                        };
                        // Only fresh data: the agent never acts on an old snapshot. A full queue
                        // drops this tick for it; its requests expire in 3 s.
                        let fan_info: HashMap<String, FanInfo> = snapshot
                            .fans
                            .iter()
                            .map(|f| {
                                let info = FanInfo {
                                    curve: engine.curve_output(&f.id),
                                    agent: engine.agent_request(&f.id),
                                    commanded: engine.written(&f.id),
                                };
                                (f.id.clone(), info)
                            })
                            .collect();
                        if engine.snapshot_fresh() && !shared.flags.paused.load(Ordering::SeqCst) {
                            let curve = fan_info
                                .iter()
                                .filter_map(|(id, i)| Some((id.clone(), i.curve?)))
                                .collect();
                            let _ = agent_tx.try_send(AgentTick {
                                snapshot: snapshot.clone(),
                                curve,
                            });
                        }
                        {
                            let mut c = cache.write().unwrap_or_else(PoisonError::into_inner);
                            c.fan_info = fan_info;
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
                                    apply(&mut engine, began, &cache, &shared, cmd);
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
            flags,
            engine: Some(engine),
            history: Some(history),
            agent: Some(agent),
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
            flags: self.flags.clone(),
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
        // Its input closed with the engine thread, so it ends; it catches its own panics.
        let _ = self.agent.take().map(JoinHandle::join);
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
            Ok((now, sensors, fans, metrics)) => {
                let _ = db
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .record_tick(now, &sensors, &fans, &metrics);
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

pub(crate) fn unix_now() -> i64 {
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
    fn a_hardware_fault_reports_degraded_with_its_reason_but_is_not_s4() {
        struct Stuck(Arc<MockHardwareLayer>);
        impl HardwareLayer for Stuck {
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
            fn degraded_reasons(&self) -> Vec<String> {
                vec!["lhm_fan_5: still manual".into()]
            }
        }
        let mock = Arc::new(MockHardwareLayer::new(&["f1"], vec![at(40.0)]));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(Arc::new(Stuck(mock.clone())), &active, vec![], db(), fast());
        wait_until(|| d.cache().ticks > 3);
        let c = d.cache();
        assert!(c.status.degraded);
        assert_eq!(c.status.degraded_reasons, ["lhm_fan_5: still manual"]);
        assert!(
            mock.writes().iter().all(|(_, duty)| *duty == 30),
            "one stuck header must not send every fan to 100%: {:?}",
            mock.writes()
        );
    }

    #[test]
    fn a_snapshot_with_no_fans_is_degraded_and_says_so_never_a_healthy_empty_engine() {
        let hal = Arc::new(MockHardwareLayer::new(&[], vec![at(40.0)]));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal, &active, vec![], db(), fast());
        wait_until(|| d.cache().ticks > 3);
        let c = d.cache();
        assert!(c.status.degraded);
        assert!(
            c.status
                .degraded_reasons
                .iter()
                .any(|r| r.contains("no fans")),
            "{:?}",
            c.status.degraded_reasons
        );
    }

    #[test]
    fn an_active_profile_fan_no_backend_lists_is_degraded_and_named() {
        let hal = Arc::new(MockHardwareLayer::new(&["other"], vec![at(40.0)]));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal, &active, vec![], db(), fast());
        wait_until(|| d.cache().ticks > 3);
        let c = d.cache();
        assert!(c.status.degraded);
        let why = c.status.degraded_reasons.join("\n");
        assert!(why.contains("'f1'"), "{why}");
    }

    #[tokio::test]
    async fn switching_to_a_profile_with_an_unlisted_fan_is_refused_by_name_and_changes_nothing() {
        let (_, d) = start(vec![at(40.0)], vec![], &db(), fast());
        wait_until(|| d.cache().ticks > 1);
        let bad = Profile::from_toml(
            &FLAT
                .replace("name = \"flat\"", "name = \"bad\"")
                .replace("f1", "gone"),
        )
        .unwrap();
        let err = d.handle().set_profile(bad).await.unwrap_err();
        let CommandError::Rejected(m) = err else {
            panic!("{err:?}")
        };
        assert!(m.contains("'gone'"), "{m}");
        assert_eq!(d.cache().active_profile, "flat");
    }

    #[test]
    fn fans_present_means_no_missing_fans_reason() {
        let hal = Arc::new(MockHardwareLayer::new(&["f1"], vec![at(40.0)]));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal, &active, vec![], db(), fast());
        wait_until(|| d.cache().ticks > 3);
        assert!(d.cache().status.degraded_reasons.is_empty());
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
    async fn a_noted_reason_shows_in_the_cache_and_marks_it_degraded() {
        let (_, d) = start(vec![at(40.0)], vec![], &db(), fast());
        wait_until(|| d.cache().ticks > 0);
        assert!(!d.cache().status.degraded);
        d.handle().note_degraded("the board changed".into());
        let c = d.cache();
        assert!(
            c.status.degraded
                && c.status
                    .degraded_reasons
                    .contains(&"the board changed".into())
        );
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn an_active_profile_with_no_controls_is_degraded_until_one_with_controls_is_active() {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], vec![at(40.0)]));
        let empty = Profile::from_toml("name = \"bare\"\n").unwrap();
        let d = Daemon::start(hal, &empty, vec![], db(), fast());
        wait_until(|| d.cache().ticks > 2);
        let c = d.cache();
        let want = "profile 'bare' controls no fans: run thermaliq-daemon identify";
        assert!(c.status.degraded, "{:?}", c.status);
        assert!(
            c.status.degraded_reasons.iter().any(|r| r == want),
            "{:?}",
            c.status
        );
        d.handle()
            .set_profile(Profile::from_toml(FLAT).unwrap())
            .await
            .unwrap();
        wait_until(|| d.cache().status.degraded_reasons.is_empty());
        d.shutdown().unwrap();
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
    async fn set_profile_rejects_0_rpm_on_a_pump_and_keeps_the_running_profile() {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], vec![at(40.0)]).pump("f1"));
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal, &active, vec![], db(), fast());
        wait_until(|| d.cache().ticks > 0);
        let zero_rpm = FLAT.replace("name = \"flat\"", "name = \"zero\"").replace(
            "step_down_pct = 100\n",
            "step_down_pct = 100\nstart_percent = 40\nstop_percent = 30\n",
        );
        let r = d
            .handle()
            .set_profile(Profile::from_toml(&zero_rpm).unwrap())
            .await;
        assert!(matches!(r, Err(CommandError::Rejected(m)) if m.contains("pump")));
        assert_eq!(d.cache().active_profile, "flat");
        d.shutdown().unwrap();
    }

    #[test]
    fn metrics_reach_the_cache_and_the_history() {
        use thermaliq_common::{MetricKind, MetricReading};
        let m = MetricReading {
            id: "gpu_load".into(),
            kind: MetricKind::GpuLoad,
            value: 55.0,
        };
        let hal =
            Arc::new(MockHardwareLayer::new(&["f1"], vec![at(40.0)]).metrics(vec![m.clone()]));
        let db = db();
        let active = Profile::from_toml(FLAT).unwrap();
        let d = Daemon::start(hal, &active, vec![], db.clone(), fast());
        wait_until(|| {
            !db.lock()
                .unwrap()
                .metric_rows("gpu_load")
                .unwrap()
                .is_empty()
        });
        assert_eq!(d.cache().snapshot.metrics, [m]);
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

    // ---- agent thread (M20b wiring) ----

    /// Asks for the same duties every tick, or panics after `panic_after` ticks.
    struct Scripted {
        asks: Vec<(String, u8)>,
        panic_after: Option<u32>,
        steps: u32,
    }
    impl Brain for Scripted {
        fn step(&mut self, _: &AgentTick, _: Duration) -> Vec<(String, u8)> {
            self.steps += 1;
            assert!(self.panic_after.is_none_or(|n| self.steps <= n), "boom");
            self.asks.clone()
        }
    }

    fn asks(fan: &str, duty: u8, panic_after: Option<u32>) -> Scripted {
        Scripted {
            asks: vec![(fan.into(), duty)],
            panic_after,
            steps: 0,
        }
    }

    fn with_brain(hal: Arc<MockHardwareLayer>, brain: Scripted) -> Daemon {
        let active = Profile::from_toml(FLAT).unwrap();
        Daemon::start_with(hal, &active, vec![], db(), fast(), |_| Box::new(brain))
    }

    fn mock() -> Arc<MockHardwareLayer> {
        Arc::new(MockHardwareLayer::new(&["f1", "f2"], vec![at(40.0)]))
    }

    #[test]
    fn an_agent_request_reaches_the_fan_and_the_engine_clamps_it_to_15_below_the_curve() {
        let hal = mock();
        let d = with_brain(hal.clone(), asks("f1", 80, None));
        wait_until(|| last_write(&hal) == Some(("f1".into(), 80)));
        d.shutdown().unwrap();
        let hal = mock();
        let d = with_brain(hal.clone(), asks("f1", 0, None));
        wait_until(|| last_write(&hal) == Some(("f1".into(), 15))); // curve 30 − 15
        d.shutdown().unwrap();
        assert!(
            hal.writes().iter().all(|(_, d)| *d >= 15),
            "never below the clamp"
        );
    }

    #[tokio::test]
    async fn the_cache_shows_each_fans_curve_the_agents_ask_and_the_commanded_duty() {
        let hal = mock();
        let d = with_brain(hal.clone(), asks("f1", 80, None));
        let info = |d: &Daemon| d.cache().fan_info.get("f1").cloned().unwrap_or_default();
        wait_until(|| info(&d).agent == Some(80));
        let i = info(&d);
        assert_eq!((i.curve, i.commanded), (Some(30), Some(80)));
        assert_eq!(
            d.cache().fan_info.get("f2"),
            Some(&FanInfo::default()),
            "no control, nothing to say"
        );
        d.handle().pause_agent().await.unwrap();
        wait_until(|| info(&d).agent.is_none() && info(&d).commanded == Some(30));
        d.shutdown().unwrap();
    }

    #[test]
    fn an_ask_that_arrives_after_pause_is_dropped_by_the_engine_thread() {
        // The agent thread and the tick feed also check the flag; this is the race they can't close.
        let hal = mock();
        let active = Profile::from_toml(FLAT).unwrap();
        let mut engine = Engine::new(hal.clone(), &active);
        let cache = RwLock::new(Cache::default());
        let agent = AgentShared {
            flags: Arc::default(),
            profile: Arc::new(RwLock::new(active)),
        };
        let began = Instant::now();
        engine.tick(Duration::ZERO);
        agent.flags.paused.store(true, Ordering::SeqCst);
        apply(
            &mut engine,
            began,
            &cache,
            &agent,
            Cmd::Agent(vec![("f1".into(), 80)]),
        );
        engine.tick(began.elapsed());
        assert_eq!(last_write(&hal), Some(("f1".into(), 30)));
        agent.flags.paused.store(false, Ordering::SeqCst);
        apply(
            &mut engine,
            began,
            &cache,
            &agent,
            Cmd::Agent(vec![("f1".into(), 80)]),
        );
        engine.tick(began.elapsed());
        assert_eq!(
            last_write(&hal),
            Some(("f1".into(), 80)),
            "control: it does act unpaused"
        );
    }

    #[test]
    fn a_request_for_a_pump_never_reaches_it() {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], vec![at(40.0)]).pump("f1"));
        let d = with_brain(hal.clone(), asks("f1", 80, None));
        wait_until(|| hal.writes().len() > 20);
        d.shutdown().unwrap();
        assert!(
            hal.writes().iter().all(|(_, d)| *d != 80),
            "{:?}",
            hal.writes()
        );
    }

    #[tokio::test]
    async fn pause_withdraws_the_agent_and_resume_lets_it_act_again() {
        let hal = mock();
        let d = with_brain(hal.clone(), asks("f1", 80, None));
        let h = d.handle();
        assert_eq!(d.cache().status.agent, AgentState::Running);
        wait_until(|| last_write(&hal) == Some(("f1".into(), 80)));
        h.pause_agent().await.unwrap();
        assert_eq!(d.cache().status.agent, AgentState::Paused);
        wait_until(|| last_write(&hal) == Some(("f1".into(), 30)));
        let n = hal.writes().len();
        wait_until(|| hal.writes().len() >= n + 20);
        assert!(
            hal.writes()[n..].iter().all(|(_, d)| *d == 30),
            "the agent keeps asking but a paused agent is ignored"
        );
        h.resume_agent().await.unwrap();
        assert_eq!(d.cache().status.agent, AgentState::Running);
        wait_until(|| last_write(&hal) == Some(("f1".into(), 80)));
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn a_crashed_agent_is_withdrawn_reported_and_cannot_be_resumed() {
        let hal = mock();
        let d = with_brain(hal.clone(), asks("f1", 80, Some(3)));
        let h = d.handle();
        wait_until(|| d.cache().status.agent == AgentState::Crashed);
        // Withdrawn at once, not left to lapse (a request lives 3 s).
        let t0 = Instant::now();
        wait_until(|| last_write(&hal) == Some(("f1".into(), 30)));
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        let n = hal.writes().len();
        wait_until(|| hal.writes().len() >= n + 20);
        assert!(
            hal.writes()[n..].iter().all(|(_, d)| *d == 30),
            "profile resumed"
        );
        assert!(d.cache().status.engine_running, "the engine carries on");
        assert!(matches!(
            h.resume_agent().await,
            Err(CommandError::Rejected(_))
        ));
        assert!(matches!(
            h.pause_agent().await,
            Err(CommandError::Rejected(_))
        ));
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn the_latch_beats_the_agent() {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], vec![at(97.0)]));
        let d = with_brain(hal.clone(), asks("f1", 0, None));
        wait_until(|| d.cache().status.latched);
        let n = hal.writes().len();
        wait_until(|| hal.writes().len() >= n + 20);
        assert!(
            hal.writes().iter().all(|(_, d)| *d == 100),
            "{:?}",
            hal.writes()
        );
        d.shutdown().unwrap();
    }

    #[tokio::test]
    async fn pause_and_resume_after_the_engine_stopped_fail_instead_of_pretending() {
        let d = with_brain(mock(), asks("f1", 80, None));
        let h = d.handle();
        d.shutdown().unwrap();
        assert!(matches!(
            h.pause_agent().await,
            Err(CommandError::Unavailable)
        ));
        assert!(matches!(
            h.resume_agent().await,
            Err(CommandError::Unavailable)
        ));
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

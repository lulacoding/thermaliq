//! The slow layer's thread (SPEC §7.2). Separate from the fast layer, because a local call can
//! take a minute and hosted can take 2 s. It only submits to the engine (S1): profile changes
//! through the same validated path as the API. If it dies, fans carry on untouched.

use crate::history::HistoryDb;
use crate::keychain::KeyStore;
use crate::runtime::{AgentState, Handle};
use serde::Serialize;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use thermaliq_agent::classifier::Workload;
use thermaliq_agent::decision::{
    builtin_workload, clean_exe, game_like, why_not_switch, Change, Decision, DecisionProvider,
    Foreground, Known, Observation, SlowLayer, Source,
};
use thermaliq_common::{HardwareSnapshot, MetricKind, Profile, SensorKind};

const STEP: Duration = Duration::from_secs(1);
const MAX_LOG: usize = 100;

pub type ForegroundFn = Arc<dyn Fn() -> Option<Foreground> + Send + Sync>;

/// The highest reading of a kind. With two GPUs, "is one busy" is the question that matters.
fn max_metric(snap: &HardwareSnapshot, kind: MetricKind) -> Option<f64> {
    snap.metrics
        .iter()
        .filter(|m| m.kind == kind && m.value.is_finite())
        .map(|m| m.value)
        .reduce(f64::max)
}

fn max_temp(snap: &HardwareSnapshot, kind: SensorKind) -> Option<f64> {
    snap.sensors
        .iter()
        .filter(|s| s.kind == kind && s.temp_celsius.is_finite())
        .map(|s| f64::from(s.temp_celsius))
        .reduce(f64::max)
}

/// `age` is the snapshot's age in ticks: an old reading says nothing about now.
fn observation(snap: &HardwareSnapshot, age: u64, foreground: Option<Foreground>) -> Observation {
    if age > 0 {
        return Observation {
            foreground,
            ..Observation::default()
        };
    }
    Observation {
        foreground,
        cpu_load_pct: max_metric(snap, MetricKind::CpuLoad),
        gpu_load_pct: max_metric(snap, MetricKind::GpuLoad),
        cpu_power_w: max_metric(snap, MetricKind::CpuPower),
        gpu_power_w: max_metric(snap, MetricKind::GpuPower),
        cpu_temp_c: max_temp(snap, SensorKind::Cpu),
        gpu_temp_c: max_temp(snap, SensorKind::Gpu),
    }
}

/// One line of the decision log. Built-in answers show as source "built-in", with no
/// confidence: a default the user can correct.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogEntry {
    pub at: i64,
    pub exe: String,
    pub workload: Option<String>,
    pub source: Option<String>,
    pub confidence: Option<f64>,
    /// True when the user has to decide.
    pub asked: bool,
    /// The profile this put in place.
    pub profile: Option<String>,
    /// Why nothing changed, if something stopped it.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionView {
    pub exe: String,
    pub suggested_workload: Option<String>,
    pub suggested_by: Option<String>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct View {
    pub questions: Vec<QuestionView>,
    pub decisions: Vec<LogEntry>,
    /// Whether the foreground app can currently be seen. Without it, nothing is decided.
    pub foreground: bool,
    /// The local model's state: "configured", "not configured" or why it is unusable.
    pub local: String,
    pub crashed: bool,
}

/// What the API shares with the thread.
#[derive(Clone)]
pub struct SlowShared {
    view: Arc<Mutex<View>>,
    answers: Arc<Mutex<Vec<(String, Workload)>>>,
    keys: Arc<dyn KeyStore>,
    hosted_error: Arc<Mutex<Option<String>>>,
}

impl SlowShared {
    pub fn new(keys: Arc<dyn KeyStore>, local: String) -> Self {
        Self {
            view: Arc::new(Mutex::new(View {
                local,
                ..View::default()
            })),
            answers: Arc::default(),
            keys,
            hosted_error: Arc::default(),
        }
    }

    /// Why the last hosted call failed; `None` after a success or before the first call.
    pub fn hosted_error(&self) -> Arc<Mutex<Option<String>>> {
        self.hosted_error.clone()
    }

    pub fn view(&self) -> View {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn keys(&self) -> &dyn KeyStore {
        self.keys.as_ref()
    }

    /// The user's answer: saved first, so a success really is saved (S8), then applied by the
    /// thread at its next step.
    pub fn answer(
        &self,
        exe: &str,
        workload: Workload,
        db: &Mutex<HistoryDb>,
    ) -> rusqlite::Result<()> {
        db.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .save_exe_workload(exe, workload.name(), Source::User.as_str(), None)?;
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((exe.into(), workload));
        Ok(())
    }

    fn take_answers(&self) -> Vec<(String, Workload)> {
        std::mem::take(&mut *self.answers.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn update(&self, f: impl FnOnce(&mut View)) {
        f(&mut self.view.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

/// What the thread needs.
pub struct SlowParts {
    pub handle: Handle,
    pub profiles: Vec<Profile>,
    pub history: Arc<Mutex<HistoryDb>>,
    pub foreground: ForegroundFn,
    pub hosted: Option<Box<dyn DecisionProvider + Send>>,
    pub local: Option<Box<dyn DecisionProvider + Send>>,
    pub shared: SlowShared,
    pub period: Duration,
}

impl SlowParts {
    pub const DEFAULT_PERIOD: Duration = STEP;
}

pub struct SlowThread {
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SlowThread {
    pub fn start(parts: SlowParts) -> Self {
        let stopping = Arc::new(AtomicBool::new(false));
        let flag = stopping.clone();
        let shared = parts.shared.clone();
        let thread = std::thread::Builder::new()
            .name("thermaliq-slow".into())
            .spawn(move || {
                // Only this layer stops: the engine and the fast layer don't depend on it.
                if catch_unwind(AssertUnwindSafe(|| run(parts, &flag))).is_err() {
                    shared.update(|v| v.crashed = true);
                }
            })
            .expect("spawn slow-layer thread");
        Self {
            stopping,
            thread: Some(thread),
        }
    }

    /// Asks the thread to stop, which also cancels a local call. Doesn't wait.
    pub fn request_stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for SlowThread {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn load_known(db: &Mutex<HistoryDb>) -> HashMap<String, Known> {
    let rows = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .exe_workloads()
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|(exe, (workload, source, confidence))| {
            Some((
                exe,
                Known {
                    workload: Workload::from_name(&workload)?,
                    source: Source::from_cached(&source)?,
                    confidence,
                },
            ))
        })
        .collect()
}

fn unix_now() -> i64 {
    crate::runtime::unix_now()
}

fn entry(d: &Decision) -> LogEntry {
    LogEntry {
        at: unix_now(),
        exe: d.exe.clone(),
        workload: d.workload.map(|w| w.name().to_owned()),
        source: d.source.map(|s| s.as_str().to_owned()),
        confidence: d.confidence,
        asked: d.asked,
        profile: None,
        note: None,
    }
}

fn push_log(v: &mut View, e: LogEntry) {
    if v.decisions.len() == MAX_LOG {
        v.decisions.remove(0);
    }
    v.decisions.push(e);
}

/// Puts the rule's profile in place through the engine. `Err` says why it wasn't.
fn apply(
    c: &Change,
    cache: &crate::runtime::Cache,
    profiles: &[Profile],
    rt: &tokio::runtime::Runtime,
    handle: &Handle,
    layer: &mut SlowLayer,
    now: Duration,
) -> Result<bool, String> {
    if cache.active_profile == c.profile {
        layer.already_active(c);
        return Ok(false);
    }
    let find = |name: &str| profiles.iter().find(|p| p.name == name);
    let next = find(c.profile).ok_or_else(|| format!("no profile '{}' is saved", c.profile))?;
    let active = find(&cache.active_profile)
        .ok_or_else(|| format!("the active profile '{}' is unknown", cache.active_profile))?;
    if let Some(why) = why_not_switch(next, active, &cache.snapshot) {
        return Err(why);
    }
    rt.block_on(handle.set_profile(next.clone()))
        .map_err(|e| format!("the engine refused '{}': {e:?}", c.profile))?;
    layer.applied(now, c);
    Ok(true)
}

fn run(parts: SlowParts, stopping: &AtomicBool) {
    let SlowParts {
        handle,
        profiles,
        history,
        foreground,
        mut hosted,
        mut local,
        shared,
        period,
    } = parts;
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    else {
        shared.update(|v| v.crashed = true);
        return;
    };
    let mut layer = SlowLayer::new(load_known(&history));
    let began = Instant::now();
    let mut last_note: Option<String> = None;
    while !stopping.load(Ordering::SeqCst) {
        std::thread::sleep(period);
        for (exe, workload) in shared.take_answers() {
            layer.set_known(
                &exe,
                Known {
                    workload,
                    source: Source::User,
                    confidence: None,
                },
            );
        }
        let cache = handle.cache();
        let seen = foreground();
        shared.update(|v| v.foreground = seen.is_some());
        if !cache.status.engine_running || cache.status.agent != AgentState::Running {
            continue;
        }
        let obs = observation(&cache.snapshot, cache.snapshot_age, seen);
        let now = began.elapsed();
        let known_games = layer.known_games();
        // Watches for itself while the thread is blocked in a call.
        let cancel = || {
            if stopping.load(Ordering::SeqCst) {
                return true;
            }
            let c = handle.cache();
            if c.status.agent != AgentState::Running {
                return true;
            }
            let o = observation(&c.snapshot, c.snapshot_age, foreground());
            let exe = o
                .foreground
                .as_ref()
                .and_then(|f| f.exe.as_deref())
                .and_then(clean_exe);
            let known = exe.as_deref().is_some_and(|e| {
                known_games.contains(e) || builtin_workload(e) == Some(Workload::Gaming)
            });
            game_like(&o, known)
        };
        let step = layer.step(
            now,
            &obs,
            hosted
                .as_mut()
                .map(|p| p.as_mut() as &mut dyn DecisionProvider),
            local
                .as_mut()
                .map(|p| p.as_mut() as &mut dyn DecisionProvider),
            &cancel,
        );
        for (exe, k) in layer.take_learned() {
            let saved = history
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .save_exe_workload(&exe, k.workload.name(), k.source.as_str(), k.confidence);
            if saved.is_err() {
                // The answer still counts for this run; it will be asked again after a restart.
                shared.update(|v| {
                    push_log(
                        v,
                        LogEntry {
                            at: unix_now(),
                            exe: exe.clone(),
                            workload: None,
                            source: None,
                            confidence: None,
                            asked: false,
                            profile: None,
                            note: Some("the answer could not be saved".into()),
                        },
                    );
                });
            }
        }
        let mut logged = step.decision.as_ref().map(entry);
        if let Some(c) = &step.change {
            match apply(c, &cache, &profiles, &rt, &handle, &mut layer, now) {
                Ok(changed) => {
                    last_note = None;
                    if changed {
                        let mut e = logged.take().unwrap_or_else(|| {
                            entry(&Decision {
                                exe: c.exe.clone(),
                                workload: Some(c.workload),
                                source: Some(c.source),
                                confidence: c.confidence,
                                asked: false,
                            })
                        });
                        e.profile = Some(c.profile.into());
                        logged = Some(e);
                    }
                }
                Err(why) => {
                    if last_note.as_deref() != Some(&why) {
                        let mut e = logged.take().unwrap_or_else(|| {
                            entry(&Decision {
                                exe: c.exe.clone(),
                                workload: Some(c.workload),
                                source: Some(c.source),
                                confidence: c.confidence,
                                asked: false,
                            })
                        });
                        e.note = Some(why.clone());
                        logged = Some(e);
                    }
                    last_note = Some(why);
                }
            }
        }
        let questions: Vec<QuestionView> = layer
            .questions()
            .iter()
            .map(|q| QuestionView {
                exe: q.exe.clone(),
                suggested_workload: q.suggestion.map(|s| s.workload.name().to_owned()),
                suggested_by: q.suggestion.map(|s| s.source.as_str().to_owned()),
                confidence: q.suggestion.map(|s| s.confidence),
            })
            .collect();
        shared.update(|v| {
            v.questions = questions;
            if let Some(e) = logged {
                push_log(v, e);
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::keychain::MemoryKeys;
    use crate::runtime::{Daemon, Options};
    use thermaliq_common::{FanKind, FanReading, MetricReading, SensorReading};
    use thermaliq_hal::{MockHardwareLayer, Tick};

    fn toml(name: &str, fan: &str) -> Profile {
        Profile::from_toml(&format!(
            "name = \"{name}\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"{fan}\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n"
        ))
        .unwrap()
    }

    fn snapshot() -> HardwareSnapshot {
        HardwareSnapshot {
            sensors: vec![
                SensorReading {
                    id: "cpu".into(),
                    label: "cpu".into(),
                    kind: SensorKind::Cpu,
                    temp_celsius: 55.0,
                },
                SensorReading {
                    id: "gpu".into(),
                    label: "gpu".into(),
                    kind: SensorKind::Gpu,
                    temp_celsius: 65.0,
                },
                SensorReading {
                    id: "ssd".into(),
                    label: "ssd".into(),
                    kind: SensorKind::Other,
                    temp_celsius: 99.0,
                },
            ],
            fans: vec![FanReading {
                id: "f1".into(),
                label: "f1".into(),
                rpm: 1000,
                duty_percent: 30,
                kind: FanKind::Fan,
            }],
            metrics: vec![
                MetricReading {
                    id: "win_cpu_load".into(),
                    kind: MetricKind::CpuLoad,
                    value: 12.0,
                },
                MetricReading {
                    id: "nvml_gpu0_load".into(),
                    kind: MetricKind::GpuLoad,
                    value: 7.0,
                },
                MetricReading {
                    id: "nvml_gpu0_power".into(),
                    kind: MetricKind::GpuPower,
                    value: 40.0,
                },
            ],
        }
    }

    #[test]
    fn an_observation_reads_the_snapshot_and_unknown_stays_unknown() {
        let o = observation(&snapshot(), 0, None);
        assert_eq!(o.cpu_load_pct, Some(12.0));
        assert_eq!(o.gpu_load_pct, Some(7.0));
        assert_eq!(o.gpu_power_w, Some(40.0));
        assert_eq!(o.cpu_power_w, None, "not reported, not 0");
        assert_eq!(o.cpu_temp_c, Some(55.0));
        assert_eq!(o.gpu_temp_c, Some(65.0), "ssd isn't a CPU or GPU");
        assert_eq!(o.foreground, None);
    }

    #[test]
    fn two_gpus_read_as_the_busier_one_so_a_game_on_either_counts() {
        let mut s = snapshot();
        s.metrics.push(MetricReading {
            id: "nvml_gpu1_load".into(),
            kind: MetricKind::GpuLoad,
            value: 88.0,
        });
        assert_eq!(observation(&s, 0, None).gpu_load_pct, Some(88.0));
    }

    #[test]
    fn a_stale_snapshot_is_no_reading_at_all() {
        let o = observation(&snapshot(), 1, None);
        assert_eq!(
            (o.cpu_load_pct, o.gpu_load_pct, o.gpu_temp_c),
            (None, None, None)
        );
    }

    // ---- the thread, against a real engine on mock hardware ----

    struct Rig {
        daemon: Daemon,
        slow: Option<SlowThread>,
        shared: SlowShared,
        fg: Arc<Mutex<Option<Foreground>>>,
        db: Arc<Mutex<HistoryDb>>,
    }

    /// `profiles` are saved; the first is active. The slow thread steps every millisecond,
    /// but its logic's clock is still real seconds, so cached exes are what act at once.
    fn rig(profiles: Vec<Profile>, known: &[(&str, &str)]) -> Rig {
        let cpu = Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 40.0)]);
        let hal = Arc::new(MockHardwareLayer::new(&["f1"], vec![cpu]));
        let db = Arc::new(Mutex::new(HistoryDb::in_memory().unwrap()));
        for (exe, w) in known {
            db.lock()
                .unwrap()
                .save_exe_workload(exe, w, "user", None)
                .unwrap();
        }
        let opts = Options {
            tick: Duration::from_millis(5),
            prune_every: Duration::from_secs(3600),
        };
        let daemon = Daemon::start(hal, &profiles[0], profiles.clone(), db.clone(), opts);
        let fg: Arc<Mutex<Option<Foreground>>> = Arc::default();
        let read = fg.clone();
        let shared = SlowShared::new(Arc::new(MemoryKeys::default()), "not configured".into());
        let slow = SlowThread::start(SlowParts {
            handle: daemon.handle(),
            profiles,
            history: db.clone(),
            foreground: Arc::new(move || read.lock().unwrap().clone()),
            hosted: None,
            local: None,
            shared: shared.clone(),
            period: Duration::from_millis(1),
        });
        Rig {
            daemon,
            slow: Some(slow),
            shared,
            fg,
            db,
        }
    }

    fn show(r: &Rig, exe: &str) {
        *r.fg.lock().unwrap() = Some(Foreground {
            exe: Some(exe.into()),
            fullscreen: false,
        });
    }

    fn wait_until(mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn active(r: &Rig) -> String {
        r.daemon.handle().cache().active_profile
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            if let Some(s) = self.slow.take() {
                s.stop();
            }
        }
    }

    #[test]
    fn a_cached_exe_switches_the_profile_by_the_rule_through_the_engine() {
        let r = rig(
            vec![toml("balanced", "f1"), toml("gaming", "f1")],
            &[("game.exe", "gaming")],
        );
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        show(&r, "game.exe");
        wait_until(|| active(&r) == "gaming");
        let log = r.shared.view().decisions;
        let d = log.iter().find(|d| d.exe == "game.exe").unwrap();
        assert_eq!(d.source.as_deref(), Some("user"));
        assert_eq!(d.workload.as_deref(), Some("gaming"));
    }

    #[test]
    fn a_profile_that_would_release_the_real_fan_is_refused_and_says_why() {
        let r = rig(
            vec![toml("balanced", "f1"), toml("gaming", "gpu1")],
            &[("game.exe", "gaming")],
        );
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        show(&r, "game.exe");
        wait_until(|| {
            r.shared
                .view()
                .decisions
                .iter()
                .any(|d| d.note.as_deref().is_some_and(|n| n.contains("f1")))
        });
        assert_eq!(active(&r), "balanced");
    }

    #[test]
    fn a_rule_profile_that_does_not_exist_changes_nothing_and_says_so() {
        let r = rig(vec![toml("balanced", "f1")], &[("game.exe", "gaming")]);
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        show(&r, "game.exe");
        wait_until(|| {
            r.shared
                .view()
                .decisions
                .iter()
                .any(|d| d.note.as_deref().is_some_and(|n| n.contains("gaming")))
        });
        assert_eq!(active(&r), "balanced");
    }

    #[test]
    fn a_paused_agent_changes_nothing() {
        let r = rig(
            vec![toml("balanced", "f1"), toml("gaming", "f1")],
            &[("game.exe", "gaming")],
        );
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(r.daemon.handle().pause_agent())
            .unwrap();
        show(&r, "game.exe");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(active(&r), "balanced");
        assert!(r.shared.view().decisions.is_empty());
    }

    #[test]
    fn the_users_answer_is_saved_and_applied() {
        let r = rig(vec![toml("balanced", "f1"), toml("gaming", "f1")], &[]);
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        show(&r, "mystery.exe");
        std::thread::sleep(Duration::from_millis(50));
        r.shared
            .answer("mystery.exe", Workload::Gaming, &r.db)
            .unwrap();
        wait_until(|| active(&r) == "gaming");
        let saved = r.db.lock().unwrap().exe_workloads().unwrap();
        assert_eq!(saved["mystery.exe"].0, "gaming");
        assert_eq!(saved["mystery.exe"].1, "user");
    }

    #[test]
    fn stopping_is_prompt_and_nothing_happens_without_a_foreground_app() {
        let mut r = rig(vec![toml("balanced", "f1")], &[]);
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        std::thread::sleep(Duration::from_millis(50));
        assert!(r.shared.view().decisions.is_empty());
        assert!(!r.shared.view().foreground);
        let t0 = Instant::now();
        r.slow.take().unwrap().stop();
        assert!(t0.elapsed() < Duration::from_secs(2));
    }
}

//! The slow layer's threads (SPEC §7.2, §4.1). The worker is separate from the fast layer,
//! because a local call can take a minute and hosted can take 2 s; the supervisor picks the
//! profile (the arbiter) and puts it in the engine through the same validated path as the API.
//! Both only submit to the engine (S1). If they die, fans carry on untouched.

use crate::arbiter::Generations;
use crate::arbiter::{Offer, ProfileSource};
use crate::history::HistoryDb;
use crate::keychain::KeyStore;
use crate::runtime::{AgentState, Handle};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use thermaliq_agent::classifier::Workload;
use thermaliq_agent::decision::{
    builtin_workload, clean_exe, game_like, profile_for, why_not_switch, Decision,
    DecisionProvider, Foreground, Known, Observation, Settled, SlowLayer, Source, Step,
};
use thermaliq_common::{DaemonConfig, HardwareSnapshot, MetricKind, Profile, SensorKind};

const STEP: Duration = Duration::from_secs(1);
const MAX_LOG: usize = 100;
/// Not a refusal: it passes by itself, so it never gets a log entry of its own.
const WAITING: &str = "hardware is changing or degraded: waiting for a stable snapshot";

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

/// What the worker decided for the foreground exe, and the generations it started under.
struct Proposal {
    exe: String,
    profile: &'static str,
    gens: Generations,
}

/// What the API shares with the threads.
#[derive(Clone)]
pub struct SlowShared {
    view: Arc<Mutex<View>>,
    answers: Arc<Mutex<Vec<(String, Workload)>>>,
    proposal: Arc<Mutex<Option<Proposal>>>,
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
            proposal: Arc::default(),
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

    fn propose(&self, p: Proposal) {
        *self.proposal.lock().unwrap_or_else(PoisonError::into_inner) = Some(p);
    }

    fn take_proposal(&self) -> Option<Proposal> {
        self.proposal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn update(&self, f: impl FnOnce(&mut View)) {
        f(&mut self.view.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

/// What the threads need.
pub struct SlowParts {
    pub handle: Handle,
    pub profiles: Vec<Profile>,
    pub config: DaemonConfig,
    pub history: Arc<Mutex<HistoryDb>>,
    pub foreground: ForegroundFn,
    pub hosted: Option<Box<dyn DecisionProvider + Send>>,
    pub local: Option<Box<dyn DecisionProvider + Send>>,
    pub shared: SlowShared,
    pub period: Duration,
    /// How long the hardware must stay the same and healthy before a profile switch.
    pub stable_for: Duration,
}

impl SlowParts {
    pub const DEFAULT_PERIOD: Duration = STEP;
    pub const DEFAULT_STABLE: Duration = Settled::FOR;
}

/// Two threads. The worker classifies the foreground app and may block in a provider call for
/// a minute. The supervisor reads the foreground, renews the Agent's profile lease every period,
/// asks the arbiter which profile wins and puts it in the engine: app rules and schedules keep
/// working when the worker is slow, paused or dead.
pub struct SlowThread {
    stopping: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl SlowThread {
    pub fn start(parts: SlowParts) -> Self {
        parts.handle.configure_selection(&parts.config);
        let stopping = Arc::new(AtomicBool::new(false));
        let (worker, supervisor) = split(parts);
        let spawn = |name: &str, run: Box<dyn FnOnce() + Send>| {
            std::thread::Builder::new()
                .name(name.into())
                .spawn(run)
                .expect("spawn slow-layer thread")
        };
        let (flag, shared) = (stopping.clone(), worker.shared.clone());
        let worker = spawn(
            "thermaliq-slow",
            Box::new(move || {
                // Only this layer stops: the engine and the fast layer don't depend on it.
                if catch_unwind(AssertUnwindSafe(|| work(worker, &flag))).is_err() {
                    shared.update(|v| v.crashed = true);
                }
            }),
        );
        let (flag, shared, handle) = (
            stopping.clone(),
            supervisor.shared.clone(),
            supervisor.handle.clone(),
        );
        let supervisor = spawn(
            "thermaliq-arbiter",
            Box::new(move || {
                if catch_unwind(AssertUnwindSafe(|| supervise(supervisor, &flag))).is_err() {
                    shared.update(|v| v.crashed = true);
                }
                // However it ended, no lease outlives its supervisor.
                handle.arbiter().invalidate_agent();
            }),
        );
        Self {
            stopping,
            threads: vec![worker, supervisor],
        }
    }

    /// Asks the threads to stop, which also cancels a local call. Doesn't wait.
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
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

struct Worker {
    handle: Handle,
    history: Arc<Mutex<HistoryDb>>,
    foreground: ForegroundFn,
    hosted: Option<Box<dyn DecisionProvider + Send>>,
    local: Option<Box<dyn DecisionProvider + Send>>,
    shared: SlowShared,
    period: Duration,
}

struct Supervisor {
    handle: Handle,
    profiles: Vec<Profile>,
    history: Arc<Mutex<HistoryDb>>,
    foreground: ForegroundFn,
    shared: SlowShared,
    period: Duration,
    stable_for: Duration,
}

fn split(p: SlowParts) -> (Worker, Supervisor) {
    (
        Worker {
            handle: p.handle.clone(),
            history: p.history.clone(),
            foreground: p.foreground.clone(),
            hosted: p.hosted,
            local: p.local,
            shared: p.shared.clone(),
            period: p.period,
        },
        Supervisor {
            handle: p.handle,
            profiles: p.profiles,
            history: p.history,
            foreground: p.foreground,
            shared: p.shared,
            period: p.period,
            stable_for: p.stable_for,
        },
    )
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

/// The profile a cached or built-in answer calls for, with no provider call.
fn known_profile(db: &Mutex<HistoryDb>, exe: &str) -> Option<&'static str> {
    let saved = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .exe_workloads()
        .unwrap_or_default();
    let workload = saved
        .get(exe)
        .and_then(|(w, _, _)| Workload::from_name(w))
        .or_else(|| builtin_workload(exe))?;
    Some(profile_for(workload))
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

/// When it is, whether the hardware has been steady for long enough, and every fan it has ever
/// listed.
struct Gate<'a> {
    stable: bool,
    ever_seen: &'a HashSet<String>,
}

/// Puts a profile in place through the engine. `Ok(false)`: it already was. `Err` says why not.
fn apply_named(
    profile_name: &str,
    cache: &crate::runtime::Cache,
    profiles: &[Profile],
    rt: &tokio::runtime::Runtime,
    handle: &Handle,
    gate: &Gate,
) -> Result<bool, String> {
    if cache.active_profile == profile_name {
        return Ok(false);
    }
    // A restarting or degraded backend may be missing fans that come back a moment later.
    if !gate.stable {
        return Err(WAITING.into());
    }
    let find = |name: &str| profiles.iter().find(|p| p.name == name);
    let next = find(profile_name).ok_or_else(|| format!("no profile '{profile_name}' is saved"))?;
    let active = find(&cache.active_profile)
        .ok_or_else(|| format!("the active profile '{}' is unknown", cache.active_profile))?;
    if let Some(why) = why_not_switch(next, active, &cache.snapshot, gate.ever_seen) {
        return Err(why);
    }
    rt.block_on(handle.apply_profile(next.clone()))
        .map_err(|e| format!("the engine refused '{profile_name}': {e:?}"))?;
    Ok(true)
}

/// The classifier. Its answer is a proposal, never a profile change: the supervisor decides.
fn work(w: Worker, stopping: &AtomicBool) {
    let Worker {
        handle,
        history,
        foreground,
        mut hosted,
        mut local,
        shared,
        period,
    } = w;
    let mut layer = SlowLayer::new(load_known(&history));
    let began = Instant::now();
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
        if !cache.status.engine_running {
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
        // The answer is only good for the app and settings it was asked under.
        let started = handle.arbiter().generations();
        let step = if cache.status.agent == AgentState::Running {
            layer.step(
                now,
                &obs,
                hosted
                    .as_mut()
                    .map(|p| p.as_mut() as &mut dyn DecisionProvider),
                local
                    .as_mut()
                    .map(|p| p.as_mut() as &mut dyn DecisionProvider),
                &cancel,
            )
        } else {
            Step::default()
        };
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
        if let Some(c) = &step.change {
            shared.propose(Proposal {
                exe: c.exe.clone(),
                profile: c.profile,
                gens: started,
            });
        }
        let logged = step.decision.as_ref().map(entry);
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

/// Reads the foreground, keeps the Agent's profile lease alive, and puts the winning profile in
/// the engine. Nothing here calls a provider, so a slow classifier cannot let a lease lapse.
fn supervise(s: Supervisor, stopping: &AtomicBool) {
    let Supervisor {
        handle,
        profiles,
        history,
        foreground,
        shared,
        period,
        stable_for,
    } = s;
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    else {
        shared.update(|v| v.crashed = true);
        return;
    };
    let mut settled = Settled::default();
    let mut ever_seen: HashSet<String> = HashSet::new();
    // The last accepted decision per exe, re-offered every period with no provider call.
    let mut accepted: HashMap<String, &'static str> = HashMap::new();
    let mut probed: HashSet<String> = HashSet::new();
    let mut config_gen = handle.arbiter().generations().config;
    let mut last_note: Option<String> = None;
    while !stopping.load(Ordering::SeqCst) {
        std::thread::sleep(period);
        let cache = handle.cache();
        let now = handle.elapsed();
        // Before anything can `continue`: a pause or a stopped engine is part of what changed.
        ever_seen.extend(cache.snapshot.fans.iter().map(|f| f.id.clone()));
        settled.observe(
            now,
            &cache.snapshot,
            &cache.status.unavailable_sources,
            cache.status.degraded || !cache.status.engine_running,
            cache.snapshot_age,
        );
        if !cache.status.engine_running {
            continue;
        }
        let exe = foreground()
            .as_ref()
            .and_then(|f| f.exe.as_deref())
            .and_then(clean_exe);
        let agent_up = cache.status.agent == AgentState::Running && !shared.view().crashed;
        // A target is usable when it exists and the engine would not release a fan for it.
        // Until the snapshot is steady a missing fan may just not be listed yet (the bridge
        // starts late), so the target is skipped quietly; `apply_named` waits for it too.
        let steady = settled.is_settled(now, stable_for);
        let skipped = RefCell::new(Vec::<String>::new());
        let usable = |name: &str| -> bool {
            if name == cache.active_profile {
                return true;
            }
            let find = |n: &str| profiles.iter().find(|p| p.name == n);
            let Some(next) = find(name) else {
                skipped
                    .borrow_mut()
                    .push(format!("no profile '{name}' is saved"));
                return false;
            };
            let Some(active) = find(&cache.active_profile) else {
                return true; // `apply_named` reports an unknown active profile
            };
            match why_not_switch(next, active, &cache.snapshot, &ever_seen) {
                Some(why) => {
                    if steady {
                        skipped.borrow_mut().push(why);
                    }
                    false
                }
                None => true,
            }
        };
        let alert_wanted = handle.alert_profile();
        let selection = {
            let mut a = handle.arbiter();
            a.set_alert(alert_wanted);
            if a.generations().config != config_gen {
                config_gen = a.generations().config;
                accepted.clear();
                probed.clear();
            }
            a.set_foreground(exe.as_deref());
            if agent_up {
                if let Some(p) = shared.take_proposal() {
                    if matches!(
                        a.offer_intent(now, p.profile, p.gens, &usable),
                        Offer::Activated | Offer::Renewed
                    ) {
                        accepted.insert(p.exe, p.profile);
                    }
                }
                if let Some(exe) = &exe {
                    if !accepted.contains_key(exe) && probed.insert(exe.clone()) {
                        if let Some(p) = known_profile(&history, exe) {
                            accepted.insert(exe.clone(), p);
                        }
                    }
                    if let Some(&profile) = accepted.get(exe) {
                        let now_gens = a.generations();
                        a.offer_intent(now, profile, now_gens, &usable);
                    }
                }
            } else {
                a.invalidate_agent();
            }
            a.select(now, &usable)
        };
        let mut logged: Option<LogEntry> = None;
        if let Some(why) = skipped.into_inner().into_iter().next() {
            if last_note.as_deref() != Some(&why) {
                note(&shared, &exe, &selection.effective, &why);
                last_note = Some(why);
            }
        }
        if selection.effective == cache.active_profile {
            handle.arbiter().applied(&selection);
        } else {
            let gate = Gate {
                stable: steady,
                ever_seen: &ever_seen,
            };
            match apply_named(&selection.effective, &cache, &profiles, &rt, &handle, &gate) {
                Ok(changed) => {
                    last_note = None;
                    handle.arbiter().record_applied(&selection, changed, now);
                    if changed && selection.source == ProfileSource::Agent {
                        let profile = selection.effective.clone();
                        let exe = exe.clone().unwrap_or_default();
                        shared.update(|v| {
                            // The decision that asked for it shows what it put in place.
                            if let Some(e) =
                                v.decisions.iter_mut().rev().find(|e| {
                                    e.exe == exe && e.profile.is_none() && e.note.is_none()
                                })
                            {
                                e.profile = Some(profile);
                            }
                        });
                    } else if changed {
                        logged = Some(LogEntry {
                            at: unix_now(),
                            exe: exe.clone().unwrap_or_default(),
                            workload: None,
                            source: Some(selection.source.as_str().into()),
                            confidence: None,
                            asked: false,
                            profile: Some(selection.effective.clone()),
                            note: selection.rule_id.clone().map(|r| format!("rule {r}")),
                        });
                    }
                }
                Err(why) => {
                    if last_note.as_deref() != Some(&why) {
                        note(&shared, &exe, &selection.effective, &why);
                    }
                    last_note = Some(why);
                }
            }
        }
        if let Some(e) = logged {
            shared.update(|v| push_log(v, e));
        }
    }
}

/// Says why a profile was not put in place: on the decision that asked for it if there is one,
/// else as an entry of its own.
fn note(shared: &SlowShared, exe: &Option<String>, profile: &str, why: &str) {
    let exe = exe.clone().unwrap_or_default();
    shared.update(|v| {
        let open = v
            .decisions
            .iter_mut()
            .rev()
            .find(|e| e.exe == exe && e.profile.is_none() && e.note.is_none());
        match open {
            Some(e) => e.note = Some(why.into()),
            None if why == WAITING => {}
            None => push_log(
                v,
                LogEntry {
                    at: unix_now(),
                    exe,
                    workload: None,
                    source: Some("profile-rule".into()),
                    confidence: None,
                    asked: false,
                    profile: Some(profile.into()),
                    note: Some(why.into()),
                },
            ),
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::keychain::MemoryKeys;
    use crate::runtime::{Daemon, Options};
    use thermaliq_common::{FanKind, FanReading, MetricReading, SensorReading};
    use thermaliq_hal::{MockHardwareLayer, Tick};

    /// The wait for a stable snapshot, short for the tests (10 s in the service).
    const STABLE: Duration = Duration::from_millis(400);

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
        rig_on(hal, profiles, known)
    }

    fn rig_with_config(
        profiles: Vec<Profile>,
        known: &[(&str, &str)],
        config: DaemonConfig,
    ) -> Rig {
        let cpu = Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 40.0)]);
        let hal = Arc::new(MockHardwareLayer::new(&["f1"], vec![cpu]));
        rig_on_with_config(hal, profiles, known, config)
    }

    /// Hardware that can lose `f2` (a bridge restarting) or report a fault, as the tests ask.
    struct Flaky {
        inner: MockHardwareLayer,
        hide_f2: Arc<AtomicBool>,
        fault: Arc<AtomicBool>,
    }

    impl thermaliq_hal::HardwareLayer for Flaky {
        fn snapshot(&self) -> Result<HardwareSnapshot, thermaliq_hal::HalError> {
            let mut s = self.inner.snapshot()?;
            if self.hide_f2.load(Ordering::SeqCst) {
                s.fans.retain(|f| f.id != "f2");
            }
            Ok(s)
        }
        fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), thermaliq_hal::HalError> {
            self.inner.set_fan_duty(fan, duty)
        }
        fn restore_auto(&self, fan: &str) -> Result<(), thermaliq_hal::HalError> {
            self.inner.restore_auto(fan)
        }
        fn capabilities(&self) -> thermaliq_hal::Capabilities {
            self.inner.capabilities()
        }
        fn degraded_reasons(&self) -> Vec<String> {
            if self.fault.load(Ordering::SeqCst) {
                vec!["lhm-bridge: restarting".into()]
            } else {
                vec![]
            }
        }
    }

    fn flaky() -> (Arc<Flaky>, Arc<AtomicBool>, Arc<AtomicBool>) {
        let cpu = Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 40.0)]);
        let (hide_f2, fault) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let hal = Arc::new(Flaky {
            inner: MockHardwareLayer::new(&["f1", "f2"], vec![cpu]),
            hide_f2: hide_f2.clone(),
            fault: fault.clone(),
        });
        (hal, hide_f2, fault)
    }

    fn toml2(name: &str, fans: &[&str]) -> Profile {
        let controls: String = fans
            .iter()
            .map(|f| format!("[[controls]]\nfan = \"{f}\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n"))
            .collect();
        Profile::from_toml(&format!(
            "name = \"{name}\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n{controls}"
        ))
        .unwrap()
    }

    #[test]
    fn a_bridge_restart_mid_idle_never_causes_a_switch_that_drops_a_profile_fan() {
        let (hal, hide_f2, _) = flaky();
        let r = rig_on(
            hal,
            vec![toml2("balanced", &["f1", "f2"]), toml2("gaming", &["f1"])],
            &[("game.exe", "gaming")],
        );
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        // Long enough for the stable wait, so only the restart itself can be what lets it switch.
        std::thread::sleep(STABLE * 3);
        hide_f2.store(true, Ordering::SeqCst); // the bridge is restarting: f2 is gone
        std::thread::sleep(STABLE);
        show(&r, "game.exe");
        std::thread::sleep(STABLE * 3);
        assert_eq!(active(&r), "balanced", "switched while f2 was missing");
        hide_f2.store(false, Ordering::SeqCst);
        std::thread::sleep(STABLE * 4);
        assert_eq!(active(&r), "balanced", "gaming would drop f2");
        assert!(
            r.shared
                .view()
                .decisions
                .iter()
                .any(|d| d.note.as_deref().is_some_and(|n| n.contains("f2"))),
            "the refusal names f2: {:?}",
            r.shared.view().decisions
        );
    }

    #[test]
    fn no_switch_while_degraded_or_newly_back_then_it_switches_once_stable() {
        let (hal, _, fault) = flaky();
        let r = rig_on(
            hal,
            vec![
                toml2("balanced", &["f1", "f2"]),
                toml2("gaming", &["f1", "f2"]),
            ],
            &[("game.exe", "gaming")],
        );
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        std::thread::sleep(STABLE * 3);
        fault.store(true, Ordering::SeqCst);
        std::thread::sleep(STABLE / 2);
        show(&r, "game.exe");
        std::thread::sleep(STABLE * 3);
        assert_eq!(active(&r), "balanced", "switched while degraded");
        fault.store(false, Ordering::SeqCst);
        std::thread::sleep(STABLE / 4);
        assert_eq!(active(&r), "balanced", "switched the moment it came back");
        wait_until(|| active(&r) == "gaming");
    }

    #[test]
    fn a_fresh_start_waits_for_a_stable_snapshot_before_the_first_switch() {
        let r = rig(
            vec![toml("balanced", "f1"), toml("gaming", "f1")],
            &[("game.exe", "gaming")],
        );
        show(&r, "game.exe");
        std::thread::sleep(STABLE / 3);
        assert_eq!(active(&r), "balanced");
        wait_until(|| active(&r) == "gaming");
    }

    /// M16 (e): after a restart the LHM fans are listed a moment late. Until the snapshot has
    /// been steady, a missing fan is not a reason to log a refusal.
    #[test]
    fn a_fan_not_listed_yet_after_a_restart_is_waited_for_not_logged_as_a_refusal() {
        let (hal, hide_f2, _) = flaky();
        hide_f2.store(true, Ordering::SeqCst); // the bridge has not listed f2 yet
        let r = rig_on(
            hal,
            vec![toml2("balanced", &["f1"]), toml2("gaming", &["f1", "f2"])],
            &[("game.exe", "gaming")],
        );
        show(&r, "game.exe");
        std::thread::sleep(STABLE / 3);
        let refusals = |r: &Rig| {
            r.shared
                .view()
                .decisions
                .iter()
                .filter(|d| d.note.as_deref().is_some_and(|n| n.contains("f2")))
                .count()
        };
        assert_eq!(refusals(&r), 0, "{:?}", r.shared.view().decisions);
        hide_f2.store(false, Ordering::SeqCst); // it shows up
        wait_until(|| active(&r) == "gaming");
        assert_eq!(refusals(&r), 0, "{:?}", r.shared.view().decisions);
    }

    fn rig_on(
        hal: Arc<dyn thermaliq_hal::HardwareLayer>,
        profiles: Vec<Profile>,
        known: &[(&str, &str)],
    ) -> Rig {
        let config = DaemonConfig {
            default_profile: profiles[0].name.clone(),
            ..DaemonConfig::default()
        };
        rig_on_with_config(hal, profiles, known, config)
    }

    fn rig_on_with_config(
        hal: Arc<dyn thermaliq_hal::HardwareLayer>,
        profiles: Vec<Profile>,
        known: &[(&str, &str)],
        config: DaemonConfig,
    ) -> Rig {
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
            config,
            history: db.clone(),
            foreground: Arc::new(move || read.lock().unwrap().clone()),
            hosted: None,
            local: None,
            shared: shared.clone(),
            period: Duration::from_millis(1),
            stable_for: STABLE,
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
        // Generous: a slow CI runner must not turn a wait into a failure.
        let end = Instant::now() + Duration::from_secs(10);
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

    fn source(r: &Rig) -> Option<ProfileSource> {
        r.daemon.handle().cache().selection.map(|s| s.source)
    }

    /// Rewritten from the reviewer's patch (it flipped the profile by hand, which a manual
    /// choice no longer does): pause drops the intent, resume registers it again.
    #[test]
    fn reviewer_agent_reactivations_obey_six_per_hour_limit() {
        let gaming = toml("gaming", "f1");
        let r = rig(
            vec![toml("balanced", "f1"), gaming.clone()],
            &[("game.exe", "gaming")],
        );
        let h = r.daemon.handle();
        let rt = tokio::runtime::Runtime::new().unwrap();
        show(&r, "game.exe");
        // The activation is counted once the supervisor has recorded it (`source`), not when the
        // engine applies the profile (`active`): pausing in between would lose the count.
        let by_agent = |r: &Rig| active(r) == "gaming" && source(r) == Some(ProfileSource::Agent);
        wait_until(|| by_agent(&r)); // effective activation 1
        for _ in 2..=6 {
            rt.block_on(h.pause_agent()).unwrap();
            wait_until(|| active(&r) == "balanced" && source(&r) == Some(ProfileSource::Fallback));
            rt.block_on(h.resume_agent()).unwrap();
            wait_until(|| by_agent(&r));
        }
        rt.block_on(h.pause_agent()).unwrap();
        wait_until(|| active(&r) == "balanced" && source(&r) == Some(ProfileSource::Fallback));
        rt.block_on(h.resume_agent()).unwrap();
        let ticks = h.cache().ticks;
        wait_until(|| h.cache().ticks > ticks + 100);
        assert_eq!(active(&r), "balanced", "the seventh activation got through");
        assert_eq!(source(&r), Some(ProfileSource::Fallback));

        // An intent whose profile is already effective is checked against the limit too.
        rt.block_on(h.set_profile(gaming)).unwrap();
        let ticks = h.cache().ticks;
        wait_until(|| h.cache().ticks > ticks + 100);
        assert_eq!(active(&r), "gaming");
        assert_eq!(
            source(&r),
            Some(ProfileSource::Fallback),
            "the agent's intent for gaming was still refused"
        );
    }
    #[test]
    fn reviewer_manual_profile_selection_is_not_undone_without_automation() {
        let manual = toml("manual", "f1");
        let r = rig(vec![toml("balanced", "f1"), manual.clone()], &[]);
        wait_until(|| r.daemon.handle().cache().ticks > 100);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(r.daemon.handle().pause_agent()).unwrap();
        rt.block_on(r.daemon.handle().set_profile(manual)).unwrap();
        let ticks = r.daemon.handle().cache().ticks;
        wait_until(|| r.daemon.handle().cache().ticks > ticks + 100);
        assert_eq!(
            active(&r),
            "manual",
            "manual choice was undone even with no app, schedule or agent request"
        );
    }

    #[test]
    fn reviewer_two_exes_with_same_workload_keep_agent_profile_ownership() {
        let r = rig(
            vec![toml("balanced", "f1"), toml("gaming", "f1")],
            &[("a.exe", "gaming"), ("b.exe", "gaming")],
        );
        show(&r, "a.exe");
        wait_until(|| active(&r) == "gaming");
        show(&r, "b.exe");
        let ticks = r.daemon.handle().cache().ticks;
        wait_until(|| r.daemon.handle().cache().ticks > ticks + 100);
        assert_eq!(
            active(&r),
            "gaming",
            "second exe lost ownership because its desired profile was already active"
        );
    }
    /// M16: an active `apply_profile` alert wins over the user's choice and an app rule.
    #[test]
    fn an_active_alert_puts_its_profile_in_place_through_the_engine() {
        let hot = Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 90.0)]);
        let hal = Arc::new(MockHardwareLayer::new(&["f1"], vec![hot]));
        let cfg = cfg(&rule("game.exe", "gaming"));
        let r = rig_on_with_config(
            hal,
            vec![
                toml("balanced", "f1"),
                toml("gaming", "f1"),
                toml("performance", "f1"),
            ],
            &[],
            cfg,
        );
        show(&r, "game.exe");
        wait_until(|| active(&r) == "gaming");
        r.daemon
            .handle()
            .configure_alerts(vec![thermaliq_common::AlertEntry {
                id: "hot".into(),
                enabled: true,
                sensor: "cpu".into(),
                threshold: 85.0,
                severity: thermaliq_common::Severity::Crit,
                action: thermaliq_common::AlertAction::ApplyProfile {
                    profile: "performance".into(),
                },
            }]);
        wait_until(|| active(&r) == "performance");
        assert_eq!(source(&r), Some(ProfileSource::Alert));
        let sel = r.daemon.handle().cache().selection.unwrap();
        assert_eq!(sel.rule_id.as_deref(), Some("hot"));
        assert_eq!(sel.chosen, "balanced", "the user's choice is untouched");
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
        // The profile switches before the decision is logged.
        wait_until(|| {
            r.shared
                .view()
                .decisions
                .iter()
                .any(|d| d.exe == "game.exe")
        });
        let log = r.shared.view().decisions;
        let d = log.iter().find(|d| d.exe == "game.exe").unwrap();
        assert_eq!(d.source.as_deref(), Some("user"));
        assert_eq!(d.workload.as_deref(), Some("gaming"));
    }

    fn cfg(extra: &str) -> DaemonConfig {
        DaemonConfig::from_toml(&format!("default_profile='balanced'\n{extra}")).unwrap()
    }

    const ALWAYS: &str = "[[schedule]]\nid='all-day'\ntime='00:00-24:00'\nprofile='scheduled'\n";

    fn rule(exe: &str, profile: &str) -> String {
        format!("[[app_rule]]\nid='r-{exe}'\nexe='{exe}'\nprofile='{profile}'\n")
    }

    fn quiet() -> Vec<Profile> {
        ["balanced", "gaming", "silent", "app", "scheduled"]
            .map(|n| toml(n, "f1"))
            .into()
    }

    #[test]
    fn a_config_app_rule_wins_while_its_exe_is_in_the_foreground_and_lets_go_after() {
        let r = rig_with_config(quiet(), &[], cfg(&rule("game.exe", "gaming")));
        show(&r, "GAME.EXE");
        // `selection` is recorded just after the engine applies the profile: wait for it.
        wait_until(|| active(&r) == "gaming" && source(&r) == Some(ProfileSource::App));
        let s = r.daemon.handle().cache().selection.unwrap();
        assert_eq!(
            (s.source, s.rule_id.as_deref(), s.chosen.as_str()),
            (ProfileSource::App, Some("r-game.exe"), "balanced")
        );
        show(&r, "notepad.exe");
        wait_until(|| active(&r) == "balanced" && source(&r) == Some(ProfileSource::Fallback));
    }

    #[test]
    fn an_app_rule_beats_the_agents_profile_intent_and_the_schedule() {
        let r = rig_with_config(
            quiet(),
            &[("game.exe", "gaming")],
            cfg(&format!("{}{ALWAYS}", rule("game.exe", "silent"))),
        );
        wait_until(|| active(&r) == "scheduled");
        show(&r, "game.exe");
        wait_until(|| active(&r) == "silent");
        std::thread::sleep(STABLE * 2);
        assert_eq!(active(&r), "silent", "the agent would have chosen gaming");
        assert_eq!(source(&r), Some(ProfileSource::App));
    }

    #[test]
    fn the_agents_intent_beats_the_schedule_and_the_schedule_beats_the_fallback() {
        let r = rig_with_config(quiet(), &[("game.exe", "gaming")], cfg(ALWAYS));
        wait_until(|| active(&r) == "scheduled");
        assert_eq!(source(&r), Some(ProfileSource::Schedule));
        show(&r, "game.exe");
        wait_until(|| active(&r) == "gaming");
        assert_eq!(source(&r), Some(ProfileSource::Agent));
    }

    #[test]
    fn app_rules_and_schedules_keep_working_while_the_agent_is_paused() {
        let r = rig_with_config(
            quiet(),
            &[("game.exe", "gaming")],
            cfg(&format!("{}{ALWAYS}", rule("editor.exe", "app"))),
        );
        let h = r.daemon.handle();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(h.pause_agent())
            .unwrap();
        wait_until(|| active(&r) == "scheduled");
        show(&r, "game.exe");
        std::thread::sleep(STABLE * 2);
        assert_eq!(active(&r), "scheduled", "the paused agent chose nothing");
        show(&r, "editor.exe");
        wait_until(|| active(&r) == "app");
    }

    #[test]
    fn a_crashed_worker_drops_the_intent_but_not_app_rules_or_schedules() {
        let r = rig_with_config(
            quiet(),
            &[("game.exe", "gaming")],
            cfg(&format!("{}{ALWAYS}", rule("editor.exe", "app"))),
        );
        show(&r, "game.exe");
        wait_until(|| active(&r) == "gaming");
        r.shared.update(|v| v.crashed = true); // what the worker's panic handler does
        wait_until(|| active(&r) == "scheduled");
        show(&r, "editor.exe");
        wait_until(|| active(&r) == "app");
    }

    #[test]
    fn an_intent_is_a_lease_that_the_supervisor_renews_without_the_worker() {
        let r = rig(
            vec![toml("balanced", "f1"), toml("gaming", "f1")],
            &[("game.exe", "gaming")],
        );
        show(&r, "game.exe");
        wait_until(|| active(&r) == "gaming");
        // The worker would decide again only at the one-minute cadence; the lease is 3 s.
        std::thread::sleep(crate::arbiter::LEASE + Duration::from_secs(1));
        assert_eq!(active(&r), "gaming");
        assert_eq!(source(&r), Some(ProfileSource::Agent));
    }

    fn propose(r: &Rig, exe: &str, gens: Generations) {
        r.shared.propose(Proposal {
            exe: exe.into(),
            profile: "gaming",
            gens,
        });
    }

    #[test]
    fn a_decision_started_under_an_old_foreground_or_config_is_discarded() {
        let r = rig(vec![toml("balanced", "f1"), toml("gaming", "f1")], &[]);
        let h = r.daemon.handle();
        wait_until(|| h.cache().ticks > 0);
        let old = h.arbiter().generations();
        show(&r, "x.exe");
        wait_until(|| h.arbiter().generations().foreground > old.foreground);
        propose(&r, "x.exe", old); // the provider answered after the app changed
        std::thread::sleep(STABLE * 2);
        assert_eq!(active(&r), "balanced", "late work from the old foreground");

        let current = h.arbiter().generations();
        h.configure_selection(&cfg(&rule("other.exe", "silent")));
        propose(&r, "x.exe", current); // started before the configuration was replaced
        std::thread::sleep(STABLE * 2);
        assert_eq!(
            active(&r),
            "balanced",
            "late work from the old configuration"
        );

        propose(&r, "x.exe", h.arbiter().generations());
        wait_until(|| active(&r) == "gaming");
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
    fn an_app_rule_that_would_release_a_real_fan_is_skipped_and_names_it() {
        let (hal, _, _) = flaky();
        let r = rig_on_with_config(
            hal,
            vec![toml2("balanced", &["f1", "f2"]), toml2("gaming", &["f1"])],
            &[],
            cfg(&rule("game.exe", "gaming")),
        );
        wait_until(|| r.daemon.handle().cache().ticks > 0);
        std::thread::sleep(STABLE * 3);
        show(&r, "game.exe");
        std::thread::sleep(STABLE * 3);
        assert_eq!(active(&r), "balanced");
        assert_eq!(
            source(&r),
            Some(ProfileSource::Fallback),
            "skipped, not applied"
        );
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

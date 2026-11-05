//! The agent's brain (SPEC §7.2 fast layer) for the daemon: which readings and fans belong
//! to which component, thermal models fitted from the last 24 h of history (refitted hourly),
//! and one `Optimiser` per fan. It decides; `runtime.rs` runs it on its own thread and only
//! submits requests to the engine (S1).
//!
//! Until M20d (fan-to-component mapping) a fan cools a component if its profile curve reads
//! that component's temperature sensor. Anything ambiguous binds nothing: no binding means
//! no sample, so the fan is left to its curve.
//! shortcut: GPU = the one NVML GPU; CPU = the one `Cpu` sensor. Multi-GPU and multi-sensor
//! machines get no agent until M20d maps them. History must be 1 sample/s (the default tick).

use crate::history::HistoryDb;
use crate::runtime::unix_now;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;
use thermaliq_agent::inputs::{Component, ComponentBinding, Sample};
use thermaliq_agent::model::ThermalModel;
use thermaliq_agent::optimiser::{target, Cooled, Optimiser, AHEAD};
use thermaliq_common::{FanKind, HardwareSnapshot, MetricKind, Profile, SensorKind};

/// Per-workload sliders need the classifier wiring (M20c); until then every fan uses this.
pub const DEFAULT_SLIDER: u8 = 50;
/// Models are fitted on this much history.
const WINDOW_S: i64 = 86_400;
/// Bindings are rediscovered, and fans without a model retried, this often.
const REFRESH: Duration = Duration::from_secs(60);
const REFIT: Duration = Duration::from_secs(3600);

/// What the engine thread hands the agent each fresh tick.
pub struct AgentTick {
    pub snapshot: HardwareSnapshot,
    /// The engine's `curve_output` for every fan that has one right now.
    pub curve: HashMap<String, u8>,
}

/// Decides what the agent asks of the fans this tick: `(fan, duty)`. `now` is monotonic.
pub trait Brain: Send + 'static {
    fn step(&mut self, tick: &AgentTick, now: Duration) -> Vec<(String, u8)>;
}

/// The metric of `kind` with id `preferred`, else the only metric of that kind, else none.
fn choose(snap: &HardwareSnapshot, kind: MetricKind, preferred: &str) -> Option<String> {
    let of_kind: Vec<_> = snap.metrics.iter().filter(|m| m.kind == kind).collect();
    of_kind
        .iter()
        .find(|m| m.id == preferred)
        .or(if of_kind.len() == 1 {
            of_kind.first()
        } else {
            None
        })
        .map(|m| m.id.clone())
}

/// Every component this machine can model, bound by explicit ids (never "the first gpu_power").
pub fn discover(snap: &HardwareSnapshot, profile: &Profile) -> Vec<ComponentBinding> {
    let curve_source = |curve: &str| {
        profile
            .curves
            .iter()
            .find(|c| c.id() == curve)
            .and_then(|c| {
                use thermaliq_common::Curve::{Graph, Linear, Trigger};
                match c {
                    Linear { source, .. } | Graph { source, .. } | Trigger { source, .. } => {
                        Some(source.as_str())
                    }
                    _ => None,
                }
            })
    };
    let fans_on = |temp_id: &str| -> Vec<String> {
        profile
            .controls
            .iter()
            .filter(|c| curve_source(&c.curve) == Some(temp_id))
            .map(|c| c.fan.clone())
            .collect()
    };
    let only = |pred: &dyn Fn(&str, SensorKind) -> bool| {
        let mut it = snap.sensors.iter().filter(|s| pred(&s.id, s.kind));
        match (it.next(), it.next()) {
            (Some(s), None) => Some(s.id.clone()),
            _ => None,
        }
    };
    let mut out = Vec::new();
    let gpu = only(&|id, _| {
        id.strip_prefix("nvml_gpu")
            .and_then(|r| r.strip_suffix("_temp"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    if let Some(temp_id) = gpu {
        let n = &temp_id["nvml_gpu".len()..temp_id.len() - "_temp".len()];
        let ids = (
            choose(snap, MetricKind::GpuLoad, &format!("nvml_gpu{n}_load")),
            choose(snap, MetricKind::GpuPower, &format!("nvml_gpu{n}_power")),
        );
        out.push((Component::Gpu, temp_id, ids));
    }
    if let Some(temp_id) = only(&|_, k| k == SensorKind::Cpu) {
        let ids = (
            choose(snap, MetricKind::CpuLoad, "win_cpu_load"),
            choose(snap, MetricKind::CpuPower, ""),
        );
        out.push((Component::Cpu, temp_id, ids));
    }
    out.into_iter()
        .filter_map(|(component, temp_id, (load, power))| {
            let fan_ids = fans_on(&temp_id);
            (!fan_ids.is_empty()).then_some(())?;
            Some(ComponentBinding {
                component,
                temp_id,
                load_id: load?,
                power_id: power?,
                fan_ids,
            })
        })
        .collect()
}

/// The binding's history since `since` as gap-free runs of 1-second samples. A second where
/// any reading is missing ends a run (SPEC §7.5: the fit needs consecutive ticks).
pub fn history_runs(
    db: &HistoryDb,
    b: &ComponentBinding,
    since: i64,
) -> rusqlite::Result<Vec<Vec<Sample>>> {
    let temps = db.sensor_rows_since(&b.temp_id, since)?;
    let load: HashMap<_, _> = db
        .metric_rows_since(&b.load_id, since)?
        .into_iter()
        .collect();
    let power: HashMap<_, _> = db
        .metric_rows_since(&b.power_id, since)?
        .into_iter()
        .collect();
    let mut duty: Vec<HashMap<i64, u8>> = Vec::new();
    for fan in &b.fan_ids {
        let rows = db.fan_rows_since(fan, since)?;
        duty.push(rows.into_iter().map(|(t, _, d)| (t, d)).collect());
    }
    let mut runs: Vec<Vec<Sample>> = Vec::new();
    let mut prev = None;
    // Several rows in one second (a tick under 1 s) collapse to the last.
    let temps: std::collections::BTreeMap<_, _> = temps.into_iter().collect();
    for (t, temp) in temps {
        let row = (|| {
            let mut sum = 0.0;
            for d in &duty {
                sum += f64::from(*d.get(&t)?);
            }
            Some(Sample {
                temp: f64::from(temp),
                duty: sum / duty.len() as f64,
                load: *load.get(&t)?,
                power: *power.get(&t)?,
            })
        })();
        let Some(s) = row else {
            prev = None;
            continue;
        };
        if prev != Some(t - 1) {
            runs.push(Vec::new());
        }
        if let Some(run) = runs.last_mut() {
            run.push(s);
        }
        prev = Some(t);
    }
    Ok(runs)
}

/// Fits on the longest gap-free run. `None` if there is none long enough, or the fit is
/// rejected (e.g. closed-loop history that implies a physically impossible sign).
pub fn fit_longest(runs: &[Vec<Sample>]) -> Option<ThermalModel> {
    let longest = runs.iter().max_by_key(|r| r.len())?;
    ThermalModel::fit(longest, AHEAD)
}

struct Bound {
    binding: ComponentBinding,
    model: Option<ThermalModel>,
}

fn same_ids(a: &ComponentBinding, b: &ComponentBinding) -> bool {
    (a.component, &a.temp_id, &a.load_id, &a.power_id, &a.fan_ids)
        == (b.component, &b.temp_id, &b.load_id, &b.power_id, &b.fan_ids)
}

pub struct Agent {
    history: Arc<Mutex<HistoryDb>>,
    profile: Arc<RwLock<Profile>>,
    slider: u8,
    bound: Vec<Bound>,
    optimisers: HashMap<String, Optimiser>,
    profile_name: String,
    last_refresh: Option<Duration>,
    last_refit: Option<Duration>,
}

impl Agent {
    pub fn new(history: Arc<Mutex<HistoryDb>>, profile: Arc<RwLock<Profile>>, slider: u8) -> Self {
        Self {
            history,
            profile,
            slider,
            bound: Vec::new(),
            optimisers: HashMap::new(),
            profile_name: String::new(),
            last_refresh: None,
            last_refit: None,
        }
    }

    fn fit(&self, b: &ComponentBinding) -> Option<ThermalModel> {
        let runs = history_runs(
            &self.history.lock().unwrap_or_else(PoisonError::into_inner),
            b,
            unix_now() - WINDOW_S,
        )
        .ok()?;
        fit_longest(&runs)
    }

    /// Rediscovers bindings (on a profile change and every minute) and fits models: all of
    /// them hourly, and any binding still without one at each rediscovery.
    fn refresh(&mut self, snap: &HardwareSnapshot, now: Duration) {
        let profile = self
            .profile
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let due = self
            .last_refresh
            .is_none_or(|l| now.saturating_sub(l) >= REFRESH);
        if !due && profile.name == self.profile_name {
            return;
        }
        self.last_refresh = Some(now);
        self.profile_name = profile.name.clone();
        let mut old = std::mem::take(&mut self.bound);
        let hourly = self
            .last_refit
            .is_none_or(|l| now.saturating_sub(l) >= REFIT);
        for binding in discover(snap, &profile) {
            let kept = old
                .iter_mut()
                .find(|o| same_ids(&o.binding, &binding))
                .and_then(|o| o.model.take());
            let model = match kept {
                Some(m) if !hourly => Some(m),
                _ => self.fit(&binding),
            };
            self.bound.push(Bound { binding, model });
        }
        if hourly {
            self.last_refit = Some(now);
        }
    }
}

impl Brain for Agent {
    fn step(&mut self, tick: &AgentTick, now: Duration) -> Vec<(String, u8)> {
        self.refresh(&tick.snapshot, now);
        let snap = &tick.snapshot;
        let mut out = Vec::new();
        for fan in &snap.fans {
            let Some(&curve) = tick.curve.get(&fan.id) else {
                continue;
            };
            let cooled: Vec<Cooled> = self
                .bound
                .iter()
                .filter(|b| b.binding.fan_ids.contains(&fan.id))
                .map(|b| Cooled {
                    sample: b.binding.sample(snap),
                    target: target(b.binding.component, self.slider),
                    model: b.model.as_ref(),
                })
                .collect();
            let opt = self.optimisers.entry(fan.id.clone()).or_default();
            if let Some(r) = opt.decide(now, curve, self.slider, fan.kind == FanKind::Pump, &cooled)
            {
                out.push((fan.id.clone(), r.duty));
            }
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use thermaliq_common::{FanReading, MetricReading, SensorReading};

    fn sensor(id: &str, kind: SensorKind, t: f32) -> SensorReading {
        SensorReading {
            id: id.into(),
            label: id.into(),
            kind,
            temp_celsius: t,
        }
    }

    fn metric(id: &str, kind: MetricKind, v: f64) -> MetricReading {
        MetricReading {
            id: id.into(),
            kind,
            value: v,
        }
    }

    fn fan(id: &str, duty: u8, kind: FanKind) -> FanReading {
        FanReading {
            id: id.into(),
            label: id.into(),
            rpm: 1000,
            duty_percent: duty,
            kind,
        }
    }

    /// f1 follows the GPU temperature, f2 is flat.
    fn profile() -> Profile {
        Profile::from_toml(
            "name = \"p\"\n[[curves]]\ntype = \"linear\"\nid = \"g\"\nsource = \"nvml_gpu0_temp\"\nmin_temp = 30.0\nmax_temp = 90.0\nmin_pwm = 20\nmax_pwm = 100\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"g\"\nstep_up_pct = 100\nstep_down_pct = 100\n[[controls]]\nfan = \"f2\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n",
        )
        .unwrap()
    }

    fn snap(gpu_temp: f32) -> HardwareSnapshot {
        HardwareSnapshot {
            sensors: vec![sensor("nvml_gpu0_temp", SensorKind::Gpu, gpu_temp)],
            fans: vec![fan("f1", 40, FanKind::Fan), fan("f2", 30, FanKind::Fan)],
            metrics: vec![
                metric("nvml_gpu0_load", MetricKind::GpuLoad, 50.0),
                metric("hwinfo_power", MetricKind::GpuPower, 100.0),
            ],
        }
    }

    #[test]
    fn binds_nvml_ids_the_sole_hwinfo_power_and_the_fans_whose_curve_reads_the_gpu() {
        let b = discover(&snap(60.0), &profile());
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].component, Component::Gpu);
        assert_eq!(
            (&b[0].temp_id, &b[0].load_id, &b[0].power_id),
            (
                &"nvml_gpu0_temp".to_string(),
                &"nvml_gpu0_load".to_string(),
                &"hwinfo_power".to_string()
            )
        );
        assert_eq!(
            b[0].fan_ids,
            ["f1"],
            "f2 is flat: it cools nothing we can name"
        );
    }

    #[test]
    fn nvml_power_beats_hwinfo_power() {
        let mut s = snap(60.0);
        s.metrics
            .push(metric("nvml_gpu0_power", MetricKind::GpuPower, 90.0));
        assert_eq!(discover(&s, &profile())[0].power_id, "nvml_gpu0_power");
    }

    #[test]
    fn anything_ambiguous_binds_nothing() {
        let p = profile();
        let mut two_powers = snap(60.0);
        two_powers
            .metrics
            .push(metric("other_power", MetricKind::GpuPower, 5.0));
        assert!(
            discover(&two_powers, &p).is_empty(),
            "which power is the GPU's?"
        );
        let mut no_power = snap(60.0);
        no_power.metrics.retain(|m| m.kind != MetricKind::GpuPower);
        assert!(discover(&no_power, &p).is_empty(), "unknown, not 0");
        let mut two_gpus = snap(60.0);
        two_gpus
            .sensors
            .push(sensor("nvml_gpu1_temp", SensorKind::Gpu, 50.0));
        assert!(discover(&two_gpus, &p).is_empty());
        let mut hot_spot = snap(60.0);
        hot_spot
            .sensors
            .push(sensor("hwinfo_hotspot", SensorKind::Gpu, 70.0));
        assert_eq!(discover(&hot_spot, &p).len(), 1, "only NVML names the GPU");
        let mut two_cpus = snap(60.0);
        two_cpus.sensors.push(sensor("c1", SensorKind::Cpu, 40.0));
        two_cpus.sensors.push(sensor("c2", SensorKind::Cpu, 41.0));
        assert_eq!(discover(&two_cpus, &p).len(), 1, "no CPU binding");
    }

    #[test]
    fn a_component_no_fan_follows_binds_nothing() {
        let p = Profile::from_toml(
            "name = \"p\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n",
        )
        .unwrap();
        assert!(discover(&snap(60.0), &p).is_empty());
    }

    /// One second per row; the plant cools with duty and heats with power.
    fn record(db: &mut HistoryDb, from: i64, to: i64, skip: &[i64]) {
        let mut temp = 50.0_f64;
        for t in from..to {
            let i = t - from;
            let duty = if (i / 90) % 2 == 0 { 30.0 } else { 70.0 };
            let power = 40.0 + ((i / 50) % 5) as f64 * 20.0;
            let load = power / 2.0;
            let eq = 30.0 + 0.2 * power - 0.2 * duty;
            temp += (eq - temp) * 0.08;
            if skip.contains(&t) {
                continue;
            }
            db.record_tick(
                t,
                &[sensor("nvml_gpu0_temp", SensorKind::Gpu, temp as f32)],
                &[fan("f1", duty as u8, FanKind::Fan)],
                &[
                    metric("nvml_gpu0_load", MetricKind::GpuLoad, load),
                    metric("hwinfo_power", MetricKind::GpuPower, power),
                ],
            )
            .unwrap();
        }
    }

    fn binding() -> ComponentBinding {
        discover(&snap(60.0), &profile()).remove(0)
    }

    #[test]
    fn history_splits_into_runs_at_a_missing_second() {
        let mut db = HistoryDb::in_memory().unwrap();
        record(&mut db, 1000, 1100, &[1040]);
        let runs = history_runs(&db, &binding(), 0).unwrap();
        assert_eq!(runs.iter().map(Vec::len).collect::<Vec<_>>(), [40, 59]);
        assert!(history_runs(&db, &binding(), 1090).unwrap()[0].len() == 10);
    }

    #[test]
    fn a_second_missing_any_one_reading_is_a_gap() {
        let mut db = HistoryDb::in_memory().unwrap();
        record(&mut db, 1000, 1030, &[]);
        // a fan row without sensor/metric rows can't make a sample either
        db.record_tick(1030, &[], &[fan("f1", 30, FanKind::Fan)], &[])
            .unwrap();
        record(&mut db, 1031, 1060, &[]);
        let runs = history_runs(&db, &binding(), 0).unwrap();
        assert_eq!(runs.iter().map(Vec::len).collect::<Vec<_>>(), [30, 29]);
    }

    #[test]
    fn a_fit_needs_a_long_enough_gap_free_run() {
        let mut short = HistoryDb::in_memory().unwrap();
        record(
            &mut short,
            1000,
            1300,
            &(1000..1300).step_by(40).collect::<Vec<_>>(),
        );
        let runs = history_runs(&short, &binding(), 0).unwrap();
        assert!(fit_longest(&runs).is_none(), "runs of 39 s are too short");
        let mut long = HistoryDb::in_memory().unwrap();
        record(&mut long, 1000, 2500, &[1100]);
        let runs = history_runs(&long, &binding(), 0).unwrap();
        let m = fit_longest(&runs).expect("fits the 1399 s run");
        let s = Sample {
            temp: 60.0,
            duty: 50.0,
            load: 30.0,
            power: 80.0,
        };
        assert!(m.lowering_is_safe(&s, AHEAD), "this plant cools with duty");
    }

    fn agent_with(db: HistoryDb) -> Agent {
        let p = Arc::new(RwLock::new(profile()));
        Agent::new(Arc::new(Mutex::new(db)), p, DEFAULT_SLIDER)
    }

    fn tick(temp: f32, curve: u8) -> AgentTick {
        AgentTick {
            snapshot: snap(temp),
            curve: [("f1".to_string(), curve), ("f2".to_string(), 30)].into(),
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn with_no_history_a_fan_is_raise_only() {
        let mut a = agent_with(HistoryDb::in_memory().unwrap());
        // slider 50 → GPU target 72.5
        assert!(
            a.step(&tick(60.0, 40), secs(0)).is_empty(),
            "cool, but no model: no lowering"
        );
        assert_eq!(
            a.step(&tick(80.0, 40), secs(1)),
            [("f1".to_string(), 60)],
            "over target: boost"
        );
    }

    #[test]
    fn fits_from_the_last_24_h_and_then_lowers_a_cool_fan() {
        let mut db = HistoryDb::in_memory().unwrap();
        let now = unix_now();
        record(&mut db, now - 2000, now - 500, &[]);
        let mut a = agent_with(db);
        a.slider = 0; // target 80 °C, lowers by the full 15
        assert_eq!(a.step(&tick(60.0, 40), secs(0)), [("f1".to_string(), 25)]);
        // f2 (flat) is never touched; a pump never is either
        let mut t = tick(60.0, 40);
        t.snapshot.fans[0].kind = FanKind::Pump;
        let mut b = agent_with(HistoryDb::in_memory().unwrap());
        assert!(b.step(&t, secs(0)).is_empty());
    }

    #[test]
    fn history_older_than_24_h_is_not_used() {
        let mut db = HistoryDb::in_memory().unwrap();
        let now = unix_now();
        record(&mut db, now - WINDOW_S - 3000, now - WINDOW_S - 500, &[]);
        let mut a = agent_with(db);
        a.slider = 0;
        assert!(a.step(&tick(60.0, 40), secs(0)).is_empty());
    }

    #[test]
    fn a_fan_without_a_model_is_retried_each_minute_and_not_before() {
        let db = Arc::new(Mutex::new(HistoryDb::in_memory().unwrap()));
        let p = Arc::new(RwLock::new(profile()));
        let mut a = Agent::new(db.clone(), p, 0);
        assert!(a.step(&tick(60.0, 40), secs(0)).is_empty());
        let now = unix_now();
        record(&mut db.lock().unwrap(), now - 2000, now - 500, &[]);
        assert!(a.step(&tick(60.0, 40), secs(30)).is_empty(), "not due yet");
        assert_eq!(
            a.step(&tick(60.0, 40), secs(61)).len(),
            1,
            "fitted at the minute"
        );
    }

    #[test]
    fn a_profile_switch_rediscovers_the_bindings() {
        let p = Arc::new(RwLock::new(profile()));
        let mut a = Agent::new(
            Arc::new(Mutex::new(HistoryDb::in_memory().unwrap())),
            p.clone(),
            50,
        );
        assert_eq!(a.step(&tick(80.0, 40), secs(0)).len(), 1);
        *p.write().unwrap() = Profile::from_toml(
            "name = \"flat\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n",
        )
        .unwrap();
        assert!(
            a.step(&tick(80.0, 40), secs(1)).is_empty(),
            "nothing follows the GPU any more"
        );
    }
}

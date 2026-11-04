use crate::{CurveEngine, FanControl, SensorEngine, Source, Temps};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use thermaliq_common::{Control, FanKind, HardwareSnapshot, Profile, SensorKind, SensorReading};
use thermaliq_hal::HardwareLayer;

/// SPEC S2 thresholds (°C) and hold time.
const CPU_TRIP: f32 = 95.0;
const GPU_TRIP: f32 = 90.0;
const CPU_RELEASE: f32 = 85.0;
const GPU_RELEASE: f32 = 80.0;
const RELEASE_HOLD: Duration = Duration::from_secs(30);
/// SPEC S3/S4: consecutive ticks before a fault is acted on.
const FAULT_TICKS: u8 = 3;
/// A pump is never written below this (SPEC §7: never stop a pump).
/// shortcut: one fixed floor for every pump; take it from the pump's calibration (M20a) when that exists.
const PUMP_MIN_DUTY: u8 = 20;

/// S2: trips on a hot CPU or GPU; releases after 30 s continuously cool.
/// A NaN reading or a missing CPU/GPU sensor (one seen while hot) is not cool, so it can't release the latch.
#[derive(Default)]
struct Latch {
    on: bool,
    /// CPU/GPU sensor ids seen while hot; all must be present and cool to release.
    watch: BTreeSet<String>,
    cool_since: Option<Duration>,
    /// Watched sensors that are missing, invalid or still above their release threshold.
    waiting: Vec<String>,
}

impl Latch {
    fn update(&mut self, sensors: &[SensorReading], now: Duration) -> bool {
        let of = |k: SensorKind| {
            sensors
                .iter()
                .filter(move |s| s.kind == k)
                .map(|s| s.temp_celsius)
        };
        let hot = of(SensorKind::Cpu).any(|t| t >= CPU_TRIP)
            || of(SensorKind::Gpu).any(|t| t >= GPU_TRIP);
        let cool = of(SensorKind::Cpu).all(|t| t < CPU_RELEASE)
            && of(SensorKind::Gpu).all(|t| t < GPU_RELEASE);
        if hot {
            self.on = true;
            self.watch.extend(
                sensors
                    .iter()
                    .filter(|s| matches!(s.kind, SensorKind::Cpu | SensorKind::Gpu))
                    .map(|s| s.id.clone()),
            );
        }
        let all_present = self
            .watch
            .iter()
            .all(|id| sensors.iter().any(|s| &s.id == id));
        if !self.on || !cool || !all_present {
            self.cool_since = None;
        } else if now.saturating_sub(*self.cool_since.get_or_insert(now)) >= RELEASE_HOLD {
            self.on = false;
            self.cool_since = None;
            self.watch.clear();
        }
        self.waiting = if self.on {
            let blocked = |id: &&String| match sensors.iter().find(|s| &s.id == *id) {
                Some(s) => {
                    let release = if s.kind == SensorKind::Gpu {
                        GPU_RELEASE
                    } else {
                        CPU_RELEASE
                    };
                    s.temp_celsius.is_nan() || s.temp_celsius >= release
                }
                None => true,
            };
            self.watch.iter().filter(blocked).cloned().collect()
        } else {
            Vec::new()
        };
        self.on
    }
}

struct Fan {
    id: String,
    curve: String,
    ctl: FanControl,
    /// Consecutive ticks the curve had / lacked valid input (S3).
    good: u8,
    bad: u8,
    lost: bool,
    /// Ramp state taken from the hardware or the last write.
    seeded: bool,
}

impl Fan {
    fn new(c: &Control) -> Self {
        Self {
            id: c.fan.clone(),
            curve: c.curve.clone(),
            ctl: FanControl::new(c),
            good: 0,
            bad: 0,
            lost: false,
            seeded: false,
        }
    }
}

/// The control loop for one profile. The only thing that writes to fans (S1).
/// Dropping it, including on panic, hands every touched fan back (S5).
pub struct Engine {
    hal: Arc<dyn HardwareLayer>,
    sensors: SensorEngine,
    curves: CurveEngine,
    fans: Vec<Fan>,
    controllable: HashSet<String>,
    touched: BTreeSet<String>,
    latch: Latch,
    snapshot_fails: u8,
    /// Last duty each fan accepted, to seed ramping across a profile switch.
    written: HashMap<String, u8>,
    /// The snapshot the last successful tick used, for the daemon's shared cache.
    last: Option<HardwareSnapshot>,
    /// Fans the last snapshot reported as pumps: never written 0 (SPEC §7).
    pumps: HashSet<String>,
}

impl Engine {
    pub fn new(hal: Arc<dyn HardwareLayer>, profile: &Profile) -> Self {
        Self {
            sensors: SensorEngine::new(profile),
            curves: CurveEngine::new(profile),
            fans: profile.controls.iter().map(Fan::new).collect(),
            controllable: hal.capabilities().controllable_fans.into_iter().collect(),
            hal,
            touched: BTreeSet::new(),
            latch: Latch::default(),
            snapshot_fails: 0,
            written: HashMap::new(),
            last: None,
            pumps: HashSet::new(),
        }
    }

    /// Rejects a profile that puts 0-RPM (`stop_percent`) on a fan the last snapshot called a
    /// pump. With no snapshot yet nothing is known; `write` still never stops a pump.
    pub fn check_profile(&self, profile: &Profile) -> Result<(), String> {
        match profile
            .controls
            .iter()
            .find(|c| c.stop_percent.is_some() && self.pumps.contains(&c.fan))
        {
            Some(c) => Err(format!(
                "fan '{}' is a pump and cannot use 0-RPM (stop_percent)",
                c.fan
            )),
            None => Ok(()),
        }
    }

    /// Swap to another profile without leaving the control loop: the latch, S3/S4 counters
    /// and ramp state carry over, and kept fans are not handed back. Fans the new
    /// profile drops are restored. Non-profile requests (override, agent, ...) carry over for kept fans (SPEC §4).
    pub fn set_profile(&mut self, profile: &Profile) {
        self.sensors = SensorEngine::new(profile);
        self.curves = CurveEngine::new(profile);
        let mut old = std::mem::take(&mut self.fans);
        self.fans = profile
            .controls
            .iter()
            .map(|c| {
                let mut fan = Fan::new(c);
                if let Some(i) = old.iter().position(|o| o.id == c.fan) {
                    let mut o = old.swap_remove(i);
                    fan.ctl.put_requests(o.ctl.take_requests());
                    (fan.good, fan.bad, fan.lost) = (o.good, o.bad, o.lost);
                }
                if let Some(&d) = self.written.get(&c.fan) {
                    fan.ctl.seed(d, d > 0);
                    fan.seeded = true;
                }
                fan
            })
            .collect();
        for gone in old {
            self.written.remove(&gone.id);
            if self.touched.remove(&gone.id) {
                let _ = self.hal.restore_auto(&gone.id);
            }
        }
    }

    /// S4: the daemon reports `degraded` while this is true.
    pub fn degraded(&self) -> bool {
        self.snapshot_fails >= FAULT_TICKS
    }

    pub fn latched(&self) -> bool {
        self.latch.on
    }

    /// While latched: the CPU/GPU sensors that are missing, invalid or still too hot to release.
    pub fn latch_waiting_on(&self) -> &[String] {
        &self.latch.waiting
    }

    /// The snapshot the last successful tick used; kept across failed ticks.
    pub fn last_snapshot(&self) -> Option<&HardwareSnapshot> {
        self.last.as_ref()
    }

    /// False when the latest tick's snapshot failed, so `last_snapshot` is old data.
    pub fn snapshot_fresh(&self) -> bool {
        self.snapshot_fails == 0
    }

    /// Route a request to a fan's `FanControl` (M4 priority rules apply).
    pub fn submit(
        &mut self,
        fan: &str,
        source: Source,
        duty: u8,
        expires_at: Option<Duration>,
    ) -> Result<(), String> {
        let f = self.fans.iter_mut().find(|f| f.id == fan);
        f.ok_or_else(|| format!("no control for fan '{fan}'"))?
            .ctl
            .submit(source, duty, expires_at)
    }

    /// Drops the manual override on a fan. `Ok(false)`: it had none.
    pub fn cancel_override(&mut self, fan: &str) -> Result<bool, String> {
        let f = self.fans.iter_mut().find(|f| f.id == fan);
        let f = f.ok_or_else(|| format!("no control for fan '{fan}'"))?;
        Ok(f.ctl.withdraw(Source::Override))
    }

    /// Why an override on this fan is not (yet) what the hardware gets: safety (S2–S4) wins.
    pub fn override_blocked_by(&self, fan: &str) -> Option<&'static str> {
        if self.latch.on {
            Some("latched")
        } else if self.degraded() {
            Some("degraded")
        } else if self.fans.iter().any(|f| f.id == fan && f.lost) {
            Some("sensor_lost")
        } else {
            None
        }
    }

    /// True if the active profile controls this fan and the hardware lets us write to it.
    pub fn can_control(&self, fan: &str) -> bool {
        self.controllable.contains(fan) && self.fans.iter().any(|f| f.id == fan)
    }

    /// One control step. `now` is monotonic time since the engine started.
    pub fn tick(&mut self, now: Duration) {
        let Ok(snap) = self.hal.snapshot() else {
            self.snapshot_fails = self.snapshot_fails.saturating_add(1);
            if self.degraded() {
                self.all_full();
            }
            return;
        };
        self.snapshot_fails = 0;
        self.last = Some(snap.clone());
        self.pumps = snap
            .fans
            .iter()
            .filter(|f| f.kind == FanKind::Pump)
            .map(|f| f.id.clone())
            .collect();
        for f in self.fans.iter_mut().filter(|f| !f.seeded) {
            if let Some(r) = snap.fans.iter().find(|r| r.id == f.id) {
                f.ctl.seed(r.duty_percent, r.rpm > 0);
                f.seeded = true;
            }
        }
        if self.latch.update(&snap.sensors, now) {
            self.all_full();
            return;
        }
        let real: Temps = snap
            .sensors
            .iter()
            .map(|s| (s.id.clone(), s.temp_celsius))
            .collect();
        let temps = self.sensors.evaluate(&real, now);
        for i in 0..self.fans.len() {
            let f = &mut self.fans[i];
            let duty = self.curves.duty(&f.curve, &temps, now);
            if duty.is_some() {
                (f.good, f.bad) = (f.good.saturating_add(1), 0);
                f.lost &= f.good < FAULT_TICKS;
            } else {
                (f.good, f.bad) = (0, f.bad.saturating_add(1));
                f.lost |= f.bad >= FAULT_TICKS;
            }
            let out = if f.lost {
                f.ctl.seed(100, true);
                Some(100)
            } else {
                match duty {
                    Some(d) => {
                        // Profile never needs an expiry, so submit cannot fail.
                        let _ = f.ctl.submit(Source::Profile, d, None);
                    }
                    None => {
                        f.ctl.withdraw(Source::Profile);
                    }
                }
                f.ctl.tick(now)
            };
            if let Some(d) = out {
                let id = f.id.clone();
                self.write(&id, d);
            }
        }
    }

    /// Safety output: every controllable fan to 100%, no ramp.
    fn all_full(&mut self) {
        let ids: Vec<String> = self.controllable.iter().cloned().collect();
        for id in ids {
            if let Some(f) = self.fans.iter_mut().find(|f| f.id == id) {
                f.ctl.seed(100, true);
                f.seeded = true;
            }
            self.write(&id, 100);
        }
    }

    /// The only call site of `set_fan_duty` (S1). Clamps (S7) and never touches a
    /// read-only fan. shortcut: a failed write is retried next tick, not reported; add logging with the daemon.
    fn write(&mut self, fan: &str, duty: u8) {
        if self.controllable.contains(fan) {
            self.touched.insert(fan.to_string());
            let mut duty = duty.min(100);
            if self.pumps.contains(fan) {
                duty = duty.max(PUMP_MIN_DUTY);
            }
            if self.hal.set_fan_duty(fan, duty).is_ok() {
                self.written.insert(fan.to_string(), duty);
            }
        }
    }
}

impl Drop for Engine {
    /// S5: one failed restore must not stop the others. shortcut: failures are not logged yet.
    fn drop(&mut self) {
        for fan in std::mem::take(&mut self.touched) {
            let _ = self.hal.restore_auto(&fan);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use thermaliq_hal::{MockHardwareLayer, Tick};

    const PROFILE: &str = r#"
name = "t"
[[curves]]
type = "linear"
id = "c"
source = "cpu"
min_temp = 30.0
max_temp = 90.0
min_pwm = 20
max_pwm = 100
[[controls]]
fan = "f1"
curve = "c"
step_up_pct = 100
step_down_pct = 100
[[controls]]
fan = "f2"
curve = "c"
step_up_pct = 100
step_down_pct = 100
"#;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn at(cpu: f32, gpu: f32) -> Tick {
        Tick::Temps(vec![
            ("cpu".into(), SensorKind::Cpu, cpu),
            ("gpu".into(), SensorKind::Gpu, gpu),
        ])
    }

    fn rig(fans: &[&str], script: Vec<Tick>) -> (Arc<MockHardwareLayer>, Engine) {
        let hal = Arc::new(MockHardwareLayer::new(fans, script));
        let p = Profile::from_toml(PROFILE).unwrap();
        (hal.clone(), Engine::new(hal, &p))
    }

    fn last_writes(hal: &MockHardwareLayer, n: usize) -> Vec<u8> {
        let w = hal.writes();
        w[w.len() - n..].iter().map(|(_, d)| *d).collect()
    }

    /// PROFILE with 0-RPM (stop at 30, start above 40) on both fans.
    fn zero_rpm_profile() -> Profile {
        let p = PROFILE.replace(
            "step_down_pct = 100\n",
            "step_down_pct = 100\nstart_percent = 40\nstop_percent = 30\n",
        );
        Profile::from_toml(&p).unwrap()
    }

    fn pump_rig(script: Vec<Tick>) -> (Arc<MockHardwareLayer>, Engine) {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], script).pump("f2"));
        let e = Engine::new(hal.clone(), &zero_rpm_profile());
        (hal, e)
    }

    #[test]
    fn a_pump_is_never_written_zero_even_when_its_profile_says_0_rpm() {
        let (hal, mut e) = pump_rig(vec![at(30.0, 30.0)]);
        for t in 0..5 {
            e.tick(secs(t));
        }
        let duty = |fan: &str| -> Vec<u8> {
            hal.writes()
                .iter()
                .filter(|(f, _)| f == fan)
                .map(|&(_, d)| d)
                .collect()
        };
        assert!(duty("f1").contains(&0), "the plain fan does stop");
        assert!(duty("f2").iter().all(|&d| d > 0), "{:?}", duty("f2"));
    }

    #[test]
    fn check_profile_rejects_0_rpm_on_a_pump_and_accepts_it_on_a_fan() {
        let (_, mut e) = pump_rig(vec![at(30.0, 30.0)]);
        let p = zero_rpm_profile();
        assert!(
            e.check_profile(&p).is_ok(),
            "no snapshot yet: nothing known"
        );
        e.tick(secs(0));
        let err = e.check_profile(&p).unwrap_err();
        assert!(err.contains("f2") && !err.contains("f1"), "{err}");
        assert!(e
            .check_profile(&Profile::from_toml(PROFILE).unwrap())
            .is_ok());
    }

    #[test]
    fn s2_97c_holds_100_for_30s() {
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 40.0), at(97.0, 40.0)]);
        e.tick(secs(0));
        for t in 1..=30 {
            e.tick(secs(t));
        }
        let w = hal.writes();
        assert!(w[2..].iter().all(|(_, d)| *d == 100), "{w:?}");
        assert!(e.latched());
    }

    #[test]
    fn s2_gpu_91c_trips_latch() {
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 91.0)]);
        e.tick(secs(0));
        assert!(e.latched());
        assert_eq!(last_writes(&hal, 2), [100, 100]);
    }

    #[test]
    fn s2_latch_releases_only_after_30s_below_thresholds() {
        let (_, mut e) = rig(&["f1", "f2"], vec![at(97.0, 40.0), at(80.0, 79.0)]);
        e.tick(secs(0));
        e.tick(secs(1)); // cool starts
        e.tick(secs(30));
        assert!(e.latched(), "29 s cool is not enough");
        e.tick(secs(31));
        assert!(!e.latched());
    }

    #[test]
    fn s2_latch_holds_when_one_sensor_stays_warm() {
        // CPU is cool but GPU sits at 85 (>= 80 release): never releases.
        let (_, mut e) = rig(&["f1", "f2"], vec![at(97.0, 40.0), at(60.0, 85.0)]);
        e.tick(secs(0));
        for t in 1..=100 {
            e.tick(secs(t));
        }
        assert!(e.latched());
    }

    #[test]
    fn s3_sensor_missing_3_ticks_gives_100() {
        let cpu_gone = Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, 40.0)]);
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 40.0), cpu_gone]);
        e.tick(secs(0));
        let before = hal.writes().len();
        e.tick(secs(1));
        e.tick(secs(2));
        assert_eq!(hal.writes().len(), before, "ticks 1-2 hold, no write");
        e.tick(secs(3));
        assert_eq!(last_writes(&hal, 2), [100, 100]);
    }

    #[test]
    fn s3_recovers_after_3_good_ticks() {
        let gone = || Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, 40.0)]);
        let (hal, mut e) = rig(&["f1", "f2"], vec![gone(), gone(), gone(), at(30.0, 40.0)]);
        for t in 0..5 {
            e.tick(secs(t));
        }
        assert_eq!(last_writes(&hal, 2), [100, 100], "still lost after 2 good");
        e.tick(secs(5));
        assert_eq!(last_writes(&hal, 2), [20, 20]);
    }

    #[test]
    fn s4_hal_failing_3_ticks_gives_all_fans_100() {
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 40.0), Tick::Fail]);
        e.tick(secs(0));
        e.tick(secs(1));
        e.tick(secs(2));
        assert!(!e.degraded());
        let before = hal.writes().len();
        e.tick(secs(3));
        assert!(e.degraded());
        assert_eq!(hal.writes().len(), before + 2);
        assert_eq!(last_writes(&hal, 2), [100, 100]);
    }

    #[test]
    fn s5_drop_restores_every_touched_fan_even_if_one_fails() {
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 40.0)]);
        hal.fail_restores("f1");
        e.tick(secs(0));
        drop(e);
        assert_eq!(hal.restores(), ["f1", "f2"]);
    }

    #[test]
    fn s5_panic_restores_every_touched_fan_even_if_one_fails() {
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 40.0)]);
        hal.fail_restores("f1");
        let r = catch_unwind(AssertUnwindSafe(move || {
            e.tick(secs(0));
            panic!("boom");
        }));
        assert!(r.is_err());
        assert_eq!(hal.restores(), ["f1", "f2"]);
    }

    #[test]
    fn s5_untouched_fans_are_not_restored() {
        let (hal, e) = rig(&["f1", "f2"], vec![at(40.0, 40.0)]);
        drop(e);
        assert!(hal.restores().is_empty());
    }

    #[test]
    fn failed_write_does_not_stop_other_fans() {
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 40.0)]);
        hal.fail_writes("f1");
        e.tick(secs(0));
        assert_eq!(hal.writes().len(), 2);
    }

    #[test]
    fn read_only_fan_is_never_written() {
        let hal =
            Arc::new(MockHardwareLayer::new(&["f1", "f2"], vec![at(40.0, 91.0)]).read_only("f2"));
        let mut e = Engine::new(hal.clone(), &Profile::from_toml(PROFILE).unwrap());
        e.tick(secs(0)); // emergency path
        e.submit("f2", Source::Override, 50, Some(secs(99)))
            .unwrap();
        e.tick(secs(1));
        drop(e);
        assert!(hal.writes().iter().all(|(f, _)| f == "f1"));
        assert_eq!(hal.restores(), ["f1"]);
    }

    #[test]
    fn s7_duty_is_clamped_to_100() {
        let (hal, mut e) = rig(&["f1", "f2"], vec![at(40.0, 40.0)]);
        e.submit("f1", Source::Override, 250, Some(secs(99)))
            .unwrap();
        e.tick(secs(0));
        assert!(hal.writes().iter().all(|(_, d)| *d <= 100));
        assert_eq!(hal.writes()[0], ("f1".into(), 100));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod review_tests {
    use super::*;
    use thermaliq_hal::{MockHardwareLayer, Tick};

    const PROFILE: &str = r#"
name = "t"
[[curves]]
type = "flat"
id = "c"
pwm = 30
[[controls]]
fan = "f1"
curve = "c"
step_up_pct = 100
step_down_pct = 100
[[controls]]
fan = "f2"
curve = "c"
step_up_pct = 100
step_down_pct = 100
"#;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn at(cpu: f32, gpu: f32) -> Tick {
        Tick::Temps(vec![
            ("cpu".into(), SensorKind::Cpu, cpu),
            ("gpu".into(), SensorKind::Gpu, gpu),
        ])
    }

    fn rig(hal: MockHardwareLayer) -> (Arc<MockHardwareLayer>, Engine) {
        let hal = Arc::new(hal);
        let e = Engine::new(hal.clone(), &Profile::from_toml(PROFILE).unwrap());
        (hal, e)
    }

    fn run(e: &mut Engine, from: u64, to: u64) {
        for t in from..=to {
            e.tick(secs(t));
        }
    }

    // Item 1
    #[test]
    fn latch_does_not_release_when_the_cpu_sensor_disappears() {
        let gone = Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, 40.0)]);
        let (hal, mut e) = rig(MockHardwareLayer::new(
            &["f1", "f2"],
            vec![at(97.0, 40.0), gone],
        ));
        run(&mut e, 0, 100);
        assert!(e.latched());
        assert!(hal.writes().iter().all(|(_, d)| *d == 100));
    }

    #[test]
    fn latch_releases_once_the_tripping_sensor_is_back_and_cool_for_30s() {
        let gone = Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, 40.0)]);
        let mut script = vec![at(97.0, 40.0)];
        script.extend(std::iter::repeat_n(gone, 5));
        script.push(at(60.0, 40.0));
        let (_, mut e) = rig(MockHardwareLayer::new(&["f1", "f2"], script));
        run(&mut e, 0, 35); // back from tick 6 on, cool since t=6
        assert!(
            e.latched(),
            "a sensor that just came back isn't 30 s cool yet"
        );
        run(&mut e, 36, 36);
        assert!(!e.latched());
    }

    // Item 3
    #[test]
    fn latch_holds_between_release_and_trip_thresholds() {
        let (_, mut e) = rig(MockHardwareLayer::new(
            &["f1", "f2"],
            vec![at(97.0, 40.0), at(90.0, 40.0)],
        ));
        run(&mut e, 0, 60);
        assert!(e.latched());
    }

    // Item 4
    #[test]
    fn read_only_fan_is_never_written_on_the_normal_path() {
        let (hal, mut e) =
            rig(MockHardwareLayer::new(&["f1", "f2"], vec![at(40.0, 40.0)]).read_only("f2"));
        run(&mut e, 0, 5);
        assert!(!e.latched());
        assert!(hal.writes().iter().all(|(f, _)| f == "f1"));
        assert!(!hal.writes().is_empty());
        drop(e);
        assert_eq!(hal.restores(), ["f1"]);
    }

    // Item 5
    #[test]
    fn latch_holds_while_a_reading_is_nan() {
        let (_, mut e) = rig(MockHardwareLayer::new(
            &["f1", "f2"],
            vec![at(97.0, 40.0), at(f32::NAN, 40.0)],
        ));
        run(&mut e, 0, 60);
        assert!(e.latched());
    }

    // Item 6
    #[test]
    fn clock_going_backwards_does_not_panic() {
        let (_, mut e) = rig(MockHardwareLayer::new(
            &["f1", "f2"],
            vec![at(97.0, 40.0), at(60.0, 40.0)],
        ));
        e.tick(secs(100));
        e.tick(secs(110)); // cool_since = 110
        e.tick(secs(50));
        assert!(e.latched());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod profile_switch_tests {
    use super::*;
    use thermaliq_hal::{MockHardwareLayer, Tick};

    fn profile(pwm: u8, fans: &[&str]) -> Profile {
        let mut s = format!("name = \"t\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = {pwm}\n");
        for f in fans {
            s += &format!("[[controls]]\nfan = \"{f}\"\ncurve = \"c\"\nstep_up_pct = 10\nstep_down_pct = 100\n");
        }
        Profile::from_toml(&s).unwrap()
    }

    fn rig(cpu: f32) -> (Arc<MockHardwareLayer>, Engine) {
        let hal = Arc::new(MockHardwareLayer::new(
            &["f1", "f2"],
            vec![Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, cpu)])],
        ));
        (hal.clone(), Engine::new(hal, &profile(30, &["f1", "f2"])))
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn switching_profile_while_latched_stays_at_100_and_hands_nothing_back() {
        let (hal, mut e) = rig(97.0);
        e.tick(secs(0));
        e.set_profile(&profile(30, &["f1", "f2"]));
        e.tick(secs(1));
        assert!(e.latched());
        assert!(hal.writes().iter().all(|(_, d)| *d == 100));
        assert!(hal.restores().is_empty());
    }

    #[test]
    fn switching_profile_keeps_ramp_and_restores_only_dropped_fans() {
        let (hal, mut e) = rig(40.0);
        e.tick(secs(0)); // first snapshot seeds at duty 0, writes 10
        for t in 1..=2 {
            e.tick(secs(t)); // 20, 30
        }
        e.set_profile(&profile(80, &["f1"]));
        e.tick(secs(3));
        let w = hal.writes();
        assert_eq!(
            w.last().unwrap(),
            &("f1".to_string(), 40),
            "ramps on, no jump to 80"
        );
        assert_eq!(hal.restores(), ["f2"]);
        drop(e);
        assert_eq!(hal.restores(), ["f2", "f1"]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod rereview_tests {
    use super::*;
    use thermaliq_hal::{MockHardwareLayer, Tick};

    /// Fans f1, f2 on one curve, full-size ramp steps. `Flat` or `Linear` on cpu.
    fn profile(linear: bool) -> Profile {
        let curve = if linear {
            "type = \"linear\"\nid = \"c\"\nsource = \"cpu\"\nmin_temp = 30.0\nmax_temp = 90.0\nmin_pwm = 20\nmax_pwm = 100\n"
        } else {
            "type = \"flat\"\nid = \"c\"\npwm = 30\n"
        };
        let mut s = format!("name = \"t\"\n[[curves]]\n{curve}");
        for f in ["f1", "f2"] {
            s += &format!(
                "[[controls]]\nfan = \"{f}\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n"
            );
        }
        Profile::from_toml(&s).unwrap()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn at(cpu: f32, gpu: f32) -> Tick {
        Tick::Temps(vec![
            ("cpu".into(), SensorKind::Cpu, cpu),
            ("gpu".into(), SensorKind::Gpu, gpu),
        ])
    }

    fn gpu_only(t: f32) -> Tick {
        Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, t)])
    }

    fn rig(linear: bool, script: Vec<Tick>) -> (Arc<MockHardwareLayer>, Engine) {
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], script));
        (hal.clone(), Engine::new(hal, &profile(linear)))
    }

    fn last_writes(hal: &MockHardwareLayer, n: usize) -> Vec<u8> {
        let w = hal.writes();
        w[w.len() - n..].iter().map(|(_, d)| *d).collect()
    }

    // Re-review 1
    #[test]
    fn latch_survives_a_profile_switch_below_the_trip_threshold() {
        let (hal, mut e) = rig(false, vec![at(97.0, 40.0), at(90.0, 40.0)]);
        e.tick(secs(0));
        e.tick(secs(1)); // 90 °C: latched, not re-tripped
        e.set_profile(&profile(false));
        e.tick(secs(2));
        assert!(e.latched());
        assert_eq!(last_writes(&hal, 2), [100, 100]);
    }

    // Re-review 2
    #[test]
    fn override_survives_a_profile_switch() {
        let (hal, mut e) = rig(false, vec![at(40.0, 40.0)]);
        e.tick(secs(0));
        e.submit("f1", Source::Override, 80, Some(secs(99)))
            .unwrap();
        e.tick(secs(1));
        assert_eq!(last_writes(&hal, 2), [80, 30]);
        e.set_profile(&profile(false));
        e.tick(secs(2));
        assert_eq!(last_writes(&hal, 2), [80, 30]);
    }

    // Re-review 3
    #[test]
    fn lost_sensor_state_survives_a_profile_switch() {
        let (hal, mut e) = rig(true, vec![at(40.0, 40.0), gpu_only(40.0)]);
        for t in 0..=3 {
            e.tick(secs(t));
        }
        assert_eq!(last_writes(&hal, 2), [100, 100], "lost after 3 ticks");
        e.set_profile(&profile(true));
        let before = hal.writes().len();
        e.tick(secs(4));
        e.tick(secs(5));
        assert_eq!(hal.writes().len(), before + 4);
        assert_eq!(last_writes(&hal, 4), [100; 4]);
    }

    // Re-review 4
    #[test]
    fn release_forgets_the_old_watch_list() {
        let script = vec![
            at(97.0, 40.0),
            at(60.0, 40.0),
            at(60.0, 40.0),
            gpu_only(40.0),
            gpu_only(91.0),
            gpu_only(40.0),
            gpu_only(40.0),
        ];
        let (_, mut e) = rig(false, script);
        let mut latched = vec![];
        for t in [0, 1, 31, 32, 33, 34, 64] {
            e.tick(secs(t));
            latched.push(e.latched());
        }
        // trip; cool; released; (cpu gone, not latched); gpu trips; still hot-ish; released
        assert_eq!(latched, [true, true, false, false, true, true, false]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod status_tests {
    use super::*;
    use thermaliq_hal::{MockHardwareLayer, Tick};

    fn rig(script: Vec<Tick>) -> Engine {
        let p = Profile::from_toml(
            "name = \"t\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n",
        )
        .unwrap();
        Engine::new(Arc::new(MockHardwareLayer::new(&["f1"], script)), &p)
    }

    fn at(cpu: f32, gpu: f32) -> Tick {
        Tick::Temps(vec![
            ("cpu".into(), SensorKind::Cpu, cpu),
            ("gpu".into(), SensorKind::Gpu, gpu),
        ])
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn last_snapshot_is_the_one_the_tick_used() {
        let mut e = rig(vec![at(40.0, 41.0)]);
        assert!(e.last_snapshot().is_none());
        e.tick(secs(0));
        let s = e.last_snapshot().unwrap();
        assert_eq!(s.sensors.len(), 2);
        assert_eq!(s.sensors[0].temp_celsius, 40.0);
    }

    #[test]
    fn a_failed_snapshot_keeps_the_previous_one() {
        let mut e = rig(vec![at(40.0, 41.0), Tick::Fail]);
        e.tick(secs(0));
        assert!(e.snapshot_fresh());
        e.tick(secs(1));
        assert_eq!(e.last_snapshot().unwrap().sensors[0].temp_celsius, 40.0);
        assert!(!e.snapshot_fresh(), "stale: must not be recorded as new");
    }

    #[test]
    fn latch_waits_on_the_hot_sensor_until_it_is_cool() {
        let mut e = rig(vec![at(97.0, 40.0), at(90.0, 40.0), at(60.0, 40.0)]);
        assert!(e.latch_waiting_on().is_empty());
        e.tick(secs(0));
        assert_eq!(e.latch_waiting_on(), ["cpu"]);
        e.tick(secs(1));
        assert_eq!(e.latch_waiting_on(), ["cpu"], "90 °C is above release");
        e.tick(secs(2));
        assert!(
            e.latch_waiting_on().is_empty(),
            "cool, only the hold is left"
        );
        assert!(e.latched());
    }

    #[test]
    fn latch_waits_on_a_sensor_that_went_missing_or_nan() {
        let gone = Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, 40.0)]);
        let mut e = rig(vec![at(97.0, 40.0), gone, at(f32::NAN, 40.0)]);
        e.tick(secs(0));
        e.tick(secs(1));
        assert_eq!(e.latch_waiting_on(), ["cpu"], "missing");
        e.tick(secs(2));
        assert_eq!(e.latch_waiting_on(), ["cpu"], "NaN");
    }

    #[test]
    fn waiting_list_is_empty_after_release() {
        let mut e = rig(vec![at(97.0, 40.0), at(60.0, 40.0)]);
        e.tick(secs(0));
        e.tick(secs(1));
        e.tick(secs(31));
        assert!(!e.latched());
        assert!(e.latch_waiting_on().is_empty());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod command_tests {
    use super::*;
    use thermaliq_hal::{MockHardwareLayer, Tick};

    fn rig() -> (Arc<MockHardwareLayer>, Engine) {
        let p = Profile::from_toml(
            "name = \"t\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n",
        )
        .unwrap();
        let script = vec![Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 40.0)])];
        let hal = Arc::new(MockHardwareLayer::new(&["f1", "f2"], script).read_only("f2"));
        (hal.clone(), Engine::new(hal, &p))
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn cancelling_an_override_returns_the_fan_to_the_profile() {
        let (hal, mut e) = rig();
        e.submit("f1", Source::Override, 80, Some(secs(99)))
            .unwrap();
        e.tick(secs(0));
        assert_eq!(e.cancel_override("f1"), Ok(true));
        e.tick(secs(1));
        let w = hal.writes();
        assert_eq!(w.last().unwrap(), &("f1".to_string(), 30));
    }

    #[test]
    fn cancel_says_when_there_was_nothing_to_cancel_or_no_such_fan() {
        let (_, mut e) = rig();
        assert_eq!(e.cancel_override("f1"), Ok(false));
        assert!(e.cancel_override("nope").is_err());
    }

    fn blocked(linear: bool, script: Vec<Tick>, ticks: u64) -> Option<&'static str> {
        let curve = if linear {
            "type = \"linear\"\nid = \"c\"\nsource = \"cpu\"\nmin_temp = 30.0\nmax_temp = 90.0\nmin_pwm = 20\nmax_pwm = 100\n"
        } else {
            "type = \"flat\"\nid = \"c\"\npwm = 30\n"
        };
        let p = Profile::from_toml(&format!(
            "name = \"t\"\n[[curves]]\n{curve}[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n"
        ))
        .unwrap();
        let mut e = Engine::new(Arc::new(MockHardwareLayer::new(&["f1"], script)), &p);
        for t in 0..ticks {
            e.tick(secs(t));
        }
        e.override_blocked_by("f1")
    }

    #[test]
    fn an_override_is_in_effect_unless_safety_overrides_it() {
        let at = |c: f32| Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, c)]);
        let gpu_only = Tick::Temps(vec![("gpu".into(), SensorKind::Gpu, 40.0)]);
        assert_eq!(blocked(false, vec![at(40.0)], 2), None);
        assert_eq!(blocked(false, vec![at(97.0)], 2), Some("latched"));
        assert_eq!(
            blocked(true, vec![at(40.0), gpu_only], 5),
            Some("sensor_lost")
        );
        assert_eq!(
            blocked(false, vec![at(40.0), Tick::Fail], 5),
            Some("degraded")
        );
    }

    #[test]
    fn can_control_is_false_for_read_only_and_unknown_fans() {
        let (_, e) = rig();
        assert!(e.can_control("f1"));
        assert!(!e.can_control("f2"), "read-only");
        assert!(!e.can_control("nope"));
    }
}

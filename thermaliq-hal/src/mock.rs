use crate::{Capabilities, HalError, HardwareLayer};
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use thermaliq_common::{
    FanKind, FanReading, HardwareSnapshot, MetricReading, SensorKind, SensorReading,
};

/// One scripted `snapshot()` outcome.
#[derive(Debug, Clone)]
pub enum Tick {
    /// (sensor id, kind, °C) triples.
    Temps(Vec<(String, SensorKind, f32)>),
    Fail,
}

#[derive(Default)]
struct State {
    script: VecDeque<Tick>,
    fans: Vec<(String, u8)>,
    writes: Vec<(String, u8)>,
    restores: Vec<String>,
    read_only: Vec<String>,
    fail_writes: Vec<String>,
    fail_restores: Vec<String>,
    pumps: Vec<String>,
    metrics: Vec<MetricReading>,
}

/// Scriptable fake hardware. Each `snapshot()` consumes the next scripted tick;
/// the last one repeats forever. With an empty script, `snapshot()` fails.
pub struct MockHardwareLayer(Mutex<State>);

impl MockHardwareLayer {
    pub fn new(fan_ids: &[&str], script: Vec<Tick>) -> Self {
        Self(Mutex::new(State {
            script: script.into(),
            fans: fan_ids.iter().map(|f| (f.to_string(), 0)).collect(),
            ..State::default()
        }))
    }

    /// Every `set_fan_duty` call received, in order, unmodified (no clamping).
    pub fn writes(&self) -> Vec<(String, u8)> {
        self.state().writes.clone()
    }

    /// Every `restore_auto` call received, in order.
    pub fn restores(&self) -> Vec<String> {
        self.state().restores.clone()
    }

    /// Fan shows in snapshots but is not controllable; writes and restores error.
    pub fn read_only(self, fan: &str) -> Self {
        self.state().read_only.push(fan.to_string());
        self
    }

    /// This fan shows up as a pump (`FanKind::Pump`) in snapshots.
    pub fn pump(self, fan: &str) -> Self {
        self.state().pumps.push(fan.to_string());
        self
    }

    /// Every snapshot carries these metrics.
    pub fn metrics(self, metrics: Vec<MetricReading>) -> Self {
        self.state().metrics = metrics;
        self
    }

    /// Make every later `set_fan_duty` on this fan fail (the call is still recorded).
    pub fn fail_writes(&self, fan: &str) {
        self.state().fail_writes.push(fan.to_string());
    }

    /// Make every later `restore_auto` on this fan fail (the call is still recorded).
    pub fn fail_restores(&self, fan: &str) {
        self.state().fail_restores.push(fan.to_string());
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn unknown(fan: &str) -> HalError {
    HalError(format!("unknown fan '{fan}'"))
}

impl HardwareLayer for MockHardwareLayer {
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        let mut s = self.state();
        let tick = if s.script.len() > 1 {
            s.script.pop_front()
        } else {
            s.script.front().cloned()
        };
        let Some(Tick::Temps(temps)) = tick else {
            return Err(HalError("scripted snapshot failure".into()));
        };
        Ok(HardwareSnapshot {
            sensors: temps
                .into_iter()
                .map(|(id, kind, temp_celsius)| SensorReading {
                    label: id.clone(),
                    id,
                    kind,
                    temp_celsius,
                })
                .collect(),
            fans: s
                .fans
                .iter()
                .map(|(id, duty)| FanReading {
                    id: id.clone(),
                    label: id.clone(),
                    rpm: u32::from(*duty) * 20,
                    duty_percent: *duty,
                    kind: if s.pumps.contains(id) {
                        FanKind::Pump
                    } else {
                        FanKind::Fan
                    },
                })
                .collect(),
            metrics: s.metrics.clone(),
        })
    }

    fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
        let mut s = self.state();
        s.writes.push((fan.to_string(), duty));
        if s.read_only.iter().any(|f| f == fan) {
            return Err(HalError(format!("fan '{fan}' is read-only")));
        }
        if s.fail_writes.iter().any(|f| f == fan) {
            return Err(HalError(format!("injected write failure on '{fan}'")));
        }
        let slot = s
            .fans
            .iter_mut()
            .find(|(id, _)| id == fan)
            .ok_or_else(|| unknown(fan))?;
        slot.1 = duty;
        Ok(())
    }

    fn restore_auto(&self, fan: &str) -> Result<(), HalError> {
        let mut s = self.state();
        s.restores.push(fan.to_string());
        if s.read_only.iter().any(|f| f == fan) {
            return Err(HalError(format!("fan '{fan}' is read-only")));
        }
        if s.fail_restores.iter().any(|f| f == fan) {
            return Err(HalError(format!("injected restore failure on '{fan}'")));
        }
        if s.fans.iter().any(|(id, _)| id == fan) {
            Ok(())
        } else {
            Err(unknown(fan))
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            controllable_fans: {
                let s = self.state();
                s.fans
                    .iter()
                    .filter(|(id, _)| !s.read_only.contains(id))
                    .map(|(id, _)| id.clone())
                    .collect()
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use thermaliq_common::SensorKind;

    fn temps(v: f32) -> Tick {
        Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, v)])
    }

    fn cpu_temp(m: &MockHardwareLayer) -> Result<f32, HalError> {
        m.snapshot().map(|s| s.sensors[0].temp_celsius)
    }

    #[test]
    fn temps_follow_script_per_tick_and_last_repeats() {
        let m = MockHardwareLayer::new(&["f1"], vec![temps(40.0), temps(50.0), temps(60.0)]);
        let got: Vec<_> = (0..5).map(|_| cpu_temp(&m).unwrap()).collect();
        assert_eq!(got, [40.0, 50.0, 60.0, 60.0, 60.0]);
    }

    #[test]
    fn pumps_and_metrics_show_in_snapshots() {
        let load = MetricReading {
            id: "cpu_load".into(),
            kind: thermaliq_common::MetricKind::CpuLoad,
            value: 12.0,
        };
        let m = MockHardwareLayer::new(&["f1", "p1"], vec![temps(40.0)])
            .pump("p1")
            .metrics(vec![load.clone()]);
        let s = m.snapshot().unwrap();
        let kinds: Vec<_> = s.fans.iter().map(|f| f.kind).collect();
        assert_eq!(kinds, [FanKind::Fan, FanKind::Pump]);
        assert_eq!(s.metrics, [load]);
    }

    #[test]
    fn scripted_failure_and_empty_script_error() {
        let m = MockHardwareLayer::new(&["f1"], vec![temps(40.0), Tick::Fail, temps(41.0)]);
        assert!(cpu_temp(&m).is_ok());
        assert!(cpu_temp(&m).is_err());
        assert_eq!(cpu_temp(&m).unwrap(), 41.0);
        assert!(MockHardwareLayer::new(&["f1"], vec![]).snapshot().is_err());
    }

    #[test]
    fn records_every_write_unmodified_and_reflects_in_snapshot() {
        let m = MockHardwareLayer::new(&["f1", "f2"], vec![temps(40.0)]);
        m.set_fan_duty("f1", 30).unwrap();
        m.set_fan_duty("f2", 255).unwrap();
        m.set_fan_duty("f1", 70).unwrap();
        assert_eq!(
            m.writes(),
            [("f1".into(), 30), ("f2".into(), 255), ("f1".into(), 70)]
        );
        let snap = m.snapshot().unwrap();
        assert_eq!(snap.fans[0].duty_percent, 70);
    }

    #[test]
    fn records_every_restore_auto() {
        let m = MockHardwareLayer::new(&["f1", "f2"], vec![temps(40.0)]);
        m.restore_auto("f2").unwrap();
        m.restore_auto("f1").unwrap();
        m.restore_auto("f2").unwrap();
        assert_eq!(m.restores(), ["f2", "f1", "f2"]);
    }

    #[test]
    fn unknown_fan_errors_but_is_still_recorded() {
        let m = MockHardwareLayer::new(&["f1"], vec![temps(40.0)]);
        assert!(m.set_fan_duty("nope", 50).is_err());
        assert!(m.restore_auto("nope").is_err());
        assert_eq!(m.writes().len(), 1);
        assert_eq!(m.restores().len(), 1);
    }

    #[test]
    fn scripted_kinds_reach_the_snapshot() {
        let m = MockHardwareLayer::new(
            &["f1"],
            vec![Tick::Temps(vec![
                ("c".into(), SensorKind::Cpu, 50.0),
                ("g".into(), SensorKind::Gpu, 91.0),
                ("o".into(), SensorKind::Other, 30.0),
            ])],
        );
        let kinds: Vec<_> = m
            .snapshot()
            .unwrap()
            .sensors
            .iter()
            .map(|s| s.kind)
            .collect();
        assert_eq!(kinds, [SensorKind::Cpu, SensorKind::Gpu, SensorKind::Other]);
    }

    #[test]
    fn injected_write_failure_is_per_fan_and_recorded() {
        let m = MockHardwareLayer::new(&["f1", "f2"], vec![temps(40.0)]);
        m.fail_writes("f1");
        assert!(m.set_fan_duty("f1", 80).is_err());
        assert!(m.set_fan_duty("f2", 80).is_ok());
        assert_eq!(m.writes(), [("f1".into(), 80), ("f2".into(), 80)]);
        assert_eq!(m.snapshot().unwrap().fans[0].duty_percent, 0);
    }

    #[test]
    fn injected_restore_failure_is_per_fan_and_recorded() {
        let m = MockHardwareLayer::new(&["f1", "f2"], vec![temps(40.0)]);
        m.fail_restores("f1");
        assert!(m.restore_auto("f1").is_err());
        assert!(m.restore_auto("f2").is_ok());
        assert_eq!(m.restores(), ["f1", "f2"]);
    }

    #[test]
    fn read_only_fan_is_visible_but_not_controllable() {
        let m = MockHardwareLayer::new(&["f1", "ro"], vec![temps(40.0)]).read_only("ro");
        assert_eq!(m.snapshot().unwrap().fans.len(), 2);
        assert_eq!(m.capabilities().controllable_fans, ["f1"]);
        assert!(m.set_fan_duty("ro", 50).is_err());
        assert_eq!(m.snapshot().unwrap().fans[1].duty_percent, 0);
        assert_eq!(m.writes(), [("ro".into(), 50)]);
    }

    #[test]
    fn capabilities_list_fans() {
        let m = MockHardwareLayer::new(&["f1", "f2"], vec![]);
        assert_eq!(m.capabilities().controllable_fans, ["f1", "f2"]);
    }
}

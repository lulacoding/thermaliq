use crate::{Capabilities, HalError, HardwareLayer};
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading};

/// One scripted `snapshot()` outcome.
#[derive(Debug, Clone)]
pub enum Tick {
    /// (sensor id, °C) pairs.
    Temps(Vec<(String, f32)>),
    Fail,
}

#[derive(Default)]
struct State {
    script: VecDeque<Tick>,
    fans: Vec<(String, u8)>,
    writes: Vec<(String, u8)>,
    restores: Vec<String>,
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
                .map(|(id, temp_celsius)| SensorReading {
                    label: id.clone(),
                    id,
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
                })
                .collect(),
        })
    }

    fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
        let mut s = self.state();
        s.writes.push((fan.to_string(), duty));
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
        if s.fans.iter().any(|(id, _)| id == fan) {
            Ok(())
        } else {
            Err(unknown(fan))
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            controllable_fans: self.state().fans.iter().map(|(id, _)| id.clone()).collect(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temps(v: f32) -> Tick {
        Tick::Temps(vec![("cpu".into(), v)])
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
    fn capabilities_list_fans() {
        let m = MockHardwareLayer::new(&["f1", "f2"], vec![]);
        assert_eq!(m.capabilities().controllable_fans, ["f1", "f2"]);
    }
}

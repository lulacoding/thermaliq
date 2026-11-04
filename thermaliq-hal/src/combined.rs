//! Merges several backends into one `HardwareLayer` and routes each fan to its owner.

use crate::{Capabilities, HalError, HardwareLayer};
use std::sync::Arc;
use thermaliq_common::HardwareSnapshot;

pub struct CombinedHardwareLayer;

impl CombinedHardwareLayer {
    pub fn new(_backends: Vec<Arc<dyn HardwareLayer>>) -> Result<Self, HalError> {
        Err(HalError("not implemented".into()))
    }
}

impl HardwareLayer for CombinedHardwareLayer {
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        Err(HalError("not implemented".into()))
    }
    fn set_fan_duty(&self, _: &str, _: u8) -> Result<(), HalError> {
        Err(HalError("not implemented".into()))
    }
    fn restore_auto(&self, _: &str) -> Result<(), HalError> {
        Err(HalError("not implemented".into()))
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{MockHardwareLayer, Tick};
    use thermaliq_common::SensorKind;

    fn mock(fans: &[&str], sensor: &str, kind: SensorKind) -> Arc<MockHardwareLayer> {
        let t = Tick::Temps(vec![(sensor.into(), kind, 40.0)]);
        Arc::new(MockHardwareLayer::new(fans, vec![t]))
    }

    fn pair() -> (
        Arc<MockHardwareLayer>,
        Arc<MockHardwareLayer>,
        CombinedHardwareLayer,
    ) {
        let a = mock(&["a1", "a2"], "gpu", SensorKind::Gpu);
        let b = mock(&["b1"], "cpu", SensorKind::Cpu);
        let c = CombinedHardwareLayer::new(vec![a.clone(), b.clone()]).unwrap();
        (a, b, c)
    }

    #[test]
    fn snapshot_merges_sensors_and_fans_in_backend_order() {
        let (_, _, c) = pair();
        let snap = c.snapshot().unwrap();
        let sensors: Vec<_> = snap.sensors.iter().map(|s| s.id.as_str()).collect();
        let fans: Vec<_> = snap.fans.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(sensors, ["gpu", "cpu"]);
        assert_eq!(fans, ["a1", "a2", "b1"]);
    }

    #[test]
    fn capabilities_are_the_union() {
        let (_, _, c) = pair();
        assert_eq!(c.capabilities().controllable_fans, ["a1", "a2", "b1"]);
    }

    #[test]
    fn writes_and_restores_reach_only_the_owner() {
        let (a, b, c) = pair();
        c.set_fan_duty("b1", 70).unwrap();
        c.set_fan_duty("a2", 20).unwrap();
        c.restore_auto("b1").unwrap();
        assert_eq!(a.writes(), [("a2".into(), 20)]);
        assert_eq!(b.writes(), [("b1".into(), 70)]);
        assert!(a.restores().is_empty());
        assert_eq!(b.restores(), ["b1"]);
    }

    #[test]
    fn an_owner_error_comes_back() {
        let (a, _, c) = pair();
        a.fail_writes("a1");
        assert!(c.set_fan_duty("a1", 50).is_err());
    }

    #[test]
    fn unknown_fans_error_and_reach_no_backend() {
        let (a, b, c) = pair();
        assert!(c.set_fan_duty("zz", 50).is_err());
        assert!(c.restore_auto("zz").is_err());
        assert!(a.writes().is_empty() && b.writes().is_empty());
        assert!(a.restores().is_empty() && b.restores().is_empty());
    }

    #[test]
    fn a_read_only_fan_is_not_routable() {
        let a = Arc::new(
            MockHardwareLayer::new(&["a1", "ro"], vec![Tick::Temps(vec![])]).read_only("ro"),
        );
        let c = CombinedHardwareLayer::new(vec![a.clone()]).unwrap();
        assert!(c.set_fan_duty("ro", 50).is_err());
        assert!(a.writes().is_empty());
    }

    #[test]
    fn two_backends_claiming_one_fan_is_refused_at_construction() {
        let a = mock(&["f"], "x", SensorKind::Other);
        let b = mock(&["f"], "y", SensorKind::Other);
        let err = CombinedHardwareLayer::new(vec![a, b]).err().unwrap();
        assert!(err.0.contains("'f'"), "{err}");
    }

    #[test]
    fn one_failing_backend_fails_the_snapshot() {
        let a = mock(&["a1"], "gpu", SensorKind::Gpu);
        let b = Arc::new(MockHardwareLayer::new(&["b1"], vec![Tick::Fail]));
        let c = CombinedHardwareLayer::new(vec![a, b]).unwrap();
        assert!(c.snapshot().is_err());
    }

    #[test]
    fn no_backends_is_an_error() {
        assert!(CombinedHardwareLayer::new(vec![]).is_err());
    }
}

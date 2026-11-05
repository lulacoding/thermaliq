//! Merges several backends into one `HardwareLayer` and routes each fan to its owner.

use crate::{Capabilities, HalError, HardwareLayer};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use thermaliq_common::{HardwareSnapshot, MetricKind};

pub struct CombinedHardwareLayer {
    backends: Vec<Arc<dyn HardwareLayer>>,
    /// Controllable fan id -> the backend that owns it.
    owner: HashMap<String, Arc<dyn HardwareLayer>>,
    optional: Vec<(String, Arc<dyn HardwareLayer>)>,
    /// Optional sources whose last snapshot failed.
    unavailable: Mutex<Vec<String>>,
}

impl CombinedHardwareLayer {
    /// Errors if no backend is given or two backends claim the same controllable fan.
    pub fn new(backends: Vec<Arc<dyn HardwareLayer>>) -> Result<Self, HalError> {
        if backends.is_empty() {
            return Err(HalError("no hardware backends".into()));
        }
        let mut owner = HashMap::new();
        for b in &backends {
            for fan in b.capabilities().controllable_fans {
                if owner.insert(fan.clone(), b.clone()).is_some() {
                    return Err(HalError(format!("fan '{fan}' is claimed by two backends")));
                }
            }
        }
        Ok(Self {
            backends,
            owner,
            optional: Vec::new(),
            unavailable: Mutex::new(Vec::new()),
        })
    }

    /// Adds a read-only source that may be missing: if its snapshot fails, only its own
    /// sensors drop out and `unavailable_sources` names it. Refused if it can control fans.
    pub fn with_optional(
        mut self,
        name: &str,
        backend: Arc<dyn HardwareLayer>,
    ) -> Result<Self, HalError> {
        if !backend.capabilities().controllable_fans.is_empty() {
            return Err(HalError(format!(
                "optional source '{name}' must be read-only, but it controls fans"
            )));
        }
        self.optional.push((name.to_string(), backend));
        Ok(self)
    }

    /// A source that may be missing and owns fans (the LHM bridge). If its snapshot fails, its
    /// readings drop out as with `with_optional`, and S4 stays out of it: its own watchdog gives
    /// its fans to the BIOS (S6). Writes to them fail until it is back.
    pub fn with_optional_owner(
        mut self,
        name: &str,
        backend: Arc<dyn HardwareLayer>,
    ) -> Result<Self, HalError> {
        for fan in backend.capabilities().controllable_fans {
            if self.owner.insert(fan.clone(), backend.clone()).is_some() {
                return Err(HalError(format!("fan '{fan}' is claimed by two backends")));
            }
        }
        self.optional.push((name.to_string(), backend));
        Ok(self)
    }

    fn owner_of(&self, fan: &str) -> Result<&dyn HardwareLayer, HalError> {
        self.owner
            .get(fan)
            .map(AsRef::as_ref)
            .ok_or_else(|| HalError(format!("no backend controls fan '{fan}'")))
    }
}

impl HardwareLayer for CombinedHardwareLayer {
    /// A required backend failing fails the snapshot: a missing backend would silently drop
    /// its sensors, and S4 wants a failed read to be loud. An optional one only drops itself.
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        let mut all = HardwareSnapshot::default();
        for b in &self.backends {
            let s = b.snapshot()?;
            all.sensors.extend(s.sensors);
            all.fans.extend(s.fans);
            all.metrics.extend(s.metrics);
        }
        let mut down = Vec::new();
        for (name, b) in &self.optional {
            match b.snapshot() {
                Ok(s) => {
                    // shortcut: all-or-nothing, not per GPU; match readings to GPUs when a
                    // multi-GPU machine has NVML power on one card and not the other.
                    // Built-in CPU load wins over HWiNFO's the same way (one CPU).
                    let have = |all: &HardwareSnapshot, k| all.metrics.iter().any(|m| m.kind == k);
                    let (gpu_power, cpu_load) = (
                        have(&all, MetricKind::GpuPower),
                        have(&all, MetricKind::CpuLoad),
                    );
                    all.sensors.extend(s.sensors);
                    all.fans.extend(s.fans);
                    all.metrics.extend(s.metrics.into_iter().filter(|m| {
                        !(gpu_power && m.kind == MetricKind::GpuPower
                            || cpu_load && m.kind == MetricKind::CpuLoad)
                    }));
                }
                Err(_) => down.push(name.clone()),
            }
        }
        *self
            .unavailable
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = down;
        Ok(all)
    }

    fn unavailable_sources(&self) -> Vec<String> {
        self.unavailable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn degraded_reasons(&self) -> Vec<String> {
        self.backends
            .iter()
            .chain(self.optional.iter().map(|(_, b)| b))
            .flat_map(|b| b.degraded_reasons())
            .collect()
    }

    fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
        self.owner_of(fan)?.set_fan_duty(fan, duty)
    }

    fn restore_auto(&self, fan: &str) -> Result<(), HalError> {
        self.owner_of(fan)?.restore_auto(fan)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            controllable_fans: self
                .backends
                .iter()
                .chain(self.optional.iter().map(|(_, b)| b))
                .flat_map(|b| b.capabilities().controllable_fans)
                .collect(),
        }
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
    fn snapshot_merges_metrics_and_fan_kinds_from_required_and_optional_backends() {
        use thermaliq_common::{FanKind, MetricKind, MetricReading};
        let m = |id: &str| {
            vec![MetricReading {
                id: id.into(),
                kind: MetricKind::CpuClock,
                value: 1.0,
            }]
        };
        let req = mock(&["p"], "gpu", SensorKind::Gpu);
        let req = Arc::new(Arc::into_inner(req).unwrap().pump("p").metrics(m("a")));
        let opt = Arc::new(
            Arc::into_inner(mock(&[], "hw", SensorKind::Cpu))
                .unwrap()
                .metrics(m("b")),
        );
        let c = CombinedHardwareLayer::new(vec![req])
            .unwrap()
            .with_optional("hwinfo", opt)
            .unwrap();
        let snap = c.snapshot().unwrap();
        let ids: Vec<_> = snap.metrics.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(snap.fans[0].kind, FanKind::Pump);
    }

    fn gpu_power(id: &str, w: f64) -> Vec<thermaliq_common::MetricReading> {
        vec![thermaliq_common::MetricReading {
            id: id.into(),
            kind: thermaliq_common::MetricKind::GpuPower,
            value: w,
        }]
    }

    fn with_gpu_power(required: Option<f64>) -> Vec<(String, f64)> {
        let req = mock(&[], "gpu", SensorKind::Gpu);
        let req = Arc::new(
            Arc::into_inner(req)
                .unwrap()
                .metrics(required.map_or(vec![], |w| gpu_power("nvml", w))),
        );
        let opt = mock(&[], "hw", SensorKind::Cpu);
        let opt = Arc::new(
            Arc::into_inner(opt)
                .unwrap()
                .metrics(gpu_power("hwinfo", 50.0)),
        );
        let c = CombinedHardwareLayer::new(vec![req])
            .unwrap()
            .with_optional("hwinfo", opt)
            .unwrap();
        let snap = c.snapshot().unwrap();
        snap.metrics.into_iter().map(|m| (m.id, m.value)).collect()
    }

    #[test]
    fn hwinfo_gpu_power_fills_in_only_when_the_required_backend_has_none() {
        assert_eq!(with_gpu_power(None), [("hwinfo".to_string(), 50.0)]);
        assert_eq!(with_gpu_power(Some(120.0)), [("nvml".to_string(), 120.0)]);
    }

    #[test]
    fn a_later_optional_cpu_load_is_dropped_when_an_earlier_source_has_one() {
        use thermaliq_common::{MetricKind, MetricReading};
        let load = |id: &str| {
            vec![MetricReading {
                id: id.into(),
                kind: MetricKind::CpuLoad,
                value: 10.0,
            }]
        };
        let opt = |id: &str| {
            Arc::new(
                Arc::into_inner(mock(&[], "x", SensorKind::Cpu))
                    .unwrap()
                    .metrics(load(id)),
            )
        };
        let ids = |first: bool| {
            let mut c =
                CombinedHardwareLayer::new(vec![mock(&["a1"], "gpu", SensorKind::Gpu)]).unwrap();
            if first {
                c = c.with_optional("cpu-load", opt("builtin")).unwrap();
            }
            let snap = c
                .with_optional("hwinfo", opt("hwinfo"))
                .unwrap()
                .snapshot()
                .unwrap();
            snap.metrics.into_iter().map(|m| m.id).collect::<Vec<_>>()
        };
        assert_eq!(ids(true), ["builtin"]);
        assert_eq!(ids(false), ["hwinfo"]);
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

    fn required_plus(optional: Arc<MockHardwareLayer>) -> CombinedHardwareLayer {
        let req = mock(&["a1"], "gpu", SensorKind::Gpu);
        CombinedHardwareLayer::new(vec![req])
            .unwrap()
            .with_optional("hwinfo", optional)
            .unwrap()
    }

    #[test]
    fn optional_sensors_are_merged_after_the_required_ones() {
        let c = required_plus(mock(&[], "hw", SensorKind::Cpu));
        let snap = c.snapshot().unwrap();
        let ids: Vec<_> = snap.sensors.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["gpu", "hw"]);
        assert!(c.unavailable_sources().is_empty());
    }

    #[test]
    fn a_failing_optional_drops_only_its_sensors_and_sets_the_flag() {
        let opt = Arc::new(MockHardwareLayer::new(&[], vec![Tick::Fail]));
        let c = required_plus(opt);
        let snap = c.snapshot().unwrap();
        assert_eq!(snap.sensors.len(), 1);
        assert_eq!(snap.fans.len(), 1);
        assert_eq!(c.unavailable_sources(), ["hwinfo"]);
    }

    #[test]
    fn the_flag_clears_when_the_optional_source_comes_back() {
        let t = Tick::Temps(vec![("hw".into(), SensorKind::Cpu, 40.0)]);
        let opt = Arc::new(MockHardwareLayer::new(&[], vec![Tick::Fail, t]));
        let c = required_plus(opt);
        c.snapshot().unwrap();
        assert_eq!(c.unavailable_sources(), ["hwinfo"]);
        assert_eq!(c.snapshot().unwrap().sensors.len(), 2);
        assert!(c.unavailable_sources().is_empty());
    }

    #[test]
    fn a_failing_required_backend_still_fails_the_snapshot_with_an_optional_present() {
        let req = Arc::new(MockHardwareLayer::new(&["a1"], vec![Tick::Fail]));
        let opt = mock(&[], "hw", SensorKind::Cpu);
        let c = CombinedHardwareLayer::new(vec![req])
            .unwrap()
            .with_optional("hwinfo", opt)
            .unwrap();
        assert!(c.snapshot().is_err());
    }

    #[test]
    fn an_optional_backend_cannot_own_fans() {
        let req = mock(&["a1"], "gpu", SensorKind::Gpu);
        let opt = mock(&["b1"], "hw", SensorKind::Cpu);
        let err = CombinedHardwareLayer::new(vec![req])
            .unwrap()
            .with_optional("hwinfo", opt)
            .err()
            .unwrap();
        assert!(err.0.contains("hwinfo"), "{err}");
    }

    #[test]
    fn an_optional_owner_can_be_written_and_its_failure_only_drops_itself() {
        let req = mock(&["a1"], "gpu", SensorKind::Gpu);
        let opt = Arc::new(MockHardwareLayer::new(
            &["b1"],
            vec![
                Tick::Temps(vec![("hw".into(), SensorKind::Cpu, 40.0)]),
                Tick::Fail,
            ],
        ));
        let c = CombinedHardwareLayer::new(vec![req])
            .unwrap()
            .with_optional_owner("lhm-bridge", opt.clone())
            .unwrap();
        assert_eq!(c.capabilities().controllable_fans, ["a1", "b1"]);
        c.set_fan_duty("b1", 70).unwrap();
        c.restore_auto("b1").unwrap();
        assert_eq!(opt.writes(), [("b1".into(), 70)]);
        assert_eq!(opt.restores(), ["b1"]);
        assert_eq!(c.snapshot().unwrap().fans.len(), 2);
        let snap = c.snapshot().unwrap();
        assert_eq!(snap.sensors.len(), 1, "the GPU reading survives");
        assert_eq!(snap.fans.len(), 1);
        assert_eq!(c.unavailable_sources(), ["lhm-bridge"]);
    }

    #[test]
    fn an_optional_owner_cannot_claim_a_fan_another_backend_owns() {
        let req = mock(&["f"], "gpu", SensorKind::Gpu);
        let opt = mock(&["f"], "hw", SensorKind::Cpu);
        let err = CombinedHardwareLayer::new(vec![req])
            .unwrap()
            .with_optional_owner("lhm-bridge", opt)
            .err()
            .unwrap();
        assert!(err.0.contains("'f'"), "{err}");
    }

    #[test]
    fn builtin_cpu_load_still_wins_over_an_optional_owner() {
        use thermaliq_common::{MetricKind, MetricReading};
        let load = |id: &str| {
            vec![MetricReading {
                id: id.into(),
                kind: MetricKind::CpuLoad,
                value: 10.0,
            }]
        };
        let builtin = Arc::new(
            Arc::into_inner(mock(&[], "x", SensorKind::Other))
                .unwrap()
                .metrics(load("builtin")),
        );
        let bridge = Arc::new(
            Arc::into_inner(mock(&["b1"], "y", SensorKind::Other))
                .unwrap()
                .metrics(load("lhm")),
        );
        let c = CombinedHardwareLayer::new(vec![mock(&["a1"], "gpu", SensorKind::Gpu)])
            .unwrap()
            .with_optional("cpu-load", builtin)
            .unwrap()
            .with_optional_owner("lhm-bridge", bridge)
            .unwrap();
        let ids: Vec<_> = c.snapshot().unwrap().metrics.into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["builtin"]);
    }

    #[test]
    fn degraded_reasons_come_from_every_backend() {
        struct Faulty;
        impl HardwareLayer for Faulty {
            fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
                Ok(HardwareSnapshot::default())
            }
            fn set_fan_duty(&self, _: &str, _: u8) -> Result<(), HalError> {
                Ok(())
            }
            fn restore_auto(&self, _: &str) -> Result<(), HalError> {
                Ok(())
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            fn degraded_reasons(&self) -> Vec<String> {
                vec!["header still manual".into()]
            }
        }
        let c = CombinedHardwareLayer::new(vec![mock(&["a1"], "gpu", SensorKind::Gpu)])
            .unwrap()
            .with_optional_owner("lhm-bridge", Arc::new(Faulty))
            .unwrap();
        assert_eq!(c.degraded_reasons(), ["header still manual"]);
    }

    #[test]
    fn no_backends_is_an_error() {
        assert!(CombinedHardwareLayer::new(vec![]).is_err());
    }
}

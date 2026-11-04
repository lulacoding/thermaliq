//! Dev-only fault injection for gate G1 (SPEC §8). Built only with `--features gate-tests`
//! and refused in release builds; it never writes to a fan itself.
//!
//! `THERMALIQ_INJECT_CPU_TEMP` is read on every snapshot. Its value is a temperature in °C, or
//! the path of a file holding one: a service's environment is fixed when it starts, so the
//! gate script points the variable at a file and rewrites the file to change the injection.
//! Unset, empty or unparsable means no injection.

#[cfg(not(debug_assertions))]
compile_error!("the gate-tests feature is for debug builds only, never release");

use std::sync::Arc;
use thermaliq_common::{HardwareSnapshot, SensorKind, SensorReading};
use thermaliq_hal::{Capabilities, HalError, HardwareLayer};

pub const VAR: &str = "THERMALIQ_INJECT_CPU_TEMP";

pub struct InjectCpuTemp {
    inner: Arc<dyn HardwareLayer>,
    var: &'static str,
}

impl InjectCpuTemp {
    pub fn new(inner: Arc<dyn HardwareLayer>) -> Self {
        Self::with_var(inner, VAR)
    }

    fn with_var(inner: Arc<dyn HardwareLayer>, var: &'static str) -> Self {
        Self { inner, var }
    }

    fn injected(&self) -> Option<f32> {
        let v = std::env::var(self.var).ok()?;
        let v = v.trim();
        v.parse().ok().or_else(|| {
            std::fs::read_to_string(v)
                .ok()
                .and_then(|s| s.trim().parse().ok())
        })
    }
}

impl HardwareLayer for InjectCpuTemp {
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        let mut snap = self.inner.snapshot()?;
        if std::env::var(self.var).is_ok() {
            let t = self.injected().filter(|t| t.is_finite());
            let mut seen = false;
            for s in snap
                .sensors
                .iter_mut()
                .filter(|s| s.kind == SensorKind::Cpu)
            {
                s.temp_celsius = t.unwrap_or(s.temp_celsius);
                seen = true;
            }
            if !seen {
                // No CPU sensor on this machine (HWiNFO is optional): own one while armed, cool
                // when not injecting, because a CPU sensor that vanishes mid-latch never releases it.
                snap.sensors.push(SensorReading {
                    id: "inject_cpu".into(),
                    label: "Injected CPU".into(),
                    kind: SensorKind::Cpu,
                    temp_celsius: t.unwrap_or(40.0),
                });
            }
        }
        Ok(snap)
    }

    fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
        self.inner.set_fan_duty(fan, duty)
    }

    fn restore_auto(&self, fan: &str) -> Result<(), HalError> {
        self.inner.restore_auto(fan)
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn unavailable_sources(&self) -> Vec<String> {
        self.inner.unavailable_sources()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use thermaliq_hal::{MockHardwareLayer, Tick};

    fn wrap(var: &'static str, sensors: Vec<(&str, SensorKind, f32)>) -> InjectCpuTemp {
        let t = Tick::Temps(
            sensors
                .into_iter()
                .map(|(i, k, t)| (i.into(), k, t))
                .collect(),
        );
        InjectCpuTemp::with_var(Arc::new(MockHardwareLayer::new(&["f"], vec![t])), var)
    }

    fn temps(h: &InjectCpuTemp) -> Vec<f32> {
        h.snapshot()
            .unwrap()
            .sensors
            .iter()
            .map(|s| s.temp_celsius)
            .collect()
    }

    #[test]
    fn overrides_only_cpu_sensors_and_is_re_read_every_snapshot() {
        const V: &str = "TIQ_TEST_INJECT_A";
        let h = wrap(
            V,
            vec![
                ("cpu", SensorKind::Cpu, 40.0),
                ("gpu", SensorKind::Gpu, 50.0),
            ],
        );
        std::env::remove_var(V);
        assert_eq!(temps(&h), [40.0, 50.0]);
        std::env::set_var(V, "97");
        assert_eq!(temps(&h), [97.0, 50.0]);
        std::env::remove_var(V);
        assert_eq!(temps(&h), [40.0, 50.0]);
    }

    #[test]
    fn adds_a_cpu_sensor_when_the_machine_has_none() {
        const V: &str = "TIQ_TEST_INJECT_B";
        let h = wrap(V, vec![("gpu", SensorKind::Gpu, 50.0)]);
        std::env::set_var(V, "97");
        let s = h.snapshot().unwrap().sensors;
        std::env::remove_var(V);
        assert_eq!(s.len(), 2);
        assert!(s
            .iter()
            .any(|s| s.kind == SensorKind::Cpu && s.temp_celsius == 97.0));
    }

    #[test]
    fn an_armed_wrapper_keeps_a_cool_cpu_sensor_when_not_injecting() {
        const V: &str = "TIQ_TEST_INJECT_D";
        let h = wrap(V, vec![("gpu", SensorKind::Gpu, 50.0)]);
        std::env::remove_var(V);
        assert_eq!(temps(&h), [50.0]); // not armed: untouched
        std::env::set_var(V, "");
        assert_eq!(temps(&h), [50.0, 40.0]);
        std::env::remove_var(V);
    }

    #[test]
    fn a_file_value_is_read_each_time_and_junk_injects_nothing() {
        const V: &str = "TIQ_TEST_INJECT_C";
        let h = wrap(V, vec![("cpu", SensorKind::Cpu, 40.0)]);
        let f = tempfile::NamedTempFile::new().unwrap();
        std::env::set_var(V, f.path());
        std::fs::write(f.path(), "97\n").unwrap();
        assert_eq!(temps(&h), [97.0]);
        std::fs::write(f.path(), "").unwrap();
        assert_eq!(temps(&h), [40.0]);
        std::fs::write(f.path(), "hot").unwrap();
        assert_eq!(temps(&h), [40.0]);
        std::fs::write(f.path(), "NaN").unwrap();
        assert_eq!(temps(&h), [40.0]);
        std::env::set_var(V, "no-such-file");
        assert_eq!(temps(&h), [40.0]);
        std::env::remove_var(V);
    }
}

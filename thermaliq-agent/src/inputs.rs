//! Pull model/classifier inputs out of a snapshot by explicit sensor and metric ids.
//! Never "the first gpu_power": an iGPU + dGPU machine has two (PR #45 review). The
//! caller binds each id to its component. Anything missing or invalid gives `None`.

use thermaliq_common::{HardwareSnapshot, MetricKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Cpu,
    Gpu,
}

/// One model input row: temperature, mean duty of the fans that cool it, load %, power W.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub temp: f64,
    pub duty: f64,
    pub load: f64,
    pub power: f64,
}

/// Which readings belong to one controlled component.
#[derive(Debug, Clone)]
pub struct ComponentBinding {
    pub component: Component,
    pub temp_id: String,
    pub load_id: String,
    pub power_id: String,
    pub fan_ids: Vec<String>,
}

fn metric(snap: &HardwareSnapshot, id: &str, kind: MetricKind) -> Option<f64> {
    snap.metrics
        .iter()
        .find(|m| m.id == id && m.kind == kind && kind.is_valid(m.value))
        .map(|m| m.value)
}

impl ComponentBinding {
    pub fn sample(&self, snap: &HardwareSnapshot) -> Option<Sample> {
        let (load_kind, power_kind) = match self.component {
            Component::Cpu => (MetricKind::CpuLoad, MetricKind::CpuPower),
            Component::Gpu => (MetricKind::GpuLoad, MetricKind::GpuPower),
        };
        let temp = snap
            .sensors
            .iter()
            .find(|s| s.id == self.temp_id)?
            .temp_celsius as f64;
        if !temp.is_finite() || self.fan_ids.is_empty() {
            return None;
        }
        let mut duty = 0.0;
        for id in &self.fan_ids {
            duty += snap.fans.iter().find(|f| &f.id == id)?.duty_percent as f64;
        }
        Some(Sample {
            temp,
            duty: duty / self.fan_ids.len() as f64,
            load: metric(snap, &self.load_id, load_kind)?,
            power: metric(snap, &self.power_id, power_kind)?,
        })
    }
}

/// Ids of the metrics the workload classifier reads. HWiNFO is optional, so a machine
/// may have no CPU load or power: leave those `None`. A bound id that is missing in a
/// snapshot makes the whole reading unknown.
#[derive(Debug, Clone, Default)]
pub struct WorkloadBinding {
    pub cpu_load: Option<String>,
    pub gpu_load: Option<String>,
    pub cpu_power: Option<String>,
    pub gpu_power: Option<String>,
}

impl WorkloadBinding {
    /// Which of [cpu load, gpu load, cpu power, gpu power] this machine has.
    pub fn used(&self) -> [bool; 4] {
        [
            self.cpu_load.is_some(),
            self.gpu_load.is_some(),
            self.cpu_power.is_some(),
            self.gpu_power.is_some(),
        ]
    }

    /// The four features, 0.0 in the slots that are not bound (the classifier ignores
    /// them). `None` if a bound metric is missing or invalid, or nothing is bound.
    pub fn features(&self, snap: &HardwareSnapshot) -> Option<[f64; 4]> {
        let get = |id: &Option<String>, kind| match id {
            None => Some(0.0),
            Some(id) => metric(snap, id, kind),
        };
        self.used().contains(&true).then_some(())?;
        Some([
            get(&self.cpu_load, MetricKind::CpuLoad)?,
            get(&self.gpu_load, MetricKind::GpuLoad)?,
            get(&self.cpu_power, MetricKind::CpuPower)?,
            get(&self.gpu_power, MetricKind::GpuPower)?,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thermaliq_common::{FanKind, FanReading, MetricReading, SensorKind, SensorReading};

    fn m(id: &str, kind: MetricKind, value: f64) -> MetricReading {
        MetricReading {
            id: id.into(),
            kind,
            value,
        }
    }

    fn fan(id: &str, duty: u8) -> FanReading {
        FanReading {
            id: id.into(),
            label: id.into(),
            rpm: 1000,
            duty_percent: duty,
            kind: FanKind::Fan,
        }
    }

    /// iGPU + dGPU: two GpuPower and two GpuLoad readings.
    fn snap() -> HardwareSnapshot {
        HardwareSnapshot {
            sensors: vec![SensorReading {
                id: "nvml_gpu0_temp".into(),
                label: "GPU 0".into(),
                kind: SensorKind::Gpu,
                temp_celsius: 70.0,
            }],
            fans: vec![fan("f1", 40), fan("f2", 60)],
            metrics: vec![
                m("igpu_load", MetricKind::GpuLoad, 5.0),
                m("igpu_power", MetricKind::GpuPower, 3.0),
                m("nvml_gpu0_load", MetricKind::GpuLoad, 90.0),
                m("nvml_gpu0_power", MetricKind::GpuPower, 120.0),
            ],
        }
    }

    fn binding(power_id: &str) -> ComponentBinding {
        ComponentBinding {
            component: Component::Gpu,
            temp_id: "nvml_gpu0_temp".into(),
            load_id: "nvml_gpu0_load".into(),
            power_id: power_id.into(),
            fan_ids: vec!["f1".into(), "f2".into()],
        }
    }

    #[test]
    fn each_gpu_gets_its_own_metrics() {
        let s = binding("nvml_gpu0_power").sample(&snap()).expect("sample");
        assert_eq!((s.temp, s.duty, s.load, s.power), (70.0, 50.0, 90.0, 120.0));
        assert_eq!(
            binding("igpu_power").sample(&snap()).expect("sample").power,
            3.0
        );
    }

    #[test]
    fn missing_or_wrong_kind_is_none() {
        assert!(binding("nope").sample(&snap()).is_none());
        // right id, wrong kind: a CPU power id must not feed a GPU binding
        let mut s = snap();
        s.metrics[3].kind = MetricKind::CpuPower;
        assert!(binding("nvml_gpu0_power").sample(&s).is_none());
        let mut b = binding("nvml_gpu0_power");
        b.fan_ids.push("gone".into());
        assert!(b.sample(&snap()).is_none());
        b.fan_ids.clear();
        assert!(b.sample(&snap()).is_none());
    }

    #[test]
    fn invalid_values_are_none_not_zero() {
        let mut s = snap();
        s.metrics[2].value = f64::NAN;
        assert!(binding("nvml_gpu0_power").sample(&s).is_none());
        s.metrics[2].value = 140.0; // load > 100
        assert!(binding("nvml_gpu0_power").sample(&s).is_none());
        let mut s = snap();
        s.sensors[0].temp_celsius = f32::NAN;
        assert!(binding("nvml_gpu0_power").sample(&s).is_none());
    }

    #[test]
    fn workload_features_need_all_four() {
        let b = WorkloadBinding {
            cpu_load: Some("c".into()),
            gpu_load: Some("nvml_gpu0_load".into()),
            cpu_power: Some("cp".into()),
            gpu_power: Some("nvml_gpu0_power".into()),
        };
        assert!(b.features(&snap()).is_none());
        let mut s = snap();
        s.metrics.push(m("c", MetricKind::CpuLoad, 10.0));
        s.metrics.push(m("cp", MetricKind::CpuPower, 40.0));
        assert_eq!(b.features(&s), Some([10.0, 90.0, 40.0, 120.0]));
    }

    #[test]
    fn gpu_only_machine_still_has_features() {
        let b = WorkloadBinding {
            gpu_load: Some("nvml_gpu0_load".into()),
            gpu_power: Some("nvml_gpu0_power".into()),
            ..Default::default()
        };
        assert_eq!(b.used(), [false, true, false, true]);
        assert_eq!(b.features(&snap()), Some([0.0, 90.0, 0.0, 120.0]));
        assert!(WorkloadBinding::default().features(&snap()).is_none());
    }
}

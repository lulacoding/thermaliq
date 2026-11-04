//! NVIDIA GPU backend over NVML (SPEC §2, M10).

use crate::{Capabilities, HalError, HardwareLayer};
use std::time::Duration;
use thermaliq_common::HardwareSnapshot;

/// Every HAL call returns within this, success or error (PORTING M10).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// The NVML calls this backend needs, so tests can swap in a fake. Indices are NVML's.
pub trait Nvml: Send + 'static {
    fn gpu_count(&mut self) -> Result<u32, String>;
    /// GPU core temperature in °C.
    fn temperature(&mut self, gpu: u32) -> Result<u32, String>;
    /// 0 for a GPU without controllable fans.
    fn fan_count(&mut self, gpu: u32) -> Result<u32, String>;
    fn fan_percent(&mut self, gpu: u32, fan: u32) -> Result<u32, String>;
    fn fan_rpm(&mut self, gpu: u32, fan: u32) -> Result<u32, String>;
    fn set_fan_percent(&mut self, gpu: u32, fan: u32, percent: u32) -> Result<(), String>;
    /// `nvmlDeviceSetDefaultFanSpeed_v2`: back to the driver's own curve.
    fn set_default_fan(&mut self, gpu: u32, fan: u32) -> Result<(), String>;
}

pub struct Nvidia;

impl Nvidia {
    pub fn with_api(
        _make: impl FnOnce() -> Result<Box<dyn Nvml>, String> + Send + 'static,
        _timeout: Duration,
    ) -> Result<Self, HalError> {
        Err(HalError("not implemented".into()))
    }
}

impl HardwareLayer for Nvidia {
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use thermaliq_common::SensorKind;

    const FAST: Duration = Duration::from_millis(100);

    struct Fan {
        duty: u32,
        rpm: u32,
    }
    struct Gpu {
        temp: u32,
        fans: Vec<Fan>,
    }
    #[derive(Default)]
    struct World {
        gpus: Vec<Gpu>,
        calls: Vec<String>,
        fail: Vec<&'static str>,
        hang: bool,
    }

    /// Fake NVML: records calls, can fail an operation by name, can hang every call.
    #[derive(Clone)]
    struct Fake(Arc<Mutex<World>>);

    impl Fake {
        /// One entry per GPU: (temp °C, [(duty %, rpm)] per fan).
        fn new(gpus: &[(u32, &[(u32, u32)])]) -> Self {
            Self(Arc::new(Mutex::new(World {
                gpus: gpus
                    .iter()
                    .map(|(temp, fans)| Gpu {
                        temp: *temp,
                        fans: fans.iter().map(|&(duty, rpm)| Fan { duty, rpm }).collect(),
                    })
                    .collect(),
                ..World::default()
            })))
        }
        fn w(&self) -> std::sync::MutexGuard<'_, World> {
            self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
        fn calls(&self, prefix: &str) -> Vec<String> {
            self.w()
                .calls
                .iter()
                .filter(|c| c.starts_with(prefix))
                .cloned()
                .collect()
        }
        fn fail(&self, op: &'static str) {
            self.w().fail.push(op);
        }
        fn hang(&self, on: bool) {
            self.w().hang = on;
        }
        fn enter(&self, op: &'static str, args: String) -> Result<(), String> {
            while self.w().hang {
                std::thread::sleep(Duration::from_millis(5));
            }
            let mut w = self.w();
            w.calls.push(format!("{op} {args}"));
            if w.fail.contains(&op) {
                return Err(format!("injected {op} failure"));
            }
            Ok(())
        }
    }

    impl Nvml for Fake {
        fn gpu_count(&mut self) -> Result<u32, String> {
            self.enter("count", String::new())?;
            Ok(self.w().gpus.len() as u32)
        }
        fn temperature(&mut self, gpu: u32) -> Result<u32, String> {
            self.enter("temp", gpu.to_string())?;
            Ok(self.w().gpus[gpu as usize].temp)
        }
        fn fan_count(&mut self, gpu: u32) -> Result<u32, String> {
            self.enter("fans", gpu.to_string())?;
            Ok(self.w().gpus[gpu as usize].fans.len() as u32)
        }
        fn fan_percent(&mut self, gpu: u32, fan: u32) -> Result<u32, String> {
            self.enter("duty", format!("{gpu} {fan}"))?;
            Ok(self.w().gpus[gpu as usize].fans[fan as usize].duty)
        }
        fn fan_rpm(&mut self, gpu: u32, fan: u32) -> Result<u32, String> {
            self.enter("rpm", format!("{gpu} {fan}"))?;
            Ok(self.w().gpus[gpu as usize].fans[fan as usize].rpm)
        }
        fn set_fan_percent(&mut self, gpu: u32, fan: u32, percent: u32) -> Result<(), String> {
            self.enter("set", format!("{gpu} {fan} {percent}"))?;
            self.w().gpus[gpu as usize].fans[fan as usize].duty = percent;
            Ok(())
        }
        fn set_default_fan(&mut self, gpu: u32, fan: u32) -> Result<(), String> {
            self.enter("default", format!("{gpu} {fan}"))
        }
    }

    fn nvidia(fake: &Fake) -> Nvidia {
        let f = fake.clone();
        Nvidia::with_api(move || Ok(Box::new(f)), FAST).unwrap()
    }

    /// GPU 0: one fan at 30 %. GPU 1: two fans.
    fn two_gpus() -> Fake {
        Fake::new(&[(55, &[(30, 900)]), (70, &[(40, 1200), (50, 1500)])])
    }

    #[test]
    fn snapshot_has_one_gpu_sensor_per_gpu_and_every_fan() {
        let snap = nvidia(&two_gpus()).snapshot().unwrap();
        let sensors: Vec<_> = snap
            .sensors
            .iter()
            .map(|s| (s.id.as_str(), s.kind, s.temp_celsius))
            .collect();
        assert_eq!(
            sensors,
            [
                ("nvml_gpu0_temp", SensorKind::Gpu, 55.0),
                ("nvml_gpu1_temp", SensorKind::Gpu, 70.0)
            ]
        );
        let fans: Vec<_> = snap
            .fans
            .iter()
            .map(|f| (f.id.as_str(), f.duty_percent, f.rpm))
            .collect();
        assert_eq!(
            fans,
            [
                ("nvml_gpu0_fan0", 30, 900),
                ("nvml_gpu1_fan0", 40, 1200),
                ("nvml_gpu1_fan1", 50, 1500)
            ]
        );
    }

    #[test]
    fn capabilities_list_every_fan() {
        assert_eq!(
            nvidia(&two_gpus()).capabilities().controllable_fans,
            ["nvml_gpu0_fan0", "nvml_gpu1_fan0", "nvml_gpu1_fan1"]
        );
    }

    #[test]
    fn a_gpu_without_fans_still_reports_its_temperature() {
        let fake = Fake::new(&[(48, &[])]);
        let n = nvidia(&fake);
        let snap = n.snapshot().unwrap();
        assert_eq!(snap.sensors.len(), 1);
        assert!(snap.fans.is_empty());
        assert!(n.capabilities().controllable_fans.is_empty());
    }

    #[test]
    fn set_fan_duty_passes_device_fan_and_percent() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        n.set_fan_duty("nvml_gpu1_fan1", 60).unwrap();
        assert_eq!(fake.calls("set"), ["set 1 1 60"]);
        assert_eq!(n.snapshot().unwrap().fans[2].duty_percent, 60);
    }

    #[test]
    fn a_duty_over_100_is_clamped_by_the_backend() {
        let fake = two_gpus();
        nvidia(&fake).set_fan_duty("nvml_gpu0_fan0", 255).unwrap();
        assert_eq!(fake.calls("set"), ["set 0 0 100"]);
    }

    #[test]
    fn restore_auto_sets_the_default_fan_speed_and_never_a_duty() {
        let fake = two_gpus();
        nvidia(&fake).restore_auto("nvml_gpu1_fan0").unwrap();
        assert_eq!(fake.calls("default"), ["default 1 0"]);
        assert!(fake.calls("set").is_empty());
    }

    #[test]
    fn unknown_or_malformed_fan_ids_error_and_reach_no_device() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        for id in [
            "",
            "cpu_fan",
            "nvml_gpu9_fan0",
            "nvml_gpu0_fan1",
            "nvml_gpu00_fan0",
            "nvml_gpu0_fan0 ",
            "nvml_gpu0_temp",
            "nvml_gpu-1_fan0",
        ] {
            assert!(n.set_fan_duty(id, 50).is_err(), "set {id:?}");
            assert!(n.restore_auto(id).is_err(), "restore {id:?}");
        }
        assert!(fake.calls("set").is_empty() && fake.calls("default").is_empty());
    }

    #[test]
    fn nvml_errors_come_back_as_errors() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        fake.fail("set");
        assert!(n.set_fan_duty("nvml_gpu0_fan0", 50).is_err());
        fake.fail("default");
        assert!(n.restore_auto("nvml_gpu0_fan0").is_err());
        fake.fail("temp");
        assert!(n.snapshot().is_err());
    }

    #[test]
    fn a_failing_fan_read_fails_the_snapshot_instead_of_inventing_a_duty() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        fake.fail("duty");
        assert!(n.snapshot().is_err());
    }

    #[test]
    fn nvml_is_loaded_once_not_per_call() {
        let inits = Arc::new(AtomicUsize::new(0));
        let fake = two_gpus();
        let (i, f) = (inits.clone(), fake.clone());
        let n = Nvidia::with_api(
            move || {
                i.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(f))
            },
            FAST,
        )
        .unwrap();
        for _ in 0..5 {
            n.snapshot().unwrap();
            n.set_fan_duty("nvml_gpu0_fan0", 40).unwrap();
            n.restore_auto("nvml_gpu0_fan0").unwrap();
        }
        assert_eq!(inits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_init_is_an_error_naming_the_cause() {
        let err = Nvidia::with_api(|| Err("nvml.dll not found".into()), FAST)
            .err()
            .unwrap();
        assert!(err.0.contains("nvml.dll not found"), "{err}");
    }

    #[test]
    fn every_call_errors_inside_the_timeout_when_nvml_hangs() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        fake.hang(true);
        let t0 = Instant::now();
        assert!(n.snapshot().is_err());
        assert!(n.set_fan_duty("nvml_gpu0_fan0", 50).is_err());
        assert!(n.restore_auto("nvml_gpu0_fan0").is_err());
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
        fake.hang(false);
    }

    #[test]
    fn a_write_that_timed_out_is_dropped_not_applied_late() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        fake.hang(true);
        assert!(n.snapshot().is_err()); // the worker is now stuck inside this
        assert!(n.set_fan_duty("nvml_gpu0_fan0", 10).is_err()); // queued behind it
        fake.hang(false);
        n.snapshot().unwrap(); // runs after the queued write was dealt with
        assert!(fake.calls("set").is_empty(), "{:?}", fake.calls("set"));
    }

    #[test]
    fn a_restore_that_timed_out_still_happens_once_nvml_answers() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        fake.hang(true);
        assert!(n.snapshot().is_err());
        assert!(n.restore_auto("nvml_gpu0_fan0").is_err());
        fake.hang(false);
        n.snapshot().unwrap();
        assert_eq!(fake.calls("default"), ["default 0 0"]);
    }

    #[test]
    fn dropping_does_not_wait_for_a_stuck_call() {
        let fake = two_gpus();
        let n = nvidia(&fake);
        fake.hang(true);
        assert!(n.snapshot().is_err());
        let t0 = Instant::now();
        drop(n);
        assert!(t0.elapsed() < Duration::from_millis(500));
        fake.hang(false);
    }
}

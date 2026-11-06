//! NVIDIA GPU backend over NVML (SPEC §2, M10).

use crate::{Capabilities, HalError, HardwareLayer};
use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use thermaliq_common::{
    FanKind, FanReading, HardwareSnapshot, MetricKind, MetricReading, SensorKind, SensorReading,
};

/// Every HAL call returns within this, success or error (PORTING M10).
#[cfg(windows)]
pub const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// The NVML calls this backend needs, so tests can swap in a fake. Indices are NVML's.
pub trait Nvml: Send + 'static {
    fn gpu_count(&mut self) -> Result<u32, String>;
    /// NVML's UUID for the GPU: unlike the index, it does not change when the GPUs reorder.
    fn uuid(&mut self, gpu: u32) -> Result<String, String>;
    /// GPU core temperature in °C.
    fn temperature(&mut self, gpu: u32) -> Result<u32, String>;
    /// GPU core load in %.
    fn utilization(&mut self, gpu: u32) -> Result<u32, String>;
    /// Board power draw in milliwatts.
    fn power_mw(&mut self, gpu: u32) -> Result<u32, String>;
    /// Graphics clock in MHz.
    fn graphics_clock(&mut self, gpu: u32) -> Result<u32, String>;
    /// NVML's current clock-throttle-reason bitmask (0 = not throttled).
    fn throttle_reasons(&mut self, gpu: u32) -> Result<u64, String>;
    /// 0 for a GPU without controllable fans.
    fn fan_count(&mut self, gpu: u32) -> Result<u32, String>;
    fn fan_percent(&mut self, gpu: u32, fan: u32) -> Result<u32, String>;
    fn fan_rpm(&mut self, gpu: u32, fan: u32) -> Result<u32, String>;
    fn set_fan_percent(&mut self, gpu: u32, fan: u32, percent: u32) -> Result<(), String>;
    /// `nvmlDeviceSetDefaultFanSpeed_v2`: back to the driver's own curve.
    fn set_default_fan(&mut self, gpu: u32, fan: u32) -> Result<(), String>;
}

/// Loading `nvml.dll` and `nvmlInit` can be slow on a cold start; this is not a per-tick call.
const INIT_TIMEOUT: Duration = Duration::from_secs(10);

type Job = Box<dyn FnOnce(&mut dyn Nvml) + Send>;

/// NVML runs on one worker thread that owns the library, so it is loaded and initialised
/// once, and a wedged driver call only stalls that thread: callers give up at the deadline.
pub struct Nvidia {
    jobs: Sender<Job>,
    timeout: Duration,
    /// (fan id, gpu index, fan index) for every fan found at startup.
    fans: Vec<(String, u32, u32)>,
    /// Metric warnings already logged, so a permanent failure prints once, not every tick.
    warned: Mutex<HashSet<String>>,
}

fn fan_id(uuid: &str, fan: u32) -> String {
    format!("nvml_{uuid}_fan{fan}")
}

/// The GPU's UUID, which goes into every id. An empty one or one with `_` would make the ids
/// ambiguous (SPEC §5.2), so it is an error, not a guess.
fn gpu_uuid(api: &mut dyn Nvml, gpu: u32) -> Result<String, String> {
    let uuid = api.uuid(gpu)?;
    if uuid.is_empty() || uuid.contains(['_', ' ']) {
        return Err(format!("GPU {gpu} has an unusable UUID {uuid:?}"));
    }
    Ok(uuid)
}

fn enumerate(api: &mut dyn Nvml) -> Result<Vec<(String, u32, u32)>, String> {
    let mut fans = Vec::new();
    for gpu in 0..api.gpu_count()? {
        let uuid = gpu_uuid(api, gpu)?;
        for fan in 0..api.fan_count(gpu)? {
            fans.push((fan_id(&uuid, fan), gpu, fan));
        }
    }
    Ok(fans)
}

/// The snapshot plus why any metric was left out (the caller logs each reason once).
fn read_all(api: &mut dyn Nvml) -> Result<(HardwareSnapshot, Vec<String>), String> {
    let mut snap = HardwareSnapshot::default();
    let mut warnings = Vec::new();
    for gpu in 0..api.gpu_count()? {
        let uuid = gpu_uuid(api, gpu)?;
        snap.sensors.push(SensorReading {
            id: format!("nvml_{uuid}_temp"),
            label: format!("GPU {gpu}"),
            kind: SensorKind::Gpu,
            temp_celsius: api.temperature(gpu)? as f32,
        });
        // A metric NVML can't give, or gives out of range, is left out, never reported as 0.
        let metrics = [
            (
                "load",
                MetricKind::GpuLoad,
                api.utilization(gpu).map(f64::from),
            ),
            (
                "power",
                MetricKind::GpuPower,
                api.power_mw(gpu).map(|mw| f64::from(mw) / 1000.0),
            ),
            (
                "clock",
                MetricKind::GpuClock,
                api.graphics_clock(gpu).map(f64::from),
            ),
            (
                "throttle",
                MetricKind::GpuThrottle,
                api.throttle_reasons(gpu).map(|b| b as f64),
            ),
        ];
        for (name, kind, value) in metrics {
            let id = format!("nvml_{uuid}_{name}");
            match value {
                Ok(value) if kind.is_valid(value) => {
                    snap.metrics.push(MetricReading { id, kind, value })
                }
                Ok(value) => warnings.push(format!("{id}: out-of-range value {value}")),
                Err(e) => warnings.push(format!("{id}: {e}")),
            }
        }
        for fan in 0..api.fan_count(gpu)? {
            snap.fans.push(FanReading {
                id: fan_id(&uuid, fan),
                label: format!("GPU {gpu} fan {fan}"),
                // Older drivers have no RPM query; the duty is still real.
                rpm: api.fan_rpm(gpu, fan).unwrap_or(0),
                duty_percent: api.fan_percent(gpu, fan)?.min(100) as u8,
                kind: FanKind::Fan,
            });
        }
    }
    Ok((snap, warnings))
}

impl Nvidia {
    /// `make` runs on the worker thread, once. `timeout` bounds every later call.
    pub fn with_api(
        make: impl FnOnce() -> Result<Box<dyn Nvml>, String> + Send + 'static,
        timeout: Duration,
    ) -> Result<Self, HalError> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let (ready, started) = mpsc::channel();
        thread::Builder::new()
            .name("nvml".into())
            .spawn(move || {
                let mut api = match make() {
                    Ok(api) => api,
                    Err(e) => return drop(ready.send(Err(e))),
                };
                let _ = ready.send(Ok(()));
                // Ends when `Nvidia` is dropped (the sender goes away).
                for job in queue {
                    job(api.as_mut());
                }
            })
            .map_err(|e| HalError(format!("cannot start the NVML thread: {e}")))?;
        match started.recv_timeout(INIT_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(HalError(e)),
            Err(_) => return Err(HalError("NVML did not initialise in time".into())),
        }
        let mut nvidia = Self {
            jobs,
            timeout,
            fans: Vec::new(),
            warned: Mutex::default(),
        };
        nvidia.fans = nvidia.call(true, enumerate)?;
        Ok(nvidia)
    }

    /// Runs `f` on the worker and waits up to the timeout. A job still queued after its
    /// deadline is skipped unless `late_ok`: a late write would act on a stale decision,
    /// but a late hand-back is still wanted.
    fn call<T: Send + 'static>(
        &self,
        late_ok: bool,
        f: impl FnOnce(&mut dyn Nvml) -> Result<T, String> + Send + 'static,
    ) -> Result<T, HalError> {
        let deadline = Instant::now() + self.timeout;
        let (reply, answer) = mpsc::channel();
        let job: Job = Box::new(move |api| {
            if late_ok || Instant::now() < deadline {
                let _ = reply.send(f(api));
            }
        });
        self.jobs
            .send(job)
            .map_err(|_| HalError("the NVML thread has stopped".into()))?;
        match answer.recv_timeout(self.timeout) {
            Ok(r) => r.map_err(HalError),
            Err(RecvTimeoutError::Timeout) => Err(HalError(format!(
                "NVML call timed out after {:?}",
                self.timeout
            ))),
            Err(RecvTimeoutError::Disconnected) => {
                Err(HalError("the NVML thread stopped during the call".into()))
            }
        }
    }

    fn find(&self, fan: &str) -> Result<(u32, u32), HalError> {
        self.fans
            .iter()
            .find(|(id, ..)| id == fan)
            .map(|&(_, gpu, idx)| (gpu, idx))
            .ok_or_else(|| HalError(format!("unknown NVIDIA fan '{fan}'")))
    }
}

impl HardwareLayer for Nvidia {
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        let (snap, warnings) = self.call(true, read_all)?;
        let mut warned = self.warned.lock().unwrap_or_else(|e| e.into_inner());
        for w in warnings {
            if warned.insert(w.clone()) {
                eprintln!("thermaliq: NVML metric skipped: {w}");
            }
        }
        Ok(snap)
    }

    fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError> {
        let (gpu, idx) = self.find(fan)?;
        let percent = u32::from(duty.min(100));
        self.call(false, move |api| api.set_fan_percent(gpu, idx, percent))
    }

    fn restore_auto(&self, fan: &str) -> Result<(), HalError> {
        let (gpu, idx) = self.find(fan)?;
        self.call(true, move |api| api.set_default_fan(gpu, idx))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            controllable_fans: self.fans.iter().map(|(id, ..)| id.clone()).collect(),
        }
    }
}

#[cfg(windows)]
mod real {
    use super::*;
    use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
    use nvml_wrapper::error::NvmlError;

    /// The real library, through nvml-wrapper's safe API (no `unsafe` in this crate).
    struct RealNvml(nvml_wrapper::Nvml);

    fn msg(e: NvmlError) -> String {
        format!("NVML: {e}")
    }

    impl RealNvml {
        fn device(&self, gpu: u32) -> Result<nvml_wrapper::Device<'_>, String> {
            self.0.device_by_index(gpu).map_err(msg)
        }
    }

    impl Nvml for RealNvml {
        fn gpu_count(&mut self) -> Result<u32, String> {
            self.0.device_count().map_err(msg)
        }
        fn uuid(&mut self, gpu: u32) -> Result<String, String> {
            self.device(gpu)?.uuid().map_err(msg)
        }
        fn temperature(&mut self, gpu: u32) -> Result<u32, String> {
            self.device(gpu)?
                .temperature(TemperatureSensor::Gpu)
                .map_err(msg)
        }
        fn utilization(&mut self, gpu: u32) -> Result<u32, String> {
            Ok(self.device(gpu)?.utilization_rates().map_err(msg)?.gpu)
        }
        fn power_mw(&mut self, gpu: u32) -> Result<u32, String> {
            self.device(gpu)?.power_usage().map_err(msg)
        }
        fn graphics_clock(&mut self, gpu: u32) -> Result<u32, String> {
            self.device(gpu)?.clock_info(Clock::Graphics).map_err(msg)
        }
        fn throttle_reasons(&mut self, gpu: u32) -> Result<u64, String> {
            Ok(self
                .device(gpu)?
                .current_throttle_reasons()
                .map_err(msg)?
                .bits())
        }
        fn fan_count(&mut self, gpu: u32) -> Result<u32, String> {
            match self.device(gpu)?.num_fans() {
                Err(NvmlError::NotSupported) => Ok(0),
                r => r.map_err(msg),
            }
        }
        fn fan_percent(&mut self, gpu: u32, fan: u32) -> Result<u32, String> {
            self.device(gpu)?.fan_speed(fan).map_err(msg)
        }
        fn fan_rpm(&mut self, gpu: u32, fan: u32) -> Result<u32, String> {
            self.device(gpu)?.fan_speed_rpm(fan).map_err(msg)
        }
        fn set_fan_percent(&mut self, gpu: u32, fan: u32, percent: u32) -> Result<(), String> {
            self.device(gpu)?.set_fan_speed(fan, percent).map_err(msg)
        }
        fn set_default_fan(&mut self, gpu: u32, fan: u32) -> Result<(), String> {
            self.device(gpu)?.set_default_fan_speed(fan).map_err(msg)
        }
    }

    impl Nvidia {
        /// Loads `nvml.dll` and calls `nvmlInit` once, on the worker thread.
        pub fn new() -> Result<Self, HalError> {
            Self::with_api(
                || {
                    let nvml = nvml_wrapper::Nvml::init()
                        .map_err(|e| format!("cannot load or initialise NVML: {e}"))?;
                    Ok(Box::new(RealNvml(nvml)))
                },
                CALL_TIMEOUT,
            )
        }
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
        uuid: String,
        temp: u32,
        fans: Vec<Fan>,
        load: u32,
        power_mw: u32,
        clock: u32,
        throttle: u64,
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
                    .enumerate()
                    .map(|(i, (temp, fans))| Gpu {
                        uuid: format!("gpu{i}"),
                        temp: *temp,
                        fans: fans.iter().map(|&(duty, rpm)| Fan { duty, rpm }).collect(),
                        load: 37,
                        power_mw: 120_500,
                        clock: 1800,
                        throttle: 4,
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
        fn uuid(&mut self, gpu: u32) -> Result<String, String> {
            Ok(self.w().gpus[gpu as usize].uuid.clone())
        }
        fn temperature(&mut self, gpu: u32) -> Result<u32, String> {
            self.enter("temp", gpu.to_string())?;
            Ok(self.w().gpus[gpu as usize].temp)
        }
        fn utilization(&mut self, gpu: u32) -> Result<u32, String> {
            self.enter("load", gpu.to_string())?;
            Ok(self.w().gpus[gpu as usize].load)
        }
        fn power_mw(&mut self, gpu: u32) -> Result<u32, String> {
            self.enter("power", gpu.to_string())?;
            Ok(self.w().gpus[gpu as usize].power_mw)
        }
        fn graphics_clock(&mut self, gpu: u32) -> Result<u32, String> {
            self.enter("clock", gpu.to_string())?;
            Ok(self.w().gpus[gpu as usize].clock)
        }
        fn throttle_reasons(&mut self, gpu: u32) -> Result<u64, String> {
            self.enter("throttle", gpu.to_string())?;
            Ok(self.w().gpus[gpu as usize].throttle)
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

    fn metrics(snap: &HardwareSnapshot) -> Vec<(&str, f64)> {
        snap.metrics
            .iter()
            .map(|m| (m.id.as_str(), m.value))
            .collect()
    }

    #[test]
    fn snapshot_has_load_power_clock_and_throttle_per_gpu() {
        let snap = nvidia(&Fake::new(&[(55, &[(30, 900)])]))
            .snapshot()
            .unwrap();
        assert_eq!(
            metrics(&snap),
            [
                ("nvml_gpu0_load", 37.0),
                ("nvml_gpu0_power", 120.5),
                ("nvml_gpu0_clock", 1800.0),
                ("nvml_gpu0_throttle", 4.0)
            ]
        );
        let kinds: Vec<_> = snap.metrics.iter().map(|m| m.kind).collect();
        assert_eq!(
            kinds,
            [
                MetricKind::GpuLoad,
                MetricKind::GpuPower,
                MetricKind::GpuClock,
                MetricKind::GpuThrottle
            ]
        );
        assert!(snap.fans.iter().all(|f| f.kind == FanKind::Fan));
    }

    #[test]
    fn a_metric_that_fails_is_skipped_not_zeroed_and_the_rest_survive() {
        let fake = Fake::new(&[(55, &[(30, 900)])]);
        fake.fail("power");
        fake.fail("throttle");
        let snap = nvidia(&fake).snapshot().unwrap();
        assert_eq!(
            metrics(&snap),
            [("nvml_gpu0_load", 37.0), ("nvml_gpu0_clock", 1800.0)]
        );
        assert_eq!(snap.sensors.len(), 1);
        assert_eq!(snap.fans.len(), 1);
    }

    #[test]
    fn a_metric_nvml_does_not_support_is_logged_once_per_gpu_not_every_tick() {
        let fake = Fake::new(&[(55, &[]), (60, &[])]);
        fake.fail("power");
        let n = nvidia(&fake);
        for _ in 0..5 {
            let snap = n.snapshot().unwrap();
            assert!(!metrics(&snap).iter().any(|(id, _)| id.ends_with("power")));
        }
        let mut warned: Vec<_> = n.warned.lock().unwrap().iter().cloned().collect();
        warned.sort();
        assert_eq!(
            warned,
            [
                "nvml_gpu0_power: injected power failure",
                "nvml_gpu1_power: injected power failure"
            ]
        );
    }

    #[test]
    fn an_out_of_range_load_is_skipped() {
        let fake = Fake::new(&[(55, &[])]);
        fake.w().gpus[0].load = 101;
        let snap = nvidia(&fake).snapshot().unwrap();
        assert!(!metrics(&snap).iter().any(|(id, _)| id.ends_with("load")));
    }

    /// The same two cards, enumerated in either order (a BIOS update, a moved slot).
    fn cards(order: [usize; 2]) -> Fake {
        let fake = Fake::new(&[(55, &[(30, 900)]), (70, &[(40, 1200), (50, 1500)])]);
        for (gpu, uuid) in fake.w().gpus.iter_mut().zip(["GPU-aaa", "GPU-bbb"]) {
            gpu.uuid = uuid.into();
        }
        fake.w()
            .gpus
            .sort_by_key(|g| order[(g.uuid == "GPU-bbb") as usize]);
        fake
    }

    #[test]
    fn ids_follow_the_gpu_uuid_not_its_enumeration_index() {
        let ids = |order| {
            let n = nvidia(&cards(order));
            let mut ids = n.capabilities().controllable_fans;
            ids.extend(n.snapshot().unwrap().sensors.into_iter().map(|s| s.id));
            ids.sort();
            ids
        };
        let expected = [
            "nvml_GPU-aaa_fan0",
            "nvml_GPU-aaa_temp",
            "nvml_GPU-bbb_fan0",
            "nvml_GPU-bbb_fan1",
            "nvml_GPU-bbb_temp",
        ];
        assert_eq!(ids([0, 1]), expected);
        assert_eq!(ids([1, 0]), expected);
    }

    #[test]
    fn a_write_after_a_reorder_reaches_the_card_the_id_names() {
        let fake = cards([1, 0]); // GPU-bbb is index 0 now
        nvidia(&fake).set_fan_duty("nvml_GPU-aaa_fan0", 60).unwrap();
        assert_eq!(fake.calls("set"), ["set 1 0 60"]);
    }

    #[test]
    fn a_gpu_with_an_unusable_uuid_is_not_started() {
        for bad in ["", "GPU_1", "GPU 1"] {
            let fake = Fake::new(&[(55, &[(30, 900)])]);
            fake.w().gpus[0].uuid = bad.into();
            let f = fake.clone();
            let err = Nvidia::with_api(move || Ok(Box::new(f)), FAST).err();
            assert!(err.is_some_and(|e| e.0.contains("UUID")), "{bad:?}");
        }
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

    /// Hands the fan back even if an assertion in the hardware test fails.
    #[cfg(windows)]
    struct Restore<'a>(&'a Nvidia, String);
    #[cfg(windows)]
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            let _ = self.0.restore_auto(&self.1);
        }
    }

    /// Real GPU. Run: `cargo test -p thermaliq-hal -- --ignored nvidia --nocapture`. Needs an
    /// elevated shell; moves the first controllable fan to 75 % (or 35 %), kept
    /// inside the range the driver allows, until it gets there (20 s at most).
    #[cfg(windows)]
    #[test]
    #[ignore = "needs an NVIDIA GPU with a controllable fan and admin rights"]
    fn nvidia_hardware_reads_writes_and_restores_a_real_fan() {
        use nvml_wrapper::enums::device::FanControlPolicy;

        let hw = Nvidia::new().unwrap();
        let snap = hw.snapshot().unwrap();
        let gpu = snap.sensors.iter().find(|s| s.kind == SensorKind::Gpu);
        let temp = gpu.expect("no GPU temperature").temp_celsius;
        assert!((10.0..110.0).contains(&temp), "implausible GPU temp {temp}");
        let id = hw.capabilities().controllable_fans.remove(0);
        let duty = |hw: &Nvidia| {
            let fans = hw.snapshot().unwrap().fans;
            fans.iter().find(|f| f.id == id).unwrap().duty_percent
        };
        let before = duty(&hw);
        let _restore = Restore(&hw, id.clone());

        let (uuid, f) = id
            .strip_prefix("nvml_")
            .and_then(|r| r.rsplit_once("_fan"))
            .unwrap();
        let nvml = nvml_wrapper::Nvml::init().unwrap();
        let dev = nvml.device_by_uuid(uuid).unwrap();
        // The driver clamps writes to the range the card allows, so aim inside it.
        let (min, max) = dev.min_max_fan_speed().unwrap();
        println!("{id}: driver allows {min}-{max} %, now {before} %");
        // At least 20 points from where it is now, so a reading at the target proves movement.
        let target = [75.min(max), 35.max(min)]
            .into_iter()
            .find(|t| before.abs_diff(*t as u8) >= 20)
            .expect("no target 20+ points from the current duty inside the allowed range");
        let target = target as u8;
        hw.set_fan_duty(&id, target).unwrap();
        // The readback follows the fan's own slew (0 -> 65 % took 5 s on the dev card), so wait
        // for the target instead of reading once.
        let mut during = before;
        for s in 1..=20 {
            std::thread::sleep(Duration::from_secs(1));
            during = duty(&hw);
            println!("  +{s} s: {during} %");
            if during.abs_diff(target) <= 2 {
                break;
            }
        }
        println!("{id}: {before} % -> {during} % (asked for {target})");
        assert!(
            during.abs_diff(target) <= 2,
            "fan did not move to {target} %: {during}"
        );

        hw.restore_auto(&id).unwrap();
        let policy = dev.fan_control_policy(f.parse().unwrap()).unwrap();
        assert_eq!(policy, FanControlPolicy::TemperatureContinousSw);
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

//! Built-in CPU load %, read-only. Needs no HWiNFO: it is the agent's load input when
//! HWiNFO is absent (SPEC §7).

use crate::{Capabilities, HalError, HardwareLayer};
use std::sync::{Mutex, PoisonError};
use thermaliq_common::{HardwareSnapshot, MetricKind, MetricReading};

pub const CPU_LOAD_ID: &str = "win_cpu_load";

/// Cumulative `(idle, total)` CPU time, in any consistent unit.
type Times = (u64, u64);

pub struct CpuLoad {
    read: Box<dyn Fn() -> Option<Times> + Send + Sync>,
    last: Mutex<Option<Times>>,
}

#[cfg(windows)]
impl Default for CpuLoad {
    /// The OS counters. Windows only: elsewhere v2.0 has no backend at all.
    fn default() -> Self {
        Self::with_reader(system_times)
    }
}

impl CpuLoad {
    #[cfg(windows)]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(any(windows, test))]
    fn with_reader(read: impl Fn() -> Option<Times> + Send + Sync + 'static) -> Self {
        Self {
            read: Box::new(read),
            last: Mutex::new(None),
        }
    }
}

/// Load over the interval between two reads. `None` when the counters went backwards or did
/// not advance: no reading, never a made-up 0 (an idle PC and a missing metric differ).
fn load_pct(prev: Times, cur: Times) -> Option<f64> {
    let total = cur.1.checked_sub(prev.1).filter(|&t| t > 0)?;
    let idle = cur.0.checked_sub(prev.0).filter(|&i| i <= total)?;
    let pct = 100.0 * (1.0 - idle as f64 / total as f64);
    MetricKind::CpuLoad.is_valid(pct).then_some(pct)
}

impl HardwareLayer for CpuLoad {
    /// The first call only primes the counters, so it reports no metric.
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
        let cur = (self.read)();
        let prev = std::mem::replace(
            &mut *self.last.lock().unwrap_or_else(PoisonError::into_inner),
            cur,
        );
        let metrics = prev
            .zip(cur)
            .and_then(|(p, c)| load_pct(p, c))
            .map(|value| MetricReading {
                id: CPU_LOAD_ID.into(),
                kind: MetricKind::CpuLoad,
                value,
            })
            .into_iter()
            .collect();
        Ok(HardwareSnapshot {
            metrics,
            ..HardwareSnapshot::default()
        })
    }

    fn set_fan_duty(&self, _: &str, _: u8) -> Result<(), HalError> {
        Err(HalError("CPU load is read-only".into()))
    }

    fn restore_auto(&self, _: &str) -> Result<(), HalError> {
        Err(HalError("CPU load is read-only".into()))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
}

/// shortcut: with more than 64 logical processors `GetSystemTimes` covers only the calling
/// thread's processor group, so load is wrong there (e.g. a 64-core/128-thread Threadripper);
/// upgrade to PDH `\Processor Information(_Total)\% Processor Time` if that ever matters.
#[cfg(windows)]
#[allow(unsafe_code)]
fn system_times() -> Option<Times> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (zero, zero, zero);
    // SAFETY: three valid, writable FILETIMEs that outlive the call.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    // Kernel time already includes idle time.
    Some((t(idle), t(kernel) + t(user)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A reader that returns the given reads in order, then `None`.
    fn seq(reads: Vec<Option<Times>>) -> CpuLoad {
        let reads = Arc::new(Mutex::new(reads.into_iter()));
        CpuLoad::with_reader(move || reads.lock().unwrap().next().flatten())
    }

    fn values(c: &CpuLoad) -> Vec<f64> {
        c.snapshot()
            .unwrap()
            .metrics
            .iter()
            .map(|m| m.value)
            .collect()
    }

    #[test]
    fn the_first_read_only_primes_and_reports_nothing() {
        let c = seq(vec![Some((0, 0)), Some((50, 100))]);
        assert!(values(&c).is_empty());
        assert_eq!(values(&c), [50.0]);
    }

    #[test]
    fn load_is_busy_share_of_the_interval_with_id_and_kind() {
        let c = seq(vec![Some((100, 200)), Some((100, 300)), Some((175, 400))]);
        c.snapshot().unwrap();
        assert_eq!(values(&c), [100.0]); // never idle
        assert_eq!(values(&c), [25.0]); // 75 idle of 100
        let c = seq(vec![Some((0, 0)), Some((75, 100))]);
        c.snapshot().unwrap();
        let m = &c.snapshot().unwrap().metrics[0];
        assert_eq!(
            (m.id.as_str(), m.kind, m.value),
            ("win_cpu_load", MetricKind::CpuLoad, 25.0)
        );
    }

    #[test]
    fn a_failed_read_is_skipped_not_zeroed_and_reprimes() {
        let c = seq(vec![Some((0, 0)), None, Some((10, 100)), Some((60, 200))]);
        c.snapshot().unwrap();
        assert!(values(&c).is_empty()); // read failed
        assert!(values(&c).is_empty()); // no valid previous, so primes again
        assert_eq!(values(&c), [50.0]);
    }

    #[test]
    fn bad_counters_give_no_reading() {
        assert_eq!(load_pct((0, 100), (0, 100)), None); // no time passed
        assert_eq!(load_pct((0, 100), (0, 50)), None); // total went backwards
        assert_eq!(load_pct((50, 100), (40, 200)), None); // idle went backwards
        assert_eq!(load_pct((0, 100), (200, 200)), None); // more idle than time
        assert_eq!(load_pct((0, 0), (0, 10)), Some(100.0));
    }

    #[test]
    fn it_is_read_only() {
        let c = seq(vec![]);
        assert!(c.set_fan_duty("f", 50).is_err());
        assert!(c.restore_auto("f").is_err());
        assert!(c.capabilities().controllable_fans.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn the_real_counters_give_a_valid_load() {
        let c = CpuLoad::new();
        c.snapshot().unwrap();
        // Busy-spin so the counters advance past their coarse (~15 ms) granularity.
        let end = std::time::Instant::now() + std::time::Duration::from_millis(100);
        while std::time::Instant::now() < end {}
        let m = c.snapshot().unwrap().metrics;
        assert_eq!(m.len(), 1);
        assert!(MetricKind::CpuLoad.is_valid(m[0].value));
    }
}

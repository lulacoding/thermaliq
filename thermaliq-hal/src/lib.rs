//! `HardwareLayer` trait, mock backend, platform backends. See SPEC.md §5.
//!
//! Only `thermaliq-engine` may call `set_fan_duty` (SPEC S1).

mod combined;
mod cpu_load;
mod hwinfo;
#[cfg(windows)]
#[allow(unsafe_code)]
mod hwinfo_win;
mod lhm;
mod mock;
mod nvidia;

pub use combined::CombinedHardwareLayer;
pub use cpu_load::CpuLoad;
pub use hwinfo::{Hwinfo, HwinfoError};
pub use lhm::{BridgeConnection, BridgeStarter, LhmBridge, Timing};
pub use mock::{MockHardwareLayer, Tick};
pub use nvidia::{Nvidia, Nvml};

use std::fmt;
use thermaliq_common::HardwareSnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HalError(pub String);

impl fmt::Display for HalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HalError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// IDs of fans this backend can write to.
    pub controllable_fans: Vec<String>,
}

pub trait HardwareLayer: Send + Sync {
    fn snapshot(&self) -> Result<HardwareSnapshot, HalError>;
    /// Callers pass a duty already clamped 0–100 (S7); backends must not rely on it.
    fn set_fan_duty(&self, fan: &str, duty: u8) -> Result<(), HalError>;
    /// Hand the fan back to firmware/driver control (S5).
    fn restore_auto(&self, fan: &str) -> Result<(), HalError>;
    fn capabilities(&self) -> Capabilities;
    /// Names of optional sources that failed on the last snapshot and were left out of it.
    fn unavailable_sources(&self) -> Vec<String> {
        Vec::new()
    }
    /// Hardware faults the daemon must report as `degraded`, such as a fan header that would not
    /// go back to the BIOS (SPEC §5.1).
    fn degraded_reasons(&self) -> Vec<String> {
        Vec::new()
    }
    /// Names the motherboard and its fan chip, for the saved inventory (SPEC §5.2). `None` when
    /// no backend knows the board.
    fn board(&self) -> Option<String> {
        None
    }
}

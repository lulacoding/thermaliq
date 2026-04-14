//! Linux hardware backend.
//!
//! Sensor reads: hwmon sysfs (`/sys/class/hwmon/hwmonX/tempY_input`).
//!
//! Fan control:
//! - Most GPUs / case fans: `/sys/class/hwmon/hwmonX/pwm1` + `pwm1_enable=1`
//! - RDNA3 (RX 7000 series): separate API at
//!   `/sys/class/drm/cardX/device/gpu_od/fan_ctrl/fan_curve`.
//!   Requires kernel 6.7+ and `amdgpu.ppfeaturemask=0xffffffff` boot param.
//!   Standard hwmon pwm1 path is IGNORED on RDNA3 — silently no-ops.
//!   Linux 6.13 also adds `fan_zero_rpm` and `fan_zero_rpm_stop_temperature`
//!   for RX 7000 series zero-RPM mode control.

use anyhow::Result;
use async_trait::async_trait;
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use crate::HardwareLayer;

pub struct LinuxHardwareLayer;

impl LinuxHardwareLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxHardwareLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HardwareLayer for LinuxHardwareLayer {
    async fn snapshot(&self) -> Result<HardwareSnapshot> {
        // TODO(v0.1.0): enumerate /sys/class/hwmon, read tempY_input files,
        // detect RDNA3 via PCI device ID and route to gpu_od fan_curve API.
        todo!("LinuxHardwareLayer::snapshot not yet implemented")
    }

    async fn set_fan_duty(&self, _cmd: SetFanDuty) -> Result<()> {
        // TODO(v0.1.0): write pwm1_enable=1 then pwm1=<value>,
        // or write to gpu_od/fan_ctrl/fan_curve for RDNA3.
        todo!("LinuxHardwareLayer::set_fan_duty not yet implemented")
    }
}

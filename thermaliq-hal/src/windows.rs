//! Windows hardware backend.
//!
//! ## Driver Status (April 2026) — Decision Required
//!
//! LibreHardwareMonitor (LHM) depends on WinRing0.sys. As of March–April 2025,
//! Microsoft Defender classifies WinRing0.sys as `VulnerableDriver:WinNT/
//! Winring0` (CVE-2020-14979) and **automatically quarantines it** on user
//! machines. FanControl migrated to PawnIO in Sept 2025, but PawnIO is blocked
//! by FACEIT, Vanguard, and other anti-cheat software.
//!
//! **Choose one before implementing this module:**
//! - (a) PawnIO-based LHM — accept anti-cheat incompatibility, document it
//! - (b) Wait for HYTE's Microsoft-signed WinRing0 fork (timeline unknown)
//! - (c) WMI-only — loses ISA/SMBus sensors (most Super I/O chips)
//! - (d) HWiNFO64 shared-memory export — adds user setup burden
//!
//! ## License Correction
//!
//! LHM is licensed under **MPL 2.0**, not LGPL as the v0.2 spec states.

use anyhow::Result;
use async_trait::async_trait;
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use crate::HardwareLayer;

pub struct WindowsHardwareLayer;

impl WindowsHardwareLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsHardwareLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HardwareLayer for WindowsHardwareLayer {
    async fn snapshot(&self) -> Result<HardwareSnapshot> {
        // TODO(v0.1.0): implement after driver strategy decision above.
        todo!("WindowsHardwareLayer::snapshot not yet implemented")
    }

    async fn set_fan_duty(&self, _cmd: SetFanDuty) -> Result<()> {
        // TODO(v0.1.0): implement after driver strategy decision above.
        todo!("WindowsHardwareLayer::set_fan_duty not yet implemented")
    }
}

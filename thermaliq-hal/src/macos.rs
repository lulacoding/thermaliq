//! macOS hardware backend.
//!
//! ## Fan Control — Apple Silicon IS Writable
//!
//! **The v0.2 spec was incorrect.** macOS fan control is NOT read-only on Apple
//! Silicon. The `Ftst=1` SMC key suppresses `thermalmonitord` and enables fan
//! write access on M1–M4. This mechanism is publicly documented (agoodkind/
//! macos-smc-fan, Jan 2026) and is how TG Pro and Macs Fan Control operate.
//!
//! ## Architecture Required (not yet implemented)
//!
//! Production macOS fan control requires:
//! 1. A privileged helper tool installed via `SMJobBless` (runs as root).
//! 2. The Tauri app communicates with the helper via XPC.
//! 3. The helper writes the `Ftst` key via IOKit to unlock fan control.
//! 4. On sleep/wake, `Ftst` resets to 0 — the helper must re-acquire the lock
//!    by listening for `IOPMSleepWakeInterest` wake notifications.
//!
//! ## IOKit Threading Constraint
//!
//! IOKit is NOT thread-safe. The SMC worker MUST be single-threaded. Wrap all
//! IOKit calls in a dedicated `std::thread` with a channel-based request API
//! (mpsc sender/receiver) rather than spawning Tokio tasks directly.
//!
//! ## SMC Key Encoding Divergence
//!
//! - Intel Mac SMC fan keys use **fp2_14 fixed-point** encoding.
//! - Apple Silicon SMC fan keys use **IEEE 754 float (little-endian)**.
//! Both must be handled; detect via IOKit registry property `board-id`.
//!
//! ## M3/M4 Firmware Floor
//!
//! On M3 Pro/Max and M4 Pro/Max, SoC firmware disables fans below ~77–80°C
//! regardless of software commands. This is a hardware enforcement floor, not
//! a software limit — do not surface it as a ThermalIQ error.
//!
//! ## Notarization
//!
//! Use `xcrun notarytool submit` with an explicit timeout rather than Tauri's
//! built-in notarize action, which has a known hang bug (tauri-apps/tauri
//! issues #14579, #8630).

use anyhow::Result;
use async_trait::async_trait;
use thermaliq_common::{HardwareSnapshot, SetFanDuty};

use crate::HardwareLayer;

pub struct MacosHardwareLayer;

impl MacosHardwareLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacosHardwareLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HardwareLayer for MacosHardwareLayer {
    async fn snapshot(&self) -> Result<HardwareSnapshot> {
        // TODO(v0.1.0): read SMC keys via IOKit on a dedicated single thread.
        // Handle fp2_14 (Intel) vs IEEE 754 float (Apple Silicon) encoding.
        todo!("MacosHardwareLayer::snapshot not yet implemented")
    }

    async fn set_fan_duty(&self, _cmd: SetFanDuty) -> Result<()> {
        // TODO(v0.1.0): write Ftst=1 via privileged SMJobBless helper, then
        // write fan speed key. Re-acquire lock after sleep/wake events.
        todo!("MacosHardwareLayer::set_fan_duty not yet implemented")
    }
}

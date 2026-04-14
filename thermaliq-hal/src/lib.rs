use anyhow::Result;
use async_trait::async_trait;
use thermaliq_common::{HardwareSnapshot, SetFanDuty};

pub mod mock;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

/// Platform-agnostic hardware abstraction layer.
///
/// All platform implementations must be Send + Sync so the HAL can be shared
/// across Tokio tasks behind an Arc.
#[async_trait]
pub trait HardwareLayer: Send + Sync {
    /// Read the current hardware state (sensors + fans).
    async fn snapshot(&self) -> Result<HardwareSnapshot>;

    /// Set a fan's duty cycle. Returns Err if the fan_id is unknown or
    /// the platform rejects the write.
    async fn set_fan_duty(&self, cmd: SetFanDuty) -> Result<()>;
}

/// Construct the appropriate HAL for the current platform and build config.
///
/// When compiled with `--features mock-hardware`, always returns the mock
/// backend regardless of platform — safe for CI runners with no physical
/// hardware.
pub fn platform_hal() -> Box<dyn HardwareLayer> {
    #[cfg(feature = "mock-hardware")]
    {
        tracing::info!("mock-hardware feature enabled — using MockHardwareLayer");
        return Box::new(mock::MockHardwareLayer::new());
    }

    #[cfg(all(target_os = "linux", not(feature = "mock-hardware")))]
    {
        return Box::new(linux::LinuxHardwareLayer::new());
    }

    #[cfg(all(target_os = "macos", not(feature = "mock-hardware")))]
    {
        return Box::new(macos::MacosHardwareLayer::new());
    }

    #[cfg(all(target_os = "windows", not(feature = "mock-hardware")))]
    {
        return Box::new(windows::WindowsHardwareLayer::new());
    }

    #[cfg(not(any(
        feature = "mock-hardware",
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    )))]
    {
        panic!("unsupported platform — compile with --features mock-hardware for CI");
    }
}

//! Mock hardware backend for CI and development without physical hardware.
//!
//! Enabled by compiling with `--features mock-hardware`. Returns deterministic
//! fake sensor and fan data so the full daemon, API, and agent stack can run
//! and be tested on any machine including GitHub Actions runners.
//!
//! DO NOT use in production builds. The feature gate in `lib.rs::platform_hal()`
//! ensures this can never be selected at runtime unless the feature flag was
//! explicitly passed at compile time.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use crate::HardwareLayer;

/// Simulates a system with one CPU, one GPU, and two fan headers.
pub struct MockHardwareLayer {
    cpu_fan_duty: Arc<AtomicU8>,
    gpu_fan_duty: Arc<AtomicU8>,
}

impl MockHardwareLayer {
    pub fn new() -> Self {
        Self {
            cpu_fan_duty: Arc::new(AtomicU8::new(50)),
            gpu_fan_duty: Arc::new(AtomicU8::new(40)),
        }
    }
}

impl Default for MockHardwareLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HardwareLayer for MockHardwareLayer {
    async fn snapshot(&self) -> Result<HardwareSnapshot> {
        let cpu_duty = self.cpu_fan_duty.load(Ordering::Relaxed);
        let gpu_duty = self.gpu_fan_duty.load(Ordering::Relaxed);

        Ok(HardwareSnapshot {
            sensors: vec![
                SensorReading {
                    id: "cpu_tctl".into(),
                    label: "CPU Tctl".into(),
                    temp_celsius: 45.0,
                },
                SensorReading {
                    id: "gpu_temp".into(),
                    label: "GPU Junction".into(),
                    temp_celsius: 52.0,
                },
            ],
            fans: vec![
                FanReading {
                    id: "cpu_fan".into(),
                    label: "CPU Fan".into(),
                    rpm: duty_to_rpm(cpu_duty, 2400),
                    duty_percent: cpu_duty,
                },
                FanReading {
                    id: "gpu_fan".into(),
                    label: "GPU Fan".into(),
                    rpm: duty_to_rpm(gpu_duty, 3000),
                    duty_percent: gpu_duty,
                },
            ],
        })
    }

    async fn set_fan_duty(&self, cmd: SetFanDuty) -> Result<()> {
        let duty = cmd.duty_percent.min(100);
        match cmd.fan_id.as_str() {
            "cpu_fan" => {
                self.cpu_fan_duty.store(duty, Ordering::Relaxed);
                tracing::debug!(fan = "cpu_fan", duty, "mock: set fan duty");
            }
            "gpu_fan" => {
                self.gpu_fan_duty.store(duty, Ordering::Relaxed);
                tracing::debug!(fan = "gpu_fan", duty, "mock: set fan duty");
            }
            id => anyhow::bail!("mock: unknown fan id '{id}'"),
        }
        Ok(())
    }
}

fn duty_to_rpm(duty_percent: u8, max_rpm: u32) -> u32 {
    (max_rpm as f32 * duty_percent as f32 / 100.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_returns_two_fans() {
        let hal = MockHardwareLayer::new();
        let snap = hal.snapshot().await.unwrap();
        assert_eq!(snap.fans.len(), 2);
        assert_eq!(snap.sensors.len(), 2);
    }

    #[tokio::test]
    async fn set_fan_duty_updates_snapshot() {
        let hal = MockHardwareLayer::new();
        hal.set_fan_duty(SetFanDuty {
            fan_id: "cpu_fan".into(),
            duty_percent: 75,
            dc_mode: false,
            start_percent: None,
            stop_percent: None,
        })
        .await
        .unwrap();
        let snap = hal.snapshot().await.unwrap();
        let cpu = snap.fans.iter().find(|f| f.id == "cpu_fan").unwrap();
        assert_eq!(cpu.duty_percent, 75);
    }

    #[tokio::test]
    async fn set_fan_duty_unknown_fan_errors() {
        let hal = MockHardwareLayer::new();
        let result = hal
            .set_fan_duty(SetFanDuty {
                fan_id: "nonexistent".into(),
                duty_percent: 50,
                dc_mode: false,
                start_percent: None,
                stop_percent: None,
            })
            .await;
        assert!(result.is_err());
    }
}

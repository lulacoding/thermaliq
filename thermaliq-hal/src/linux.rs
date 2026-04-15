//! Linux Hardware Abstraction Layer — reads sensors via hwmon sysfs.

use anyhow::Result;
use async_trait::async_trait;
use std::fs;
use std::path::{Path, PathBuf};
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use super::HardwareLayer;

pub struct LinuxHardwareLayer {
    hwmon_base: PathBuf,
}

impl LinuxHardwareLayer {
    pub fn new() -> Self {
        Self {
            hwmon_base: PathBuf::from("/sys/class/hwmon"),
        }
    }

    fn read_file(path: &Path) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    fn enumerate_hwmon_chips(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(&self.hwmon_base) else { return Vec::new() };
        let mut chips: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("hwmon"))
                .unwrap_or(false))
            .collect();
        chips.sort();
        chips
    }

    fn read_sensors(&self) -> Vec<SensorReading> {
        let mut sensors = Vec::new();

        for chip_path in self.enumerate_hwmon_chips() {
            let chip_name = Self::read_file(&chip_path.join("name"))
                .unwrap_or_else(|| "unknown".to_string());

            // Find all temp*_input files
            let Ok(entries) = fs::read_dir(&chip_path) else { continue };
            let mut temp_files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("temp") && n.ends_with("_input"))
                        .unwrap_or(false)
                })
                .collect();
            temp_files.sort();

            for temp_file in temp_files {
                let Some(temp_str) = Self::read_file(&temp_file) else { continue };
                let Ok(raw_millideg): Result<i64, _> = temp_str.parse() else { continue };
                let temp_celsius = raw_millideg as f32 / 1000.0;

                // Get optional label
                let label_file = temp_file.to_str().unwrap_or("").replace("_input", "_label");
                let label = Self::read_file(Path::new(&label_file))
                    .unwrap_or_else(|| {
                        let stem = temp_file.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("temp");
                        format!("{} {}", chip_name, stem)
                    });

                let stem = temp_file.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("temp");
                let id = format!("{}_{}", chip_name, stem);

                if temp_celsius > -40.0 && temp_celsius < 150.0 {
                    sensors.push(SensorReading {
                        id,
                        label,
                        temp_celsius,
                    });
                }
            }
        }

        sensors
    }

    fn read_fans(&self) -> Vec<FanReading> {
        let mut fans = Vec::new();

        for chip_path in self.enumerate_hwmon_chips() {
            let chip_name = Self::read_file(&chip_path.join("name"))
                .unwrap_or_else(|| "unknown".to_string());

            let Ok(entries) = fs::read_dir(&chip_path) else { continue };
            let mut fan_files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("fan") && n.ends_with("_input"))
                        .unwrap_or(false)
                })
                .collect();
            fan_files.sort();

            for fan_file in fan_files {
                let Some(rpm_str) = Self::read_file(&fan_file) else { continue };
                let Ok(rpm): Result<u32, _> = rpm_str.parse() else { continue };

                let stem = fan_file.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("fan");
                let fan_num = stem.trim_start_matches("fan");

                // Try to read corresponding pwm* for duty%
                let pwm_file = chip_path.join(format!("pwm{}", fan_num));
                let duty_percent = if let Some(pwm_str) = Self::read_file(&pwm_file) {
                    pwm_str.parse::<u32>().ok()
                        .map(|pwm| (pwm * 100 / 255) as u8)
                        .unwrap_or(0)
                } else {
                    0
                };

                // Label: check fan*_label
                let label_file = fan_file.to_str().unwrap_or("").replace("_input", "_label");
                let label = Self::read_file(Path::new(&label_file))
                    .unwrap_or_else(|| format!("{} {}", chip_name, stem));

                fans.push(FanReading {
                    id: format!("{}_{}", chip_name, stem),
                    label,
                    rpm,
                    duty_percent,
                });
            }
        }

        fans
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
        // File I/O can block; run on blocking thread pool
        let hwmon_base = self.hwmon_base.clone();
        let hal = LinuxHardwareLayer { hwmon_base };
        tokio::task::spawn_blocking(move || {
            let sensors = hal.read_sensors();
            let fans = hal.read_fans();
            Ok(HardwareSnapshot { sensors, fans })
        }).await?
    }

    async fn set_fan_duty(&self, cmd: SetFanDuty) -> Result<()> {
        // cmd.fan_id format: "chipname_fanN" e.g. "nct6775_fan1"
        let chip_path = self.enumerate_hwmon_chips()
            .into_iter()
            .find(|p| {
                let name = Self::read_file(&p.join("name")).unwrap_or_default();
                cmd.fan_id.starts_with(&name)
            });

        let Some(chip) = chip_path else {
            anyhow::bail!("fan {} not found", cmd.fan_id);
        };

        // Extract fan number from id: "chipname_fanN" → "N"
        let fan_part = cmd.fan_id.rsplit("fan").next().unwrap_or("1");
        let pwm_path = chip.join(format!("pwm{}", fan_part));
        let pwm_enable_path = chip.join(format!("pwm{}_enable", fan_part));

        let pwm_value = (cmd.duty_percent as u32 * 255 / 100).clamp(0, 255);

        tokio::task::spawn_blocking(move || {
            // Enable manual control
            fs::write(&pwm_enable_path, "1\n").ok();
            // Write PWM value
            fs::write(&pwm_path, format!("{}\n", pwm_value))?;
            tracing::info!(
                fan = %cmd.fan_id,
                pwm = pwm_value,
                duty = cmd.duty_percent,
                "Linux: fan PWM set"
            );
            Ok(())
        }).await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_hwmon_nonexistent_graceful() {
        let hal = LinuxHardwareLayer {
            hwmon_base: PathBuf::from("/nonexistent/path"),
        };
        // Should return empty vec without panic
        assert!(hal.enumerate_hwmon_chips().is_empty());
    }

    #[test]
    fn read_sensors_nonexistent_graceful() {
        let hal = LinuxHardwareLayer {
            hwmon_base: PathBuf::from("/nonexistent/path"),
        };
        assert!(hal.read_sensors().is_empty());
        assert!(hal.read_fans().is_empty());
    }
}

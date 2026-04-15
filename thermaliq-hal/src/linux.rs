//! Linux Hardware Abstraction Layer — reads sensors via hwmon sysfs.
//!
//! ## AMD GPU Fan Control
//!
//! Two code paths exist depending on GPU generation:
//!
//! - **RDNA2 and older** (`pwm1_enable=1` + `pwm1`): traditional hwmon sysfs,
//!   works on any kernel.
//!
//! - **RDNA3 / Navi3x** (`gpu_od/fan_ctrl/fan_curve`, kernel 6.7+): OverDrive8
//!   fan curve interface. Requires `amdgpu.ppfeaturemask=0xffffffff` (bit 22 =
//!   `PP_OVERDRIVE_MASK`). If the ppfeaturemask bit is missing, ThermalIQ logs a
//!   clear warning and falls back to the traditional pwm path.
//!
//! Fan curve write format (kernel 6.7+):
//!   `"<temp_celsius> <fan_speed_percent>\n"` — one pair per line, 2–5 points.
//!   Zero-RPM mode toggled via `fan_zero_rpm` (kernel 6.13+).

use anyhow::Result;
use async_trait::async_trait;
use std::fs;
use std::path::{Path, PathBuf};
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use super::HardwareLayer;

// ── RDNA3 OverDrive constants ──────────────────────────────────────────────

/// amdgpu kernel parameter path for OverDrive feature mask.
const AMDGPU_PPFEATUREMASK: &str = "/sys/module/amdgpu/parameters/ppfeaturemask";
/// Bit 22 — PP_OVERDRIVE_MASK — required for OD fan curve writes.
const PP_OVERDRIVE_BIT: u64 = 1 << 22;
/// sysfs root for DRM cards.
const DRM_CLASS: &str = "/sys/class/drm";

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
        let Ok(entries) = fs::read_dir(&self.hwmon_base) else {
            return Vec::new();
        };
        let mut chips: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("hwmon"))
                    .unwrap_or(false)
            })
            .collect();
        chips.sort();
        chips
    }

    fn read_sensors(&self) -> Vec<SensorReading> {
        let mut sensors = Vec::new();

        for chip_path in self.enumerate_hwmon_chips() {
            let chip_name =
                Self::read_file(&chip_path.join("name")).unwrap_or_else(|| "unknown".to_string());

            // Find all temp*_input files
            let Ok(entries) = fs::read_dir(&chip_path) else {
                continue;
            };
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
                let Some(temp_str) = Self::read_file(&temp_file) else {
                    continue;
                };
                let Ok(raw_millideg): Result<i64, _> = temp_str.parse() else {
                    continue;
                };
                let temp_celsius = raw_millideg as f32 / 1000.0;

                // Get optional label
                let label_file = temp_file.to_str().unwrap_or("").replace("_input", "_label");
                let label = Self::read_file(Path::new(&label_file)).unwrap_or_else(|| {
                    let stem = temp_file
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("temp");
                    format!("{} {}", chip_name, stem)
                });

                let stem = temp_file
                    .file_stem()
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
            let chip_name =
                Self::read_file(&chip_path.join("name")).unwrap_or_else(|| "unknown".to_string());

            let Ok(entries) = fs::read_dir(&chip_path) else {
                continue;
            };
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
                let Some(rpm_str) = Self::read_file(&fan_file) else {
                    continue;
                };
                let Ok(rpm): Result<u32, _> = rpm_str.parse() else {
                    continue;
                };

                let stem = fan_file
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("fan");
                let fan_num = stem.trim_start_matches("fan");

                // Try to read corresponding pwm* for duty%
                let pwm_file = chip_path.join(format!("pwm{}", fan_num));
                let duty_percent = if let Some(pwm_str) = Self::read_file(&pwm_file) {
                    pwm_str
                        .parse::<u32>()
                        .ok()
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

// ── RDNA3 helpers ─────────────────────────────────────────────────────────

/// Map an hwmon chip path to its DRM card path by comparing PCI device symlinks.
///
/// e.g. `/sys/class/hwmon/hwmon5` → `/sys/class/drm/card0`
fn find_drm_card_for_hwmon(hwmon_chip: &Path) -> Option<PathBuf> {
    let hwmon_dev = fs::canonicalize(hwmon_chip.join("device")).ok()?;
    let entries = fs::read_dir(DRM_CLASS).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Skip render nodes and port connectors (cardN-HDMI-A-N etc.)
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let card_path = entry.path();
        if let Ok(card_dev) = fs::canonicalize(card_path.join("device")) {
            if card_dev == hwmon_dev {
                return Some(card_path);
            }
        }
    }
    None
}

/// Return the RDNA3 fan_curve sysfs path for a DRM card, if it exists.
///
/// Present on kernel 6.7+ with RDNA3 (Navi3x) hardware.
fn rdna3_fan_curve_path(card_path: &Path) -> Option<PathBuf> {
    let p = card_path.join("device/gpu_od/fan_ctrl/fan_curve");
    if p.exists() { Some(p) } else { None }
}

/// Check whether amdgpu has OverDrive enabled (ppfeaturemask bit 22).
fn amdgpu_overdrive_enabled() -> bool {
    let s = fs::read_to_string(AMDGPU_PPFEATUREMASK)
        .unwrap_or_default();
    let s = s.trim();
    let mask: u64 = if s.starts_with("0x") || s.starts_with("0X") {
        u64::from_str_radix(&s[2..], 16).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    };
    (mask & PP_OVERDRIVE_BIT) != 0
}

/// Write a flat fan duty curve via the RDNA3 OverDrive sysfs interface.
///
/// Curve format (kernel 6.7+): one `"<temp_celsius> <fan_pct>\n"` per line.
/// Five evenly-spaced temperature anchors are used so the GPU firmware
/// interpolates a smooth ramp rather than a step function.
fn write_rdna3_fan_curve(fan_curve: &Path, duty_percent: u8) -> anyhow::Result<()> {
    let d = duty_percent.clamp(0, 100);
    // Five anchor points spanning 0–100 °C, all at the requested duty.
    let curve = format!(
        "0 {d}\n25 {d}\n50 {d}\n75 {d}\n100 {d}\n",
        d = d
    );
    fs::write(fan_curve, &curve)?;
    tracing::info!(
        path = %fan_curve.display(),
        duty = duty_percent,
        "Linux RDNA3: fan curve written via OverDrive sysfs"
    );
    Ok(())
}

/// Toggle zero-RPM mode for RDNA3 (kernel 6.13+). Silently ignored on older kernels.
fn set_rdna3_zero_rpm(card_path: &Path, enable: bool) {
    let p = card_path.join("device/gpu_od/fan_ctrl/fan_zero_rpm");
    if p.exists() {
        let _ = fs::write(&p, if enable { "1\n" } else { "0\n" });
    }
}

// ── Laptop EC / vendor-specific fan control ───────────────────────────────
//
// Priority order within set_fan_duty for laptop chips:
//   1. ThinkPad  — /proc/acpi/ibm/fan  (thinkpad_acpi kernel module)
//   2. ASUS      — asus-nb-wmi sysfs   (asus-nb-wmi kernel module)
//   3. NBFC      — EC register writes via debugfs (user-supplied config)
//   4. Generic   — hwmon pwmN (dell-smm-hwmon, other laptop drivers)

/// DMI chassis types that indicate a laptop / portable form factor.
const LAPTOP_CHASSIS_TYPES: &[u8] = &[
    8,  // Portable
    9,  // Laptop
    10, // Notebook
    11, // Hand Held
    14, // Sub Notebook
    31, // Convertible
    32, // Detachable
];

fn dmi_read(field: &str) -> Option<String> {
    let path = format!("/sys/class/dmi/id/{field}");
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// True when DMI chassis type reports a portable form factor.
fn is_laptop() -> bool {
    dmi_read("chassis_type")
        .and_then(|s| s.parse::<u8>().ok())
        .map(|t| LAPTOP_CHASSIS_TYPES.contains(&t))
        .unwrap_or(false)
}

// ── ThinkPad (/proc/acpi/ibm/fan) ────────────────────────────────────────

/// Map duty% → IBM fan level (0–7). Level 7 is written as "disengaged"
/// (no firmware cap) so the fan can reach its true maximum.
fn thinkpad_set_fan(duty_percent: u8) -> Option<anyhow::Result<()>> {
    const PATH: &str = "/proc/acpi/ibm/fan";
    if !Path::new(PATH).exists() {
        return None;
    }
    let level = (duty_percent as u32 * 7 / 100).clamp(0, 7);
    let cmd = if level == 7 {
        "level disengaged\n".to_string()
    } else {
        format!("level {level}\n")
    };
    Some(
        fs::write(PATH, &cmd)
            .map_err(|e| anyhow::anyhow!("ThinkPad fan write failed: {e}"))
            .map(|()| {
                tracing::info!(
                    duty = duty_percent,
                    level,
                    "Linux ThinkPad: fan level written to /proc/acpi/ibm/fan"
                );
            }),
    )
}

// ── ASUS nb-wmi ───────────────────────────────────────────────────────────

/// ASUS fan_boost_mode values: 0=balanced, 1=boost, 2=silent.
/// Map duty ≥ 80% → boost, duty ≤ 20% → silent, else balanced.
fn asus_set_fan_boost(duty_percent: u8) -> Option<anyhow::Result<()>> {
    const BOOST_PATH: &str = "/sys/devices/platform/asus-nb-wmi/fan_boost_mode";
    if !Path::new(BOOST_PATH).exists() {
        return None;
    }
    let mode = if duty_percent >= 80 {
        1u8 // boost
    } else if duty_percent <= 20 {
        2u8 // silent
    } else {
        0u8 // balanced
    };
    Some(
        fs::write(BOOST_PATH, format!("{mode}\n"))
            .map_err(|e| anyhow::anyhow!("ASUS fan_boost_mode write failed: {e}"))
            .map(|()| {
                tracing::info!(
                    duty = duty_percent,
                    mode,
                    "Linux ASUS: fan_boost_mode written via asus-nb-wmi"
                );
            }),
    )
}

// ── NBFC EC register control ──────────────────────────────────────────────
//
// Config file: /etc/thermaliq/nbfc/<product_name>.ini  (case-insensitive)
//
// Format (lines beginning with '#' are comments):
//   fan_count=2
//   fan0_write_reg=0x2F
//   fan0_min_value=0
//   fan0_max_value=255
//   fan1_write_reg=0x30
//   fan1_min_value=0
//   fan1_max_value=255
//
// EC writes via /sys/kernel/debug/ec/<acpi_path>/io (seek + write one byte).

#[derive(Debug, Clone)]
struct NbfcFanConfig {
    write_reg: u8,
    min_value: u8,
    max_value: u8,
}

#[derive(Debug)]
struct NbfcConfig {
    fans: Vec<NbfcFanConfig>,
}

fn find_nbfc_config() -> Option<NbfcConfig> {
    let product = dmi_read("product_name")?;
    // Sanitize product name for use as filename (replace spaces/slashes).
    let safe = product
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect::<String>();

    let search_dirs = ["/etc/thermaliq/nbfc", "/usr/share/thermaliq/nbfc"];
    let config_text = search_dirs.iter().find_map(|dir| {
        let path = PathBuf::from(dir).join(format!("{safe}.ini"));
        fs::read_to_string(&path).ok()
    })?;

    parse_nbfc_ini(&config_text)
}

fn parse_nbfc_ini(text: &str) -> Option<NbfcConfig> {
    let mut fan_count: usize = 0;
    let mut entries: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=')?;
        let k = k.trim().to_lowercase();
        let v = v.trim().to_string();
        if k == "fan_count" {
            fan_count = v.parse().ok()?;
        } else {
            entries.insert(k, v);
        }
    }

    if fan_count == 0 {
        return None;
    }

    let parse_u8 = |s: &str| -> Option<u8> {
        if s.starts_with("0x") || s.starts_with("0X") {
            u8::from_str_radix(&s[2..], 16).ok()
        } else {
            s.parse().ok()
        }
    };

    let fans: Vec<NbfcFanConfig> = (0..fan_count)
        .filter_map(|i| {
            let reg = parse_u8(entries.get(&format!("fan{i}_write_reg"))?)?;
            let min = parse_u8(entries.get(&format!("fan{i}_min_value")).map(|s| s.as_str()).unwrap_or("0")).unwrap_or(0);
            let max = parse_u8(entries.get(&format!("fan{i}_max_value")).map(|s| s.as_str()).unwrap_or("255")).unwrap_or(255);
            Some(NbfcFanConfig { write_reg: reg, min_value: min, max_value: max })
        })
        .collect();

    if fans.len() != fan_count {
        return None;
    }

    Some(NbfcConfig { fans })
}

/// Write a single byte to an EC register via the ACPI EC debugfs interface.
///
/// The `io` file under `/sys/kernel/debug/ec/<path>/io` is a 256-byte binary
/// file; byte offset == register address.
fn ec_write_register(reg: u8, value: u8) -> anyhow::Result<()> {
    use std::io::{Seek, SeekFrom, Write};

    // Find the first EC io file (usually exactly one: /sys/kernel/debug/ec/ec0/io).
    let ec_glob = "/sys/kernel/debug/ec";
    let ec_dir = fs::read_dir(ec_glob)
        .map_err(|e| anyhow::anyhow!("EC debugfs not available ({ec_glob}): {e}"))?;

    let io_path = ec_dir
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("io"))
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("no EC io file found under {ec_glob}"))?;

    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(&io_path)
        .map_err(|e| anyhow::anyhow!("open {}: {e}", io_path.display()))?;
    f.seek(SeekFrom::Start(reg as u64))?;
    f.write_all(&[value])?;
    Ok(())
}

/// Write fan duty via NBFC EC config for the given fan slot index.
fn nbfc_set_fan(config: &NbfcConfig, fan_slot: usize, duty_percent: u8) -> anyhow::Result<()> {
    let fc = config
        .fans
        .get(fan_slot)
        .ok_or_else(|| anyhow::anyhow!("NBFC config has no fan slot {fan_slot}"))?;

    let range = fc.max_value.saturating_sub(fc.min_value) as u32;
    let ec_value = fc.min_value + (duty_percent as u32 * range / 100).min(range) as u8;
    ec_write_register(fc.write_reg, ec_value)?;
    tracing::info!(
        fan_slot,
        reg = fc.write_reg,
        ec_value,
        duty = duty_percent,
        "Linux NBFC: EC register written"
    );
    Ok(())
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
        })
        .await?
    }

    async fn set_fan_duty(&self, cmd: SetFanDuty) -> Result<()> {
        // cmd.fan_id format: "chipname_fanN" e.g. "nct6775_fan1" or "amdgpu_fan1"
        let chip_path = self.enumerate_hwmon_chips().into_iter().find(|p| {
            let name = Self::read_file(&p.join("name")).unwrap_or_default();
            cmd.fan_id.starts_with(&name)
        });

        let Some(chip) = chip_path else {
            anyhow::bail!("fan '{}' not found in hwmon", cmd.fan_id);
        };

        let chip_name = Self::read_file(&chip.join("name")).unwrap_or_default();
        let duty = cmd.duty_percent;

        // ── RDNA3 path (kernel 6.7+, amdgpu OverDrive) ──────────────────────
        if chip_name == "amdgpu" {
            if let Some(card) = find_drm_card_for_hwmon(&chip) {
                if let Some(fan_curve) = rdna3_fan_curve_path(&card) {
                    if !amdgpu_overdrive_enabled() {
                        tracing::warn!(
                            fan = %cmd.fan_id,
                            "RDNA3 OverDrive fan curve requires amdgpu.ppfeaturemask=0xffffffff. \
                             Add 'options amdgpu ppfeaturemask=0xffffffff' to \
                             /etc/modprobe.d/amdgpu.conf and reboot. \
                             Falling back to traditional PWM."
                        );
                        // Fall through to traditional PWM below.
                    } else {
                        let card_clone = card.clone();
                        return tokio::task::spawn_blocking(move || {
                            write_rdna3_fan_curve(&fan_curve, duty)?;
                            // Disable zero-RPM when duty > 0 so fan spins immediately.
                            set_rdna3_zero_rpm(&card_clone, duty == 0);
                            Ok(())
                        })
                        .await?;
                    }
                }
            }
        }

        // ── ThinkPad ACPI fan control (/proc/acpi/ibm/fan) ──────────────────
        if chip_name.starts_with("thinkpad") {
            if let Some(result) = thinkpad_set_fan(duty) {
                return result.map_err(Into::into);
            }
        }

        // ── ASUS nb-wmi fan boost mode ────────────────────────────────────────
        if chip_name.starts_with("asus") {
            if let Some(result) = asus_set_fan_boost(duty) {
                return result.map_err(Into::into);
            }
            // Fall through to generic hwmon PWM if boost mode not available.
        }

        // ── NBFC EC register control (user-supplied laptop configs) ───────────
        // Only attempted when on a laptop without a vendor-specific kernel path.
        if is_laptop() && !chip_name.starts_with("thinkpad") && !chip_name.starts_with("asus") {
            if let Some(nbfc) = find_nbfc_config() {
                // Derive fan slot from the fan_id suffix (e.g. "ec_fan1" → slot 1).
                let fan_slot = cmd
                    .fan_id
                    .chars()
                    .rev()
                    .next()
                    .and_then(|c| c.to_digit(10))
                    .map(|d| d as usize)
                    .unwrap_or(0);
                return nbfc_set_fan(&nbfc, fan_slot, duty).map_err(Into::into);
            }
        }

        // ── Traditional hwmon PWM (RDNA2, Navi1x, motherboard fans, Dell SMM) ─
        let fan_part = cmd.fan_id.rsplit("fan").next().unwrap_or("1");
        let pwm_path = chip.join(format!("pwm{}", fan_part));
        let pwm_enable_path = chip.join(format!("pwm{}_enable", fan_part));
        let pwm_value = (duty as u32 * 255 / 100).clamp(0, 255);

        tokio::task::spawn_blocking(move || {
            // Enable manual PWM control (pwm1_enable=1).
            fs::write(&pwm_enable_path, "1\n").ok();
            fs::write(&pwm_path, format!("{}\n", pwm_value))?;
            tracing::info!(
                fan = %cmd.fan_id,
                pwm = pwm_value,
                duty = duty,
                "Linux: fan PWM set"
            );
            Ok(())
        })
        .await?
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

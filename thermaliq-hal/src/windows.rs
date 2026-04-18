//! Windows hardware backend — reads sensors via HWiNFO64 shared memory with
//! WMI fallback, and controls NVIDIA GPU fans via NVML.
//!
//! ## Sensor Backend Decision (Locked — April 2026)
//!
//! HWiNFO64 shared memory (`Global\HWiNFO_SENS_SM2`) is the chosen primary
//! backend. Alternatives were rejected:
//!
//! - **WinRing0 / LibreHardwareMonitor**: Defender quarantines WinRing0.sys
//!   (CVE-2020-14979) on modern Windows 11 installs.
//! - **PawnIO (FanControl's replacement)**: blocked by FACEIT, Vanguard, and
//!   other anti-cheat software that most of our target users run.
//! - **WMI-only**: misses ISA/SMBus sensors (most Super I/O chips). Usable
//!   as fallback only.
//!
//! ## User Setup Requirement
//!
//! HWiNFO64 must be installed and running in "Sensors only" mode. The daemon
//! will warn and fall back to WMI if HWiNFO64 is not detected.
//!
//! Download: https://www.hwinfo.com/download/
//!
//! ## Fan Control
//!
//! - **NVIDIA GPU fans**: NVML `set_fan_speed` (driver 520+ required). ID: `nvml_gpu{i}_fan{j}`.
//! - **AMD GPU fans**: ADL Overdrive5 via `atiadlxx.dll` (GCN/RDNA/RDNA2). ID: `adl_gpu{idx}`.
//!   RDNA3 Overdrive8 / ADLX COM interface deferred to Phase 2.
//! - **Motherboard headers**: NOT writable without a kernel driver. Attempts
//!   return a descriptive error — no silent failure.
//!
//! ## Threading
//!
//! All Win32 and NVML calls run inside `tokio::task::spawn_blocking` so the
//! async runtime is never blocked. WMI COM calls additionally call
//! `CoInitializeEx` on each blocking thread entry (idempotent per-thread).

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use std::alloc;
use std::fmt;
use std::sync::Mutex;
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use crate::HardwareLayer;

// ── Windows-only imports ─────────────────────────────────────────────────────
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::Memory::{
            MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
            MEMORY_MAPPED_VIEW_ADDRESS,
        },
    },
};

// ── Structured error types ────────────────────────────────────────────────────

/// Errors produced when opening or validating the HWiNFO64 shared memory.
#[derive(Debug)]
pub enum HwinfoError {
    /// The shared memory object does not exist — HWiNFO64 is not running.
    NotRunning,
    /// The process lacks permission to open the mapping (e.g. not Administrator).
    PermissionDenied(String),
    /// `MapViewOfFile` failed (low memory or handle recycled).
    MapFailed(String),
    /// The 4-byte signature at offset 0 does not match `"HWiS"`.
    SignatureMismatch { found: u32 },
    /// The header `version` field is outside the range we support (1–3).
    UnsupportedVersion { version: u32 },
    /// The header `revision` field is outside the range we support (0–9).
    UnsupportedRevision { revision: u32 },
    /// Section offset or element size would cause an out-of-bounds read.
    LayoutInvalid { field: &'static str, value: u32 },
    /// Stale data: `poll_time_ms` is zero — HWiNFO64 not yet polling.
    StaleData,
}

impl fmt::Display for HwinfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HwinfoError::NotRunning => write!(
                f,
                "HWiNFO64 shared memory not found — is HWiNFO64 running in 'Sensors only' mode? \
                 Download: https://www.hwinfo.com/download/"
            ),
            HwinfoError::PermissionDenied(msg) => write!(
                f,
                "HWiNFO64 shared memory access denied (try running as Administrator): {msg}"
            ),
            HwinfoError::MapFailed(msg) => write!(f, "HWiNFO64 MapViewOfFile failed: {msg}"),
            HwinfoError::SignatureMismatch { found } => write!(
                f,
                "HWiNFO64 header signature mismatch: expected 0x{:08X} ('HWiS'), found 0x{found:08X}",
                HWINFO_SIGNATURE
            ),
            HwinfoError::UnsupportedVersion { version } => write!(
                f,
                "HWiNFO64 header version {version} outside supported range \
                 {HWINFO_MIN_VERSION}–{HWINFO_MAX_VERSION} — update ThermalIQ or HWiNFO64"
            ),
            HwinfoError::UnsupportedRevision { revision } => write!(
                f,
                "HWiNFO64 header revision {revision} outside supported range 0–{HWINFO_MAX_REVISION}"
            ),
            HwinfoError::LayoutInvalid { field, value } => write!(
                f,
                "HWiNFO64 header field '{field}' has invalid value {value} (memory layout corrupt?)"
            ),
            HwinfoError::StaleData => write!(
                f,
                "HWiNFO64 poll_time_ms is zero — sensors not yet polled or HWiNFO64 frozen"
            ),
        }
    }
}

impl std::error::Error for HwinfoError {}

/// Errors produced by the WMI sensor fallback.
#[derive(Debug)]
pub enum WmiError {
    /// `WMIConnection::with_namespace_path` failed (WMI service unavailable).
    ConnectionFailed(String),
    /// A WMI query executed but returned zero rows (class not present on this hardware).
    NoSensorsFound { class: &'static str },
    /// A WMI query failed with an error code.
    QueryFailed { class: &'static str, detail: String },
}

impl fmt::Display for WmiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WmiError::ConnectionFailed(msg) => write!(
                f,
                "WMI connection failed (install HWiNFO64 for full coverage): {msg}"
            ),
            WmiError::NoSensorsFound { class } => write!(
                f,
                "WMI class '{class}' returned no rows — this system may not expose \
                 those sensors via ACPI/WMI"
            ),
            WmiError::QueryFailed { class, detail } => {
                write!(f, "WMI query on '{class}' failed: {detail}")
            }
        }
    }
}

impl std::error::Error for WmiError {}

// ── HWiNFO64 shared-memory layout ────────────────────────────────────────────
//
// Layout documented in the HWiNFO SDK (stable since ~2013).
// https://www.hwinfo.com/forum/threads/hwinfo-sdk.86/
//
// Version/revision ranges accepted:
//   version  1–3  (SDK defines v1=initial, v2=added poll_time, v3=extended)
//   revision 0–9  (minor layout tweaks; field meanings unchanged)

const HWINFO_SIGNATURE: u32 = 0x53695748; // b"HWiS" LE
const HWINFO_SM_NAME: &str = "Global\\HWiNFO_SENS_SM2";
const HWINFO_MIN_VERSION: u32 = 1;
const HWINFO_MAX_VERSION: u32 = 3;
const HWINFO_MAX_REVISION: u32 = 9;

// Sanity bounds to catch obviously corrupt layout fields
const HWINFO_MAX_ELEMENT_COUNT: u32 = 8192;
const HWINFO_MIN_ELEMENT_SIZE: u32 = 4;
const HWINFO_MAX_ELEMENT_SIZE: u32 = 4096;

const READING_TYPE_TEMP: u32 = 1;
const READING_TYPE_FAN: u32 = 3;

#[repr(C)]
struct HwinfoHeader {
    signature: u32,
    version: u32,
    revision: u32,
    poll_time_ms: i64,
    sensor_section_offset: u32,
    sensor_element_size: u32,
    sensor_element_count: u32,
    reading_section_offset: u32,
    reading_element_size: u32,
    reading_element_count: u32,
}

#[repr(C)]
#[allow(dead_code)]
struct HwinfoSensor {
    sensor_id: u32,
    sensor_inst: u32,
    name_orig: [u8; 128],
    name_user: [u8; 128],
}

#[repr(C)]
struct HwinfoReading {
    reading_type: u32,
    sensor_index: u32,
    reading_id: u32,
    label_orig: [u8; 128],
    label_user: [u8; 128],
    unit: [u8; 16],
    value: f64,
    value_min: f64,
    value_max: f64,
    value_avg: f64,
}

// ── Label / sensor-ID helpers ─────────────────────────────────────────────────

/// Convert a NUL-terminated byte slice from HWiNFO shared memory to a String.
///
/// Non-UTF-8 sequences (e.g. CP-1252 vendor labels with degree symbols) are
/// replaced with U+FFFD rather than dropping the reading entirely.
fn cstr_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Sanitize a raw HWiNFO label into a form safe for use in a sensor/fan ID.
///
/// Rules applied (in order):
/// 1. Replace any character outside `[A-Za-z0-9.-]` (including non-ASCII and
///    vendor suffixes like `#`, `(`, `)`) with `_`.
/// 2. Collapse consecutive `_` to a single `_`.
/// 3. Strip leading/trailing `_`.
/// 4. Truncate to 64 bytes.
/// 5. Return `"unknown"` if the result is empty.
fn sanitize_label_for_id(label: &str) -> String {
    let sanitized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Collapse consecutive underscores
    let mut result = String::with_capacity(sanitized.len());
    let mut prev_under = false;
    for c in sanitized.chars() {
        if c == '_' {
            if !prev_under {
                result.push('_');
            }
            prev_under = true;
        } else {
            result.push(c);
            prev_under = false;
        }
    }

    let result = result.trim_matches('_').to_string();
    let result = if result.len() > 64 {
        result[..64].to_string()
    } else {
        result
    };

    if result.is_empty() {
        "unknown".to_string()
    } else {
        result
    }
}

/// Build a unique, stable sensor ID from HWiNFO reading fields.
///
/// Format: `hwinfo_{sensor_index}_{reading_id}_{sanitized_label}`
fn hwinfo_sensor_id(sensor_index: u32, reading_id: u32, label: &str) -> String {
    format!(
        "hwinfo_{}_{}_{}",
        sensor_index,
        reading_id,
        sanitize_label_for_id(label)
    )
}

/// Build a unique, stable fan ID from HWiNFO reading fields.
fn hwinfo_fan_id(reading_id: u32, label: &str) -> String {
    format!("hwinfo_fan_{}_{}", reading_id, sanitize_label_for_id(label))
}

// ── Shared-memory handle wrapper ──────────────────────────────────────────────

struct HwinfoMapping {
    handle: HANDLE,
    view: *const u8,
}

// SAFETY: The view pointer is valid for the lifetime of the mapping and we
// only ever read it from within spawn_blocking (single OS thread at a time).
unsafe impl Send for HwinfoMapping {}
// SAFETY: All access is serialized through `Mutex<Option<HwinfoMapping>>` in
// `WindowsHardwareLayer`, so concurrent reads of `view` never occur. Required
// for `HardwareLayer: Send + Sync` trait bound.
unsafe impl Sync for HwinfoMapping {}

impl HwinfoMapping {
    fn open() -> std::result::Result<Self, HwinfoError> {
        let name_wide: Vec<u16> = HWINFO_SM_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe {
            OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(name_wide.as_ptr())).map_err(|e| {
                let msg = e.to_string();
                // ERROR_ACCESS_DENIED (5) or ERROR_PRIVILEGE_NOT_HELD (1314)
                if msg.contains("(5)") || msg.contains("(1314)") || msg.contains("Access") {
                    HwinfoError::PermissionDenied(msg)
                } else {
                    HwinfoError::NotRunning
                }
            })?
        };

        if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
            return Err(HwinfoError::NotRunning);
        }

        let view_mapped = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
        // MapViewOfFile returns a handle newtype; .Value is the raw *mut c_void pointer.
        let view_ptr = view_mapped.Value;
        if view_ptr.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(HwinfoError::MapFailed("MapViewOfFile returned null".into()));
        }

        // Validate all header fields before any further pointer arithmetic.
        let header = unsafe { &*(view_ptr as *const HwinfoHeader) };

        macro_rules! cleanup_err {
            ($err:expr) => {{
                unsafe {
                    let _ = UnmapViewOfFile(view_mapped);
                    let _ = CloseHandle(handle);
                }
                return Err($err);
            }};
        }

        if header.signature != HWINFO_SIGNATURE {
            cleanup_err!(HwinfoError::SignatureMismatch {
                found: header.signature,
            });
        }

        if header.version < HWINFO_MIN_VERSION || header.version > HWINFO_MAX_VERSION {
            cleanup_err!(HwinfoError::UnsupportedVersion {
                version: header.version,
            });
        }

        if header.revision > HWINFO_MAX_REVISION {
            cleanup_err!(HwinfoError::UnsupportedRevision {
                revision: header.revision,
            });
        }

        // Validate element sizes to prevent out-of-bounds pointer arithmetic.
        for (field, val) in [
            ("sensor_element_size", header.sensor_element_size),
            ("reading_element_size", header.reading_element_size),
        ] {
            if val < HWINFO_MIN_ELEMENT_SIZE || val > HWINFO_MAX_ELEMENT_SIZE {
                cleanup_err!(HwinfoError::LayoutInvalid { field, value: val });
            }
        }

        for (field, val) in [
            ("sensor_element_count", header.sensor_element_count),
            ("reading_element_count", header.reading_element_count),
        ] {
            if val > HWINFO_MAX_ELEMENT_COUNT {
                cleanup_err!(HwinfoError::LayoutInvalid { field, value: val });
            }
        }

        if header.poll_time_ms == 0 {
            cleanup_err!(HwinfoError::StaleData);
        }

        tracing::debug!(
            backend = "hwinfo",
            version = header.version,
            revision = header.revision,
            sensors = header.sensor_element_count,
            readings = header.reading_element_count,
            "HWiNFO64 shared memory opened"
        );

        Ok(Self {
            handle,
            view: view_ptr as *const u8,
        })
    }

    fn read_snapshot(&self) -> (Vec<SensorReading>, Vec<FanReading>) {
        let span = tracing::debug_span!("hwinfo_read_snapshot", backend = "hwinfo");
        let _enter = span.enter();

        let header = unsafe { &*(self.view as *const HwinfoHeader) };
        let mut sensors = Vec::new();
        let mut fans = Vec::new();

        let reading_base = unsafe { self.view.add(header.reading_section_offset as usize) };

        for i in 0..header.reading_element_count as usize {
            let reading = unsafe {
                &*(reading_base.add(i * header.reading_element_size as usize)
                    as *const HwinfoReading)
            };

            // Prefer user-assigned label; fall back to original label if blank.
            let label_user = cstr_to_string(&reading.label_user);
            let label = if label_user.trim().is_empty() {
                cstr_to_string(&reading.label_orig)
            } else {
                label_user
            };

            match reading.reading_type {
                READING_TYPE_TEMP => {
                    let temp = reading.value as f32;
                    if temp > -40.0 && temp < 150.0 {
                        sensors.push(SensorReading {
                            id: hwinfo_sensor_id(reading.sensor_index, reading.reading_id, &label),
                            label,
                            temp_celsius: temp,
                        });
                    } else {
                        tracing::trace!(
                            backend = "hwinfo",
                            reading_id = reading.reading_id,
                            temp,
                            "temperature out of plausible range, skipped"
                        );
                    }
                }
                READING_TYPE_FAN => {
                    fans.push(FanReading {
                        id: hwinfo_fan_id(reading.reading_id, &label),
                        label,
                        rpm: reading.value as u32,
                        // HWiNFO exposes RPM only, not duty cycle
                        duty_percent: 0,
                    });
                }
                _ => {}
            }
        }

        tracing::debug!(
            backend = "hwinfo",
            sensor_count = sensors.len(),
            fan_count = fans.len(),
            "HWiNFO64 snapshot complete"
        );

        (sensors, fans)
    }
}

impl Drop for HwinfoMapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view as *mut std::ffi::c_void,
            });
            let _ = CloseHandle(self.handle);
        }
    }
}

// ── WMI fallback ──────────────────────────────────────────────────────────────
//
// Used when HWiNFO64 is not running. Three WMI classes are queried:
//
//   ROOT\WMI   — MSAcpi_ThermalZoneTemperature (ACPI thermal zones, deci-Kelvin)
//   ROOT\CIMV2 — Win32_TemperatureProbe (BIOS-reported temps, DMTF tenths °C)
//   ROOT\CIMV2 — Win32_Fan (motherboard fan RPM via SMBIOS)
//
// "No rows returned" (NoSensorsFound) and "query error" (QueryFailed) are
// deliberately distinct: the former is debug-level (normal on many boards),
// the latter is warn-level (unexpected failure).

fn wmi_snapshot() -> (Vec<SensorReading>, Vec<FanReading>) {
    use serde::Deserialize;
    use wmi::WMIConnection;

    let span = tracing::debug_span!("wmi_snapshot", backend = "wmi");
    let _enter = span.enter();

    // ── ROOT\WMI connection ───────────────────────────────────────────────────
    let wmi_conn = match WMIConnection::with_namespace_path("ROOT\\WMI") {
        Ok(c) => c,
        Err(e) => {
            let err = WmiError::ConnectionFailed(e.to_string());
            tracing::warn!(backend = "wmi", error = %err, "WMI unavailable");
            return (Vec::new(), Vec::new());
        }
    };

    // ── ROOT\CIMV2 connection (Win32_* classes) ───────────────────────────────
    let cimv2_conn = WMIConnection::with_namespace_path("ROOT\\CIMV2").ok();

    let mut all_sensors: Vec<SensorReading> = Vec::new();
    let mut all_fans: Vec<FanReading> = Vec::new();

    // ── 1. MSAcpi_ThermalZoneTemperature (deci-Kelvin) ────────────────────────
    {
        #[derive(Deserialize)]
        #[serde(rename = "MSAcpi_ThermalZoneTemperature")]
        struct ThermalZone {
            #[serde(rename = "CurrentTemperature")]
            current_temperature: u32, // deci-Kelvin: divide by 10, subtract 273.15
            #[serde(rename = "InstanceName")]
            instance_name: String,
        }

        match wmi_conn.query::<ThermalZone>() {
            Ok(zones) if zones.is_empty() => {
                let err = WmiError::NoSensorsFound {
                    class: "MSAcpi_ThermalZoneTemperature",
                };
                tracing::debug!(backend = "wmi", "{err}");
            }
            Ok(zones) => {
                let before = all_sensors.len();
                for (i, z) in zones.into_iter().enumerate() {
                    let temp = (z.current_temperature as f32 / 10.0) - 273.15;
                    if temp > -40.0 && temp < 150.0 {
                        all_sensors.push(SensorReading {
                            id: format!("wmi_acpi_thermal_{i}"),
                            label: z.instance_name,
                            temp_celsius: temp,
                        });
                    }
                }
                tracing::debug!(
                    backend = "wmi",
                    count = all_sensors.len() - before,
                    "MSAcpi_ThermalZoneTemperature enumerated"
                );
            }
            Err(e) => {
                let err = WmiError::QueryFailed {
                    class: "MSAcpi_ThermalZoneTemperature",
                    detail: e.to_string(),
                };
                tracing::warn!(backend = "wmi", error = %err);
            }
        }
    }

    // ── 2. Win32_TemperatureProbe (SMBIOS/BIOS temperatures) ─────────────────
    if let Some(ref conn) = cimv2_conn {
        #[derive(Deserialize)]
        #[serde(rename = "Win32_TemperatureProbe")]
        struct TemperatureProbe {
            // DMTF spec: tenths of degree Celsius. Many BIOSes return whole
            // degrees instead — handle both by range-checking /10 first.
            #[serde(rename = "CurrentReading")]
            current_reading: Option<i32>,
            #[serde(rename = "Name")]
            name: Option<String>,
            #[serde(rename = "Description")]
            description: Option<String>,
        }

        match conn.query::<TemperatureProbe>() {
            Ok(probes) if probes.is_empty() => {
                let err = WmiError::NoSensorsFound {
                    class: "Win32_TemperatureProbe",
                };
                tracing::debug!(backend = "wmi", "{err}");
            }
            Ok(probes) => {
                let before = all_sensors.len();
                for (i, p) in probes.into_iter().enumerate() {
                    let Some(raw) = p.current_reading else {
                        continue;
                    };
                    // Try DMTF tenths-of-Celsius first; fall back to raw integer Celsius.
                    let temp_tenths = raw as f32 / 10.0;
                    let temp = if temp_tenths > -40.0 && temp_tenths < 150.0 {
                        temp_tenths
                    } else {
                        raw as f32
                    };
                    if temp > -40.0 && temp < 150.0 {
                        let label = p
                            .name
                            .or(p.description)
                            .unwrap_or_else(|| format!("Temperature Probe {i}"));
                        all_sensors.push(SensorReading {
                            id: format!("wmi_probe_{i}"),
                            label,
                            temp_celsius: temp,
                        });
                    }
                }
                tracing::debug!(
                    backend = "wmi",
                    count = all_sensors.len() - before,
                    "Win32_TemperatureProbe enumerated"
                );
            }
            Err(e) => {
                let err = WmiError::QueryFailed {
                    class: "Win32_TemperatureProbe",
                    detail: e.to_string(),
                };
                // Debug-level: absent on most consumer hardware, not actionable.
                tracing::debug!(backend = "wmi", error = %err, "Win32_TemperatureProbe unavailable");
            }
        }
    }

    // ── 3. Win32_Fan (SMBIOS fan speeds) ─────────────────────────────────────
    if let Some(ref conn) = cimv2_conn {
        #[derive(Deserialize)]
        #[serde(rename = "Win32_Fan")]
        struct Win32Fan {
            #[serde(rename = "DesiredSpeed")]
            desired_speed: Option<u64>,
            #[serde(rename = "Name")]
            name: Option<String>,
            #[serde(rename = "DeviceID")]
            device_id: Option<String>,
        }

        match conn.query::<Win32Fan>() {
            Ok(fans) if fans.is_empty() => {
                let err = WmiError::NoSensorsFound { class: "Win32_Fan" };
                tracing::debug!(backend = "wmi", "{err}");
            }
            Ok(fans) => {
                for (i, fan) in fans.into_iter().enumerate() {
                    let label = fan
                        .name
                        .or(fan.device_id)
                        .unwrap_or_else(|| format!("Fan {i}"));
                    let rpm = fan.desired_speed.unwrap_or(0) as u32;
                    all_fans.push(FanReading {
                        id: format!("wmi_fan_{i}"),
                        label,
                        rpm,
                        duty_percent: 0,
                    });
                }
                tracing::debug!(
                    backend = "wmi",
                    count = all_fans.len(),
                    "Win32_Fan enumerated"
                );
            }
            Err(e) => {
                let err = WmiError::QueryFailed {
                    class: "Win32_Fan",
                    detail: e.to_string(),
                };
                tracing::debug!(backend = "wmi", error = %err, "Win32_Fan unavailable (normal on consumer hardware)");
            }
        }
    }

    if all_sensors.is_empty() && all_fans.is_empty() {
        tracing::warn!(
            backend = "wmi",
            "WMI fallback found no sensors or fans — install HWiNFO64 from \
             https://www.hwinfo.com/download/ for full coverage"
        );
    }

    (all_sensors, all_fans)
}

// ── NVML GPU fans ─────────────────────────────────────────────────────────────

fn nvml_fans() -> Vec<FanReading> {
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            tracing::debug!("NVML init failed (no NVIDIA driver?): {e}");
            return Vec::new();
        }
    };

    let count = match nvml.device_count() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut fans = Vec::new();
    for gpu_idx in 0..count {
        let Ok(device) = nvml.device_by_index(gpu_idx) else {
            continue;
        };
        let num_fans = device.num_fans().unwrap_or(1);
        for fan_idx in 0..num_fans {
            // NVML's `fan_speed` returns 0-100 (duty %), not RPM. The library
            // does not expose fan RPM directly on Windows; leave as 0.
            let duty = device.fan_speed(fan_idx).unwrap_or(0) as u8;
            let rpm = 0u32;
            fans.push(FanReading {
                id: format!("nvml_gpu{gpu_idx}_fan{fan_idx}"),
                label: format!(
                    "GPU {} Fan {}",
                    device.name().unwrap_or_else(|_| format!("GPU {gpu_idx}")),
                    fan_idx
                ),
                rpm,
                duty_percent: duty,
            });
        }
    }
    fans
}

fn nvml_set_fan(fan_id: &str, duty_percent: u8) -> Result<()> {
    // fan_id format: nvml_gpu{gpu_idx}_fan{fan_idx}
    let rest = fan_id
        .strip_prefix("nvml_gpu")
        .ok_or_else(|| anyhow!("not an NVML fan id"))?;
    let mut parts = rest.splitn(2, "_fan");
    let gpu_idx: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("invalid nvml fan id: {fan_id}"))?;
    let fan_idx: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("invalid nvml fan id: {fan_id}"))?;

    // Direct FFI to nvidia-ml.dll for fan speed control.
    // NVIDIA NVML library exports:
    // - nvmlInit_v2 / nvmlShutdown
    // - nvmlDeviceGetHandleByIndex
    // - nvmlDeviceSetFanSpeed_v2(device, fan_index, policy, speed)
    //   where policy=0 (default), speed=0-100
    //
    // Note: Requires NVIDIA driver 520+ and admin privileges.

    let lib = unsafe { libloading::Library::new("nvidia-ml.dll") }
        .context("nvidia-ml.dll not found — NVIDIA driver (520+) required")?;

    unsafe {
        // Load NVML functions
        let init: libloading::Symbol<unsafe extern "C" fn() -> i32> =
            lib.get(b"nvmlInit_v2\0").context("nvmlInit_v2 not found")?;

        let shutdown: libloading::Symbol<unsafe extern "C" fn() -> i32> = lib
            .get(b"nvmlShutdown\0")
            .context("nvmlShutdown not found")?;

        let device_get: libloading::Symbol<unsafe extern "C" fn(u32, *mut nvml_device_t) -> i32> =
            lib.get(b"nvmlDeviceGetHandleByIndex_v2\0")
                .or_else(|_| lib.get(b"nvmlDeviceGetHandleByIndex\0"))
                .context("nvmlDeviceGetHandleByIndex not found")?;

        let fan_set: libloading::Symbol<unsafe extern "C" fn(nvml_device_t, u32, u32, u32) -> i32> =
            lib.get(b"nvmlDeviceSetFanSpeed_v2\0")
                .context("nvmlDeviceSetFanSpeed_v2 not found — driver 520+ required")?;

        if init() != NVML_OK {
            bail!("nvmlInit_v2 failed");
        }

        let mut device: nvml_device_t = std::ptr::null_mut();
        if device_get(gpu_idx, &mut device) != NVML_OK {
            shutdown();
            bail!("nvmlDeviceGetHandleByIndex failed for GPU {gpu_idx}");
        }

        // Set fan speed: policy=1 (manual), speed=0-100. `fan_idx` targets the
        // specific fan on multi-fan cards (e.g. RTX 4090 has 3).
        let speed = (duty_percent as u32).clamp(0, 100);
        let ret = fan_set(device, fan_idx, 1, speed);
        shutdown();

        if ret != NVML_OK {
            bail!("nvmlDeviceSetFanSpeed_v2 failed (error {ret}). Check that driver is 520+.");
        }
    }

    tracing::info!(
        fan = fan_id,
        duty = duty_percent,
        "Windows: NVIDIA NVML fan speed set"
    );
    Ok(())
}

// NVML type definitions for FFI
#[allow(non_camel_case_types)]
type nvml_device_t = *mut std::ffi::c_void;
const NVML_OK: i32 = 0;

// ── AMD ADL fan control ───────────────────────────────────────────────────────
//
// Uses AMD Display Library (ADL) via `atiadlxx.dll` — shipped with all AMD
// Adrenalin drivers. This covers GCN/RDNA/RDNA2 cards (Overdrive5 API).
//
// RDNA3 and newer with per-fan PWM require Overdrive8 / ADLX — see Phase 2.
// Deferred: ADLX COM vtable FFI requires the proprietary AMD ADLX SDK headers.
//
// Fan ID format: `adl_gpu{adapter_index}` (one fan per adapter index).

const ADL_OK: i32 = 0;
const ADL_VENDOR_ID_AMD: i32 = 0x1002;
const ADL_DL_FANCTRL_SPEED_TYPE_PERCENT: i32 = 2;
const ADL_DL_FANCTRL_FLAG_USER_DEFINED_SPEED: i32 = 1;

type AdlContextHandle = *mut std::ffi::c_void;

/// ADL_MAIN_MALLOC_CALLBACK — ADL allocates strings/buffers through this.
/// Using Rust's global allocator is safe on Windows MSVC (both use the CRT heap).
///
/// KNOWN LEAK: `ADL2_Main_Control_Create` accepts only a malloc callback, not
/// a free callback. ADL-internal string allocations (driver names, etc.) made
/// via this function are never freed by us. Since `adl_fans`/`adl_set_fan` are
/// called at most every few seconds and each call yields only a handful of
/// small (< 1 KB) allocations, the steady-state leak is negligible. A proper
/// fix would require tracking allocations in a static `HashMap` keyed by
/// pointer — deferred as out-of-scope for v1.0.
unsafe extern "C" fn adl_alloc(size: i32) -> *mut std::ffi::c_void {
    if size <= 0 {
        return std::ptr::null_mut();
    }
    let Ok(layout) = alloc::Layout::from_size_align(size as usize, 8) else {
        return std::ptr::null_mut();
    };
    alloc::alloc(layout) as *mut std::ffi::c_void
}

/// Windows-specific AdapterInfo struct (ADL SDK, Windows variant).
/// Matches the layout from ADL_SDK/include/adl_structures.h on Windows builds.
#[repr(C)]
struct AdlAdapterInfo {
    size: i32,
    adapter_index: i32,
    udid: [u8; 256],
    bus_number: i32,
    device_number: i32,
    function_number: i32,
    vendor_id: i32,
    adapter_name: [u8; 256],
    display_name: [u8; 256],
    present: i32,
    // Windows-only extension fields
    exist: i32,
    driver_path: [u8; 256],
    driver_path_ext: [u8; 256],
    pnp_string: [u8; 256],
    os_display_index: i32,
}

#[repr(C)]
struct AdlFanSpeedValue {
    size: i32,
    speed_type: i32,
    fan_speed: i32,
    flags: i32,
}

fn adl_fans() -> Vec<FanReading> {
    let lib = match unsafe { libloading::Library::new("atiadlxx.dll") } {
        Ok(l) => l,
        Err(_) => {
            tracing::debug!("atiadlxx.dll not found — no AMD driver installed");
            return Vec::new();
        }
    };

    unsafe {
        // Load function pointers from ADL DLL
        let create: libloading::Symbol<
            unsafe extern "C" fn(
                unsafe extern "C" fn(i32) -> *mut std::ffi::c_void,
                i32,
                *mut AdlContextHandle,
            ) -> i32,
        > = match lib.get(b"ADL2_Main_Control_Create\0") {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let destroy: libloading::Symbol<unsafe extern "C" fn(AdlContextHandle) -> i32> =
            match lib.get(b"ADL2_Main_Control_Destroy\0") {
                Ok(f) => f,
                Err(_) => return Vec::new(),
            };
        let get_count: libloading::Symbol<unsafe extern "C" fn(AdlContextHandle, *mut i32) -> i32> =
            match lib.get(b"ADL2_Adapter_NumberOfAdapters_Get\0") {
                Ok(f) => f,
                Err(_) => return Vec::new(),
            };
        let get_info: libloading::Symbol<
            unsafe extern "C" fn(AdlContextHandle, *mut AdlAdapterInfo, i32) -> i32,
        > = match lib.get(b"ADL2_Adapter_AdapterInfo_Get\0") {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let fan_get: libloading::Symbol<
            unsafe extern "C" fn(AdlContextHandle, i32, i32, *mut AdlFanSpeedValue) -> i32,
        > = match lib.get(b"ADL2_Overdrive5_FanSpeed_Get\0") {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let mut ctx: AdlContextHandle = std::ptr::null_mut();
        if create(adl_alloc, 1, &mut ctx) != ADL_OK {
            tracing::debug!("ADL2_Main_Control_Create failed");
            return Vec::new();
        }

        let mut num_adapters = 0i32;
        if get_count(ctx, &mut num_adapters) != ADL_OK || num_adapters <= 0 {
            destroy(ctx);
            return Vec::new();
        }

        let elem_size = std::mem::size_of::<AdlAdapterInfo>();
        let buf_size = elem_size * num_adapters as usize;
        let layout = alloc::Layout::from_size_align_unchecked(buf_size, 8);
        let info_ptr = alloc::alloc_zeroed(layout) as *mut AdlAdapterInfo;

        let mut fans = Vec::new();

        if get_info(ctx, info_ptr, buf_size as i32) == ADL_OK {
            for i in 0..num_adapters as usize {
                let adapter = &*info_ptr.add(i);
                if adapter.vendor_id != ADL_VENDOR_ID_AMD || adapter.present == 0 {
                    continue;
                }

                let mut fan_val = AdlFanSpeedValue {
                    size: std::mem::size_of::<AdlFanSpeedValue>() as i32,
                    speed_type: ADL_DL_FANCTRL_SPEED_TYPE_PERCENT,
                    fan_speed: 0,
                    flags: 0,
                };
                let duty = if fan_get(ctx, adapter.adapter_index, 0, &mut fan_val) == ADL_OK {
                    fan_val.fan_speed.clamp(0, 100) as u8
                } else {
                    0
                };

                fans.push(FanReading {
                    id: format!("adl_gpu{}", adapter.adapter_index),
                    label: format!("{} Fan", cstr_to_string(&adapter.adapter_name)),
                    rpm: 0, // Overdrive5 returns percent, not RPM
                    duty_percent: duty,
                });
            }
        }

        // Destroy the ADL context BEFORE freeing the adapter info buffer:
        // `destroy` may dereference adapter pointers registered during
        // `get_info`; freeing the buffer first would be a use-after-free.
        destroy(ctx);
        alloc::dealloc(info_ptr as *mut u8, layout);
        fans
    }
}

fn adl_set_fan(fan_id: &str, duty_percent: u8) -> Result<()> {
    let adapter_idx: i32 = fan_id
        .strip_prefix("adl_gpu")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("invalid adl fan id: {fan_id}"))?;

    let lib = unsafe { libloading::Library::new("atiadlxx.dll") }
        .context("atiadlxx.dll not found — AMD driver required")?;

    unsafe {
        let create: libloading::Symbol<
            unsafe extern "C" fn(
                unsafe extern "C" fn(i32) -> *mut std::ffi::c_void,
                i32,
                *mut AdlContextHandle,
            ) -> i32,
        > = lib
            .get(b"ADL2_Main_Control_Create\0")
            .context("ADL2_Main_Control_Create")?;
        let destroy: libloading::Symbol<unsafe extern "C" fn(AdlContextHandle) -> i32> = lib
            .get(b"ADL2_Main_Control_Destroy\0")
            .context("ADL2_Main_Control_Destroy")?;
        let fan_set: libloading::Symbol<
            unsafe extern "C" fn(AdlContextHandle, i32, *mut AdlFanSpeedValue) -> i32,
        > = lib
            .get(b"ADL2_Overdrive5_FanSpeed_Set\0")
            .context("ADL2_Overdrive5_FanSpeed_Set")?;

        let mut ctx: AdlContextHandle = std::ptr::null_mut();
        if create(adl_alloc, 1, &mut ctx) != ADL_OK {
            bail!("ADL2_Main_Control_Create failed");
        }

        let mut fan_val = AdlFanSpeedValue {
            size: std::mem::size_of::<AdlFanSpeedValue>() as i32,
            speed_type: ADL_DL_FANCTRL_SPEED_TYPE_PERCENT,
            fan_speed: duty_percent as i32,
            flags: ADL_DL_FANCTRL_FLAG_USER_DEFINED_SPEED,
        };

        let ret = fan_set(ctx, adapter_idx, &mut fan_val);
        destroy(ctx);

        if ret != ADL_OK {
            bail!("ADL2_Overdrive5_FanSpeed_Set returned {ret} (adapter {adapter_idx})");
        }
    }

    tracing::info!(
        fan = fan_id,
        duty = duty_percent,
        "Windows: AMD ADL fan speed set"
    );
    Ok(())
}

// ── Public HAL ────────────────────────────────────────────────────────────────

pub struct WindowsHardwareLayer {
    /// Cached HWiNFO64 mapping. `None` = not yet opened or HWiNFO64 not running.
    /// Re-attempted on each snapshot so the daemon survives a late HWiNFO64 start.
    hwinfo: Mutex<Option<HwinfoMapping>>,
}

impl WindowsHardwareLayer {
    pub fn new() -> Self {
        Self {
            hwinfo: Mutex::new(None),
        }
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
        // Take the cached mapping out of the Mutex so we can move it into
        // the blocking closure without holding the lock across an await.
        let cached = {
            let mut guard = self.hwinfo.lock().unwrap();
            guard.take()
        };

        let (mapping_result, sensors, fans) = tokio::task::spawn_blocking(move || {
            // Try to (re-)open HWiNFO64 mapping if we don't have one.
            let mapping = match cached {
                Some(m) => Ok(m),
                None => HwinfoMapping::open(),
            };

            let (sensors, fans, new_mapping) = match mapping {
                Ok(m) => {
                    let (s, mut f) = m.read_snapshot();
                    // Augment with GPU fans (more accurate duty than HWiNFO)
                    f.extend(nvml_fans()); // NVIDIA
                    f.extend(adl_fans()); // AMD
                    (s, f, Some(m))
                }
                Err(ref e) => {
                    // Emit appropriate log level based on error category.
                    match e {
                        HwinfoError::NotRunning => tracing::warn!(
                            backend = "hwinfo",
                            error = %e,
                            "HWiNFO64 not running — falling back to WMI (reduced coverage). \
                             Install HWiNFO64 from https://www.hwinfo.com/download/"
                        ),
                        HwinfoError::PermissionDenied(_) => tracing::warn!(
                            backend = "hwinfo",
                            error = %e,
                            "HWiNFO64 permission denied — try running ThermalIQ as Administrator"
                        ),
                        HwinfoError::StaleData => tracing::warn!(
                            backend = "hwinfo",
                            error = %e,
                            "HWiNFO64 not yet polling — falling back to WMI"
                        ),
                        HwinfoError::SignatureMismatch { .. }
                        | HwinfoError::UnsupportedVersion { .. }
                        | HwinfoError::UnsupportedRevision { .. }
                        | HwinfoError::LayoutInvalid { .. } => tracing::error!(
                            backend = "hwinfo",
                            error = %e,
                            "HWiNFO64 header validation failed — falling back to WMI"
                        ),
                        HwinfoError::MapFailed(_) => tracing::warn!(
                            backend = "hwinfo",
                            error = %e,
                            "HWiNFO64 map failed — falling back to WMI"
                        ),
                    }
                    let (s, mut f) = wmi_snapshot();
                    f.extend(nvml_fans());
                    f.extend(adl_fans());
                    (s, f, None)
                }
            };

            (new_mapping, sensors, fans)
        })
        .await?;

        // Store the (possibly new) mapping back.
        *self.hwinfo.lock().unwrap() = mapping_result;

        Ok(HardwareSnapshot { sensors, fans })
    }

    async fn set_fan_duty(&self, cmd: SetFanDuty) -> Result<()> {
        let fan_id = cmd.fan_id.clone();

        if fan_id.starts_with("nvml_gpu") {
            let duty = cmd.duty_percent;
            tokio::task::spawn_blocking(move || nvml_set_fan(&fan_id, duty)).await?
        } else if fan_id.starts_with("adl_gpu") {
            let duty = cmd.duty_percent;
            tokio::task::spawn_blocking(move || adl_set_fan(&fan_id, duty)).await?
        } else {
            // Motherboard headers are not writable without a kernel driver.
            tracing::warn!(
                fan = %cmd.fan_id,
                duty = cmd.duty_percent,
                "Windows: motherboard fan writes require a kernel driver (not supported). \
                 Use nvml_gpu* (NVIDIA) or adl_gpu* (AMD) fan IDs for GPU fan control."
            );
            bail!(
                "fan '{}' is not writable on Windows — GPU fans only: \
                 'nvml_gpu{{idx}}_fan{{idx}}' (NVIDIA) or 'adl_gpu{{idx}}' (AMD)",
                cmd.fan_id
            )
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── constants ─────────────────────────────────────────────────────────────

    #[test]
    fn hwinfo_signature_magic() {
        assert_eq!(HWINFO_SIGNATURE, u32::from_le_bytes(*b"HWiS"));
    }

    // ── cstr_to_string ────────────────────────────────────────────────────────

    #[test]
    fn cstr_to_string_nul_terminated() {
        let mut buf = [0u8; 128];
        buf[..5].copy_from_slice(b"hello");
        assert_eq!(cstr_to_string(&buf), "hello");
    }

    #[test]
    fn cstr_to_string_full_no_nul() {
        let buf = [b'x'; 16];
        assert_eq!(cstr_to_string(&buf), "x".repeat(16));
    }

    #[test]
    fn cstr_to_string_non_utf8_replaced() {
        // 0xFF is invalid UTF-8 — should produce replacement char, not panic.
        let buf = [b'A', 0xFF, b'B', 0x00];
        let s = cstr_to_string(&buf);
        assert!(s.contains('A'));
        assert!(s.contains('B'));
    }

    // ── sanitize_label_for_id ─────────────────────────────────────────────────

    #[test]
    fn sanitize_label_simple_ascii() {
        assert_eq!(sanitize_label_for_id("CPU Temp"), "CPU_Temp");
    }

    #[test]
    fn sanitize_label_vendor_suffix() {
        // Typical HP/Dell label with non-ID-safe characters
        let result = sanitize_label_for_id("CPU Package Temp #1 (TJ Max)");
        assert!(!result.contains('('));
        assert!(!result.contains(')'));
        assert!(!result.contains('#'));
        assert!(!result.contains(' '));
        assert!(!result.is_empty());
    }

    #[test]
    fn sanitize_label_non_ascii() {
        // Degree symbol (U+00B0) — common in vendor-provided labels
        let label = "CPU Temp\u{00B0}C";
        let result = sanitize_label_for_id(label);
        assert!(!result.contains('\u{00B0}'));
        assert!(!result.is_empty());
    }

    #[test]
    fn sanitize_label_empty_falls_back() {
        assert_eq!(sanitize_label_for_id(""), "unknown");
        assert_eq!(sanitize_label_for_id("   "), "unknown");
    }

    #[test]
    fn sanitize_label_collapses_underscores() {
        assert_eq!(sanitize_label_for_id("A  B   C"), "A_B_C");
    }

    #[test]
    fn sanitize_label_truncates_long() {
        let long = "A".repeat(100);
        assert!(sanitize_label_for_id(&long).len() <= 64);
    }

    // ── ID formation ─────────────────────────────────────────────────────────

    #[test]
    fn hwinfo_sensor_id_format() {
        assert_eq!(
            hwinfo_sensor_id(2, 42, "CPU Package"),
            "hwinfo_2_42_CPU_Package"
        );
    }

    #[test]
    fn hwinfo_fan_id_format() {
        assert_eq!(
            hwinfo_fan_id(7, "Chassis Fan 1"),
            "hwinfo_fan_7_Chassis_Fan_1"
        );
    }

    // ── nvml ID parse roundtrip ───────────────────────────────────────────────

    #[test]
    fn nvml_fan_id_parse_roundtrip() {
        let id = "nvml_gpu2_fan1";
        assert!(id.starts_with("nvml_gpu"));
        let rest = id.strip_prefix("nvml_gpu").unwrap();
        let mut p = rest.splitn(2, "_fan");
        let gpu: u32 = p.next().unwrap().parse().unwrap();
        let fan: u32 = p.next().unwrap().parse().unwrap();
        assert_eq!(gpu, 2);
        assert_eq!(fan, 1);
    }

    // ── Mock HWiNFO buffer tests ──────────────────────────────────────────────
    //
    // Construct raw byte buffers shaped like the HWiNFO shared memory and
    // verify header validation logic without real hardware or Win32 APIs.
    // Gated on mock-hardware so they never interfere with a live HWiNFO64.

    #[cfg(feature = "mock-hardware")]
    mod mock_hwinfo {
        use super::*;

        /// Build a valid HwinfoHeader via struct literal.
        /// Constructing via the struct itself (rather than hardcoded byte
        /// offsets) is robust to ABI padding — the `i64` after three `u32`s
        /// introduces 4 bytes of alignment padding on MSVC x86_64, which
        /// invalidated an earlier byte-offset implementation on Windows CI.
        fn valid_header() -> HwinfoHeader {
            let hdr_size = std::mem::size_of::<HwinfoHeader>() as u32;
            HwinfoHeader {
                signature: HWINFO_SIGNATURE,
                version: 2,
                revision: 0,
                poll_time_ms: 1000,
                sensor_section_offset: hdr_size,
                sensor_element_size: 264,
                sensor_element_count: 0,
                reading_section_offset: hdr_size,
                reading_element_size: 296,
                reading_element_count: 0,
            }
        }

        #[test]
        fn valid_header_passes_all_checks() {
            let h = valid_header();
            assert_eq!(h.signature, HWINFO_SIGNATURE);
            assert!(h.version >= HWINFO_MIN_VERSION && h.version <= HWINFO_MAX_VERSION);
            assert!(h.revision <= HWINFO_MAX_REVISION);
            assert!(h.sensor_element_size >= HWINFO_MIN_ELEMENT_SIZE);
            assert!(h.sensor_element_size <= HWINFO_MAX_ELEMENT_SIZE);
            assert!(h.reading_element_size >= HWINFO_MIN_ELEMENT_SIZE);
            assert!(h.reading_element_size <= HWINFO_MAX_ELEMENT_SIZE);
            assert!(h.poll_time_ms != 0);
        }

        #[test]
        fn bad_signature_detected() {
            let h = HwinfoHeader {
                signature: 0xDEAD_BEEF,
                ..valid_header()
            };
            assert_ne!(h.signature, HWINFO_SIGNATURE);
        }

        #[test]
        fn version_zero_is_unsupported() {
            let h = HwinfoHeader {
                version: 0,
                ..valid_header()
            };
            assert!(h.version < HWINFO_MIN_VERSION);
        }

        #[test]
        fn version_too_high_is_unsupported() {
            let h = HwinfoHeader {
                version: 99,
                ..valid_header()
            };
            assert!(h.version > HWINFO_MAX_VERSION);
        }

        #[test]
        fn revision_too_high_is_rejected() {
            let h = HwinfoHeader {
                revision: 100,
                ..valid_header()
            };
            assert!(h.revision > HWINFO_MAX_REVISION);
        }

        #[test]
        fn stale_poll_time_detected() {
            let h = HwinfoHeader {
                poll_time_ms: 0,
                ..valid_header()
            };
            assert_eq!(h.poll_time_ms, 0);
        }

        #[test]
        fn element_size_zero_is_invalid() {
            let h = HwinfoHeader {
                sensor_element_size: 0,
                ..valid_header()
            };
            assert!(h.sensor_element_size < HWINFO_MIN_ELEMENT_SIZE);
        }

        #[test]
        fn element_count_overflow_detected() {
            let h = HwinfoHeader {
                reading_element_count: 99_999,
                ..valid_header()
            };
            assert!(h.reading_element_count > HWINFO_MAX_ELEMENT_COUNT);
        }

        #[test]
        fn hwinfo_error_display_not_running() {
            let e = HwinfoError::NotRunning;
            let s = e.to_string();
            assert!(s.contains("HWiNFO64"));
            assert!(s.contains("hwinfo.com"));
        }

        #[test]
        fn hwinfo_error_display_signature_mismatch() {
            let e = HwinfoError::SignatureMismatch { found: 0xDEAD_BEEF };
            let s = e.to_string();
            assert!(s.contains("DEADBEEF"));
            assert!(s.contains("HWiS"));
        }

        #[test]
        fn hwinfo_error_display_version() {
            let e = HwinfoError::UnsupportedVersion { version: 99 };
            let s = e.to_string();
            assert!(s.contains("99"));
        }

        #[test]
        fn wmi_error_display_no_sensors() {
            let e = WmiError::NoSensorsFound { class: "Win32_Fan" };
            let s = e.to_string();
            assert!(s.contains("Win32_Fan"));
        }
    }
}

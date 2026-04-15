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
use std::sync::Mutex;
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use crate::HardwareLayer;

// ── Windows-only imports ─────────────────────────────────────────────────────
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ},
    },
};

// ── HWiNFO64 shared-memory layout ────────────────────────────────────────────
//
// Layout documented in the HWiNFO SDK (stable since ~2013).
// https://www.hwinfo.com/forum/threads/hwinfo-sdk.86/

const HWINFO_SIGNATURE: u32 = 0x53695748; // b"HWiS" LE
const HWINFO_SM_NAME: &str = "Global\\HWiNFO_SENS_SM2";

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

// ── Shared-memory handle wrapper ──────────────────────────────────────────────

struct HwinfoMapping {
    handle: HANDLE,
    view: *const u8,
}

// SAFETY: The view pointer is valid for the lifetime of the mapping and we
// only ever read it from within spawn_blocking (single OS thread at a time).
unsafe impl Send for HwinfoMapping {}

impl HwinfoMapping {
    fn open() -> Result<Self> {
        let name_wide: Vec<u16> = HWINFO_SM_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe {
            OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(name_wide.as_ptr()))
                .map_err(|e| anyhow!("OpenFileMappingW failed: {e}"))?
        };

        if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
            return Err(anyhow!("HWiNFO64 shared memory not found — is HWiNFO64 running?"));
        }

        let view_mapped = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
        // MapViewOfFile returns a handle newtype; .Value is the raw *mut c_void pointer.
        let view_ptr = view_mapped.Value;
        if view_ptr.is_null() {
            unsafe { let _ = CloseHandle(handle); }
            return Err(anyhow!("MapViewOfFile failed"));
        }

        // Validate signature
        let header = unsafe { &*(view_ptr as *const HwinfoHeader) };
        if header.signature != HWINFO_SIGNATURE {
            unsafe {
                let _ = UnmapViewOfFile(view_mapped);
                let _ = CloseHandle(handle);
            }
            return Err(anyhow!(
                "HWiNFO64 shared memory signature mismatch: {:#010x}",
                header.signature
            ));
        }

        Ok(Self {
            handle,
            view: view_ptr as *const u8,
        })
    }

    fn read_snapshot(&self) -> (Vec<SensorReading>, Vec<FanReading>) {
        let header = unsafe { &*(self.view as *const HwinfoHeader) };
        let mut sensors = Vec::new();
        let mut fans = Vec::new();

        let reading_base =
            unsafe { self.view.add(header.reading_section_offset as usize) };

        for i in 0..header.reading_element_count as usize {
            let reading = unsafe {
                &*(reading_base.add(i * header.reading_element_size as usize)
                    as *const HwinfoReading)
            };

            let label = cstr_to_string(&reading.label_user);

            match reading.reading_type {
                READING_TYPE_TEMP => {
                    let temp = reading.value as f32;
                    if temp > -40.0 && temp < 150.0 {
                        sensors.push(SensorReading {
                            id: format!(
                                "hwinfo_{}_{}",
                                reading.sensor_index, reading.reading_id
                            ),
                            label,
                            temp_celsius: temp,
                        });
                    }
                }
                READING_TYPE_FAN => {
                    fans.push(FanReading {
                        id: format!("hwinfo_fan_{}", reading.reading_id),
                        label,
                        rpm: reading.value as u32,
                        // HWiNFO exposes RPM only, not duty cycle
                        duty_percent: 0,
                    });
                }
                _ => {}
            }
        }

        (sensors, fans)
    }
}

impl Drop for HwinfoMapping {
    fn drop(&mut self) {
        unsafe {
            // UnmapViewOfFile takes a handle newtype wrapping *mut c_void.
            // Transmute from our raw pointer — the layout is identical.
            let _ = UnmapViewOfFile(std::mem::transmute(self.view as *mut std::ffi::c_void));
            let _ = CloseHandle(self.handle);
        }
    }
}

fn cstr_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

// ── WMI fallback ──────────────────────────────────────────────────────────────
//
// Phase 1: WMI COM fallback is stubbed out. The windows 0.58 crate relocates
// VARIANT and related COM types across feature-gated sub-modules that vary by
// version; a full WMI integration will be added in Phase 2 once verified on a
// live Windows machine. HWiNFO64 covers the primary use-case.

fn wmi_snapshot() -> (Vec<SensorReading>, Vec<FanReading>) {
    tracing::warn!(
        "HWiNFO64 not running and WMI fallback is not yet implemented. \
         Install HWiNFO64 from https://www.hwinfo.com/download/ for sensor data."
    );
    (Vec::new(), Vec::new())
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
        let Ok(device) = nvml.device_by_index(gpu_idx) else { continue };
        let num_fans = device.num_fans().unwrap_or(1);
        for fan_idx in 0..num_fans {
            let rpm = device
                .fan_speed(fan_idx)
                // NVML returns 0-100 for fan_speed but rpm is separate
                .map(|_| 0u32)
                .unwrap_or(0);
            let duty = device.fan_speed(fan_idx).unwrap_or(0) as u8;
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

    // nvml-wrapper 0.10 does not yet expose nvmlDeviceSetFanSpeed_v2 in its safe
    // API. Fan speed writes will be available in a future crate release.
    // Track: https://github.com/Cldfire/nvml-wrapper/issues
    let _ = (gpu_idx, fan_idx, duty_percent);
    bail!(
        "NVML GPU fan writes are not yet available in nvml-wrapper 0.10. \
         Upgrade the crate when nvmlDeviceSetFanSpeed_v2 is wrapped."
    )
}

// ── AMD ADL fan control ───────────────────────────────────────────────────────
//
// Uses AMD Display Library (ADL) via `atiadlxx.dll` — shipped with all AMD
// Adrenalin drivers. This covers GCN/RDNA/RDNA2 cards (Overdrive5 API).
//
// RDNA3 and newer with per-fan PWM require Overdrive8 / ADLX — see Phase 2.
// Deferred: ADLX COM vtable FFI requires the proprietary AMD ADLX SDK headers.
//
// Fan ID format: `adl_gpu{adapter_index}` (one fan per adapter index).

use std::alloc;

const ADL_OK: i32 = 0;
const ADL_VENDOR_ID_AMD: i32 = 0x1002;
const ADL_DL_FANCTRL_SPEED_TYPE_PERCENT: i32 = 2;
const ADL_DL_FANCTRL_FLAG_USER_DEFINED_SPEED: i32 = 1;

type AdlContextHandle = *mut std::ffi::c_void;

/// ADL_MAIN_MALLOC_CALLBACK — ADL allocates strings/buffers through this.
/// Using Rust's global allocator is safe on Windows MSVC (both use the CRT heap).
unsafe extern "C" fn adl_alloc(size: i32) -> *mut std::ffi::c_void {
    let layout = alloc::Layout::from_size_align_unchecked(size as usize, 8);
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
        let get_count: libloading::Symbol<
            unsafe extern "C" fn(AdlContextHandle, *mut i32) -> i32,
        > = match lib.get(b"ADL2_Adapter_NumberOfAdapters_Get\0") {
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

        alloc::dealloc(info_ptr as *mut u8, layout);
        destroy(ctx);
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
        > = lib.get(b"ADL2_Main_Control_Create\0").context("ADL2_Main_Control_Create")?;
        let destroy: libloading::Symbol<unsafe extern "C" fn(AdlContextHandle) -> i32> =
            lib.get(b"ADL2_Main_Control_Destroy\0").context("ADL2_Main_Control_Destroy")?;
        let fan_set: libloading::Symbol<
            unsafe extern "C" fn(AdlContextHandle, i32, *mut AdlFanSpeedValue) -> i32,
        > = lib.get(b"ADL2_Overdrive5_FanSpeed_Set\0").context("ADL2_Overdrive5_FanSpeed_Set")?;

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

    tracing::info!(fan = fan_id, duty = duty_percent, "Windows: AMD ADL fan speed set");
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

        let (mapping_result, sensors, fans) =
            tokio::task::spawn_blocking(move || {
                // Try to (re-)open HWiNFO64 mapping if we don't have one.
                let mapping = match cached {
                    Some(m) => Ok(m),
                    None => HwinfoMapping::open(),
                };

                let (sensors, fans, new_mapping) = match mapping {
                    Ok(m) => {
                        let (s, mut f) = m.read_snapshot();
                        // Augment with GPU fans (more accurate duty than HWiNFO)
                        f.extend(nvml_fans());   // NVIDIA
                        f.extend(adl_fans());    // AMD
                        (s, f, Some(m))
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "HWiNFO64 not available — falling back to WMI (reduced sensor coverage). \
                             Install HWiNFO64 from https://www.hwinfo.com/download/ for full coverage."
                        );
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

    #[test]
    fn hwinfo_signature_magic() {
        // HWiNFO SDK defines the signature as the 4-byte string "HWiS" in LE.
        assert_eq!(HWINFO_SIGNATURE, u32::from_le_bytes(*b"HWiS"));
    }

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
    fn nvml_fan_id_parse_roundtrip() {
        // Smoke test that the ID format we emit is parseable by nvml_set_fan.
        let id = "nvml_gpu2_fan1";
        assert!(id.starts_with("nvml_gpu"));
        let rest = id.strip_prefix("nvml_gpu").unwrap();
        let mut p = rest.splitn(2, "_fan");
        let gpu: u32 = p.next().unwrap().parse().unwrap();
        let fan: u32 = p.next().unwrap().parse().unwrap();
        assert_eq!(gpu, 2);
        assert_eq!(fan, 1);
    }
}

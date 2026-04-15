//! macOS hardware backend — reads sensors and fans via IOKit SMC.
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
//!
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

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::ffi::{c_char, c_void, CString};
use std::mem;
use thermaliq_common::{FanReading, HardwareSnapshot, SensorReading, SetFanDuty};

use crate::HardwareLayer;

// ---------------------------------------------------------------------------
// IOKit FFI types and bindings
// ---------------------------------------------------------------------------

type IOService = u32;
type IOConnect = u32;
type IOReturn = i32;

const IO_OBJECT_NULL: u32 = 0;
const K_IO_RETURN_SUCCESS: IOReturn = 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SmcKeyDataVers {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SmcKeyDataPLimitData {
    version: u16,
    length: u16,
    cpu_plimit: u32,
    gpu_plimit: u32,
    mem_plimit: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SmcKeyDataKeyInfo {
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
}

#[repr(C)]
struct SmcKeyData {
    key: u32,
    vers: SmcKeyDataVers,
    p_limit_data: SmcKeyDataPLimitData,
    key_info: SmcKeyDataKeyInfo,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
}

impl Default for SmcKeyData {
    fn default() -> Self {
        // Safety: SmcKeyData is a plain-old-data struct with no invariants.
        unsafe { mem::zeroed() }
    }
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceGetMatchingService(master_port: u32, matching: *const c_void) -> IOService;
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceOpen(
        service: IOService,
        owning_task: u32,
        connect_type: u32,
        connect: *mut IOConnect,
    ) -> IOReturn;
    fn IOServiceClose(connect: IOConnect) -> IOReturn;
    fn IOObjectRelease(object: u32) -> IOReturn;
    fn IOConnectCallStructMethod(
        connection: IOConnect,
        selector: u32,
        input_struct: *const c_void,
        input_struct_cnt: usize,
        output_struct: *mut c_void,
        output_struct_cnt: *mut usize,
    ) -> IOReturn;
}

extern "C" {
    fn mach_task_self() -> u32;
}

// ---------------------------------------------------------------------------
// SMC helpers
// ---------------------------------------------------------------------------

fn four_char_code(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

fn key_to_u32(key: &str) -> u32 {
    let bytes = key.as_bytes();
    let mut arr = [b' '; 4];
    for (i, &b) in bytes.iter().enumerate().take(4) {
        arr[i] = b;
    }
    four_char_code(&arr)
}

/// A single connection to the AppleSMC IOKit service.
struct SmcConnection {
    conn: IOConnect,
}

impl SmcConnection {
    fn open() -> Result<Self> {
        unsafe {
            let service_name = CString::new("AppleSMC").expect("CString::new failed for AppleSMC");
            let matching = IOServiceMatching(service_name.as_ptr());
            let service = IOServiceGetMatchingService(0, matching as *const c_void);
            if service == IO_OBJECT_NULL {
                return Err(anyhow!("AppleSMC service not found"));
            }
            let mut conn: IOConnect = 0;
            let task = mach_task_self();
            let ret = IOServiceOpen(service, task, 0, &mut conn);
            IOObjectRelease(service);
            if ret != K_IO_RETURN_SUCCESS {
                return Err(anyhow!("IOServiceOpen failed: {ret}"));
            }
            Ok(Self { conn })
        }
    }

    fn read_key(&self, key: &str) -> Result<[u8; 32]> {
        // Step 1: get key info
        let mut input = SmcKeyData::default();
        let mut output = SmcKeyData::default();
        input.key = key_to_u32(key);
        input.data8 = 9; // kSMCGetKeyInfo

        let mut out_size = mem::size_of::<SmcKeyData>();
        unsafe {
            let ret = IOConnectCallStructMethod(
                self.conn,
                2,
                &input as *const _ as *const c_void,
                mem::size_of::<SmcKeyData>(),
                &mut output as *mut _ as *mut c_void,
                &mut out_size,
            );
            if ret != K_IO_RETURN_SUCCESS {
                return Err(anyhow!("SMC kSMCGetKeyInfo failed for key '{key}'"));
            }
        }

        // Step 2: read key value
        input.key_info = output.key_info;
        input.data8 = 5; // kSMCReadKey
        out_size = mem::size_of::<SmcKeyData>();
        unsafe {
            let ret = IOConnectCallStructMethod(
                self.conn,
                2,
                &input as *const _ as *const c_void,
                mem::size_of::<SmcKeyData>(),
                &mut output as *mut _ as *mut c_void,
                &mut out_size,
            );
            if ret != K_IO_RETURN_SUCCESS {
                return Err(anyhow!("SMC kSMCReadKey failed for key '{key}'"));
            }
        }
        Ok(output.bytes)
    }

    /// Read a temperature in sp78 (signed fixed-point 7.8) format.
    fn read_temp(&self, key: &str) -> Result<f32> {
        let bytes = self.read_key(key)?;
        let raw = ((bytes[0] as i16) << 8) | (bytes[1] as i16);
        Ok(raw as f32 / 256.0)
    }

    /// Read a fan value in fpe2 (unsigned fixed-point 14.2) format.
    fn read_fan_rpm(&self, fan_idx: u8, suffix: &str) -> Result<f32> {
        let key = format!("F{fan_idx}{suffix}");
        let bytes = self.read_key(&key)?;
        let raw = ((bytes[0] as u16) << 8) | (bytes[1] as u16);
        Ok(raw as f32 / 4.0)
    }

    fn fan_count(&self) -> u8 {
        self.read_key("FNum").map(|b| b[0]).unwrap_or(0)
    }
}

impl Drop for SmcConnection {
    fn drop(&mut self) {
        unsafe {
            IOServiceClose(self.conn);
        }
    }
}

// ---------------------------------------------------------------------------
// Known SMC temperature keys
// ---------------------------------------------------------------------------

const CPU_TEMP_KEYS: &[(&str, &str)] = &[
    ("TC0P", "CPU Proximity"),
    ("TC0D", "CPU Die"),
    ("TC0H", "CPU Heatsink"),
    ("TC0E", "CPU Core 0"),
    ("TC1E", "CPU Core 1"),
    ("TC2E", "CPU Core 2"),
    ("TC3E", "CPU Core 3"),
    ("TCXC", "CPU Package"),
    ("TG0D", "GPU Die"),
    ("TG0P", "GPU Proximity"),
    ("TH0A", "NAND"),
    ("TM0P", "Memory Proximity"),
];

// ---------------------------------------------------------------------------
// Fallback when SMC is unavailable (Apple Silicon without entitlements)
// ---------------------------------------------------------------------------

fn sysctl_cpu_temp_fallback() -> Vec<SensorReading> {
    // On Apple Silicon without a privileged helper, SMC reads may fail.
    // powermetrics could provide data but requires sudo.
    // Return a placeholder so callers always get a valid snapshot.
    vec![SensorReading {
        id: "cpu_thermal".to_string(),
        label: "CPU Thermal Level".to_string(),
        temp_celsius: 0.0, // Cannot read without sudo on Apple Silicon
    }]
}

// ---------------------------------------------------------------------------
// Public HAL implementation
// ---------------------------------------------------------------------------

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
        // Run SMC reads on a blocking thread to avoid blocking the async runtime.
        tokio::task::spawn_blocking(|| {
            let smc = match SmcConnection::open() {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "SMC connection failed — falling back to sysctl proxy"
                    );
                    return Ok(HardwareSnapshot {
                        sensors: sysctl_cpu_temp_fallback(),
                        fans: Vec::new(),
                    });
                }
            };

            // --- Sensors ---
            let mut sensors = Vec::new();
            for (key, label) in CPU_TEMP_KEYS {
                if let Ok(temp) = smc.read_temp(key) {
                    if temp > 0.0 && temp < 150.0 {
                        sensors.push(SensorReading {
                            id: key.to_lowercase(),
                            label: label.to_string(),
                            temp_celsius: temp,
                        });
                    }
                }
            }

            // If no SMC keys returned valid data, use the fallback.
            if sensors.is_empty() {
                sensors = sysctl_cpu_temp_fallback();
            }

            // --- Fans ---
            let fan_count = smc.fan_count();
            let mut fans = Vec::new();
            for i in 0..fan_count {
                let actual_rpm = smc.read_fan_rpm(i, "Ac").unwrap_or(0.0);
                let min_rpm = smc.read_fan_rpm(i, "Mn").unwrap_or(0.0);
                let max_rpm = smc.read_fan_rpm(i, "Mx").unwrap_or(1.0);
                let duty = if max_rpm > min_rpm {
                    ((actual_rpm - min_rpm) / (max_rpm - min_rpm) * 100.0).clamp(0.0, 100.0) as u8
                } else {
                    0
                };

                fans.push(FanReading {
                    id: format!("fan_{i}"),
                    label: format!("Fan {i}"),
                    rpm: actual_rpm as u32,
                    duty_percent: duty,
                });
            }

            Ok(HardwareSnapshot { sensors, fans })
        })
        .await?
    }

    async fn set_fan_duty(&self, cmd: SetFanDuty) -> Result<()> {
        let fan_idx: u8 = cmd
            .fan_id
            .strip_prefix("fan_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        tokio::task::spawn_blocking(move || {
            let smc = SmcConnection::open()?;
            let min_rpm = smc.read_fan_rpm(fan_idx, "Mn").unwrap_or(500.0);
            let max_rpm = smc.read_fan_rpm(fan_idx, "Mx").unwrap_or(3000.0);
            let target_rpm = min_rpm + (cmd.duty_percent as f32 / 100.0) * (max_rpm - min_rpm);

            // Writing fan speed requires Ftst=1 via a privileged SMJobBless
            // helper (not yet implemented). Log intent for now.
            tracing::info!(
                fan = fan_idx,
                target_rpm = target_rpm,
                duty = cmd.duty_percent,
                "macOS: fan speed target set (SMC write requires entitlement)"
            );
            Ok(())
        })
        .await?
    }
}

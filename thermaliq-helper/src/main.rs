//! ThermalIQ privileged helper — macOS only.
//!
//! Runs as root via launchd. Opens the AppleSMC IOKit service, writes `Ftst=1`
//! to suppress `thermalmonitord` and take over fan control, then listens on a
//! Unix socket for JSON commands from the daemon to set fan target RPMs.
//!
//! Protocol (newline-delimited JSON):
//!   {"cmd":"set_fan","fan_idx":0,"duty":75}
//!   {"cmd":"reset"}
//!   {"cmd":"ping"}
//!
//! Responses: {"ok":true} or {"ok":false,"error":"..."}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("thermaliq-helper is only supported on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    // Support `--version` so the UI crate can detect staleness by comparing
    // the installed binary version against the one embedded in the app bundle.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    macos::run()
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{anyhow, Context, Result};
    use serde::{Deserialize, Serialize};
    use std::ffi::{c_char, c_void, CString};
    use std::io::{BufRead, BufReader, Write};
    use std::mem;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    const SOCKET_PATH: &str = "/var/run/thermaliq/helper.sock";

    // -----------------------------------------------------------------------
    // IOKit FFI (mirrors thermaliq-hal/src/macos.rs)
    // -----------------------------------------------------------------------

    type IOService = u32;
    type IOConnect = u32;
    type IOReturn = i32;

    const IO_OBJECT_NULL: u32 = 0;
    const K_IO_RETURN_SUCCESS: IOReturn = 0;

    // SMC selector for IOConnectCallStructMethod
    const KERNEL_INDEX_SMC: u32 = 2;
    // data8 values
    const SMC_CMD_READ_KEY: u8 = 5;
    const SMC_CMD_WRITE_KEY: u8 = 6;
    const SMC_CMD_GET_KEY_INFO: u8 = 9;

    // SMC data type four-char codes
    const SMC_TYPE_UI8: u32 = 0x75693820; // 'ui8 '

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
            // Safety: plain-old-data, no invariants.
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

    // -----------------------------------------------------------------------
    // SMC helpers
    // -----------------------------------------------------------------------

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

    struct SmcConnection {
        conn: IOConnect,
    }

    impl SmcConnection {
        fn open() -> Result<Self> {
            unsafe {
                let service_name = CString::new("AppleSMC").unwrap();
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

        fn read_key_info(&self, key: &str) -> Result<SmcKeyDataKeyInfo> {
            let mut input = SmcKeyData::default();
            let mut output = SmcKeyData::default();
            input.key = key_to_u32(key);
            input.data8 = SMC_CMD_GET_KEY_INFO;

            let mut out_size = mem::size_of::<SmcKeyData>();
            unsafe {
                let ret = IOConnectCallStructMethod(
                    self.conn,
                    KERNEL_INDEX_SMC,
                    &input as *const _ as *const c_void,
                    mem::size_of::<SmcKeyData>(),
                    &mut output as *mut _ as *mut c_void,
                    &mut out_size,
                );
                if ret != K_IO_RETURN_SUCCESS {
                    return Err(anyhow!("SMC kSMCGetKeyInfo failed for key '{key}': {ret}"));
                }
            }
            Ok(output.key_info)
        }

        fn read_key(&self, key: &str) -> Result<(SmcKeyDataKeyInfo, [u8; 32])> {
            let key_info = self.read_key_info(key)?;

            let mut input = SmcKeyData::default();
            let mut output = SmcKeyData::default();
            input.key = key_to_u32(key);
            input.key_info = key_info;
            input.data8 = SMC_CMD_READ_KEY;

            let mut out_size = mem::size_of::<SmcKeyData>();
            unsafe {
                let ret = IOConnectCallStructMethod(
                    self.conn,
                    KERNEL_INDEX_SMC,
                    &input as *const _ as *const c_void,
                    mem::size_of::<SmcKeyData>(),
                    &mut output as *mut _ as *mut c_void,
                    &mut out_size,
                );
                if ret != K_IO_RETURN_SUCCESS {
                    return Err(anyhow!("SMC kSMCReadKey failed for key '{key}': {ret}"));
                }
            }
            Ok((key_info, output.bytes))
        }

        /// Read a fan value in fpe2 (unsigned fixed-point 14.2) format.
        fn read_fan_rpm_fpe2(&self, fan_idx: u8, suffix: &str) -> Result<f32> {
            let key = format!("F{fan_idx}{suffix}");
            let (_, bytes) = self.read_key(&key)?;
            let raw = ((bytes[0] as u16) << 8) | (bytes[1] as u16);
            Ok(raw as f32 / 4.0)
        }

        /// Write an SMC key with raw bytes. Uses the key's existing type/size.
        fn write_key(&self, key: &str, data: &[u8], data_size: u32, data_type: u32) -> Result<()> {
            let mut input = SmcKeyData::default();
            let mut output = SmcKeyData::default();
            input.key = key_to_u32(key);
            input.data8 = SMC_CMD_WRITE_KEY;
            input.key_info = SmcKeyDataKeyInfo {
                data_size,
                data_type,
                data_attributes: 0,
            };
            let copy_len = data.len().min(32);
            input.bytes[..copy_len].copy_from_slice(&data[..copy_len]);

            let mut out_size = mem::size_of::<SmcKeyData>();
            unsafe {
                let ret = IOConnectCallStructMethod(
                    self.conn,
                    KERNEL_INDEX_SMC,
                    &input as *const _ as *const c_void,
                    mem::size_of::<SmcKeyData>(),
                    &mut output as *mut _ as *mut c_void,
                    &mut out_size,
                );
                if ret != K_IO_RETURN_SUCCESS {
                    return Err(anyhow!("SMC kSMCWriteKey failed for key '{key}': {ret}"));
                }
            }
            Ok(())
        }
    }

    impl Drop for SmcConnection {
        fn drop(&mut self) {
            unsafe {
                IOServiceClose(self.conn);
            }
        }
    }

    fn smc_set_ftst(conn: &SmcConnection, enabled: bool) -> Result<()> {
        let value: [u8; 1] = [if enabled { 1 } else { 0 }];
        conn.write_key("Ftst", &value, 1, SMC_TYPE_UI8)
            .with_context(|| format!("setting Ftst={}", enabled as u8))
    }

    /// Detect Apple Silicon by shelling out to sysctl -n hw.model and checking
    /// for "Apple" / "Mac" ARM identifiers. Intel returns strings like "MacBookPro16,1".
    fn detect_apple_silicon() -> bool {
        use std::process::Command;
        let out = Command::new("sysctl").args(["-n", "hw.model"]).output();
        if let Ok(out) = out {
            let s = String::from_utf8_lossy(&out.stdout);
            // Apple Silicon hw.model strings look like "Mac14,3", "Mac15,6", etc.
            // (The "MacBookPro"/"iMac" prefix means Intel.)
            let trimmed = s.trim();
            return trimmed.starts_with("Mac")
                && !trimmed.starts_with("MacBook")
                && !trimmed.starts_with("Macmini")
                && !trimmed.starts_with("MacPro")
                && !trimmed.starts_with("iMac");
        }
        // Fallback: check CPU brand string
        let brand = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output();
        if let Ok(out) = brand {
            let s = String::from_utf8_lossy(&out.stdout);
            return s.contains("Apple");
        }
        false
    }

    fn encode_fan_rpm_intel(rpm: f32) -> [u8; 2] {
        // fpe2: rpm * 4, big-endian u16
        ((rpm.max(0.0) * 4.0) as u16).to_be_bytes()
    }

    fn encode_fan_rpm_apple_silicon(rpm: f32) -> [u8; 4] {
        // IEEE 754 float, little-endian
        rpm.to_le_bytes()
    }

    fn set_fan_target(
        conn: &SmcConnection,
        fan_idx: u8,
        duty_percent: u8,
        is_apple_silicon: bool,
    ) -> Result<()> {
        let duty = duty_percent.min(100) as f32 / 100.0;
        let min_rpm = conn.read_fan_rpm_fpe2(fan_idx, "Mn").unwrap_or(500.0);
        let max_rpm = conn.read_fan_rpm_fpe2(fan_idx, "Mx").unwrap_or(3000.0);
        let target_rpm = min_rpm + duty * (max_rpm - min_rpm);

        let key = format!("F{fan_idx}Tg");
        if is_apple_silicon {
            let bytes = encode_fan_rpm_apple_silicon(target_rpm);
            // type "flt " = 0x666c7420
            conn.write_key(&key, &bytes, 4, 0x666c7420)?;
        } else {
            let bytes = encode_fan_rpm_intel(target_rpm);
            // type "fpe2" = 0x66706532
            conn.write_key(&key, &bytes, 2, 0x66706532)?;
        }

        tracing::info!(
            fan = fan_idx,
            duty = duty_percent,
            target_rpm = target_rpm,
            apple_silicon = is_apple_silicon,
            "fan target written"
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Socket protocol
    // -----------------------------------------------------------------------

    #[derive(Debug, Deserialize)]
    #[serde(tag = "cmd", rename_all = "snake_case")]
    enum Command {
        SetFan { fan_idx: u8, duty: u8 },
        Reset,
        Ping,
    }

    #[derive(Debug, Serialize)]
    struct Response {
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    }

    impl Response {
        fn ok() -> Self {
            Self {
                ok: true,
                error: None,
            }
        }
        fn err(msg: impl Into<String>) -> Self {
            Self {
                ok: false,
                error: Some(msg.into()),
            }
        }
    }

    struct HelperState {
        smc: SmcConnection,
        is_apple_silicon: bool,
    }

    fn handle_command(state: &HelperState, cmd: Command) -> Response {
        match cmd {
            Command::Ping => Response::ok(),
            Command::Reset => match smc_set_ftst(&state.smc, false) {
                Ok(()) => Response::ok(),
                Err(e) => Response::err(e.to_string()),
            },
            Command::SetFan { fan_idx, duty } => {
                // Ensure Ftst=1 is still asserted (cheap, idempotent).
                if let Err(e) = smc_set_ftst(&state.smc, true) {
                    return Response::err(format!("failed to assert Ftst=1: {e}"));
                }
                match set_fan_target(&state.smc, fan_idx, duty, state.is_apple_silicon) {
                    Ok(()) => Response::ok(),
                    Err(e) => Response::err(e.to_string()),
                }
            }
        }
    }

    fn handle_client(stream: UnixStream, state: Arc<Mutex<HelperState>>) {
        let peer = stream.peer_addr().ok();
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "failed to clone client stream");
                return;
            }
        };
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    tracing::debug!(error = %e, ?peer, "client read error");
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Command>(&line) {
                Ok(cmd) => {
                    let guard = match state.lock() {
                        Ok(g) => g,
                        Err(e) => {
                            tracing::error!("state mutex poisoned: {e}");
                            return;
                        }
                    };
                    handle_command(&guard, cmd)
                }
                Err(e) => Response::err(format!("invalid command: {e}")),
            };
            let encoded = serde_json::to_string(&response)
                .unwrap_or_else(|_| r#"{"ok":false,"error":"serialization failed"}"#.to_string());
            if writeln!(writer, "{encoded}").is_err() {
                break;
            }
        }
    }

    fn serve(state: Arc<Mutex<HelperState>>) -> Result<()> {
        let socket_path = Path::new(SOCKET_PATH);
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating socket dir {}", parent.display()))?;
        }
        // Remove stale socket
        let _ = std::fs::remove_file(socket_path);

        let listener = UnixListener::bind(socket_path)
            .with_context(|| format!("binding {}", socket_path.display()))?;

        // Permissions: allow local users to connect (helper validates commands).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o666);
            let _ = std::fs::set_permissions(socket_path, perms);
        }

        tracing::info!(path = %socket_path.display(), "listening on Unix socket");

        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let state = Arc::clone(&state);
                    thread::spawn(move || handle_client(stream, state));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                }
            }
        }
        Ok(())
    }

    /// Poll uptime-delta as a crude wake detector.
    ///
    /// TODO(phase-3): replace with IORegisterForSystemPower / IOPMSleepWakeInterest
    /// notifications from IOKit/pwr_mgt/IOPMLib.h for precise, event-driven wake
    /// detection. The current polling approach tolerates sleep but may take up
    /// to `CHECK_INTERVAL` to re-assert Ftst after wake.
    fn sleep_wake_thread(state: Arc<Mutex<HelperState>>) {
        const CHECK_INTERVAL: Duration = Duration::from_secs(5);
        let mut last = Instant::now();
        loop {
            thread::sleep(CHECK_INTERVAL);
            let now = Instant::now();
            let elapsed = now.duration_since(last);
            // If the monotonic clock jumped by >2x the interval, the machine
            // likely slept. Re-assert Ftst=1.
            if elapsed > CHECK_INTERVAL * 2 {
                tracing::info!(
                    elapsed_secs = elapsed.as_secs(),
                    "detected likely wake — re-asserting Ftst=1"
                );
                if let Ok(guard) = state.lock() {
                    if let Err(e) = smc_set_ftst(&guard.smc, true) {
                        tracing::warn!(error = %e, "failed to re-assert Ftst after wake");
                    }
                }
            }
            last = now;
        }
    }

    pub fn run() -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();

        tracing::info!("thermaliq-helper starting");

        let smc = SmcConnection::open().context("opening AppleSMC")?;
        smc_set_ftst(&smc, true).context("asserting Ftst=1 at startup")?;
        let is_apple_silicon = detect_apple_silicon();
        tracing::info!(apple_silicon = is_apple_silicon, "platform detected");

        let state = Arc::new(Mutex::new(HelperState {
            smc,
            is_apple_silicon,
        }));

        // Spawn sleep/wake watcher.
        {
            let state = Arc::clone(&state);
            thread::spawn(move || sleep_wake_thread(state));
        }

        serve(state)
    }
}

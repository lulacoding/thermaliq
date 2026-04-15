//! Windows Service Control Manager integration.
//!
//! The daemon can run in two modes:
//!   1. Interactive (default): launched from a terminal or by the Tauri UI.
//!   2. Service mode (`--service` flag): registered with SCM, starts on login.
//!
//! ## Registration
//!
//! Run once as Administrator to register:
//!
//! ```powershell
//! thermaliq-daemon.exe install-service
//! ```
//!
//! Then start / stop with:
//!
//! ```powershell
//! Start-Service ThermalIQ
//! Stop-Service ThermalIQ
//! ```
//!
//! Or let it start automatically at login (AutoStart is set at install time).

#![cfg(target_os = "windows")]

use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SERVICE_NAME: &str = "ThermalIQ";
const SERVICE_DISPLAY_NAME: &str = "ThermalIQ Fan Control Daemon";
const SERVICE_DESCRIPTION: &str =
    "AI-powered fan control daemon. Reads hardware sensors and adjusts fan speeds.";

// Macro registers `ffi_service_main` as the SCM entry point.
define_windows_service!(ffi_service_main, service_main);

/// Called by SCM when the service starts. Bridges to async Tokio runtime.
fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(error = %e, "Windows service fatal error");
    }
}

fn run_service() -> Result<()> {
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let status_handle = service_control_handler::register(SERVICE_NAME, move |ctrl| match ctrl {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = shutdown_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    tracing::info!("ThermalIQ service started");

    // Spin up the Tokio runtime and run the daemon.
    // We pass a shutdown channel that the SCM stop handler fires.
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // Launch the daemon logic in a separate task so we can wait for the
        // SCM shutdown signal on this thread.
        let daemon = tokio::spawn(crate::run_daemon_inner());

        // Wait for SCM stop
        tokio::task::spawn_blocking(move || {
            let _ = shutdown_rx.recv();
        })
        .await
        .ok();

        daemon.abort();
    });

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    Ok(())
}

/// Dispatch to SCM. Call this from `main()` when `--service` flag is set.
pub fn start_as_service() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| anyhow::anyhow!("service_dispatcher::start failed: {e}"))
}

/// Register the daemon as an auto-start Windows service.
/// Must be run as Administrator.
pub fn install_service() -> Result<()> {
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;

    let exe_path = std::env::current_exe()?;

    let service_info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe_path,
        launch_arguments: vec![OsString::from("--service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };

    let service = manager.create_service(&service_info, ServiceAccess::CHANGE_CONFIG)?;
    service.set_description(SERVICE_DESCRIPTION)?;

    println!("ThermalIQ service installed. Start with: Start-Service ThermalIQ");
    Ok(())
}

/// Unregister the service. Must be run as Administrator.
pub fn uninstall_service() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::DELETE)?;
    service.delete()?;
    println!("ThermalIQ service uninstalled.");
    Ok(())
}

//! Windows service: install, uninstall and the SCM entry point (SPEC S5, PORTING M9).
//!
//! Stop and Shutdown go through `App::stop`, which drops the engine so fans are handed back.
//! Nothing here calls `process::exit` or aborts a thread.

use crate::app::{self, Token};
use crate::{acl, store};
use std::ffi::OsString;
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

pub const NAME: &str = "ThermalIQ";
const TYPE: ServiceType = ServiceType::OWN_PROCESS;
/// What we tell the SCM a stop may take, and how long we give the engine inside it
/// (the API shutdown is not waited for).
const WAIT_HINT: Duration = Duration::from_secs(10);
pub const STOP_BUDGET: Duration = Duration::from_secs(8);

/// Locks the data dir, then registers the service (LocalSystem, automatic start).
pub fn install() -> Result<(), String> {
    app::hardware()?; // don't install a service that can only refuse to start
    let dir = store::data_dir().map_err(|e| e.to_string())?;
    // Opening the SCM first fails fast without elevation, before the dir is touched.
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)
            .map_err(|e| format!("cannot open the service manager (run as Administrator): {e}"))?;
    let sid = acl::installer_sid().map_err(|e| e.to_string())?;
    acl::lock_data_dir(&dir, &sid).map_err(|e| format!("cannot lock {}: {e}", dir.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let info = ServiceInfo {
        name: NAME.into(),
        display_name: "ThermalIQ fan control".into(),
        service_type: TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec!["--service".into()],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG)
        .map_err(|e| format!("cannot create the service: {e}"))?;
    service
        .set_description(
            "Reads temperatures and drives fans from curves. Hands fans back to firmware on stop.",
        )
        .map_err(|e| e.to_string())
}

/// Stops the service if it runs (waits for it), then deletes it. Leaves the data dir alone.
pub fn uninstall() -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("cannot open the service manager: {e}"))?;
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    let service = manager
        .open_service(NAME, access)
        .map_err(|e| format!("cannot open service {NAME}: {e}"))?;
    let state = |s: &windows_service::service::Service| {
        s.query_status()
            .map(|st| st.current_state)
            .map_err(|e| e.to_string())
    };
    if state(&service)? != ServiceState::Stopped {
        // already stopping is fine; any other error shows up when the wait times out
        let _ = service.stop();
        let end = Instant::now() + WAIT_HINT + Duration::from_secs(5);
        while state(&service)? != ServiceState::Stopped {
            if Instant::now() > end {
                return Err("service did not stop in time; not deleted".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    service.delete().map_err(|e| e.to_string())
}

/// Called with `--service`: hands the thread to the SCM dispatcher, which runs `service_main`.
pub fn run() -> Result<(), String> {
    ffi::dispatch().map_err(|e| format!("not started by the service manager: {e}"))
}

// The macro generates the `extern "system"` entry point, which needs `unsafe`.
#[allow(unsafe_code)]
mod ffi {
    use super::{service_main, NAME};
    use windows_service::{define_windows_service, service_dispatcher};

    define_windows_service!(ffi_service_main, service_main);

    pub fn dispatch() -> windows_service::Result<()> {
        service_dispatcher::start(NAME, ffi_service_main)
    }
}

/// Failures can't reach a console, so they go to a file in the (locked) data dir.
fn log(msg: &str) {
    let Ok(dir) = store::data_dir() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("service.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        log(&e);
    }
}

fn run_service() -> Result<(), String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handler = move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status = service_control_handler::register(NAME, handler).map_err(|e| e.to_string())?;
    let report = |state, accepted, exit, checkpoint, wait_hint| {
        status
            .set_service_status(ServiceStatus {
                service_type: TYPE,
                current_state: state,
                controls_accepted: accepted,
                exit_code: exit,
                checkpoint,
                wait_hint,
                process_id: None,
            })
            .map_err(|e| e.to_string())
    };
    let none = ServiceControlAccept::empty();
    let ok = ServiceExitCode::Win32(0);

    report(ServiceState::StartPending, none, ok, 1, WAIT_HINT)?;
    let started = store::data_dir()
        .map_err(|e| e.to_string())
        .and_then(|dir| Ok((dir, app::hardware()?)))
        .and_then(|(dir, hal)| app::start(&dir, hal, Token::MustExist));
    let running = match started {
        Ok(a) => a,
        Err(e) => {
            report(
                ServiceState::Stopped,
                none,
                ServiceExitCode::ServiceSpecific(1),
                0,
                Duration::ZERO,
            )?;
            return Err(format!("startup failed: {e}"));
        }
    };
    report(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ok,
        0,
        Duration::ZERO,
    )?;

    let _ = stop_rx.recv(); // a dropped handler also means stop
    report(ServiceState::StopPending, none, ok, 1, WAIT_HINT)?;
    let stopped = running.stop(STOP_BUDGET);
    let exit = if stopped.is_ok() {
        ok
    } else {
        ServiceExitCode::ServiceSpecific(2)
    };
    report(ServiceState::Stopped, none, exit, 0, Duration::ZERO)?;
    stopped
}

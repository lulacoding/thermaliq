//! Daemon entry point. See SPEC.md §5.
//!
//! Never call `std::process::exit` while the engine is alive: it skips Drop, which is what
//! hands the fans back (S5).

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;
use thermaliq_daemon::app::{self, Token};
use thermaliq_daemon::store;

const USAGE: &str =
    "usage: thermaliq-daemon [install-service [--force] | uninstall-service | identify]";
/// Same budget as the service stop.
const STOP_BUDGET: Duration = Duration::from_secs(8);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => interactive(),
        // Talks to the running service over its API; touches no hardware itself.
        ["identify"] => thermaliq_daemon::identify_cli::run_interactive(),
        // Internal: started by the daemon in the user's session. Touches no hardware.
        [thermaliq_daemon::foreground::HELPER_ARG] => thermaliq_daemon::foreground::run_helper(),
        #[cfg(windows)]
        ["--service"] => thermaliq_daemon::service::run(),
        #[cfg(windows)]
        ["install-service"] | ["install-service", "--force"] => {
            thermaliq_daemon::service::install(args.len() == 2)
                .map(|()| eprintln!("service installed; start it with: sc start ThermalIQ"))
        }
        #[cfg(windows)]
        ["uninstall-service"] => thermaliq_daemon::service::uninstall()
            .map(|()| eprintln!("service removed (the data directory was left in place)")),
        #[cfg(not(windows))]
        ["--service" | "install-service" | "uninstall-service"]
        | ["install-service", "--force"] => {
            Err("the Windows service is only available on Windows".into())
        }
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("thermaliq-daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs in the foreground; Ctrl-C (or SIGTERM) stops it cleanly.
fn interactive() -> Result<(), String> {
    let hal = app::hardware()?;
    let dir = store::data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let running = app::start(
        &dir,
        hal,
        Token::CreateIfMissing,
        app::ForegroundSource::SameSession,
    )?;
    let (stop, stopped) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop.send(());
    })
    .map_err(|e| format!("cannot install the Ctrl-C handler: {e}"))?;
    eprintln!(
        "thermaliq-daemon {} listening on http://{} (Ctrl-C to stop)",
        env!("CARGO_PKG_VERSION"),
        running.api_addr()
    );
    let _ = stopped.recv();
    eprintln!("stopping, handing fans back...");
    running.stop(STOP_BUDGET)
}

//! Startup and shutdown shared by interactive mode and the Windows service.

use crate::api::{ApiServer, ApiState};
use crate::history::HistoryDb;
use crate::runtime::{load_profiles, Daemon, Options};
use crate::store;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use thermaliq_hal::HardwareLayer;

pub struct App {
    daemon: Daemon,
    api: ApiServer,
}

/// Whether startup may create `token.txt`. The installer creates it with its ACL; a service
/// that made a new one would get the directory's inherited (Users-readable) ACL.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Token {
    CreateIfMissing,
    MustExist,
}

/// The hardware backend of this build. Fake hardware only exists behind `mock-hardware`.
pub fn hardware() -> Result<Arc<dyn HardwareLayer>, String> {
    #[cfg(feature = "mock-hardware")]
    {
        use thermaliq_common::SensorKind;
        use thermaliq_hal::{MockHardwareLayer, Tick};
        eprintln!(
            "WARNING: mock-hardware build: fake sensors and fans, no real hardware is controlled"
        );
        let cpu = Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 45.0)]);
        Ok(Arc::new(MockHardwareLayer::new(&["mock-fan"], vec![cpu])))
    }
    #[cfg(all(windows, not(feature = "mock-hardware")))]
    {
        use thermaliq_hal::{CombinedHardwareLayer, Nvidia};
        // M11/M12 add their backends here; ones that fail to start are skipped, and with
        // none left the daemon refuses to start rather than pretend.
        let mut backends: Vec<Arc<dyn HardwareLayer>> = Vec::new();
        let mut why = Vec::new();
        match Nvidia::new() {
            Ok(n) => backends.push(Arc::new(n)),
            Err(e) => why.push(format!("NVIDIA: {e}")),
        }
        if backends.is_empty() {
            return Err(format!("no hardware backend started ({})", why.join("; ")));
        }
        for w in why {
            eprintln!("hardware backend unavailable, continuing without it: {w}");
        }
        let combined = CombinedHardwareLayer::new(backends).map_err(|e| e.to_string())?;
        Ok(Arc::new(combined))
    }
    #[cfg(all(not(windows), not(feature = "mock-hardware")))]
    Err("no hardware backend on this OS (v2.0 controls fans on Windows only)".into())
}

pub fn start(dir: &Path, hal: Arc<dyn HardwareLayer>, token: Token) -> Result<App, String> {
    let cfg = store::load_config(dir).map_err(|e| format!("config.toml: {e}"))?;
    let token = match token {
        Token::CreateIfMissing => store::load_or_create_token(dir),
        Token::MustExist => store::load_token(dir).and_then(|t| {
            t.ok_or_else(|| {
                std::io::Error::other("missing or malformed; run install-service again")
            })
        }),
    }
    .map_err(|e| format!("token.txt: {e}"))?;
    let (profiles, skipped) = load_profiles(dir);
    for s in skipped {
        eprintln!("skipped profile: {s}");
    }
    let active = profiles
        .iter()
        .find(|p| p.name == cfg.default_profile)
        .ok_or_else(|| {
            format!(
                "default_profile '{}' is not a known profile",
                cfg.default_profile
            )
        })?
        .clone();
    let history =
        HistoryDb::open(&dir.join("history.db")).map_err(|e| format!("history.db: {e}"))?;
    let history = Arc::new(Mutex::new(history));
    let opts = Options {
        tick: Duration::from_millis(cfg.polling_interval_ms),
        prune_every: Duration::from_secs(3600),
    };
    let daemon = Daemon::start(hal, &active, profiles.clone(), history.clone(), opts);
    let state = ApiState::new(daemon.handle(), profiles, history, token, dir.to_owned());
    // On error `daemon` is dropped here, which stops the engine and hands the fans back.
    let api = ApiServer::start(state, cfg.api_port)
        .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", cfg.api_port))?;
    Ok(App { daemon, api })
}

impl App {
    pub fn api_addr(&self) -> SocketAddr {
        self.api.local_addr()
    }

    /// Hands the fans back (the engine is dropped, S5), then stops the API. Errors if the
    /// engine has not stopped within `budget` (a stuck HAL call); the caller then ends the process.
    pub fn stop(self, budget: Duration) -> Result<(), String> {
        let Self { daemon, api } = self;
        let (done, stopped) = mpsc::channel();
        // Off-thread so a stuck HAL call can't hold us past `budget`. The API is stopped after
        // the fans are back, and is not waited for: it holds nothing that needs flushing.
        std::thread::spawn(move || {
            let _ = done.send(daemon.shutdown());
            api.shutdown();
        });
        stopped.recv_timeout(budget).map_err(|_| {
            "engine did not stop in time; fans may not have been handed back".to_owned()
        })?
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    use thermaliq_common::{HardwareSnapshot, SensorKind};
    use thermaliq_hal::{Capabilities, HalError, MockHardwareLayer, Tick};

    const FLAT: &str = "name = \"flat\"\n[[curves]]\ntype = \"flat\"\nid = \"c\"\npwm = 30\n[[controls]]\nfan = \"f1\"\ncurve = \"c\"\nstep_up_pct = 100\nstep_down_pct = 100\n";

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A data dir with a `flat` profile (fan f1) as the default and a free API port.
    fn dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("profiles")).unwrap();
        fs::write(d.path().join("profiles").join("flat.toml"), FLAT).unwrap();
        fs::write(
            d.path().join("config.toml"),
            format!(
                "api_port = {}\npolling_interval_ms = 250\nlog_level = \"info\"\ndefault_profile = \"flat\"\n",
                free_port()
            ),
        )
        .unwrap();
        d
    }

    fn mock() -> Arc<MockHardwareLayer> {
        let cpu = Tick::Temps(vec![("cpu".into(), SensorKind::Cpu, 40.0)]);
        Arc::new(MockHardwareLayer::new(&["f1"], vec![cpu]))
    }

    fn wait_until(mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn start_serves_the_api_and_stop_hands_fans_back_then_closes_it() {
        let d = dir();
        let hal = mock();
        let app = start(d.path(), hal.clone(), Token::CreateIfMissing).unwrap();
        let addr = app.api_addr();
        assert_eq!(addr.port(), load_port(d.path()));
        assert!(TcpStream::connect(addr).is_ok());
        assert_eq!(
            fs::read_to_string(d.path().join("token.txt"))
                .unwrap()
                .len(),
            64
        );
        wait_until(|| !hal.writes().is_empty());
        assert!(hal.restores().is_empty());
        app.stop(Duration::from_secs(5)).unwrap();
        assert_eq!(hal.restores(), ["f1"]);
        wait_until(|| TcpStream::connect(addr).is_err());
    }

    fn load_port(dir: &Path) -> u16 {
        store::load_config(dir).unwrap().api_port
    }

    #[test]
    fn unknown_default_profile_refuses_to_start_and_touches_no_fan() {
        let d = dir();
        fs::write(
            d.path().join("config.toml"),
            "api_port = 7300\ndefault_profile = \"nope\"\n",
        )
        .unwrap();
        let hal = mock();
        let err = start(d.path(), hal.clone(), Token::CreateIfMissing)
            .err()
            .unwrap();
        assert!(err.contains("nope"), "{err}");
        assert!(hal.writes().is_empty() && hal.restores().is_empty());
    }

    #[test]
    fn a_service_without_its_token_refuses_to_make_one() {
        let d = dir();
        let err = start(d.path(), mock(), Token::MustExist).err().unwrap();
        assert!(err.contains("token"), "{err}");
        assert!(!d.path().join("token.txt").exists());
    }

    #[test]
    fn api_port_in_use_refuses_to_start_and_leaves_fans_alone() {
        let d = dir();
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = busy.local_addr().unwrap().port();
        fs::write(
            d.path().join("config.toml"),
            format!("api_port = {port}\ndefault_profile = \"flat\"\n"),
        )
        .unwrap();
        let hal = mock();
        let err = start(d.path(), hal.clone(), Token::CreateIfMissing)
            .err()
            .unwrap();
        assert!(err.contains(&port.to_string()), "{err}");
        // an engine that already started is dropped (and joined) before start returns, which
        // hands back whatever it touched
        assert_eq!(hal.writes().is_empty(), hal.restores().is_empty());
    }

    /// A HAL whose `snapshot()` hangs until released, like a wedged driver call.
    struct Stuck(AtomicBool);
    impl HardwareLayer for Stuck {
        fn snapshot(&self) -> Result<HardwareSnapshot, HalError> {
            while !self.0.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(HalError("released".into()))
        }
        fn set_fan_duty(&self, _: &str, _: u8) -> Result<(), HalError> {
            Ok(())
        }
        fn restore_auto(&self, _: &str) -> Result<(), HalError> {
            Ok(())
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
    }

    #[test]
    fn stop_gives_up_inside_the_budget_when_a_hal_call_is_stuck() {
        let d = dir();
        let hal = Arc::new(Stuck(AtomicBool::new(false)));
        let app = start(d.path(), hal.clone(), Token::CreateIfMissing).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let t0 = Instant::now();
        let err = app.stop(Duration::from_millis(300)).unwrap_err();
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        assert!(err.contains("fans"), "{err}");
        hal.0.store(true, Ordering::SeqCst); // let the engine thread finish
    }

    #[cfg(all(not(windows), not(feature = "mock-hardware")))]
    #[test]
    fn a_non_windows_build_has_no_backend_and_refuses() {
        let err = hardware().err().unwrap();
        assert!(err.contains("no hardware backend"), "{err}");
    }

    /// Windows without a usable NVIDIA GPU (CI) refuses and names the cause; with one, it starts.
    #[cfg(all(windows, not(feature = "mock-hardware")))]
    #[test]
    fn a_windows_build_starts_nvidia_or_refuses_naming_it() {
        if let Err(e) = hardware() {
            assert!(
                e.contains("no hardware backend") && e.contains("NVIDIA"),
                "{e}"
            );
        }
    }

    #[cfg(feature = "mock-hardware")]
    #[test]
    fn mock_hardware_is_only_there_when_asked_for() {
        assert!(hardware().is_ok());
    }
}

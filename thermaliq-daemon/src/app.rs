//! Startup and shutdown shared by interactive mode and the Windows service.

use crate::api::{ApiServer, ApiState};
use crate::decision_client::{HostedProvider, LocalConfig, LocalProvider};
use crate::foreground::Feed;
pub use crate::foreground::ForegroundSource;
use crate::history::HistoryDb;
use crate::keychain::{KeyStore, OsKeychain};
use crate::runtime::{load_profiles, Daemon, Options};
use crate::slow::{SlowParts, SlowShared, SlowThread};
use crate::store;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use thermaliq_agent::decision::DecisionProvider;
use thermaliq_hal::HardwareLayer;

pub struct App {
    daemon: Daemon,
    api: ApiServer,
    slow: SlowThread,
}

/// Whether startup may create `token.txt`. The installer creates it with its ACL; a service
/// that made a new one would get the directory's inherited (Users-readable) ACL.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Token {
    CreateIfMissing,
    MustExist,
}

/// The hardware backend of this build, wrapped in fault injection under `gate-tests`.
pub fn hardware() -> Result<Arc<dyn HardwareLayer>, String> {
    let hal = backend()?;
    #[cfg(feature = "gate-tests")]
    let hal: Arc<dyn HardwareLayer> = Arc::new(crate::inject::InjectCpuTemp::new(hal));
    Ok(hal)
}

/// Whether this machine could run at all, for `install-service`, without starting anything: the
/// bridge's pipe only opens for SYSTEM, and a started bridge outlives the check.
pub fn installable() -> Result<(), String> {
    #[cfg(all(windows, not(feature = "mock-hardware")))]
    {
        if thermaliq_hal::Nvidia::new().is_ok() || crate::lhm_bridge::installed() {
            Ok(())
        } else {
            Err(
                "no hardware backend: no NVIDIA GPU, and lhm-bridge\\lhm-bridge.exe is not \
                 installed beside the daemon"
                    .into(),
            )
        }
    }
    #[cfg(not(all(windows, not(feature = "mock-hardware"))))]
    hardware().map(drop)
}

/// Fake hardware only exists behind `mock-hardware`.
fn backend() -> Result<Arc<dyn HardwareLayer>, String> {
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
        use crate::lhm_bridge::{start_backend, Backend};
        use thermaliq_hal::{CombinedHardwareLayer, CpuLoad, Hwinfo, Nvidia};
        // Ones that fail to start are skipped, and with none left the daemon refuses to start
        // rather than pretend. Every skipped one is recorded, so /status never shows a healthy
        // engine that is quietly missing its fans (S8).
        let mut backends: Vec<Arc<dyn HardwareLayer>> = Vec::new();
        let mut unstarted: Vec<(&str, Option<String>)> = Vec::new();
        match Nvidia::new() {
            Ok(n) => backends.push(Arc::new(n)),
            Err(e) => {
                eprintln!("hardware backend unavailable, continuing without it: NVIDIA: {e}");
                unstarted.push(("nvidia", Some(e.to_string())));
            }
        }
        let dir = store::data_dir().map_err(|e| e.to_string())?;
        let mut bridge: Option<Arc<dyn HardwareLayer>> = None;
        match start_backend(&dir) {
            Backend::Running(b) => bridge = Some(b),
            Backend::Off(_) => unstarted.push(("lhm-bridge", None)),
            Backend::Failed(why) => unstarted.push(("lhm-bridge", Some(why))),
        }
        if backends.is_empty() {
            // With nothing else it's the only hardware, so losing it has to reach S4.
            match bridge.take() {
                Some(b) => backends.push(b),
                None => {
                    let why: Vec<String> = unstarted
                        .iter()
                        .map(|(n, w)| format!("{n}: {}", w.as_deref().unwrap_or("off")))
                        .collect();
                    return Err(format!("no hardware backend started ({})", why.join("; ")));
                }
            }
        }
        // Optional extras: while one is down only its own readings drop out. CPU load goes first
        // so it wins over HWiNFO's and the bridge's.
        let mut combined = CombinedHardwareLayer::new(backends)
            .and_then(|c| c.with_optional("cpu-load", Arc::new(CpuLoad::new())))
            .and_then(|c| c.with_optional("hwinfo", Arc::new(Hwinfo::new())))
            .map_err(|e| e.to_string())?;
        if let Some(b) = bridge {
            combined = combined
                .with_optional_owner("lhm-bridge", b)
                .map_err(|e| e.to_string())?;
        }
        for (name, fault) in unstarted {
            combined = combined.with_unstarted(name, fault);
        }
        Ok(Arc::new(combined))
    }
    #[cfg(all(not(windows), not(feature = "mock-hardware")))]
    Err("no hardware backend on this OS (v2.0 controls fans on Windows only)".into())
}

/// The local model, if `local-model.toml` names a usable one, and a line saying which it is.
/// A bad file costs the local route, not the daemon: fans come first.
fn local_model(dir: &Path, service: bool) -> (Option<LocalProvider>, String) {
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let checked = LocalConfig::load(dir).and_then(|c| {
        if let Some(c) = &c {
            c.verify(service, program_files.as_deref())?;
        }
        Ok(c)
    });
    match checked {
        Ok(Some(c)) => {
            let provider = LocalProvider::new(c, dir.to_owned());
            let provider = match (service, program_files) {
                (true, Some(root)) => provider.for_service(root),
                _ => provider,
            };
            (Some(provider), "configured".into())
        }
        Ok(None) => (None, "not configured".into()),
        Err(e) => {
            eprintln!("local model unavailable: {e}");
            (None, format!("unusable: {e}"))
        }
    }
}

pub fn start(
    dir: &Path,
    hal: Arc<dyn HardwareLayer>,
    token: Token,
    foreground: ForegroundSource,
) -> Result<App, String> {
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
    let keys: Arc<dyn KeyStore> = Arc::new(OsKeychain::default());
    let (local, local_state) = local_model(dir, foreground == ForegroundSource::ActiveUserSession);
    let shared = SlowShared::new(keys.clone(), local_state);
    let feed = Feed::from_source(foreground);
    let slow = SlowThread::start(SlowParts {
        handle: daemon.handle(),
        profiles: profiles.clone(),
        history: history.clone(),
        foreground: Arc::new(move || feed.latest()),
        hosted: Some(Box::new(
            HostedProvider::new(keys).reporting_to(shared.hosted_error()),
        )),
        local: local.map(|l| Box::new(l) as Box<dyn DecisionProvider + Send>),
        shared: shared.clone(),
        period: SlowParts::DEFAULT_PERIOD,
        stable_for: SlowParts::DEFAULT_STABLE,
    });
    let state = ApiState::new(
        daemon.handle(),
        profiles,
        history,
        token,
        dir.to_owned(),
        shared,
    );
    // On error `slow` and then `daemon` are dropped here. The last stops the engine and hands
    // the fans back.
    let api = ApiServer::start(state, cfg.api_port)
        .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", cfg.api_port))?;
    Ok(App { daemon, api, slow })
}

impl App {
    pub fn api_addr(&self) -> SocketAddr {
        self.api.local_addr()
    }

    /// Hands the fans back (the engine is dropped, S5), then stops the API. Errors if the
    /// engine has not stopped within `budget` (a stuck HAL call); the caller then ends the process.
    pub fn stop(self, budget: Duration) -> Result<(), String> {
        let Self { daemon, api, slow } = self;
        // At once, so a local model unloads whatever else is going on. If the budget runs out
        // the process ends, and the job object takes the model with it.
        slow.request_stop();
        let (done, stopped) = mpsc::channel();
        // Off-thread so a stuck HAL call can't hold us past `budget`. The API is stopped after
        // the fans are back, and is not waited for: it holds nothing that needs flushing.
        std::thread::spawn(move || {
            let _ = done.send(daemon.shutdown());
            slow.stop();
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
        let app = start(
            d.path(),
            hal.clone(),
            Token::CreateIfMissing,
            ForegroundSource::Off,
        )
        .unwrap();
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
        let err = start(
            d.path(),
            hal.clone(),
            Token::CreateIfMissing,
            ForegroundSource::Off,
        )
        .err()
        .unwrap();
        assert!(err.contains("nope"), "{err}");
        assert!(hal.writes().is_empty() && hal.restores().is_empty());
    }

    #[test]
    fn a_service_without_its_token_refuses_to_make_one() {
        let d = dir();
        let err = start(d.path(), mock(), Token::MustExist, ForegroundSource::Off)
            .err()
            .unwrap();
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
        let err = start(
            d.path(),
            hal.clone(),
            Token::CreateIfMissing,
            ForegroundSource::Off,
        )
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
        let app = start(
            d.path(),
            hal.clone(),
            Token::CreateIfMissing,
            ForegroundSource::Off,
        )
        .unwrap();
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
                e.contains("no hardware backend") && e.to_lowercase().contains("nvidia"),
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

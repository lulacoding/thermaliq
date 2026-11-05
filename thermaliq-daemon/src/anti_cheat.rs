//! Anti-cheat mode (SPEC §5.1): while a blocker runs, the BIOS has the motherboard fans, the
//! bridge is stopped and PawnIO is unloaded. NVML GPU control carries on. Pure logic: the caller
//! supplies the bridge, the device and whether a blocker is running.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Under the data dir. Exists only while we hold PawnIO disabled, so a restart knows it was us.
pub const MARKER_FILE: &str = "anti-cheat-mode";
/// How long every blocker must be gone, its service stopped included, before leaving the mode, so
/// a client that restarts itself doesn't bounce the driver. Chosen, not measured: M12 PR note.
pub const QUIET: Duration = Duration::from_secs(30);

/// shortcut: FACEIT only. Add others when a block is confirmed. Polling also reacts up to one
/// poll after the client starts, so a first launch can still be refused.
/// `FACEIT.exe` is the desktop app: it loads no driver, so it doesn't count.
const BLOCKERS: &[&str] = &["faceitclient.exe"];
/// Blocking in any state but stopped.
pub const BLOCKER_SERVICES: &[&str] = &["FACEITService"];

pub fn is_blocker(exe: &str) -> bool {
    BLOCKERS.iter().any(|b| b.eq_ignore_ascii_case(exe))
}

pub trait BridgeSwitch {
    /// Hands every fan back and stops the bridge. An error is reported, and the mode carries on.
    fn suspend(&mut self) -> Result<(), String>;
    fn resume(&mut self);
}

pub trait PawnIoDevice {
    fn disable(&mut self) -> Result<(), String>;
    fn enable(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartState {
    /// Start the bridge.
    Normal,
    /// Don't: a blocker is running, or PawnIO could not be brought back.
    Blocked,
}

/// SPEC §5.1: PawnIO is only re-enabled if our marker says we disabled it.
pub fn on_service_start(
    marker: &Path,
    blocker_running: bool,
    device: &mut dyn PawnIoDevice,
) -> StartState {
    if blocker_running {
        return StartState::Blocked;
    }
    if !marker.exists() {
        return StartState::Normal;
    }
    match device.enable() {
        Ok(()) => {
            if let Err(e) = fs::remove_file(marker) {
                eprintln!("anti-cheat marker not cleared: {e}");
            }
            StartState::Normal
        }
        Err(e) => {
            eprintln!("PawnIO not re-enabled, so motherboard fans stay off: {e}");
            StartState::Blocked
        }
    }
}

pub struct Mode {
    marker: PathBuf,
    active: bool,
    quiet_since: Option<Duration>,
    last_error: Option<String>,
}

impl Mode {
    pub fn new(marker: PathBuf) -> Self {
        Self {
            marker,
            active: false,
            quiet_since: None,
            last_error: None,
        }
    }

    pub fn started_blocked(marker: PathBuf) -> Self {
        Self {
            active: true,
            ..Self::new(marker)
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn step(
        &mut self,
        blocker_running: bool,
        now: Duration,
        bridge: &mut dyn BridgeSwitch,
        device: &mut dyn PawnIoDevice,
    ) {
        if blocker_running {
            self.quiet_since = None;
            if !self.active {
                self.enter(bridge, device);
            }
        } else if self.active {
            let since = *self.quiet_since.get_or_insert(now);
            if now.saturating_sub(since) >= QUIET {
                self.leave(bridge, device);
            }
        }
    }

    fn enter(&mut self, bridge: &mut dyn BridgeSwitch, device: &mut dyn PawnIoDevice) {
        self.active = true;
        self.last_error = None;
        if let Err(e) = bridge.suspend() {
            self.report(format!("hand-back on entering anti-cheat mode: {e}"));
        }
        if let Err(e) = fs::write(&self.marker, "") {
            // Disabling with no marker would leave nobody knowing to bring PawnIO back.
            self.report(format!("no marker, so PawnIO stays loaded: {e}"));
            return;
        }
        if let Err(e) = device.disable() {
            self.report(format!("PawnIO stays loaded: {e}"));
            let _ = fs::remove_file(&self.marker);
        }
    }

    fn leave(&mut self, bridge: &mut dyn BridgeSwitch, device: &mut dyn PawnIoDevice) {
        if self.marker.exists() {
            if let Err(e) = device.enable() {
                self.report(format!("PawnIO not re-enabled, will retry: {e}"));
                return;
            }
            if let Err(e) = fs::remove_file(&self.marker) {
                self.report(format!("anti-cheat marker not cleared: {e}"));
            }
        }
        self.active = false;
        self.quiet_since = None;
        bridge.resume();
    }

    fn report(&mut self, error: String) {
        eprintln!("anti-cheat mode: {error}");
        self.last_error = Some(error);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "anti_cheat_tests.rs"]
mod tests;

use super::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

type Log = Rc<RefCell<Vec<String>>>;

struct FakeBridge {
    log: Log,
    fail_suspend: bool,
}

impl BridgeSwitch for FakeBridge {
    fn suspend(&mut self) -> Result<(), String> {
        self.log.borrow_mut().push("suspend".into());
        if self.fail_suspend {
            Err("did not hand back".into())
        } else {
            Ok(())
        }
    }

    fn resume(&mut self) {
        self.log.borrow_mut().push("resume".into());
    }
}

struct FakeDevice {
    log: Log,
    marker: std::path::PathBuf,
    fail_disable: bool,
    fail_enable: bool,
}

impl PawnIoDevice for FakeDevice {
    fn disable(&mut self) -> Result<(), String> {
        // SPEC 5.1: the marker is on disk before PawnIO goes down.
        let marked = self.marker.exists();
        self.log
            .borrow_mut()
            .push(format!("disable marked={marked}"));
        if self.fail_disable {
            Err("pnputil failed".into())
        } else {
            Ok(())
        }
    }

    fn enable(&mut self) -> Result<(), String> {
        let marked = self.marker.exists();
        self.log
            .borrow_mut()
            .push(format!("enable marked={marked}"));
        if self.fail_enable {
            Err("pnputil failed".into())
        } else {
            Ok(())
        }
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    marker: std::path::PathBuf,
    log: Log,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(MARKER_FILE);
        Self {
            _dir: dir,
            marker,
            log: Log::default(),
        }
    }

    fn device(&self) -> FakeDevice {
        FakeDevice {
            log: self.log.clone(),
            marker: self.marker.clone(),
            fail_disable: false,
            fail_enable: false,
        }
    }

    fn bridge(&self) -> FakeBridge {
        FakeBridge {
            log: self.log.clone(),
            fail_suspend: false,
        }
    }

    fn lines(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

const T0: Duration = Duration::ZERO;

#[test]
fn startup_with_a_marker_and_no_anti_cheat_re_enables_and_clears() {
    let r = Rig::new();
    fs::write(&r.marker, "").unwrap();
    assert_eq!(
        on_service_start(&r.marker, false, &mut r.device()),
        StartState::Normal
    );
    assert_eq!(r.lines(), ["enable marked=true"]);
    assert!(!r.marker.exists());
}

#[test]
fn startup_with_a_marker_and_anti_cheat_running_leaves_it_disabled() {
    let r = Rig::new();
    fs::write(&r.marker, "").unwrap();
    assert_eq!(
        on_service_start(&r.marker, true, &mut r.device()),
        StartState::Blocked
    );
    assert!(r.lines().is_empty());
    assert!(r.marker.exists());
}

#[test]
fn startup_with_no_marker_never_touches_a_device_it_did_not_disable() {
    let r = Rig::new();
    assert_eq!(
        on_service_start(&r.marker, false, &mut r.device()),
        StartState::Normal
    );
    assert!(r.lines().is_empty());
}

#[test]
fn a_failed_re_enable_at_startup_keeps_the_marker_for_next_time() {
    let r = Rig::new();
    fs::write(&r.marker, "").unwrap();
    let mut device = r.device();
    device.fail_enable = true;
    assert_eq!(
        on_service_start(&r.marker, false, &mut device),
        StartState::Blocked
    );
    assert!(r.marker.exists());
}

#[test]
fn startup_with_anti_cheat_running_and_no_marker_is_blocked_so_the_bridge_does_not_start() {
    let r = Rig::new();
    assert_eq!(
        on_service_start(&r.marker, true, &mut r.device()),
        StartState::Blocked
    );
    assert!(r.lines().is_empty());
}

#[test]
fn entering_hands_back_first_then_marks_then_disables() {
    let r = Rig::new();
    let mut m = Mode::new(r.marker.clone());
    m.step(true, T0, &mut r.bridge(), &mut r.device());
    assert_eq!(r.lines(), ["suspend", "disable marked=true"]);
    assert!(m.active());
    assert!(r.marker.exists());
}

#[test]
fn a_failed_hand_back_still_disables_pawnio_because_the_game_needs_it_gone() {
    let r = Rig::new();
    let mut m = Mode::new(r.marker.clone());
    let mut bridge = r.bridge();
    bridge.fail_suspend = true;
    m.step(true, T0, &mut bridge, &mut r.device());
    assert_eq!(r.lines(), ["suspend", "disable marked=true"]);
    assert!(m.last_error().unwrap().contains("did not hand back"));
}

#[test]
fn a_failed_disable_clears_the_marker_because_pawnio_is_still_up() {
    let r = Rig::new();
    let mut m = Mode::new(r.marker.clone());
    let mut device = r.device();
    device.fail_disable = true;
    m.step(true, T0, &mut r.bridge(), &mut device);
    assert!(m.active(), "the bridge stays stopped");
    assert!(!r.marker.exists());
    assert!(m.last_error().unwrap().contains("pnputil"));
}

#[test]
fn staying_blocked_does_nothing_more() {
    let r = Rig::new();
    let mut m = Mode::new(r.marker.clone());
    m.step(true, T0, &mut r.bridge(), &mut r.device());
    r.log.borrow_mut().clear();
    m.step(
        true,
        T0 + Duration::from_secs(5),
        &mut r.bridge(),
        &mut r.device(),
    );
    assert!(r.lines().is_empty());
}

#[test]
fn leaving_waits_for_the_quiet_period_then_enables_clears_and_resumes() {
    let r = Rig::new();
    let mut m = Mode::new(r.marker.clone());
    m.step(true, T0, &mut r.bridge(), &mut r.device());
    r.log.borrow_mut().clear();
    let gone = Duration::from_secs(100);
    m.step(false, gone, &mut r.bridge(), &mut r.device());
    m.step(
        false,
        gone + QUIET - Duration::from_secs(1),
        &mut r.bridge(),
        &mut r.device(),
    );
    assert!(r.lines().is_empty());
    assert!(m.active());
    m.step(false, gone + QUIET, &mut r.bridge(), &mut r.device());
    assert_eq!(r.lines(), ["enable marked=true", "resume"]);
    assert!(!r.marker.exists());
    assert!(!m.active());
}

#[test]
fn a_blocker_coming_back_restarts_the_quiet_period() {
    let r = Rig::new();
    let mut m = Mode::new(r.marker.clone());
    m.step(true, T0, &mut r.bridge(), &mut r.device());
    r.log.borrow_mut().clear();
    m.step(
        false,
        Duration::from_secs(10),
        &mut r.bridge(),
        &mut r.device(),
    );
    m.step(
        true,
        Duration::from_secs(20),
        &mut r.bridge(),
        &mut r.device(),
    );
    m.step(
        false,
        Duration::from_secs(25),
        &mut r.bridge(),
        &mut r.device(),
    );
    m.step(
        false,
        Duration::from_secs(10) + QUIET,
        &mut r.bridge(),
        &mut r.device(),
    );
    assert!(r.lines().is_empty());
}

#[test]
fn a_failed_enable_keeps_the_marker_and_retries_without_starting_the_bridge() {
    let r = Rig::new();
    let mut m = Mode::new(r.marker.clone());
    m.step(true, T0, &mut r.bridge(), &mut r.device());
    r.log.borrow_mut().clear();
    let mut device = r.device();
    device.fail_enable = true;
    m.step(false, Duration::from_secs(1), &mut r.bridge(), &mut device);
    m.step(
        false,
        Duration::from_secs(1) + QUIET,
        &mut r.bridge(),
        &mut device,
    );
    assert_eq!(r.lines(), ["enable marked=true"]);
    assert!(r.marker.exists());
    assert!(m.active());
    m.step(
        false,
        Duration::from_secs(5) + QUIET,
        &mut r.bridge(),
        &mut r.device(),
    );
    assert_eq!(r.lines().last().unwrap(), "resume");
    assert!(!r.marker.exists());
}

#[test]
fn a_mode_started_blocked_is_already_active_and_leaves_normally() {
    let r = Rig::new();
    fs::write(&r.marker, "").unwrap();
    let mut m = Mode::started_blocked(r.marker.clone());
    assert!(m.active());
    m.step(false, T0, &mut r.bridge(), &mut r.device());
    m.step(false, QUIET, &mut r.bridge(), &mut r.device());
    assert_eq!(r.lines(), ["enable marked=true", "resume"]);
}

#[test]
fn blockers_are_recognised_by_exact_exe_name_any_case() {
    assert!(is_blocker("faceitclient.exe"));
    assert!(is_blocker("FACEITClient.exe"));
    assert!(!is_blocker("faceit.exe"), "the desktop app loads no driver");
    assert!(!is_blocker("FACEIT.exe"));
    assert!(!is_blocker("faceit-notes.exe"));
    assert!(!is_blocker("notepad.exe"));
}

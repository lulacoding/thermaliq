//! Starting the real bridge process and connecting to it (SPEC §5.1). Windows only. The protocol
//! and every fan rule are behind `thermaliq_hal::LhmBridge`.

use crate::anti_cheat::{self, BridgeSwitch, Mode, PawnIoDevice, StartState};
use crate::{acl, win};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};
use thermaliq_hal::{BridgeConnection, BridgeStarter, HalError, LhmBridge};

const PIPE_NAME: &str = "thermaliq-lhm-bridge";
const PIPE_PATH: &str = r"\\.\pipe\thermaliq-lhm-bridge";
const PAWNIO_DEVICE: &str = r"ROOT\PAWNIO\0000";
const START_BUDGET: Duration = Duration::from_secs(12);
const STOP_BUDGET: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(100);
const WATCH_EVERY: Duration = Duration::from_secs(2);
/// More than a whole snapshot, far less than a broken bridge could send.
const MAX_REPLY_BYTES: u64 = 1024 * 1024;
const SECURITY_IDENTIFICATION: u32 = 1 << 16;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;
const EXIT_PIPE_TAKEN: i32 = 12;

/// The backend, if this machine can run one. `None` means motherboard fans are absent, and why
/// has been logged: the GPU and agent carry on (SPEC §5.1).
pub fn start_backend(data_dir: &Path) -> Option<Arc<LhmBridge>> {
    let marker = data_dir.join(anti_cheat::MARKER_FILE);
    let blocked = blocker_running();
    let state = anti_cheat::on_service_start(&marker, blocked, &mut PnpDevice);
    if state == StartState::Blocked {
        eprintln!("anti-cheat mode at startup: motherboard fans stay off until a service restart");
        watch(marker, None, true);
        return None;
    }
    let exe = match bridge_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("motherboard fans absent: {e}");
            return None;
        }
    };
    let starter = Starter {
        exe,
        data_dir: data_dir.to_owned(),
        child: None,
    };
    match LhmBridge::new(Box::new(starter)) {
        Ok(bridge) => {
            let bridge = Arc::new(bridge);
            watch(marker, Some(Arc::downgrade(&bridge)), false);
            Some(bridge)
        }
        Err(HalError(e)) => {
            eprintln!("motherboard fans absent: {e}");
            None
        }
    }
}

/// Beside the daemon, so it sits in the folder the installer locks down.
fn bridge_exe() -> Result<PathBuf, String> {
    let daemon = std::env::current_exe().map_err(|e| format!("where am I: {e}"))?;
    let exe = daemon
        .parent()
        .ok_or("the daemon has no folder")?
        .join("lhm-bridge")
        .join("lhm-bridge.exe");
    if exe.exists() {
        Ok(exe)
    } else {
        Err(format!("{} is not installed", exe.display()))
    }
}

/// M20c's rule: this runs as SYSTEM, so nobody but an administrator may be able to replace it.
fn check_replaceable(exe: &Path) -> Result<(), String> {
    let root = std::env::var_os("ProgramFiles").ok_or("ProgramFiles is not set")?;
    // Lower case on both sides, as in `LocalConfig::verify`: these paths are case-insensitive.
    let exe = PathBuf::from(exe.to_string_lossy().to_lowercase());
    let root = PathBuf::from(root.to_string_lossy().to_lowercase());
    let problems = acl::replaceable_by_others(&exe, &root)
        .map_err(|e| format!("cannot check who could replace the bridge: {e}"))?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "others could replace the bridge: {}",
            problems.join("; ")
        ))
    }
}

struct Starter {
    exe: PathBuf,
    data_dir: PathBuf,
    child: Option<Child>,
}

impl Starter {
    fn spawn(&self) -> Result<Child, String> {
        // Rechecked each time: the ACLs may have changed since the service started.
        check_replaceable(&self.exe)?;
        let log = File::create(self.data_dir.join("lhm-bridge.log"))
            .map_err(|e| format!("bridge log: {e}"))?;
        // Not in the daemon's job: if the daemon dies the bridge must live to hand the fans back.
        Command::new(&self.exe)
            .args(["--pipe", PIPE_NAME, "--data-dir"])
            .arg(&self.data_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", self.exe.display()))
    }
}

impl BridgeStarter for Starter {
    fn start(&mut self) -> Result<Box<dyn BridgeConnection>, String> {
        if !self.kill() {
            return Err("the previous bridge is still alive and can't be killed".into());
        }
        let deadline = Instant::now() + START_BUDGET;
        let mut child = self.spawn()?;
        loop {
            match open_pipe() {
                Ok(pipe) => {
                    // Anyone could have made this name first. Only our own child will do.
                    let server = win::pipe_server_pid(&pipe)
                        .map_err(|e| format!("cannot identify the pipe's server: {e}"))?;
                    if server != child.id() {
                        return Err(format!(
                            "pipe is served by process {server}, not our bridge {}",
                            child.id()
                        ));
                    }
                    let reader = pipe.try_clone().map_err(|e| format!("pipe: {e}"))?;
                    self.child = Some(child);
                    return Ok(Box::new(PipeConnection {
                        reader: BufReader::new(reader),
                        writer: pipe,
                    }));
                }
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY)
                    ) => {}
                Err(e) => return Err(format!("cannot open the bridge pipe: {e}")),
            }
            if let Ok(Some(status)) = child.try_wait() {
                if status.code() == Some(EXIT_PIPE_TAKEN) && Instant::now() < deadline {
                    // An old bridge is still handing back after its client left. Try again.
                    thread::sleep(POLL * 5);
                    child = self.spawn()?;
                    continue;
                }
                return Err(exit_reason(status.code()));
            }
            if Instant::now() >= deadline {
                self.child = Some(child);
                self.kill();
                return Err("the bridge did not open its pipe in time".into());
            }
            thread::sleep(POLL);
        }
    }

    /// Never kills a bridge that is still exiting: it may be handing fans back. One still alive
    /// after the budget is wedged, and the next one restores from the saved originals.
    fn stop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if exited_within(child, STOP_BUDGET) {
            self.child = None;
        } else {
            eprintln!("the bridge did not exit in time, killing it");
            self.kill();
        }
    }

    /// A bridge stuck inside a driver call can't die until that call returns, so the wait is
    /// bounded and the child is kept for another try.
    fn kill(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return true;
        };
        let _ = child.kill();
        let gone = exited_within(child, STOP_BUDGET);
        if gone {
            self.child = None;
        }
        gone
    }
}

fn exited_within(child: &mut Child, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

fn exit_reason(code: Option<i32>) -> String {
    match code {
        Some(10) => "PawnIO is not installed".into(),
        Some(11) => "self-check failed: see lhm-bridge.log".into(),
        Some(EXIT_PIPE_TAKEN) => "the bridge pipe name is taken".into(),
        other => format!("the bridge exited with {other:?}: see lhm-bridge.log"),
    }
}

/// SECURITY_IDENTIFICATION: whoever serves this pipe must not be able to impersonate the daemon,
/// which is SYSTEM. The server's identity is only checked after connecting.
fn open_pipe() -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(PIPE_PATH)
}

struct PipeConnection {
    reader: BufReader<File>,
    writer: File,
}

impl BridgeConnection for PipeConnection {
    fn exchange(&mut self, line: &str) -> io::Result<String> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        read_reply(&mut self.reader)
    }
}

fn read_reply(reader: &mut impl BufRead) -> io::Result<String> {
    let mut reply = String::new();
    reader.take(MAX_REPLY_BYTES).read_line(&mut reply)?;
    if reply.ends_with('\n') {
        Ok(reply)
    } else {
        Err(io::Error::other("bridge reply cut short or too long"))
    }
}

fn blocker_running() -> bool {
    let process = match win::process_names() {
        Ok(names) => names.iter().any(|n| anti_cheat::is_blocker(n)),
        Err(e) => {
            eprintln!("cannot list processes, so no anti-cheat process seen: {e}");
            false
        }
    };
    process
        || anti_cheat::BLOCKER_SERVICES.iter().any(|name| {
            win::service_active(name).unwrap_or_else(|e| {
                eprintln!("cannot query service {name}, so it counts as stopped: {e}");
                false
            })
        })
}

struct PnpDevice;

impl PnpDevice {
    fn pnputil(verb: &str) -> Result<(), String> {
        // By full path: this runs as SYSTEM, so never by searching PATH.
        let root = std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?;
        let tool = Path::new(&root).join("System32").join("pnputil.exe");
        let out = Command::new(tool)
            .args([verb, PAWNIO_DEVICE])
            .output()
            .map_err(|e| format!("pnputil: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "pnputil {verb} failed ({:?}): {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).trim()
            ))
        }
    }
}

impl PawnIoDevice for PnpDevice {
    fn disable(&mut self) -> Result<(), String> {
        Self::pnputil("/disable-device")
    }

    fn enable(&mut self) -> Result<(), String> {
        Self::pnputil("/enable-device")
    }
}

/// `None`: the backend never started (blocked at startup), so there is nothing to suspend.
struct WeakBridge(Option<Weak<LhmBridge>>);

impl BridgeSwitch for WeakBridge {
    fn suspend(&mut self) -> Result<(), String> {
        match self.0.as_ref().and_then(Weak::upgrade) {
            Some(bridge) => bridge.suspend(),
            None => Ok(()),
        }
    }

    fn resume(&mut self) {
        match self.0.as_ref().and_then(Weak::upgrade) {
            Some(bridge) => bridge.resume(),
            None => eprintln!("anti-cheat mode over: restart the service to get motherboard fans"),
        }
    }
}

/// Detached: it ends with the backend, or with the process if the backend never started.
fn watch(marker: PathBuf, bridge: Option<Weak<LhmBridge>>, started_blocked: bool) {
    let mut mode = if started_blocked {
        Mode::started_blocked(marker)
    } else {
        Mode::new(marker)
    };
    let tracks_backend = bridge.is_some();
    let mut switch = WeakBridge(bridge);
    let began = Instant::now();
    let spawned = thread::Builder::new()
        .name("anti-cheat".into())
        .spawn(move || loop {
            if tracks_backend && switch.0.as_ref().is_some_and(|w| w.strong_count() == 0) {
                return;
            }
            mode.step(
                blocker_running(),
                began.elapsed(),
                &mut switch,
                &mut PnpDevice,
            );
            thread::sleep(WATCH_EVERY);
        });
    if let Err(e) = spawned {
        eprintln!("anti-cheat watcher did not start: {e}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    #[test]
    fn a_reply_is_one_whole_line() {
        let mut r = Cursor::new(b"{\"ok\":true}\nnext".to_vec());
        assert_eq!(read_reply(&mut r).unwrap(), "{\"ok\":true}\n");
    }

    #[test]
    fn a_reply_cut_short_is_an_error() {
        assert!(read_reply(&mut Cursor::new(b"{\"ok\"".to_vec())).is_err());
        assert!(read_reply(&mut Cursor::new(Vec::new())).is_err());
    }

    #[test]
    fn a_reply_that_never_ends_is_an_error_not_unbounded_memory() {
        let endless = vec![b'x'; usize::try_from(MAX_REPLY_BYTES).unwrap() + 10];
        assert!(read_reply(&mut Cursor::new(endless)).is_err());
    }

    fn live_process() -> Child {
        let root = std::env::var_os("SystemRoot").unwrap();
        Command::new(Path::new(&root).join("System32").join("ping.exe"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn starter_with(child: Option<Child>) -> (tempfile::TempDir, Starter) {
        let dir = tempfile::tempdir().unwrap();
        let starter = Starter {
            exe: dir.path().join("no-such-bridge.exe"),
            data_dir: dir.path().to_owned(),
            child,
        };
        (dir, starter)
    }

    #[test]
    fn kill_ends_a_live_bridge_and_says_so() {
        let mut child = live_process();
        assert!(child.try_wait().unwrap().is_none());
        let (_dir, mut starter) = starter_with(Some(child));
        assert!(starter.kill());
        assert!(starter.child.is_none());
    }

    #[test]
    fn kill_with_no_bridge_is_done() {
        let (_dir, mut starter) = starter_with(None);
        assert!(starter.kill());
    }

    #[test]
    fn starting_clears_a_bridge_still_left_alive_first() {
        let (_dir, mut starter) = starter_with(Some(live_process()));
        assert!(starter.start().is_err(), "this exe doesn't exist");
        assert!(starter.child.is_none());
    }

    #[test]
    fn exit_codes_name_their_reason() {
        assert!(exit_reason(Some(10)).contains("PawnIO"));
        assert!(exit_reason(Some(11)).contains("self-check"));
        assert!(exit_reason(Some(99)).contains("99"));
    }

    #[test]
    fn a_pipe_nobody_serves_is_not_found_so_the_start_keeps_waiting() {
        let e = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(r"\\.\pipe\thermaliq-nobody-serves-this")
            .unwrap_err();
        assert_eq!(e.raw_os_error(), Some(ERROR_FILE_NOT_FOUND));
    }

    #[test]
    fn the_pipe_constants_agree() {
        assert!(PIPE_PATH.ends_with(PIPE_NAME));
    }

    #[test]
    fn a_bridge_outside_program_files_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("lhm-bridge").join("lhm-bridge.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, "").unwrap();
        assert!(check_replaceable(&exe).is_err());
    }

    #[test]
    fn process_names_include_this_test() {
        let me = std::env::current_exe().unwrap();
        let me = me.file_name().unwrap().to_string_lossy().to_string();
        let names = win::process_names().unwrap();
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case(&me)), "{me}");
    }

    /// Not a pipe: the call must fail, so a failure can't pass for a matching pid. The real
    /// positive case needs the bridge's SYSTEM-only pipe, so it's a hardware check.
    #[test]
    fn the_server_pid_of_something_that_is_not_a_pipe_is_an_error() {
        let me = File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(win::pipe_server_pid(&me).is_err());
    }
}

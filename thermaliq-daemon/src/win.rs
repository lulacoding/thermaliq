//! The daemon's only `unsafe` code: the OS keychain, the foreground window, a helper in the
//! user's session (a service in session 0 can't see the desktop), a job object so that a
//! local model can't outlive the daemon, and the LHM bridge's pipe and process checks.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::process::Child;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, SetHandleInformation, SetLastError, ERROR_NOT_FOUND,
    ERROR_SERVICE_DOES_NOT_EXIST, HANDLE, HANDLE_FLAG_INHERIT, RECT,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};
use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::{CreatePipe, GetNamedPipeServerProcessId};
use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
    SERVICE_QUERY_STATUS, SERVICE_STATUS, SERVICE_STOPPED,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, UpdateProcThreadAttribute,
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use windows_sys::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
};

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

// ---- keychain (Windows Credential Manager) ----

pub fn cred_read(target: &str) -> io::Result<Option<String>> {
    let name = wide(OsStr::new(target));
    let mut cred: *mut CREDENTIALW = null_mut();
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) } == 0 {
        return match unsafe { GetLastError() } {
            ERROR_NOT_FOUND => Ok(None),
            _ => Err(last_error()),
        };
    }
    // Valid until CredFree: the OS allocated it, and the size comes with the blob.
    let bytes = unsafe {
        let c = &*cred;
        let blob = if c.CredentialBlob.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
        };
        let copy = blob.to_vec();
        CredFree(cred.cast());
        copy
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| io::Error::other("the stored credential is not text"))
}

pub fn cred_write(target: &str, secret: &str) -> io::Result<()> {
    let mut name = wide(OsStr::new(target));
    let mut blob = secret.as_bytes().to_vec();
    let size = u32::try_from(blob.len()).map_err(|_| io::Error::other("credential too long"))?;
    // Every pointer outlives the call, and the OS copies what it keeps.
    let ok = unsafe {
        let mut cred: CREDENTIALW = core::mem::zeroed();
        cred.Type = CRED_TYPE_GENERIC;
        cred.TargetName = name.as_mut_ptr();
        cred.CredentialBlobSize = size;
        cred.CredentialBlob = blob.as_mut_ptr();
        cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
        CredWriteW(&cred, 0)
    };
    blob.fill(0);
    if ok == 0 {
        return Err(last_error());
    }
    Ok(())
}

pub fn cred_delete(target: &str) -> io::Result<()> {
    let name = wide(OsStr::new(target));
    if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        return match unsafe { GetLastError() } {
            ERROR_NOT_FOUND => Ok(()),
            _ => Err(last_error()),
        };
    }
    Ok(())
}

// ---- the foreground window ----

/// The foreground window's exe and whether it fills its screen. `None` for the exe when there
/// is no window (the lock screen) or the process refuses to be read (some anti-cheats).
pub fn foreground_now() -> (Option<String>, bool) {
    let window = unsafe { GetForegroundWindow() };
    let notification_says_fullscreen = {
        let mut state = 0;
        let hr = unsafe { SHQueryUserNotificationState(&mut state) };
        hr == 0
            && matches!(
                state,
                QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
            )
    };
    if window.is_null() {
        return (None, notification_says_fullscreen);
    }
    let exe = window_exe(window);
    // The desktop itself is a window as big as the screen, and it isn't a game.
    let covers_screen = exe.as_deref() != Some("explorer.exe") && covers_monitor(window);
    (exe, notification_says_fullscreen || covers_screen)
}

fn window_exe(window: *mut core::ffi::c_void) -> Option<String> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if pid == 0 {
        return None;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(process) };
    if ok == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit('\\').next().map(str::to_lowercase)
}

fn covers_monitor(window: *mut core::ffi::c_void) -> bool {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(window, &mut rect) } == 0 {
        return false;
    }
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { core::mem::zeroed() };
    info.cbSize = core::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return false;
    }
    let m = info.rcMonitor;
    rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
}

// ---- the helper in the user's session ----

/// A process started in the console user's session, with its stdout piped back.
pub struct SessionChild {
    process: isize,
}

impl SessionChild {
    pub fn kill(self) {
        unsafe { TerminateProcess(self.process as HANDLE, 1) };
        // Drop closes the handle.
    }
}

impl Drop for SessionChild {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.process as HANDLE) };
    }
}

/// Needs the service's LocalSystem rights (`WTSQueryUserToken`). Fails while nobody is logged on,
/// and the caller retries. The child runs as the user, so it has no more rights than they do.
pub fn spawn_in_user_session(exe: &Path, arg: &str) -> io::Result<(File, SessionChild)> {
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    if session == u32::MAX {
        return Err(io::Error::other("no console session"));
    }
    let mut token: HANDLE = null_mut();
    if unsafe { WTSQueryUserToken(session, &mut token) } == 0 {
        return Err(last_error());
    }
    let spawned = spawn_with_token(token, exe, arg, &[]);
    unsafe { CloseHandle(token) };
    spawned
}

/// A SYSTEM handle in a process the user can open is an escalation path, so the child gets a
/// handle list holding only its pipe (plus `extra`, which only a test passes), never "whatever
/// is inheritable right now". Its environment is the user's, not the service's.
fn spawn_with_token(
    token: HANDLE,
    exe: &Path,
    arg: &str,
    extra: &[HANDLE],
) -> io::Result<(File, SessionChild)> {
    let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, null(), 0) } == 0 {
        return Err(last_error());
    }
    // Inheritable only now, and closed just after: as short a window as possible for a
    // `Command` spawned at the same moment to pick up this pipe.
    unsafe { SetHandleInformation(write, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
    let mut handles = vec![write];
    handles.extend_from_slice(extra);
    let result = create_with_handle_list(token, exe, arg, write, &handles);
    let e = last_error();
    unsafe { CloseHandle(write) };
    match result {
        Ok(pi) => {
            unsafe { CloseHandle(pi.hThread) };
            // `read` is ours alone from here on.
            let stdout = unsafe { File::from_raw_handle(read) };
            Ok((
                stdout,
                SessionChild {
                    process: pi.hProcess as isize,
                },
            ))
        }
        Err(()) => {
            unsafe { CloseHandle(read) };
            Err(e)
        }
    }
}

/// On failure the OS's last error says why.
fn create_with_handle_list(
    token: HANDLE,
    exe: &Path,
    arg: &str,
    stdout: HANDLE,
    handles: &[HANDLE],
) -> Result<PROCESS_INFORMATION, ()> {
    // The first call only asks for the size, and "fails" doing so.
    let mut size = 0;
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size) };
    let mut storage = vec![0usize; size / core::mem::size_of::<usize>() + 1];
    let list = storage.as_mut_ptr().cast();
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
        return Err(());
    }
    let mut environment = null_mut();
    let mut pi: PROCESS_INFORMATION = unsafe { core::mem::zeroed() };
    let mut ok = unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            core::mem::size_of_val(handles),
            null_mut(),
            null(),
        )
    } != 0
        && unsafe { CreateEnvironmentBlock(&mut environment, token, 0) } != 0;
    if ok {
        let mut desktop = wide(OsStr::new("winsta0\\default"));
        let app = wide(exe.as_os_str());
        let mut cmd = wide(OsStr::new(&format!("\"{}\" {arg}", exe.display())));
        let mut si: STARTUPINFOEXW = unsafe { core::mem::zeroed() };
        si.StartupInfo.cb = core::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.lpDesktop = desktop.as_mut_ptr();
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdOutput = stdout;
        si.StartupInfo.hStdError = stdout;
        si.lpAttributeList = list;
        ok = unsafe {
            CreateProcessAsUserW(
                token,
                app.as_ptr(),
                cmd.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                environment,
                null(),
                &si.StartupInfo,
                &mut pi,
            )
        } != 0;
    }
    // These must not overwrite the error the caller reads.
    let error = unsafe { GetLastError() };
    unsafe {
        if !environment.is_null() {
            DestroyEnvironmentBlock(environment);
        }
        DeleteProcThreadAttributeList(list);
        SetLastError(error);
    }
    if ok {
        Ok(pi)
    } else {
        Err(())
    }
}

// ---- LHM bridge: who serves a pipe, and which programs are running ----

pub fn pipe_server_pid(pipe: &File) -> io::Result<u32> {
    let mut pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &raw mut pid) } == 0 {
        return Err(last_error());
    }
    Ok(pid)
}

/// The exe name of every running process.
pub fn process_names() -> io::Result<Vec<String>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    let mut entry: PROCESSENTRY32W = unsafe { core::mem::zeroed() };
    entry.dwSize = core::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut names = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) };
    while more != 0 {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        names.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
        more = unsafe { Process32NextW(snapshot, &raw mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    Ok(names)
}

/// Sends everything `eprintln!` writes to `path` from now on. A service has no console, so without
/// this every startup and bridge message is lost, and the daemon can fail with nothing to read.
pub fn redirect_stderr(path: &Path) -> io::Result<()> {
    use std::os::windows::io::IntoRawHandle;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    // The process keeps this handle for good, so it is leaked on purpose.
    if unsafe { SetStdHandle(STD_ERROR_HANDLE, file.into_raw_handle() as HANDLE) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Whether a service is in any state but stopped. One that isn't installed counts as stopped.
pub fn service_active(name: &str) -> io::Result<bool> {
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(last_error());
    }
    let name = wide(OsStr::new(name));
    let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS) };
    let active = if service.is_null() {
        match unsafe { GetLastError() } {
            ERROR_SERVICE_DOES_NOT_EXIST => Ok(false),
            _ => Err(last_error()),
        }
    } else {
        let mut status: SERVICE_STATUS = unsafe { core::mem::zeroed() };
        let read = if unsafe { QueryServiceStatus(service, &raw mut status) } == 0 {
            Err(last_error())
        } else {
            Ok(status.dwCurrentState != SERVICE_STOPPED)
        };
        unsafe { CloseServiceHandle(service) };
        read
    };
    unsafe { CloseServiceHandle(manager) };
    active
}

// ---- a local model can't outlive the daemon ----

/// Closing it (drop, or the process ending in any way) kills everything assigned to it,
/// including the runner process Ollaya starts for a model.
pub struct Job(isize);

impl Job {
    pub fn containing(child: &Child) -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(last_error());
        }
        let job = Self(handle as isize);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { core::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(last_error());
        }
        if unsafe { AssignProcessToJobObject(handle, child.as_raw_handle()) } == 0 {
            return Err(last_error());
        }
        Ok(job)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0 as HANDLE) };
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Read;
    use windows_sys::Win32::Security::{
        DuplicateTokenEx, SecurityImpersonation, TokenPrimary, SECURITY_ATTRIBUTES,
        TOKEN_ALL_ACCESS,
    };
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const SECRET: &[u8] = b"system-only-secret";
    const SERVICE_VAR: &str = "THERMALIQ_TEST_SERVICE_ONLY_VAR";
    const PROBE: &str = "session-probe=";

    /// Runs inside the child this test binary starts of itself. Does nothing in a normal run.
    #[test]
    fn session_probe_child() {
        let Some(value) = std::env::args()
            .find_map(|a| a.strip_prefix(PROBE).and_then(|v| v.parse::<isize>().ok()))
        else {
            return;
        };
        let (mut buf, mut got, mut total) = ([0u8; 64], 0u32, 0u32);
        let ok = unsafe {
            PeekNamedPipe(
                value as HANDLE,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut got,
                &mut total,
                null_mut(),
            )
        };
        let leaked = ok != 0 && &buf[..got as usize] == SECRET;
        println!(
            "probe leaked={leaked} service_var={} user_env={}",
            std::env::var_os(SERVICE_VAR).is_some(),
            std::env::var_os("USERPROFILE").is_some()
        );
    }

    /// An inheritable handle holding a secret, as a handle inside the service might.
    fn inheritable_secret() -> HANDLE {
        let attrs = SECURITY_ATTRIBUTES {
            nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        assert_ne!(unsafe { CreatePipe(&mut read, &mut write, &attrs, 0) }, 0);
        let mut wrote = 0;
        assert_ne!(
            unsafe {
                WriteFile(
                    write,
                    SECRET.as_ptr(),
                    SECRET.len() as u32,
                    &mut wrote,
                    null_mut(),
                )
            },
            0
        );
        read
    }

    /// Our own token, duplicated as a primary one: the shape `WTSQueryUserToken` returns.
    fn own_token() -> HANDLE {
        let (mut mine, mut dup): (HANDLE, HANDLE) = (null_mut(), null_mut());
        unsafe {
            assert_ne!(
                OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut mine),
                0
            );
            assert_ne!(
                DuplicateTokenEx(
                    mine,
                    TOKEN_ALL_ACCESS,
                    null(),
                    SecurityImpersonation,
                    TokenPrimary,
                    &mut dup
                ),
                0
            );
            CloseHandle(mine);
        }
        dup
    }

    /// The child's report, or `None` when this logon may not start a process with a token.
    fn probe(secret: HANDLE, extra: &[HANDLE]) -> Option<String> {
        let exe = std::env::current_exe().unwrap();
        let arg = format!(
            "win::tests::session_probe_child --exact --nocapture --test-threads=1 {PROBE}{}",
            secret as isize
        );
        let token = own_token();
        let spawned = spawn_with_token(token, &exe, &arg, extra);
        unsafe { CloseHandle(token) };
        let (mut out, _child) = match spawned {
            Ok(s) => s,
            // ERROR_PRIVILEGE_NOT_HELD: the service has this right, a dev shell may not
            Err(e) if e.raw_os_error() == Some(1314) => {
                assert!(std::env::var_os("CI").is_none(), "{e}");
                eprintln!("skipped: {e}");
                return None;
            }
            Err(e) => panic!("{e}"),
        };
        let mut text = String::new();
        out.read_to_string(&mut text).unwrap();
        let line = text
            .lines()
            .find(|l| l.contains("probe leaked="))
            .map(str::to_owned);
        Some(line.unwrap_or_else(|| panic!("the child never reported: {text}")))
    }

    #[test]
    fn redirected_stderr_lands_in_the_file() {
        use std::io::Write;
        use windows_sys::Win32::System::Console::GetStdHandle;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("service.log");
        let before = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        redirect_stderr(&log).unwrap();
        // `io::stderr()`, not `eprintln!`: the test harness captures the macro before it gets here.
        writeln!(io::stderr(), "bridge said hello").unwrap();
        unsafe { SetStdHandle(STD_ERROR_HANDLE, before) };
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains("bridge said hello"));
    }

    #[test]
    fn a_running_service_is_active_and_a_missing_one_is_not() {
        assert!(
            service_active("RpcSs").unwrap(),
            "Windows can't run without it"
        );
        assert!(!service_active("thermaliq-no-such-service").unwrap());
    }

    #[test]
    fn the_session_child_inherits_only_its_pipe() {
        let secret = inheritable_secret();
        let Some(report) = probe(secret, &[]) else {
            return;
        };
        assert!(report.contains("leaked=false"), "{report}");
    }

    /// Shows the probe can see a leak at all, so the test above means something.
    #[test]
    fn the_probe_does_see_a_handle_that_is_passed_on() {
        let secret = inheritable_secret();
        let Some(report) = probe(secret, &[secret]) else {
            return;
        };
        assert!(report.contains("leaked=true"), "{report}");
    }

    #[test]
    fn the_session_child_gets_the_users_environment_not_ours() {
        // Unique name, only read in the child: no other test cares.
        std::env::set_var(SERVICE_VAR, "1");
        let Some(report) = probe(null_mut(), &[]) else {
            return;
        };
        assert!(report.contains("service_var=false"), "{report}");
        assert!(report.contains("user_env=true"), "{report}");
    }
}

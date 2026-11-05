//! The daemon's only `unsafe` code: the OS keychain, the foreground window, a helper in the
//! user's session (a service in session 0 can't see the desktop), and a job object so that a
//! local model can't outlive the daemon.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::process::Child;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, SetHandleInformation, ERROR_NOT_FOUND, HANDLE, HANDLE_FLAG_INHERIT,
    RECT,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    CREATE_NO_WINDOW, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOW,
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
    let attrs = SECURITY_ATTRIBUTES {
        nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, &attrs, 0) } == 0 {
        let e = last_error();
        unsafe { CloseHandle(token) };
        return Err(e);
    }
    // Only the child's end is inherited.
    unsafe { SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0) };
    let mut desktop = wide(OsStr::new("winsta0\\default"));
    let app = wide(exe.as_os_str());
    let mut cmd = wide(OsStr::new(&format!("\"{}\" {arg}", exe.display())));
    let mut si: STARTUPINFOW = unsafe { core::mem::zeroed() };
    si.cb = core::mem::size_of::<STARTUPINFOW>() as u32;
    si.lpDesktop = desktop.as_mut_ptr();
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdOutput = write;
    si.hStdError = write;
    let mut pi: PROCESS_INFORMATION = unsafe { core::mem::zeroed() };
    let ok = unsafe {
        CreateProcessAsUserW(
            token,
            app.as_ptr(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_NO_WINDOW,
            null(),
            null(),
            &si,
            &mut pi,
        )
    };
    let e = last_error();
    unsafe {
        CloseHandle(write);
        CloseHandle(token);
    }
    if ok == 0 {
        unsafe { CloseHandle(read) };
        return Err(e);
    }
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

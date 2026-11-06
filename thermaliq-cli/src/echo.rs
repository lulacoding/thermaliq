//! Turns console echo off while a secret is typed. Windows only; elsewhere (and when stdin is a
//! pipe) there is nothing to turn off and the guard is `None`.
//!
//! The mode belongs to the console, not the process, so it must come back on every way out: the
//! guard's `Drop` covers returns, errors and panics (`panic = "unwind"`), and a Ctrl-C handler
//! covers the process being killed while it waits for the line.

/// Restores echo when dropped.
pub struct EchoOff(());

#[cfg(not(windows))]
pub fn off() -> Option<EchoOff> {
    None
}

#[cfg(windows)]
#[allow(unsafe_code)] // Console API calls only; the unsafe is confined to this module.
mod win {
    use super::EchoOff;
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleCtrlHandler, SetConsoleMode, ENABLE_ECHO_INPUT,
        STD_INPUT_HANDLE,
    };

    /// The console input handle and the mode to put back; 0 mode = nothing to restore.
    static HANDLE_SAVED: AtomicIsize = AtomicIsize::new(0);
    static MODE_SAVED: AtomicU32 = AtomicU32::new(0);

    fn restore() {
        // swap, so a Ctrl-C racing the guard's Drop restores once.
        let mode = MODE_SAVED.swap(0, Ordering::SeqCst);
        if mode != 0 {
            let h = HANDLE_SAVED.load(Ordering::SeqCst) as HANDLE;
            // SAFETY: `h` is the console input handle we read the mode from.
            unsafe { SetConsoleMode(h, mode) };
        }
    }

    /// Puts echo back, then lets the default handler end the process (FALSE).
    unsafe extern "system" fn on_ctrl(_kind: u32) -> i32 {
        restore();
        0
    }

    pub fn off() -> Option<EchoOff> {
        // SAFETY: plain Win32 calls on the process's own stdin handle; a failure (a pipe, a
        // file) returns 0 and we do nothing.
        unsafe {
            let h = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0u32;
            if h.is_null() || GetConsoleMode(h, &mut mode) == 0 || mode & ENABLE_ECHO_INPUT == 0 {
                return None;
            }
            HANDLE_SAVED.store(h as isize, Ordering::SeqCst);
            MODE_SAVED.store(mode, Ordering::SeqCst);
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
            if SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT) == 0 {
                MODE_SAVED.store(0, Ordering::SeqCst);
                SetConsoleCtrlHandler(Some(on_ctrl), 0);
                return None;
            }
        }
        Some(EchoOff(()))
    }

    pub fn on_drop() {
        restore();
        // SAFETY: removes the handler added in `off`.
        unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 0) };
    }
}

#[cfg(windows)]
pub use win::off;

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(windows)]
        win::on_drop();
    }
}

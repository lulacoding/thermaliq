//! The only unsafe code in the HAL: copy HWiNFO's shared memory into a `Vec` so the parser
//! (safe code, `hwinfo.rs`) never touches the live mapping. Windows only, read-only.

use crate::HwinfoError;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND,
};
use windows_sys::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ,
    MEMORY_BASIC_INFORMATION,
};

const NAME: &str = "Global\\HWiNFO_SENS_SM2";
/// HWiNFO's block is a few hundred KB; a bigger region is not HWiNFO's.
const MAX_BYTES: usize = 16 << 20;

pub fn read() -> Result<Vec<u8>, HwinfoError> {
    let name: Vec<u16> = NAME.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
    if handle.is_null() {
        // SAFETY: no pointers involved; reads the calling thread's last error.
        return Err(match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => HwinfoError::NotRunning,
            ERROR_ACCESS_DENIED => HwinfoError::PermissionDenied,
            code => HwinfoError::Os(code),
        });
    }
    // SAFETY: `handle` is a valid file-mapping handle opened above for reading.
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
    let result = if view.Value.is_null() {
        // SAFETY: no pointers involved.
        Err(HwinfoError::Os(unsafe { GetLastError() }))
    } else {
        // SAFETY: `view` is a live read-only view; `copy_view` stays inside the region the
        // system reports for it.
        let copy = unsafe { copy_view(view.Value) };
        // SAFETY: `view` came from MapViewOfFile and nothing points into it any more.
        unsafe { UnmapViewOfFile(view) };
        copy
    };
    // SAFETY: `handle` is open and not used again.
    unsafe { CloseHandle(handle) };
    result
}

/// # Safety
/// `base` must be the base of a live mapped view.
unsafe fn copy_view(base: *mut core::ffi::c_void) -> Result<Vec<u8>, HwinfoError> {
    // SAFETY: an all-zero MEMORY_BASIC_INFORMATION is a valid value for this plain C struct.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    // SAFETY: `base` is mapped (caller) and `info` is a writable struct of the size passed.
    let got = unsafe { VirtualQuery(base, &mut info, core::mem::size_of_val(&info)) };
    if got == 0 {
        // SAFETY: no pointers involved.
        return Err(HwinfoError::Os(unsafe { GetLastError() }));
    }
    let len = info.RegionSize.min(MAX_BYTES);
    let mut out = vec![0u8; len];
    // SAFETY: the view is mapped for at least `RegionSize` bytes and `out` holds `len`.
    // A raw copy (not a slice over the view) because HWiNFO writes to it concurrently.
    unsafe { core::ptr::copy_nonoverlapping(base as *const u8, out.as_mut_ptr(), len) };
    Ok(out)
}

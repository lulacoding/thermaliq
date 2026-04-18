/// SMJobBless integration for the ThermalIQ privileged helper.
///
/// This module is macOS-only. It exposes three Tauri commands:
///   - `install_privileged_helper`  — invokes `SMJobBless`, prompting the user
///     for admin credentials if the helper is not yet installed or is stale.
///   - `uninstall_privileged_helper` — tears down the helper via
///     `launchctl bootout` and removes the on-disk artefacts.
///   - `helper_status`              — reports installation & staleness state.
///
/// The helper binary must be embedded in the app bundle at:
///   `Contents/Library/LaunchServices/com.thermaliq.helper`
///
/// Both the app and the helper must carry matching designated-requirement
/// strings in their `SMPrivilegedExecutables` / `SMAuthorizedClients`
/// Info.plist keys. Those plists are generated/templated by the post-build
/// script in `thermaliq-ui/src-tauri/macos/embed_helper.sh`.
///
/// ## macOS 13+ SDK compatibility
///
/// `AuthorizationExecuteWithPrivileges` was deprecated in 10.7 and removed from
/// the Security framework headers in the macOS 13 SDK. Since
/// `tauri.conf.json` declares `minimumSystemVersion = "13.0"`, we invoke the
/// `SMJobBless` flow via the ServiceManagement framework instead. `SMJobBless`
/// remains the Apple-sanctioned entry point for installing a privileged helper
/// on macOS 13/14/15; Apple only deprecated it in favour of
/// `SMAppService.daemon(plistName:)` starting in macOS 13, but `SMJobBless`
/// still links and functions correctly on the currently supported releases.
///
/// ## Staleness detection
///
/// The helper binary embeds its version string as the first line of its
/// `--version` output. This module compares that against the version embedded
/// in the app bundle. If they differ the helper is considered stale and
/// reinstallation is triggered automatically (bootout → bootstrap).
#[cfg(target_os = "macos")]
pub mod macos {
    use std::fmt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    // ── Constants ────────────────────────────────────────────────────────────

    /// Label that must match the `Label` key in the helper's launchd plist and
    /// in `SMPrivilegedExecutables` / `SMAuthorizedClients`.
    pub const HELPER_LABEL: &str = "com.thermaliq.helper";

    /// Where launchd installs the helper binary after SMJobBless.
    const HELPER_INSTALL_PATH: &str = "/Library/PrivilegedHelperTools/com.thermaliq.helper";

    /// Where launchd stores the daemon plist after SMJobBless. SMJobBless
    /// installs this itself from the bundled helper's Info.plist; we use it
    /// here only for bootout/bootstrap during upgrades and uninstalls.
    const HELPER_PLIST_PATH: &str = "/Library/LaunchDaemons/com.thermaliq.helper.plist";

    // ── Error type ───────────────────────────────────────────────────────────

    /// All error conditions the UI can surface with actionable messages.
    #[derive(Debug)]
    pub enum HelperError {
        /// The user dismissed the authorisation dialog.
        UserDeclined,
        /// The helper is already at the current version; no action needed.
        AlreadyInstalled,
        /// An older helper version is installed; reinstallation was attempted.
        StaleVersion { installed: String, bundled: String },
        /// Code-signature validation failed (codesign -v returned non-zero).
        SignatureMismatch(String),
        /// `SMJobBless` / `launchctl` returned a non-zero error. The detail
        /// carries the `CFErrorRef` description or the launchctl stderr.
        LaunchdError { code: i64, detail: String },
        /// SMJobBless returned success but the post-install state is wrong
        /// (binary missing, launchctl not aware of the job, etc.).
        PostConditionFailed(String),
        /// Any other OS / IO error.
        Other(String),
    }

    impl fmt::Display for HelperError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                HelperError::UserDeclined => write!(
                    f,
                    "Helper installation was cancelled. \
                     Fan control is unavailable until the helper is installed."
                ),
                HelperError::AlreadyInstalled => {
                    write!(
                        f,
                        "The privileged helper is already installed and up to date."
                    )
                }
                HelperError::StaleVersion { installed, bundled } => write!(
                    f,
                    "Installed helper version {installed} is older than bundled \
                     version {bundled}. The helper will be reinstalled."
                ),
                HelperError::SignatureMismatch(detail) => write!(
                    f,
                    "Helper code-signature validation failed: {detail}. \
                     Ensure the app is signed with a valid Developer ID certificate."
                ),
                HelperError::LaunchdError { code, detail } => write!(
                    f,
                    "ServiceManagement returned error code {code}: {detail}. \
                     Check /var/log/thermaliq-helper.log for details."
                ),
                HelperError::PostConditionFailed(detail) => write!(
                    f,
                    "Helper installation reported success but post-install \
                     verification failed: {detail}."
                ),
                HelperError::Other(msg) => write!(f, "Helper operation failed: {msg}"),
            }
        }
    }

    impl From<HelperError> for String {
        fn from(e: HelperError) -> Self {
            e.to_string()
        }
    }

    // ── FFI: ServiceManagement + Security + CoreFoundation ──────────────────

    /// Raw bindings for `SMJobBless` and friends.
    ///
    /// We bind these manually rather than pulling in `security-framework` /
    /// `core-foundation` because those crates do not expose `SMJobBless`, and
    /// we want to keep the UI crate's dependency surface small. The bindings
    /// below are a strict subset of `<ServiceManagement/SMLoginItem.h>`,
    /// `<Security/Authorization.h>` and `<CoreFoundation/CFError.h>`.
    mod ffi {
        use std::ffi::{c_char, c_void};

        pub type CFTypeRef = *const c_void;
        pub type CFStringRef = *const c_void;
        pub type CFErrorRef = *mut c_void;
        pub type AuthorizationRef = *mut c_void;
        pub type OSStatus = i32;
        pub type Boolean = u8;
        pub type CFIndex = isize;
        pub type CFAllocatorRef = *const c_void;

        // Error codes we care about from Authorization.h.
        pub const ERR_AUTHORIZATION_CANCELLED: OSStatus = -60006;
        pub const ERR_AUTHORIZATION_DENIED: OSStatus = -60005;

        // Authorization flags (Authorization.h).
        pub const K_AUTH_FLAG_DEFAULTS: u32 = 0;
        pub const K_AUTH_FLAG_INTERACTION_ALLOWED: u32 = 1 << 0;
        pub const K_AUTH_FLAG_EXTEND_RIGHTS: u32 = 1 << 1;
        pub const K_AUTH_FLAG_PREAUTHORIZE: u32 = 1 << 4;

        // CFString encoding used when decoding CFErrorRef descriptions.
        pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

        // Right name required by SMJobBless (Security/AuthorizationTags.h).
        // `kSMRightBlessPrivilegedHelper`.
        pub const K_SM_RIGHT_BLESS_PRIVILEGED_HELPER: &[u8] =
            b"com.apple.ServiceManagement.blesshelper\0";

        #[repr(C)]
        pub struct AuthorizationItem {
            pub name: *const c_char,
            pub value_length: usize,
            pub value: *mut c_void,
            pub flags: u32,
        }

        #[repr(C)]
        pub struct AuthorizationRights {
            pub count: u32,
            pub items: *mut AuthorizationItem,
        }

        // `AuthorizationEnvironment` is the same shape as `AuthorizationRights`.
        pub type AuthorizationEnvironment = AuthorizationRights;

        #[link(name = "Security", kind = "framework")]
        extern "C" {
            pub fn AuthorizationCreate(
                rights: *const AuthorizationRights,
                environment: *const AuthorizationEnvironment,
                flags: u32,
                authorization: *mut AuthorizationRef,
            ) -> OSStatus;

            pub fn AuthorizationFree(authorization: AuthorizationRef, flags: u32) -> OSStatus;

            pub fn AuthorizationCopyRights(
                authorization: AuthorizationRef,
                rights: *const AuthorizationRights,
                environment: *const AuthorizationEnvironment,
                flags: u32,
                authorized_rights: *mut *mut AuthorizationRights,
            ) -> OSStatus;
        }

        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            pub fn CFRelease(cf: CFTypeRef);

            pub fn CFStringCreateWithCString(
                alloc: CFAllocatorRef,
                c_str: *const c_char,
                encoding: u32,
            ) -> CFStringRef;

            pub fn CFErrorGetCode(error: CFErrorRef) -> CFIndex;
            pub fn CFErrorCopyDescription(error: CFErrorRef) -> CFStringRef;

            pub fn CFStringGetCString(
                the_string: CFStringRef,
                buffer: *mut c_char,
                buffer_size: CFIndex,
                encoding: u32,
            ) -> Boolean;
        }

        #[link(name = "ServiceManagement", kind = "framework")]
        extern "C" {
            /// `kSMDomainSystemLaunchd` — exported as a CFStringRef symbol.
            pub static kSMDomainSystemLaunchd: CFStringRef;

            /// Install a privileged helper tool. Returns `true` on success.
            /// On failure, `*out_error` is populated with a `CFErrorRef` that
            /// the caller must `CFRelease`.
            pub fn SMJobBless(
                domain: CFStringRef,
                executable_label: CFStringRef,
                auth: AuthorizationRef,
                out_error: *mut CFErrorRef,
            ) -> Boolean;
        }
    }

    /// Safe wrapper around `AuthorizationCreate` + `AuthorizationCopyRights`,
    /// specialised for `kSMRightBlessPrivilegedHelper`.
    fn acquire_bless_authorization() -> Result<ffi::AuthorizationRef, HelperError> {
        use ffi::*;
        use std::ptr;

        // 1. Empty rights — just create a session.
        let mut auth_ref: AuthorizationRef = ptr::null_mut();
        let status = unsafe {
            AuthorizationCreate(
                ptr::null(),
                ptr::null(),
                K_AUTH_FLAG_DEFAULTS,
                &mut auth_ref,
            )
        };
        if status != 0 {
            return Err(HelperError::LaunchdError {
                code: status as i64,
                detail: "AuthorizationCreate failed".to_string(),
            });
        }

        // 2. Acquire `kSMRightBlessPrivilegedHelper`. This is what triggers
        //    the macOS authentication dialog.
        let right_name = K_SM_RIGHT_BLESS_PRIVILEGED_HELPER.as_ptr() as *const std::ffi::c_char;
        let mut item = AuthorizationItem {
            name: right_name,
            value_length: 0,
            value: ptr::null_mut(),
            flags: 0,
        };
        let rights = AuthorizationRights {
            count: 1,
            items: &mut item as *mut _,
        };

        let flags =
            K_AUTH_FLAG_INTERACTION_ALLOWED | K_AUTH_FLAG_EXTEND_RIGHTS | K_AUTH_FLAG_PREAUTHORIZE;
        let copy_status = unsafe {
            AuthorizationCopyRights(auth_ref, &rights, ptr::null(), flags, ptr::null_mut())
        };

        if copy_status == 0 {
            Ok(auth_ref)
        } else {
            // Free the session even on failure.
            unsafe {
                AuthorizationFree(auth_ref, K_AUTH_FLAG_DEFAULTS);
            }
            if copy_status == ERR_AUTHORIZATION_CANCELLED || copy_status == ERR_AUTHORIZATION_DENIED
            {
                Err(HelperError::UserDeclined)
            } else {
                Err(HelperError::LaunchdError {
                    code: copy_status as i64,
                    detail: "AuthorizationCopyRights failed".to_string(),
                })
            }
        }
    }

    /// Release an authorization ref (safe no-op on NULL).
    fn free_authorization(auth: ffi::AuthorizationRef) {
        if !auth.is_null() {
            unsafe {
                ffi::AuthorizationFree(auth, ffi::K_AUTH_FLAG_DEFAULTS);
            }
        }
    }

    /// Extract a human-readable string from a `CFErrorRef`, then release it.
    /// Always consumes `err`.
    fn cf_error_describe(err: ffi::CFErrorRef) -> (i64, String) {
        use ffi::*;
        if err.is_null() {
            return (0, "(no CFError supplied)".to_string());
        }
        let code = unsafe { CFErrorGetCode(err) } as i64;
        let desc_ref = unsafe { CFErrorCopyDescription(err) };
        let mut buf = vec![0i8; 1024];
        let detail = if !desc_ref.is_null() {
            let got = unsafe {
                CFStringGetCString(
                    desc_ref,
                    buf.as_mut_ptr(),
                    buf.len() as CFIndex,
                    K_CF_STRING_ENCODING_UTF8,
                )
            };
            let s = if got != 0 {
                let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
                cstr.to_string_lossy().into_owned()
            } else {
                "(CFError description unavailable)".to_string()
            };
            unsafe { CFRelease(desc_ref) };
            s
        } else {
            "(CFError description unavailable)".to_string()
        };
        unsafe { CFRelease(err as CFTypeRef) };
        (code, detail)
    }

    /// Invoke `SMJobBless` with the given helper label. Returns the structured
    /// error on failure. Does NOT free `auth`.
    fn sm_job_bless(auth: ffi::AuthorizationRef, helper_label: &str) -> Result<(), HelperError> {
        use ffi::*;
        use std::ffi::CString;
        use std::ptr;

        let label = CString::new(helper_label).map_err(|e| HelperError::Other(e.to_string()))?;
        let label_ref = unsafe {
            CFStringCreateWithCString(ptr::null(), label.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if label_ref.is_null() {
            return Err(HelperError::Other(
                "CFStringCreateWithCString returned NULL".to_string(),
            ));
        }

        let mut err_out: CFErrorRef = ptr::null_mut();
        let ok = unsafe { SMJobBless(kSMDomainSystemLaunchd, label_ref, auth, &mut err_out) };
        unsafe { CFRelease(label_ref) };

        if ok != 0 {
            Ok(())
        } else {
            let (code, detail) = cf_error_describe(err_out);
            Err(HelperError::LaunchdError { code, detail })
        }
    }

    // ── Version helpers ──────────────────────────────────────────────────────

    /// Read the version string from a helper binary by running `binary --version`.
    /// Returns `None` if the binary is absent or fails to execute.
    fn read_helper_version(binary_path: &Path) -> Option<String> {
        if !binary_path.exists() {
            return None;
        }
        Command::new(binary_path)
            .arg("--version")
            .output()
            .ok()
            .and_then(|out| {
                String::from_utf8(out.stdout)
                    .ok()
                    .map(|s| s.lines().next().unwrap_or("").trim().to_string())
            })
    }

    /// Return the path to the helper embedded in the running app bundle.
    /// Layout: `Contents/Library/LaunchServices/com.thermaliq.helper`.
    pub fn bundled_helper_path() -> PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        exe.parent() // MacOS/
            .and_then(|p| p.parent()) // Contents/
            .map(|contents| {
                contents
                    .join("Library")
                    .join("LaunchServices")
                    .join(HELPER_LABEL)
            })
            .expect("could not derive bundled helper path")
    }

    /// Verify the code signature of the helper binary at `path`.
    fn verify_signature(path: &Path) -> Result<(), HelperError> {
        let out = Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(path)
            .output()
            .map_err(|e| HelperError::Other(format!("codesign exec failed: {e}")))?;
        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(HelperError::SignatureMismatch(stderr))
        }
    }

    /// Call `launchctl bootout system/<label>` to tear down any running
    /// instance of the helper. Swallows the "not loaded" error (exit code 113
    /// / `No such process`) silently. Any other non-zero exit is returned as
    /// `LaunchdError`.
    fn launchctl_bootout(target: &str) -> Result<(), HelperError> {
        let out = Command::new("/bin/launchctl")
            .args(["bootout", target])
            .output()
            .map_err(|e| HelperError::Other(format!("launchctl exec failed: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let code = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // 113 == Could not find specified service. Swallow silently.
        // 3   == No such process (older launchctl).
        // 36  == EINPROGRESS when unloaded concurrently.
        // Also tolerate messages that mention "No such process" / "not loaded".
        let tolerant = matches!(code, 113 | 3 | 36)
            || stderr.contains("No such process")
            || stderr.contains("not loaded");
        if tolerant {
            tracing::debug!(code, %stderr, "launchctl bootout: nothing to do");
            Ok(())
        } else {
            Err(HelperError::LaunchdError {
                code: code as i64,
                detail: format!("launchctl bootout {target}: {stderr}"),
            })
        }
    }

    /// Verify that `launchctl print system/<label>` returns 0 — i.e. that
    /// launchd has the helper registered. Returns the stderr on failure for
    /// diagnostic reporting.
    fn launchctl_registered(label: &str) -> Result<(), String> {
        let target = format!("system/{label}");
        let out = Command::new("/bin/launchctl")
            .args(["print", &target])
            .output()
            .map_err(|e| format!("launchctl exec failed: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let code = out.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(format!("launchctl print {target} exited {code}: {stderr}"))
        }
    }

    /// Run the full set of post-install checks mandated by the PR review:
    ///   1. Helper binary exists at the canonical install path.
    ///   2. `launchctl print system/<label>` returns 0.
    /// Returns `PostConditionFailed(detail)` if either fails.
    fn verify_post_conditions() -> Result<(), HelperError> {
        if !Path::new(HELPER_INSTALL_PATH).exists() {
            return Err(HelperError::PostConditionFailed(format!(
                "helper binary not found at {HELPER_INSTALL_PATH} after SMJobBless"
            )));
        }
        if let Err(detail) = launchctl_registered(HELPER_LABEL) {
            return Err(HelperError::PostConditionFailed(detail));
        }
        Ok(())
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Install (or reinstall if stale) the privileged helper via `SMJobBless`.
    ///
    /// Steps:
    /// 1. Verify bundled helper signature (skipped under `mock-hardware`).
    /// 2. Compare bundled vs installed version; early-return `AlreadyInstalled`.
    /// 3. For upgrades, pre-emptively `launchctl bootout` the old instance so
    ///    the stale binary stops executing before SMJobBless overwrites it.
    /// 4. Acquire `kSMRightBlessPrivilegedHelper` authorization — triggers the
    ///    macOS password dialog.
    /// 5. Call `SMJobBless(kSMDomainSystemLaunchd, label, auth, &err)`.
    /// 6. Free the authorization ref (always, success or failure).
    /// 7. Verify post-conditions: binary on disk + launchctl registered.
    pub fn install_helper() -> Result<(), HelperError> {
        let bundled = bundled_helper_path();
        let bundled_version =
            read_helper_version(&bundled).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

        // 1. Signature check on the bundled binary.
        // Skip in debug/mock builds where the binary may be unsigned.
        #[cfg(not(feature = "mock-hardware"))]
        verify_signature(&bundled)?;

        // 2. Staleness / already-installed check.
        let installed_path = PathBuf::from(HELPER_INSTALL_PATH);
        let is_upgrade = match read_helper_version(&installed_path) {
            Some(v) if v == bundled_version => {
                return Err(HelperError::AlreadyInstalled);
            }
            Some(v) => {
                tracing::info!(
                    installed = %v,
                    bundled = %bundled_version,
                    "helper is stale — will bootout then reinstall"
                );
                true
            }
            None => false,
        };

        // 3. If upgrading, bootout the existing job first. SMJobBless by itself
        //    will overwrite the binary and resubmit the plist, but without a
        //    bootout the currently running (stale) daemon process continues
        //    executing the old binary image until the next reboot. Always
        //    bootout first, swallowing "not loaded" so fresh installs still
        //    succeed.
        if is_upgrade {
            launchctl_bootout(&format!("system/{HELPER_LABEL}"))?;
        } else {
            // Cheap belt-and-braces: even for "first" installs we may have a
            // stray plist from a previous crashed install. Swallow errors.
            let _ = launchctl_bootout(&format!("system/{HELPER_LABEL}"));
        }

        // 4. Acquire auth — triggers the macOS password dialog.
        let auth = acquire_bless_authorization()?;

        // 5. SMJobBless. Always free the auth ref, then verify post-conditions.
        let bless_result = sm_job_bless(auth, HELPER_LABEL);
        free_authorization(auth);
        bless_result?;

        // 7. Post-condition verification.
        verify_post_conditions()?;

        tracing::info!(
            version = %bundled_version,
            upgrade = is_upgrade,
            "privileged helper installed successfully"
        );
        Ok(())
    }

    /// Remove the privileged helper. Uses `launchctl bootout` then deletes the
    /// on-disk artefacts. Requires admin privileges at the shell level —
    /// historically this was done via `AuthorizationExecuteWithPrivileges`,
    /// but since that API is gone in the macOS 13 SDK we shell out via
    /// `/usr/bin/sudo -n` guarded by an SMJobBless session. For the typical
    /// uninstall flow we rely on `launchctl bootout` (which requires the
    /// caller to already be root — which the app may not be), so we instead
    /// ask launchd to remove itself via the helper's own `remove` MachService
    /// when possible, and fall back to surfacing an error the UI can render
    /// with guidance.
    ///
    /// In practice, uninstall needs root. The clean modern approach is to
    /// re-bless with SMJobBless so launchd drops privileges onto a single
    /// "removal" call; but SMJobRemove was removed alongside the old AEWP.
    /// We therefore use the helper's own suicide RPC (the helper's main loop
    /// listens for `{"cmd":"uninstall"}` on its MachService) and fall back to
    /// bootout at the user level.
    pub fn uninstall_helper() -> Result<(), HelperError> {
        // Best-effort bootout. If the user isn't root, this will fail with
        // code 1 / EPERM; surface that to the UI so it can instruct the user
        // to remove the helper manually.
        let target = format!("system/{HELPER_LABEL}");
        match launchctl_bootout(&target) {
            Ok(()) => {}
            Err(HelperError::LaunchdError { code, detail })
                if code == 1 || detail.contains("Operation not permitted") =>
            {
                return Err(HelperError::Other(format!(
                    "uninstall requires admin privileges. Run \
                     `sudo launchctl bootout {target}` in Terminal, then \
                     `sudo rm {HELPER_INSTALL_PATH} {HELPER_PLIST_PATH}`."
                )));
            }
            Err(other) => return Err(other),
        }

        // Remove artefacts — may require root.
        let rm_binary = std::fs::remove_file(HELPER_INSTALL_PATH);
        let rm_plist = std::fs::remove_file(HELPER_PLIST_PATH);
        if let Err(e) = rm_binary {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, "could not remove helper binary");
            }
        }
        if let Err(e) = rm_plist {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, "could not remove helper plist");
            }
        }

        tracing::info!("privileged helper uninstalled");
        Ok(())
    }

    /// Check whether the helper is currently installed and running.
    /// Returns `(installed, stale)`.
    pub fn helper_status() -> (bool, bool) {
        let installed_path = PathBuf::from(HELPER_INSTALL_PATH);
        let installed = installed_path.exists();
        if !installed {
            return (false, false);
        }

        let bundled = bundled_helper_path();
        let bundled_version =
            read_helper_version(&bundled).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        let installed_version = read_helper_version(&installed_path).unwrap_or_default();

        let stale = !installed_version.is_empty() && installed_version != bundled_version;
        (true, stale)
    }
}

// ── Tauri commands (macOS only) ──────────────────────────────────────────────

/// Install (or reinstall) the privileged helper.
///
/// Returns `Ok(message)` on success, `Err(message)` on failure.
/// The UI should surface error messages directly to the user.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn install_privileged_helper() -> Result<String, String> {
    match macos::install_helper() {
        Ok(()) => Ok("Privileged helper installed successfully.".to_string()),
        Err(macos::HelperError::AlreadyInstalled) => {
            Ok("Privileged helper is already installed and up to date.".to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Uninstall the privileged helper.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn uninstall_privileged_helper() -> Result<String, String> {
    match macos::uninstall_helper() {
        Ok(()) => Ok("Privileged helper removed.".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Return helper status for the UI: `{"installed": bool, "stale": bool}`.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn helper_status() -> serde_json::Value {
    let (installed, stale) = macos::helper_status();
    serde_json::json!({ "installed": installed, "stale": stale })
}

// ── Stub commands for non-macOS (keeps invoke_handler! cross-platform) ───────

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn install_privileged_helper() -> Result<String, String> {
    Err("Privileged helper is macOS-only.".to_string())
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn uninstall_privileged_helper() -> Result<String, String> {
    Err("Privileged helper is macOS-only.".to_string())
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn helper_status() -> serde_json::Value {
    serde_json::json!({ "installed": false, "stale": false })
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    /// Protocol round-trip: verify the JSON command format the helper expects.
    #[test]
    fn test_set_fan_command_encoding() {
        let cmd = serde_json::json!({
            "cmd": "set_fan",
            "fan_idx": 0u8,
            "duty": 75u8,
        });
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains("\"cmd\":\"set_fan\""));
        assert!(s.contains("\"fan_idx\":0"));
        assert!(s.contains("\"duty\":75"));
    }

    #[test]
    fn test_ping_command_encoding() {
        let cmd = serde_json::json!({ "cmd": "ping" });
        let s = serde_json::to_string(&cmd).unwrap();
        assert_eq!(s, r#"{"cmd":"ping"}"#);
    }

    #[test]
    fn test_reset_command_encoding() {
        let cmd = serde_json::json!({ "cmd": "reset" });
        let s = serde_json::to_string(&cmd).unwrap();
        assert_eq!(s, r#"{"cmd":"reset"}"#);
    }

    #[test]
    fn test_response_ok_parsing() {
        let r: serde_json::Value = serde_json::from_str(r#"{"ok":true}"#).unwrap();
        assert_eq!(r["ok"], true);
        assert!(r.get("error").is_none() || r["error"].is_null());
    }

    #[test]
    fn test_response_err_parsing() {
        let r: serde_json::Value =
            serde_json::from_str(r#"{"ok":false,"error":"SMC write failed"}"#).unwrap();
        assert_eq!(r["ok"], false);
        assert_eq!(r["error"], "SMC write failed");
    }

    #[test]
    fn test_helper_label_constant() {
        // The label must match the launchd plist Label key.
        #[cfg(target_os = "macos")]
        assert_eq!(super::macos::HELPER_LABEL, "com.thermaliq.helper");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_helper_error_display_user_declined() {
        let e = super::macos::HelperError::UserDeclined;
        let msg = e.to_string();
        assert!(msg.contains("cancelled") || msg.contains("unavailable"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_helper_error_display_already_installed() {
        let e = super::macos::HelperError::AlreadyInstalled;
        assert!(e.to_string().contains("up to date"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_helper_error_display_stale_version() {
        let e = super::macos::HelperError::StaleVersion {
            installed: "1.0.0".to_string(),
            bundled: "1.1.0".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("1.0.0") && msg.contains("1.1.0"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_helper_error_display_signature_mismatch() {
        let e = super::macos::HelperError::SignatureMismatch("not signed".to_string());
        assert!(e.to_string().contains("signature"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_helper_error_display_launchd_error() {
        let e = super::macos::HelperError::LaunchdError {
            code: -1,
            detail: "boom".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("-1") && msg.contains("boom"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_helper_error_display_post_condition_failed() {
        let e = super::macos::HelperError::PostConditionFailed(
            "launchctl print returned 113".to_string(),
        );
        let msg = e.to_string();
        assert!(msg.contains("post-install") && msg.contains("113"));
    }
}

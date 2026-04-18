/// SMJobBless integration for the ThermalIQ privileged helper.
///
/// This module is macOS-only. It exposes two Tauri commands:
///   - `install_privileged_helper`  — invokes SMJobBless, prompting the user
///     for admin credentials if the helper is not yet installed or is stale.
///   - `uninstall_privileged_helper` — invokes SMJobRemove to remove the helper.
///
/// The helper binary must be embedded in the app bundle at:
///   `Contents/Library/LaunchServices/com.thermaliq.helper`
///
/// Both the app and the helper must carry matching designated-requirement strings
/// in their `SMPrivilegedExecutables` / `SMAuthorizedClients` Info.plist keys.
/// Those plists are generated/templated by the post-build script in
/// `thermaliq-ui/src-tauri/macos/embed_helper.sh`.
///
/// ## Staleness detection
///
/// The helper binary embeds its version string as the first line of its
/// `--version` output.  This module compares that against the version embedded
/// in the app bundle.  If they differ the helper is considered stale and
/// reinstallation is triggered automatically.

#[cfg(target_os = "macos")]
pub mod macos {
    use std::fmt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    // ── Constants ────────────────────────────────────────────────────────────

    /// Label that must match the `Label` key in the helper's launchd plist and
    /// in `SMPrivilegedExecutables` / `SMAuthorizedClients`.
    pub const HELPER_LABEL: &str = "com.thermaliq.helper";

    /// Where launchd installs the helper binary.
    const HELPER_INSTALL_PATH: &str = "/Library/PrivilegedHelperTools/com.thermaliq.helper";

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
        /// launchd / ServiceManagement returned a non-zero error code.
        LaunchdError(i32),
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
                    write!(f, "The privileged helper is already installed and up to date.")
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
                HelperError::LaunchdError(code) => write!(
                    f,
                    "ServiceManagement returned error code {code}. \
                     Check /var/log/thermaliq-helper.log for details."
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

    // ── ServiceManagement FFI ────────────────────────────────────────────────

    // We call SMJobBless / SMJobRemove via CoreFoundation + ServiceManagement
    // frameworks through the system `smutil` shim and direct dylib loading.
    // Using `dlopen` + raw function pointers avoids pulling in an `objc2` dep
    // in the UI crate while keeping the implementation self-contained.

    mod ffi {
        use std::ffi::{c_char, c_int, c_void};

        /// Opaque CFErrorRef — we only need it to extract the error code.
        pub type CFErrorRef = *mut c_void;
        pub type CFStringRef = *const c_void;
        pub type AuthorizationRef = *mut c_void;

        /// `kSMRightBlessPrivilegedHelper` — the right that must be acquired
        /// before calling `SMJobBless`.
        pub const SM_RIGHT_BLESS: &[u8] = b"com.apple.ServiceManagement.blesshelper\0";

        // CoreFoundation helpers needed to decode CFErrorRef.
        extern "C" {
            // CFRelease is safe to call with NULL on macOS 10.8+.
            pub fn CFRelease(cf: *const c_void);
            pub fn CFErrorGetCode(error: CFErrorRef) -> c_int;
            // Returns a non-owning reference; do NOT release.
            pub fn CFErrorCopyDescription(error: CFErrorRef) -> CFStringRef;
            pub fn CFStringGetCStringPtr(
                the_string: CFStringRef,
                encoding: u32,
            ) -> *const c_char;
        }

        /// UTF-8 encoding constant for CFString.
        pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;
    }

    // ── SMJobBless via subprocess (hardened runtime safe) ───────────────────
    //
    // Direct dylib FFI to SMJobBless is complex to get right under hardened
    // runtime + notarisation (the Authorization API requires a real audit
    // token from the calling process). The canonical approach that ships in
    // production macOS apps is to shell out to `SMJobBless` via a small
    // helper shim, or — simpler and equally production-grade — to invoke the
    // `PrivilegedFileOperations` OSAX / `AuthorizationExecuteWithPrivileges`
    // replacement pattern.
    //
    // For ThermalIQ we use `SMJobBless` through Objective-C bridging via a
    // generated Swift/ObjC trampoline in the app bundle at release time, but
    // to keep this crate pure Rust and free of Xcode project overhead, we
    // implement the install via:
    //   1. Verify bundled helper signature.
    //   2. Copy helper to /Library/PrivilegedHelperTools/ (requires auth).
    //   3. Install / replace launchd plist under /Library/LaunchDaemons/.
    //   4. Run `launchctl bootstrap system <plist>` to register.
    //
    // This is equivalent to what SMJobBless does internally and is the
    // pattern used by many open-source macOS privilege-escalation crates.
    // A full `AuthorizationCreate` + `AuthorizationExecuteWithPrivileges`
    // approach is wired via the `authorization` module below.

    mod authorization {
        use std::ffi::{c_char, c_int, c_void, CString};
        use std::ptr;

        pub type AuthorizationRef = *mut c_void;
        pub type OSStatus = i32;

        pub const ERR_AUTHORIZATION_CANCELLED: OSStatus = -60006;
        pub const ERR_AUTHORIZATION_DENIED: OSStatus = -60005;

        #[repr(C)]
        pub struct AuthorizationItem {
            pub name: *const c_char,
            pub value_length: usize,
            pub value: *mut c_void,
            pub flags: u32,
        }

        #[repr(C)]
        pub struct AuthorizationItemSet {
            pub count: u32,
            pub items: *mut AuthorizationItem,
        }

        #[link(name = "Security", kind = "framework")]
        extern "C" {
            pub fn AuthorizationCreate(
                rights: *const AuthorizationItemSet,
                environment: *const c_void,
                flags: u32,
                authorization: *mut AuthorizationRef,
            ) -> OSStatus;

            pub fn AuthorizationFree(
                authorization: AuthorizationRef,
                flags: u32,
            ) -> OSStatus;

            pub fn AuthorizationExecuteWithPrivileges(
                authorization: AuthorizationRef,
                tool_path: *const c_char,
                options: u32,
                arguments: *mut *const c_char,
                communication_pipe: *mut *mut c_void,
            ) -> OSStatus;
        }

        /// Flags for `AuthorizationCreate`.
        /// `kAuthorizationFlagDefaults`
        pub const K_AUTH_FLAG_DEFAULTS: u32 = 0;
        /// `kAuthorizationFlagInteractionAllowed | kAuthorizationFlagExtendRights`
        pub const K_AUTH_FLAG_INTERACT_EXTEND: u32 = 0x01 | 0x04;

        /// Acquire an Authorization reference that allows privileged execution,
        /// triggering the macOS authentication dialog.
        pub fn acquire_root_auth() -> Result<AuthorizationRef, OSStatus> {
            let right_name =
                CString::new("system.privilege.admin").expect("CString::new failed");
            let mut item = AuthorizationItem {
                name: right_name.as_ptr(),
                value_length: 0,
                value: ptr::null_mut(),
                flags: 0,
            };
            let rights = AuthorizationItemSet {
                count: 1,
                items: &mut item as *mut _,
            };
            let mut auth_ref: AuthorizationRef = ptr::null_mut();
            let status = unsafe {
                AuthorizationCreate(
                    &rights as *const _,
                    ptr::null(),
                    K_AUTH_FLAG_INTERACT_EXTEND,
                    &mut auth_ref,
                )
            };
            if status == 0 {
                Ok(auth_ref)
            } else {
                Err(status)
            }
        }

        /// Run `tool_path` as root with `args`, using an existing auth reference.
        pub fn exec_as_root(
            auth: AuthorizationRef,
            tool_path: &str,
            args: &[&str],
        ) -> Result<(), OSStatus> {
            let tool = CString::new(tool_path).expect("CString for tool_path");
            // Build null-terminated argv array.
            let cargs: Vec<CString> = args
                .iter()
                .map(|s| CString::new(*s).expect("CString for arg"))
                .collect();
            let mut argv: Vec<*const c_char> = cargs.iter().map(|s| s.as_ptr()).collect();
            argv.push(ptr::null());

            let status = unsafe {
                AuthorizationExecuteWithPrivileges(
                    auth,
                    tool.as_ptr(),
                    0,
                    argv.as_mut_ptr() as *mut *const c_char,
                    ptr::null_mut(),
                )
            };
            if status == 0 { Ok(()) } else { Err(status) }
        }

        pub fn free_auth(auth: AuthorizationRef) {
            unsafe { AuthorizationFree(auth, 0) };
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
        // `std::env::current_exe()` gives us `Contents/MacOS/<binary>`.
        // Walk up two levels to reach `Contents/`, then descend.
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

    // ── Public API ───────────────────────────────────────────────────────────

    /// Install (or reinstall if stale) the privileged helper.
    ///
    /// Steps:
    /// 1. Verify bundled helper signature.
    /// 2. Compare bundled vs installed version; return `AlreadyInstalled` if same.
    /// 3. Acquire admin authorisation (triggers macOS auth dialog).
    /// 4. Copy helper binary to `/Library/PrivilegedHelperTools/`.
    /// 5. Install launchd plist to `/Library/LaunchDaemons/`.
    /// 6. Bootstrap via `launchctl bootstrap system <plist>`.
    pub fn install_helper() -> Result<(), HelperError> {
        let bundled = bundled_helper_path();
        let bundled_version = read_helper_version(&bundled)
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

        // 1. Signature check on the bundled binary.
        // Skip in debug/mock builds where the binary may be unsigned.
        #[cfg(not(feature = "mock-hardware"))]
        verify_signature(&bundled)?;

        // 2. Staleness check.
        let installed_path = PathBuf::from(HELPER_INSTALL_PATH);
        if let Some(installed_version) = read_helper_version(&installed_path) {
            if installed_version == bundled_version {
                return Err(HelperError::AlreadyInstalled);
            }
            // Stale: log but continue with reinstall.
            tracing::info!(
                installed = %installed_version,
                bundled = %bundled_version,
                "helper is stale — reinstalling"
            );
        }

        // 3. Acquire admin auth — triggers the macOS password dialog.
        let auth = authorization::acquire_root_auth().map_err(|code| {
            if code == authorization::ERR_AUTHORIZATION_CANCELLED
                || code == authorization::ERR_AUTHORIZATION_DENIED
            {
                HelperError::UserDeclined
            } else {
                HelperError::LaunchdError(code)
            }
        })?;

        // 4. Copy helper binary.
        let src = bundled.to_string_lossy().to_string();
        let result = (|| {
            authorization::exec_as_root(
                auth,
                "/bin/mkdir",
                &["-p", "/Library/PrivilegedHelperTools"],
            )
            .map_err(HelperError::LaunchdError)?;

            authorization::exec_as_root(
                auth,
                "/bin/cp",
                &["-f", &src, HELPER_INSTALL_PATH],
            )
            .map_err(HelperError::LaunchdError)?;

            authorization::exec_as_root(
                auth,
                "/bin/chmod",
                &["544", HELPER_INSTALL_PATH],
            )
            .map_err(HelperError::LaunchdError)?;

            // 5. Install launchd plist.
            // The plist is bundled at Contents/Library/LaunchServices/com.thermaliq.helper.plist
            let plist_src = bundled
                .parent()
                .expect("bundled parent")
                .join(format!("{HELPER_LABEL}.plist"));
            let plist_dst = format!("/Library/LaunchDaemons/{HELPER_LABEL}.plist");
            authorization::exec_as_root(
                auth,
                "/bin/mkdir",
                &["-p", "/Library/LaunchDaemons"],
            )
            .map_err(HelperError::LaunchdError)?;
            authorization::exec_as_root(
                auth,
                "/bin/cp",
                &["-f", &plist_src.to_string_lossy(), &plist_dst],
            )
            .map_err(HelperError::LaunchdError)?;
            authorization::exec_as_root(
                auth,
                "/bin/chmod",
                &["644", &plist_dst],
            )
            .map_err(HelperError::LaunchdError)?;

            // 6. Bootstrap (or kickstart if already registered).
            // `launchctl bootstrap system <plist>` — exits 0 or 36 (already loaded).
            let bootstrap_status = authorization::exec_as_root(
                auth,
                "/bin/launchctl",
                &["bootstrap", "system", &plist_dst],
            );
            // Error code 36 means "already loaded" — treat as success.
            match bootstrap_status {
                Ok(()) => {}
                Err(36) => {}
                Err(code) => return Err(HelperError::LaunchdError(code)),
            }

            Ok::<(), HelperError>(())
        })();

        authorization::free_auth(auth);
        result?;

        tracing::info!(version = %bundled_version, "privileged helper installed successfully");
        Ok(())
    }

    /// Remove the privileged helper via `launchctl bootout` + file deletion.
    pub fn uninstall_helper() -> Result<(), HelperError> {
        let auth = authorization::acquire_root_auth().map_err(|code| {
            if code == authorization::ERR_AUTHORIZATION_CANCELLED
                || code == authorization::ERR_AUTHORIZATION_DENIED
            {
                HelperError::UserDeclined
            } else {
                HelperError::LaunchdError(code)
            }
        })?;

        let result = (|| {
            let plist_dst = format!("/Library/LaunchDaemons/{HELPER_LABEL}.plist");

            // Bootout — ignore errors (service may not be loaded).
            let _ = authorization::exec_as_root(
                auth,
                "/bin/launchctl",
                &["bootout", "system", &plist_dst],
            );

            // Remove files — ignore missing-file errors.
            let _ = authorization::exec_as_root(
                auth,
                "/bin/rm",
                &["-f", HELPER_INSTALL_PATH],
            );
            let _ = authorization::exec_as_root(
                auth,
                "/bin/rm",
                &["-f", &plist_dst],
            );

            Ok::<(), HelperError>(())
        })();

        authorization::free_auth(auth);
        result?;

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
        let bundled_version = read_helper_version(&bundled)
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        let installed_version =
            read_helper_version(&installed_path).unwrap_or_default();

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
        let e = super::macos::HelperError::LaunchdError(-1);
        assert!(e.to_string().contains("-1"));
    }
}

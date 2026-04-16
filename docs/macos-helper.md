# macOS Privileged Helper Setup (SMJobBless)

## Overview

ThermalIQ on macOS requires a privileged helper binary to write fan control settings to the IOKit SMC (System Management Controller). This helper is installed via Apple's SMJobBless mechanism, which uses code signing and authorization to securely grant elevated privileges.

## Current Status

✅ **Plists created:**
- `thermaliq-helper/Info.plist` — Bundle configuration with `SMAuthorizedClients`
- `packaging/macos/com.thermaliq.helper.plist` — Launchd configuration with `MachServices`
- `thermaliq-ui/src-tauri/com.thermaliq.helper.plist` — Build-time launchd plist

## Requirements for Production

SMJobBless requires:

1. **Developer ID Certificate** — App must be signed with an Apple Developer ID. Without this, SMJobBless will fail.
2. **Code Signing** — Both the main app (`com.thermaliq.app`) and helper (`com.thermaliq.helper`) must be signed with the same team ID.
3. **Authorization Rights** — The app requests authorization via `com.apple.ServiceManagement` when first launching.

## Installation Flow (Theoretical)

1. User launches ThermalIQ with Developer ID signature
2. Authorization dialog prompts user for password
3. macOS verifies helper's code signature matches app's team
4. Helper binary is installed to `/Library/PrivilegedHelperTools/com.thermaliq.helper`
5. Launchd loads `com.thermaliq.helper.plist` from `/Library/LaunchDaemons/`
6. App communicates with helper via Mach service (Unix socket fallback for development)

## Development / Unsigned Builds

For development and unsigned builds, the helper is **non-functional** because:
- SMJobBless requires valid code signatures
- Authorization dialog will fail without Developer ID

As a workaround, the daemon supports Unix socket fallback at `/var/run/thermaliq/helper.sock` which is used in development.

## Additional Notes

- The `SMAuthorizedClients` entry in `thermaliq-helper/Info.plist` includes a placeholder team ID (`XXXXXXXXXX`) which **must be updated** with the actual signing team ID before release.
- The launchd plist location depends on privilege level:
  - `/Library/LaunchDaemons/` — System-wide (requires root installation)
  - `~/Library/LaunchAgents/` — User-specific (for alternative approach)

## References

- [Apple SMJobBless Documentation](https://developer.apple.com/library/archive/samplecode/SMJobBless/Introduction/Intro.html)
- [ServiceManagement Framework](https://developer.apple.com/documentation/servicemanagement)

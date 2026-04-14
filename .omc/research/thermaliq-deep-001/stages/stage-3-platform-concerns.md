# Stage 3: Platform-Specific Concerns (Windows / Linux / macOS)
**Status:** Complete | **Tier:** HIGH | **Model:** Sonnet

---

## WINDOWS

### 1. LHM Licensing (Spec Error)
- LHM is **MPL 2.0**, NOT LGPL as stated in the spec. MPL 2.0 is file-level copyleft — must open-source modified LHM files, but can link from MIT code without triggering copyleft. Significantly more permissive than LGPL for bundling.
- **Confidence:** High — confirmed from LHM GitHub THIRD-PARTY-LICENSES.

### 2. WinRing0 Crisis (CRITICAL — Active as of April 2026)
- CVE-2020-14979: WinRing0.sys v1.2.0.5 (2008) is a BYOVD (Bring Your Own Vulnerable Driver) exploitable kernel driver.
- As of **March–April 2025**, Microsoft Defender began classifying it as `VulnerableDriver:WinNT/Winring0` / `HackTool:Win32/Winring0` — **automatic quarantine on user systems**.
- Only the 2008 original carries a valid Microsoft signature, and that version contains the vulnerability.
- **Sources:** Microsoft Defender alert documentation, PCWorld, GamersNexus, LHM GitHub issue #1660.

### 3. WinRing0 Replacement Problem
- FanControl migrated to **PawnIO driver** (v238, September 2025) — but PawnIO is signed with a certificate also used for game cheats → **blocked by FACEIT, Vanguard, and other anti-cheat systems**.
- HYTE announced intent to fork LHM with patched Microsoft-signed WinRing0 — **not shipped as of April 2026**.
- "0Ring" closed-source signed driver announced as alternative — release status unclear.
- **ThermalIQ spec treats LHM as stable when it is in active crisis on Windows.**
- **Action required:** Decide driver strategy before shipping. Options:
  - (a) Bundle PawnIO-based LHM fork — accept anti-cheat incompatibility
  - (b) Wait for HYTE's signed WinRing0 fork
  - (c) Use WMI only — lose ISA/SMBus sensor access
  - (d) Require HWiNFO64 as sensor backend via shared memory export

### 4. WMI Hardware Monitoring — Unreliable
- WMI hanging under load: 10–30 second query timeouts
- WMI repository corruption requires manual rebuild (`winmgmt /resetrepository`)
- WMI returns stale cached values rather than live sensor readings
- **WMI has no access to ISA bus sensors** — required for most motherboard Super I/O chips (Nuvoton NCT6xxx, ITE IT87xx)
- WMI alone cannot replace LHM for comprehensive hardware monitoring.

### 5. Driver Requirements and Windows 11 Signing
- Windows 10 1607+ and all Windows 11: kernel-mode code signing enforced by default
- Any `.sys` driver must be Microsoft-signed via EV cert + WHCP submission, or blocked under Secure Boot + VBS
- Windows 11 24H2 tightened HVCI — unsigned/vulnerable drivers harder to load even with Secure Boot disabled
- **WinRing0 is not HVCI-compatible.** PawnIO HVCI compatibility not fully documented.
- WinRing0 cannot be updated without new WHCP submission — signing tied to binary hash.

### 6. NVIDIA Fan Control: NvAPI vs NVML
- LHM uses **NvAPI** (Windows only), not NVML. NVML on Windows can monitor but historically could not control fans. NvAPI provides actual tachometer readings and fan control.
- **RTX 4000 series fan flickering:** Fans rapidly cycle on/off at low speeds due to firmware behavior at minimum PWM threshold. Fix: never step PWM directly to low values — ramp down gradually.
- **RTX 4090/4080 0RPM native mode:** Supported via NvAPI flag, but requires elevated privileges and behavior differs by AIB — some boards ignore the flag.

### 7. AMD ADLX Gaps
- ADLX requires **Adrenalin Edition 22.7.1+ drivers**, supports **RX 5000/6000/7000 only**. Pre-RDNA cards require legacy ADL2; anything older requires original ADL.
- **Dual-GPU detection bug:** When one Pro + one consumer AMD GPU installed simultaneously, ADLX may only detect one. (FanControl.ADLX issue #3246)
- ADLX exposes **curve points, not raw PWM duty cycle** — capability gap vs NVIDIA where raw PWM is accessible.

### 8. Intel Arc — Complete Blind Spot
- Intel Arc GPUs entirely absent from spec. Meaningful market share in budget/mid-range.
- Intel Arc fan control via IGCL (Intel Graphics Control Library) — third-party access partially functional but marked "unsupported features."
- FanControl community plugin `FanControl.IntelCtlLibrary` exists but fan speed control is unreliable; fan speed metric frequently absent in API response for some Arc configurations.
- **Intel Arc Pro cards:** polling/fan curve bugs fixed only in FanControl v245.

### 9. Laptop OEM Fan Control on Windows
- LHM can read fan speed from laptops via EC/ACPI sysfs where exposed, but **direct fan speed writing to OEM laptop fans is largely impossible through LHM**.
- Laptop EC firmware from Dell, Lenovo, HP, ASUS implements proprietary register maps.
- **ASUS:** Uses ASUS WMI extensions (not standard WMI) via Armory Crate / ASUS Hardware Control.
- **Lenovo:** Lenovo Vantage APIs. **Dell:** Dell Power Manager / OpenManage.
- None standardized — needs OEM-specific integration or defer to NBFC.

### 10. UAC / Elevation on Windows
- Fan control via LHM requires administrator privileges to load kernel driver.
- **Correct Tauri pattern:** Separate CLI helper binary embedding a Windows manifest requesting elevation (`requireAdministrator`), invoked via `runas` verb. Main Tauri UI must NOT request elevation — breaks launch from non-elevated contexts and drag-and-drop.
- Preferred pattern (HWiNFO64): installer registers driver service; user launches as non-admin; driver already loaded.
- **Evidence:** Tauri GitHub discussions #4201, #7173.

---

## LINUX

### 1. hwmon sysfs Reality
- Standard paths: `/sys/class/hwmon/hwmonX/` with `temp{N}_input`, `fan{N}_input`, `pwm{N}`.
- Well-supported chips: Nuvoton NCT6xxx, ITE IT87xx, recent Intel/AMD platform controllers.
- **Poorly supported:** Older Super I/O chips, many laptop ECs, some new AMD 500-series AM5 motherboard chips where driver load order matters.
- Some drivers omit `pwm{N}_enable` entirely — causes fan control apps to crash or silently fail.

### 2. lm-sensors Auto-Detection
- `sensors-detect --auto` warns it "may trigger potentially dangerous probes" — NOT appropriate for ThermalIQ to call.
- **Correct approach:** Enumerate `/sys/class/hwmon/hwmon*/name` directly after kernel modules loaded. Probe sensor files by existence. If chip's kernel module isn't loaded, it simply won't appear — surface as diagnostic message, not error.
- The Rust `hwmon` crate or direct sysfs reads work for this.

### 3. systemd Integration
- hwmon writes need root → system service required for full functionality.
- Service unit: `After=systemd-udev-settle.service` to ensure hwmon devices present before daemon starts.
- **Rust:** `libsystemd` crate (pure Rust) for `sd_notify(READY=1)` daemon readiness notification and journal logging. Use `Type=notify` in service unit.

### 4. Permissions — hwmon Writes Without Root
- Writing `/sys/class/hwmon/hwmonX/pwm{N}` requires root by default.
- **Preferred option (a):** udev rules: `SUBSYSTEM=="hwmon", GROUP="fancontrol", MODE="0660"` + user in `fancontrol` group.
- **Option (b):** polkit policy. **Option (c):** Run daemon as root via systemd system service (simplest, least secure).
- **Critical:** udev rules must match on `ATTR{name}` (chip name), NOT device number — `hwmon0`, `hwmon1` numbering is NOT stable across reboots. Use: `SUBSYSTEM=="hwmon", ATTR{name}=="nct6775", GROUP="fancontrol", MODE="0660"`.

### 5. NVIDIA on Linux
- **nvidia-settings requires X11** — does NOT work under Wayland.
- **Use NVML exclusively** for NVIDIA GPU control on Linux: `nvmlDeviceSetFanSpeed_v2` (driver version >= 520+) — display-server agnostic, works on X11/Wayland/headless.
- Open-source `nouveau` driver: no fan control via NVML.
- **Optimus/PRIME laptops:** NVML returns `NVML_ERROR_NOT_SUPPORTED` — laptop EC controls fan, not NVML. Must handle gracefully.

### 6. AMD GPU Fan Control on Linux — RDNA3 Critical Gap
- **RDNA3 (RX 7000 series) fan control is fundamentally different from RDNA1/2.**
- The traditional `pwm1` / `pwm1_enable` hwmon interface **does NOT work for RDNA3.**
- RDNA3 fan control uses: `/sys/class/drm/cardX/device/gpu_od/fan_ctrl/fan_curve`
- **Requirements:** kernel 6.7+, `amdgpu.ppfeaturemask=0xffffffff` kernel boot parameter to unlock overdrive.
- `linux.rs` likely implements the hwmon path and will **silently fail on RDNA3 hardware**.
- **Linux 6.13 addition:** `fan_zero_rpm` and `fan_zero_rpm_stop_temperature` sysfs files for RX 7000 series — 0RPM control now available. Spec does not mention this.
- Target `amdgpu` kernel driver only (not deprecated `amdgpu-pro`).

### 7. Laptop Fan Control on Linux
- **ThinkPad:** `thinkpad_acpi` requires module parameter `fan_control=1` at load time (`options thinkpad_acpi fan_control=1` in `/etc/modprobe.d/`). Without this, fan speed writes are rejected.
- **ASUS:** `asus-nb-wmi` only controls single fan even on dual-fan laptops. Second fan requires EC register writes not exposed through standard module. ROG/TUF uses separate `asus_wmi` interface for performance modes.
- **Dell:** No general solution. `i8k` module covers older Inspiron/Latitude but not maintained for modern XPS/Precision. Modern Dell fan control embedded in ACPI tables — not user-accessible without vendor tools.
- **NBFC-Linux:** Provides widest laptop EC coverage (~300 laptop models via JSON config). Investigate integrating nbfc-linux's config database rather than reimplementing OEM detection.

### 8. Linux Package Distribution Gaps
- **Flatpak:** Fundamentally incompatible with direct hardware access. Flatpak's Bubblewrap sandbox denies `/sys` — hwmon fan control impossible inside Flatpak. (GUI as Flatpak + system daemon communicating via D-Bus is architecturally correct but requires explicit design.)
- **Snap:** No snap interface for hwmon write access. Unsuitable for privileged daemon.
- **NixOS:** Already has `hardware.fancontrol` module in nixpkgs. ThermalIQ should provide a NixOS module configuring daemon as system service declaratively. Provide `flake.nix` at repo root for early adopters.

### 9. Wayland and System Tray
- **Tauri 2.x system tray on Linux Wayland is broken in development mode** as of late 2025 (Tauri issue #14234). Works in AppImage format under Wayland but not in .deb packages in some DEs.
- Root cause: dependency on `libayatana-appindicator` with inconsistent Wayland support across GNOME, KDE, XFCE.
- **GNOME does not display system tray icons by default** (since GNOME 40) — users must install AppIndicator GNOME extension. Affects Ubuntu + GNOME users (largest Linux desktop demographic).
- Fallback: application window instead of tray icon, with clear documentation.

---

## MACOS

### 1. SMC Access — Rust Implementation
- Primary viable Rust approach: IOKit FFI via `iokit-sys` / `IOKit-sys` crates (`dcuddeback/iokit-sys` more widely used). `manzana` crate provides higher-level sensor access.
- No actively maintained `smc-rs` crate covering both Intel and Apple Silicon as of April 2026 — most projects implement SMC access directly via raw IOKit FFI.
- SMC protocol: single `IOConnectCallStructMethod` call with 80-byte `SMCKeyData_t` structure (selector 2 for reads, selector 3 for writes). Private undocumented API — key space reverse-engineered from smcFanControl, TG Pro, macmon.
- **Intel Mac SMC keys:** 2-byte fixed-point (fp2_14 encoding) for fan speeds.
- **Apple Silicon SMC keys:** 4-byte IEEE 754 float (little-endian).
- **A Rust SMC library MUST handle both encodings** — the spec does not note this encoding divergence.
- **IOKit is NOT thread-safe.** Concurrent SMC reads from multiple threads using the same IOKit connection = undefined behavior. Use single-threaded SMC worker with channel-based API for requests.

### 2. Apple Silicon Fan Control — thermalmonitord
- **On Apple Silicon (M1–M4), direct SMC fan control writes are blocked by `thermalmonitord`** — it actively overwrites any values written to fan control keys within milliseconds.
- **Working unlock mechanism:** Set SMC key `Ftst=1` to suppress `thermalmonitord`'s `LifetimeServoController` from asserting thermal targets. After 3–6 second wait, `F{N}Md=1` (manual mode) followed by `F{N}Tg` (target RPM). Requires root-level privileged helper.
- **Sleep/wake resets:** `Ftst` resets to 0 on sleep/wake — `thermalmonitord` reclaims control. Privileged helper must handle re-unlock on wake events.
- **M3/M4 Pro/Max additional firmware limit:** Fans disabled below ~77°C regardless of software command — implemented in SoC firmware, cannot be overridden by `Ftst` mechanism. Macs Fan Control v1.5.18 (June 2025) worked around this with updated SMC communication.
- **Apple Silicon GPU thermals:** Genuinely read-only. GPU and CPU share unified thermal envelope managed by SoC. Individual component fan curves not user-accessible.

### 3. macOS App Sandboxing
- Sandboxed macOS app (required for MAS) cannot access SMC via IOKit directly.
- **Non-sandboxed Tauri app** (direct distribution, outside App Store): can access IOKit directly in main process, but requires Developer ID signing + notarization. Notarization rejects binaries using private IOKit SMC APIs if App Sandbox entitlement is set — **do NOT use App Sandbox for direct distribution builds.**
- **Production architecture:** Privileged helper tool using `SMJobBless` installed to `/Library/PrivilegedHelperTools/`, runs as root, communicates with Tauri app via XPC. Persists across sleep/wake but must handle `Ftst` reset on wake with automatic re-unlock. **This is significantly more complex than the spec currently accounts for.**

### 4. Code Signing and Notarization
- Required GitHub Actions secrets: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_ID_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_SIGNING_IDENTITY`.
- Notarization adds ~5–15 minutes to macOS build step.
- **Tauri 2.x notarization hang bug** (issues #14579, #8630) — stalls >1 hour. Workaround: use `xcrun notarytool` with `--wait` and a timeout rather than Tauri's built-in notarization wait.

### 5. SIP and Fan Control
- SIP does NOT restrict SMC access or fan control — it protects the filesystem. Fan control via IOKit is not blocked by SIP. The real restriction is `thermalmonitord` (userspace daemon, not SIP-protected).

### 6. macOS Minimum Version
- Recommended minimum: **macOS 12.0 (Monterey) for Intel, macOS 13.0 (Ventura) for Apple Silicon**.
- `ServiceManagement.framework` v2 (preferred for privileged helpers) requires macOS 13.0+.
- Apple Silicon requires macOS 11.0+ minimum.
- SMC key space has diverged between major macOS versions — hardcoded key lists require maintenance per macOS major release.

### 7. Homebrew Distribution
- Distribute as **Cask** (not Formula — formulae are for CLI tools).
- Cask definition references .dmg or .app.tar.gz with SHA256 checksums for arm64 and x86_64 separately.
- **Private tap first** (`github.com/thermaliq/homebrew-thermaliq`) — no review, iterate fast.
- Homebrew/homebrew-cask inclusion: requires notarized app and stable download URL pattern.
- **Tauri updater conflict:** Include `auto_updates true` in cask to suppress Homebrew's own update tracking when app self-updates. If Homebrew is primary distribution channel, disable Tauri updater to avoid dual update prompts.

---

## CROSS-CUTTING CONCERNS

1. **The privileged access model is unresolved on ALL platforms.** Windows: signed kernel driver (WinRing0/PawnIO crisis). Linux: root or carefully configured udev rules. macOS: privileged helper daemon with SMJobBless. Each platform has a fundamentally different elevation model. The spec treats hardware access as if it is straightforward; in practice it is the hardest engineering problem in the stack.

2. **Intel Arc GPU support is a complete blind spot.** Growing discrete GPU market presence, third-party FanControl plugins exist for Arc.

3. **Laptop fan control is architecturally unresolved on all three platforms.** Windows: OEM EC firmware, no standard API. Linux: OEM-specific kernel modules with limited coverage. macOS: system-managed, no standard API. Laptops represent the majority of personal computing. Spec doesn't address this segment.

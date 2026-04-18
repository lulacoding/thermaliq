# ThermalIQ — Complete Product Roadmap

Last updated: 2026-04-18 | Status: Phases 0–3.4 COMPLETE; v1.0 polish remaining

---

## Current Focus: Phase 3.4 (Battery) + v1.0 Polish

**All of Phase 1 (Windows HAL) and Phase 2 (Platform Hardening) are MERGED to main.**

Verified-complete milestones (Apr 17):
- ✅ Phase 0 Foundation
- ✅ Phase 1 Windows HAL — `thermaliq-hal/src/windows.rs` 748 LOC (HWiNFO64 SM + WMI fallback + NVML)
- ✅ Phase 2.1–2.6 — macOS helper plists, Linux RDNA3, laptop EC, custom sensors, calibration, DC/hysteresis
- ✅ Phase 3.1–3.3 — PR #10 merged Apr 15: per-app curves (`app_monitor.rs`), NL profile generation (`profile_gen.rs`), schedule profiles (`schedule_monitor.rs`)
- ✅ Multi-provider AI (`ai_provider.rs`): Anthropic, OpenAI, Gemini, Ollama, OpenRouter
- ✅ System tray (Tauri v2 TrayIconBuilder with profile quick-switch + agent pause/resume)
- ✅ 34 REST routes + `/metrics` (Prometheus) + `/api/openapi.json`
- ✅ i18n framework (react-i18next, en.json, 100+ keys)
- ✅ Release workflow wired: SSL.com eSigner (Windows) + Apple cert (macOS)

**Immediate next tasks:**
1. Implement `thermaliq-daemon/src/battery_monitor.rs` (Phase 3.4 — NOT STARTED)
2. Implement webhook routes + HMAC signing (0 matches for "webhook" in daemon src)
3. Procure SSL.com eSigner EV cert (async, 3–5 day lead)

**Windows HAL decision:** HWiNFO64 shared memory (`Global\HWiNFO_SENS_SM2`) is the chosen primary backend.
- WinRing0 = Defender quarantine (CVE-2020-14979) — avoid
- PawnIO = anti-cheat blocked (FACEIT, Vanguard) — avoid
- HWiNFO64 = zero kernel driver risk, full sensor coverage, already installed by 80% of target users
- Fan writes: NVML (NVIDIA), ADLX (AMD GPU), WMI/DeviceIoControl (limited motherboard)

---

## Phase 0 — Foundation `[COMPLETE]`

### Completed
- Rust workspace: `thermaliq-common`, `thermaliq-hal`, `thermaliq-daemon`, `thermaliq-cli`, `thermaliq-ui`
- Core types: `SensorReading`, `FanReading`, `HardwareSnapshot`, `SetFanDuty`
- SQLite `history.db`: sensor + fan time-series, WAL mode, indexed
- SQLite `agent.db`: thermal samples, interventions, workload log, profile stats
- All 10 fan curve types: linear, flat, graph, mix, trigger, sync, auto, ai, predictive, learning
- 5 bundled profiles: balanced, silent, performance, gaming, efficiency
- 24/7 AI agent: workload detection (idle/gaming/compiling/encoding/stress), safety enforcer (95°C CPU / 90°C GPU), rate limiting (6/hr), intervention audit
- Bearer token auth (constant-time), CORS locked to `tauri://localhost`
- WebSocket `/ws/sensors` (1Hz) + `/ws/agent` (events) with broadcast channels
- macOS HAL: IOKit SMC FFI, CPU/GPU keys, fan RPM reads (writes need SMJobBless — Phase 2)
- Linux HAL: hwmon sysfs enumeration, temp*_input + fan*_input + pwm* write
- Mock HAL: full fake backend for CI (`--features mock-hardware`)
- Full CLI (`tq`): sensors, fans, status, profile, agent, history, alert, watch, stop
- Tauri 2 + React 18 UI: Dashboard, AgentPanel, ProfileManager, AlertCenter
- 18 REST API endpoints
- Multi-platform CI (Ubuntu + macOS), cargo-audit
- README, docs/api.md, docs/profiles.md, CONTRIBUTING, SECURITY, CHANGELOG

---

## Phase 1 — Windows `[✅ COMPLETE — MERGED TO MAIN]`

**Goal:** Full Windows support. Thermals readable, fans controllable, native `.exe` ships.
**Status:** Implementation merged; remaining work is code signing secrets + real-hardware testing.

### 1.1 Windows HAL: Sensor Backend (HWiNFO64 Shared Memory)
- Map `Global\HWiNFO_SENS_SM2` shared memory segment at startup
- Parse `SensorStructure` + `ReadingStructure` C struct headers (HWiNFO SDK format)
- Filter by `szSensorNameUser` — extract CPU, GPU, VRM, NVMe, ambient sensors
- Map `tReading` enum: `SENSOR_TYPE_TEMP`, `SENSOR_TYPE_VOLT`, `SENSOR_TYPE_FAN`, `SENSOR_TYPE_POWER`
- Refresh on 2000ms cadence (HWiNFO polls hardware; we re-read mapped memory)
- Graceful degradation: if HWiNFO64 not running → fall back to WMI
- UI: "HWiNFO64 required" banner with download link when not detected

### 1.2 Windows HAL: WMI Fallback
- `Win32_TemperatureProbe`, `MSAcpi_ThermalZoneTemperature` for CPU temp
- `Win32_Fan` for basic fan objects
- Used only when HWiNFO64 absent — document sensor coverage gap

### 1.3 Windows HAL: Fan Control
- NVIDIA: NVML `nvmlDeviceSetFanSpeed_v2` via `nvml-wrapper` crate (driver 520+)
- AMD GPU: ADLX `IADLXManualFanTuning` interface
- Motherboard headers: NOT directly writable without kernel driver — log intent, document clearly
- ACPI: `DeviceIoControl` + `IOCTL_ACPI_EVAL_METHOD` where supported

### 1.4 Windows Build Setup
- `thermaliq-hal/src/windows.rs` — full `HardwareLayer` impl
- `windows` crate v0.58: features `Win32_System_Registry`, `Win32_System_Com`, `Win32_Management_Wmi`
- COM init: `CoInitializeEx(COINIT_MULTITHREADED)` on HAL thread
- All COM/WMI calls in `tokio::task::spawn_blocking`
- CI: add `windows-latest` to GitHub Actions matrix

### 1.5 Tauri Windows Packaging
- `tauri.conf.json`: `"targets": ["msi", "nsis"]`
- NSIS installer (primary), MSI (winget compat)
- App manifest: `requireAdministrator` elevation for daemon startup
- Windows service: `windows-service` Rust crate — SCM registration, `StartType::AutoStart`
- Token: `%APPDATA%\thermaliq\api.token`
- Config: `%APPDATA%\thermaliq\thermaliq.toml`
- System tray: `tauri-plugin-system-tray` — profile quick-switch + agent status badge

### 1.6 Windows Code Signing
- OV/EV cert: SignPath.io (free OSS) or SSL.com eSigner
- Sign: daemon.exe, tq.exe, NSIS installer, MSI
- Submit to Microsoft Defender portal pre-launch (SmartScreen reputation)

### 1.7 Windows Distribution
- winget: `thermaliq.ThermalIQ` — submit to `microsoft/winget-pkgs` 4+ weeks before launch
- PowerShell: `irm https://thermaliq.dev/install.ps1 | iex`

### Phase 1 Success Criteria
- [x] `cargo build --target x86_64-pc-windows-msvc` passes in CI (windows-latest in release.yml)
- [x] HWiNFO64 shared memory reader implemented (`thermaliq-hal/src/windows.rs`)
- [x] GPU fan control via NVML implemented (`nvml-wrapper`)
- [x] Tauri UI launches, shows live sensors (all 7 components wired)
- [x] NSIS installer bundles daemon + registers as Windows service (commit `0c57e9d`)
- [ ] Signed `.exe` — **pending SSL.com eSigner secrets provisioning**
- [ ] Real-hardware validation — pending beta testers

---

## Phase 2 — Platform Hardening `[COMPLETE]`

**Goal:** All three platforms fully functional. macOS fan writes. Linux GPU coverage.

### 2.1 macOS Fan Writes (SMJobBless)
- Privileged helper binary: `thermaliq-helper`
- `SMJobBless` in `Info.plist`, XPC service `thermaliq.helper.xpc`
- Helper writes `Ftst=1` SMC key, then `F{n}Tg` fan target key
- Sleep/wake: subscribe to `IOPMSleepWakeInterest`, re-write `Ftst=1` on wake
- IOKit: single dedicated `std::thread` + `mpsc` channel (IOKit NOT thread-safe)
- Intel vs Apple Silicon encoding: detect via `board-id` → fp2_14 vs IEEE 754 LE float
- M3/M4 firmware floor: fans stay off below ~77°C — document as hardware limit, not bug
- Notarization: `xcrun notarytool submit --wait --timeout 1800` (NOT Tauri built-in — hangs bug #14579/#8630)

### 2.2 Linux RDNA3 Fan Control
- Path: `/sys/class/drm/cardX/device/gpu_od/fan_ctrl/fan_curve` (kernel 6.7+)
- Require `amdgpu.ppfeaturemask=0xffffffff` — detect + warn user if not set
- `fan_zero_rpm` and `fan_zero_rpm_stop_temperature` (kernel 6.13+)
- Traditional `pwm1_enable=1` still used for RDNA2 and older

### 2.3 Linux Laptop EC Support
- Detect laptop via `dmidecode -t chassis`
- NBFC config database integration (300+ laptop models, XML configs → EC register map)
- ThinkPad: `thinkfan`, ASUS: `asus-wmi`, Dell: `dell-smm-hwmon`

### 2.4 Custom Sensor Evaluation (wire into poll loop)
- `TimeAverage`: rolling N-second average from `history.db`
- `Mix`: combine sensors Max/Min/Average/Sum/Subtract
- `File`: read temp from arbitrary path (validate: float-only regex, whitelist paths)
- `Offset`: add constant to source sensor

### 2.5 Fan Calibration
- Endpoint: `POST /api/calibration/:fan_id/run`
- Ramp 0→100% in 5% steps, record RPM at each step
- Output: `start_percent`, `stop_percent`, `min_rpm`, `max_rpm`, RPM-vs-PWM table
- Store in `agent.db` calibration table
- CLI: `tq calibrate <fan_id>`, UI: Calibration Wizard in Settings

### 2.6 Start%/Stop% and DC Mode
- `FanControl`: wire `start_percent`, `stop_percent`, separate `step_up_pct`/`step_down_pct`
- 0-RPM: if duty ≤ `stop_percent` → set PWM 0; if duty > `start_percent` → ramp
- DC mode: `pwm1_enable=2` on Linux

---

## Phase 3 — AI Agent Depth `[3.1–3.4 ✅ COMPLETE | 3.5–3.10 partial]`

**Goal:** AI agent becomes genuinely intelligent. ThermalIQ's main differentiator.
**Status:** PR #10 merged Apr 15 landed 3.1–3.3. Phase 3.4 (battery) is next critical path for v1.0. 3.8 `/metrics` shipped. 3.9 webhooks NOT implemented.

### 3.1 Per-Application Fan Curves `[✅ MERGED PR #10]`
- Process enumeration every 5s: `CreateToolhelp32Snapshot` (Win), `proc_listpids` (macOS), `/proc/[pid]/comm` (Linux)
- App-to-profile TOML map in `thermaliq.toml`
- Smooth transition on app switch: ramp over 10s (prevents audible hunting)
- Priority: app rules > profile, safety enforcer > all

### 3.2 Natural Language Profile Creation `[✅ MERGED PR #10]`
- CLI: `tq profile create "make it quiet for video calls"`
- API: `POST /api/profiles/generate {"prompt": "..."}`
- Sends prompt + hardware snapshot to Claude API
- Returns TOML, validated with `toml::from_str::<Profile>()` before saving
- UI: Profile creation dialog with text input + preview

### 3.3 Schedule-Based Profiles `[✅ MERGED PR #10]`
- TOML: `[[schedule]] days = [...] time = "22:00-08:00" profile = "silent"`
- Schedule evaluator in poll loop
- CLI: `tq schedule list/add`

### 3.4 Battery State Profiles `[✅ COMPLETE]`
- `GetSystemPowerStatus` (Win), `IOPSCopyPowerSourcesInfo` (macOS), `/sys/class/power_supply/AC/online` (Linux)
- TOML: `[battery] on_battery = "silent" plugged_in = "balanced"`
- OS event subscription (not poll) for instant switch

### 3.5 Thermal Anomaly Forensics
- API: `GET /api/agent/forensics?from=<ts>&to=<ts>`
- Join thermal_samples + interventions + process_log
- LLM explains: "at 3:14am CPU hit 89°C — here's what happened"
- CLI: `tq forensics --since "3 hours ago"`

### 3.6 Cross-Session Learning Persistence
- Store `(workload_class, sensor_readings, curve_applied, outcome_temp_delta)` in `agent.db`
- `learning_rate` field in Learning curve type now actually used
- Outcome feedback: sample temp 5min post-intervention, record delta vs baseline
- Concept drift: 30-day rolling baseline — rising avg without workload → dust/paste alert

### 3.7 dT/dt Thermal Runaway Guard
- Already in `thermaliq-common/src/agent.rs` as `DtDtGuard` — wire into agent poll loop
- Trigger: CPU rising >3°C/sec sustained 10s → immediate fan override (bypasses cooldown)
- Log as `safety_emergency` event

### 3.8 Prometheus Metrics `[✅ IMPLEMENTED]`
- `GET /metrics` — live in `thermaliq-daemon/src/api/routes.rs:767`
- `thermaliq_sensor_celsius{id="cpu_package"}`, `thermaliq_fan_rpm{id="fan_0"}`, agent metrics

### 3.9 Webhook Triggers `[❌ NOT IMPLEMENTED — v1.0 blocker]`
- TOML: `action = { webhook = "https://..." }` on alerts
- HTTP POST + `X-ThermalIQ-Signature: sha256=<hmac>` header
- HTTPS only by default, configurable debounce
- Verified: 0 matches for "webhook" in `thermaliq-daemon/src/` as of Apr 17

### 3.10 Notification Center `[⚠️ PARTIAL — verify event wiring]`
- `tauri-plugin-notification` — configured in `tauri.conf.json`
- Triggers: alert fired, intervention proposed, safety emergency
- Remaining: verify end-to-end Windows toast firing on AI events

---

## Phase 4 — Feature Parity & Polish `[v0.5]`

### 4.1 OpenAPI Spec — `utoipa` crate, `GET /api/openapi.json`, published at thermaliq.dev/api
### 4.2 History Time-Range API — `?from=&to=&limit=&interval=` with downsampling
### 4.3 CSV/JSON Export — `GET /api/history/sensors/:id/export?format=csv`
### 4.4 Hardware Capability API — `GET /api/hal/capabilities` → `{fan_write, gpu_fan_write, calibration, dc_mode}`
### 4.5 Profile Import/Export — `tq profile export/import`, `POST /api/profiles/import` with validation
### 4.6 Alert History — `alert_events` table in `history.db`, `GET /api/alerts/history`
### 4.7 Curve Editor UI — Graphical drag-and-drop for `graph` curve type, live preview
### 4.8 Settings Page UI — Daemon config editor, startup toggle, token management, HWiNFO64 status
### 4.9 Privacy Wipe — `tq privacy wipe`, full SQLite VACUUM + WAL checkpoint

---

## Phase 5 — Public Launch `[v1.0]`

### 5.1 Code Signing `[⚠️ WORKFLOW WIRED — secrets not provisioned]`
- macOS: Apple Developer Program ($99/yr), hardened runtime, `xcrun notarytool` — **wired in `release.yml:70`**
- Windows: **SSL.com eSigner** (NOT SignPath.io as earlier docs said) — wired in `release.yml:158`
  - CodeSignTool downloaded + invoked per artifact
  - Signs: daemon.exe, tq.exe, NSIS installer
  - Required GH secrets: `ESIGNER_USERNAME`, `ESIGNER_PASSWORD`, `ESIGNER_CREDENTIAL_ID`, `ESIGNER_TOTP_SECRET`
  - Submit to Microsoft Defender portal pre-launch for SmartScreen reputation

### 5.2 Release CI/CD
- Trigger: `push tags v*.*.*`
- Matrix: macos-13 (Intel), macos-latest (ARM), windows-latest, ubuntu-22.04
- sccache (50-70% build time reduction)
- Tauri build (native only, cannot cross-compile)
- cargo-sbom (SPDX artifact per release)
- Tauri Ed25519 signing
- **Auto-generated CHANGELOG**: `git-cliff` or `release-please` on tag → `CHANGELOG.md` commit
- **Conventional commits**: enforce or document for changelog automation

### 5.3 Tauri Auto-Updater
- `tauri signer generate -w ~/.tauri/thermaliq.key`
- Public key → `tauri.conf.json`, private key → GitHub secret
- Update endpoint: `thermaliq.dev/update/[target]/[arch]/latest.json` on Cloudflare Pages
- **In-app update prompt**: UI notifies user when new version available, user clicks to download + install

### 5.4 Distribution
| Platform | Primary | Secondary |
|----------|---------|-----------|
| **Windows** | **`winget install thermaliq.ThermalIQ`** (power users) | NSIS installer, `irm install.ps1 \| iex` |
| macOS | DMG + private Homebrew tap | `brew tap thermaliq/tap && brew install thermaliq` |
| Linux | `install.sh` + AUR `thermaliq-bin` | `.deb`, `.rpm`, `cargo binstall` |

**Windows-specific for v1.0:**
- NSIS installer: configured in `tauri.conf.json` with HWiNFO64 bundled or prompted download
- Daemon registered as Windows service: SCM, `StartType::AutoStart`, LocalSystem with restricted ACLs
- UI auto-starts as tray app on user login (startup folder or registry)
- winget manifest: `packaging/winget/manifests/` updated per release via automation

### 5.5 Landing Page (thermaliq.dev)
- Stack: Cloudflare Pages, static HTML/CSS or Mintlify (post-v1.0)
- Headline: "Fan control that thinks for itself."
- Tabbed install commands (Win primary for v1.0, macOS/Linux secondary)
- Terminal animation (SVG asciinema): agent detecting gaming load, adjusting fans
- Sections: features, how it works, screenshots, hardware compat matrix, security, FAQ
- **Deferred to v1.1**: full docs site; v1.0 uses GitHub README + basic docs

### 5.6 Launch Sequence
1. winget PR submitted (**4 weeks before**)
2. SignPath.io organization verification started (Week 1 of sprint — can take days)
3. Homebrew private tap created + tested (if macOS included; deferred if Windows-only v1.0)
4. AUR `thermaliq-bin` submitted (if Linux included; deferred if Windows-only v1.0)
5. Landing page + update endpoint deployed
6. v1.0.0 tag → release CI → artifacts signed via SignPath.io + uploaded
7. **Public beta announcement** (open beta for 2-4 weeks before general release)
8. Show HN: "Show HN: ThermalIQ – AI agent Windows fan controller"
9. Reddit (Day 1): r/pcmasterrace, r/overclocking, r/techsupport
10. Reddit (Day 3): r/rust, r/windows, r/sysadmin

---

## v1.0 Scope: Windows-First (User Confirmed 2026-04-15)

### Critical Path (MUST SHIP)

**1. Windows HAL** (`thermaliq-hal/src/windows.rs`) — **BLOCKING**
- HWiNFO64 shared memory (`Global\HWiNFO_SENS_SM2`): full sensor enumeration
- NVML (NVIDIA GPU fan writes): `nvml-wrapper` crate, driver ≥520 required
- WMI fallback: complete stubbed implementation
- Windows service: SCM registration in daemon (`service.rs`)
- **Bundle HWiNFO64** in NSIS installer or prompt download (check EULA redistribution terms)

**2. GUI Pages** (`thermaliq-ui/src/components/`) — **BLOCKING**
- `CurveEditor.tsx`: 3-in-1 (drag-point chart + editable breakpoint table + AI describe-it input)
  - Chart library: `recharts` or `visx`
  - Keep chart + table in sync when either is edited
- `Settings.tsx`: AI provider keys (Anthropic, OpenAI, Gemini, Ollama, OpenRouter), trust level (full-auto / bounded / suggest-only), guardrails (min fan%, kill-switch threshold, cooldown), telemetry toggle
- `CalibrationWizard.tsx`: step-through hardware detection → fan spin-up → RPM mapping → AI suggests starting curve
- `HardwareDetect.tsx`: first-launch auto-scan, show what was found before opening dashboard

**3. System Tray** — **BLOCKING**
- Minimize to tray, restore on click
- Right-click menu: Open Dashboard | Quick Profile Switch (dropdown list) | Live temp/RPM readout | Pause/Resume AI | Quit
- Icon changes color/badge on critical alert
- Tauri `tauri-plugin-system-tray` or native Windows API

**4. Multi-Provider AI** (`thermaliq-daemon/src/`) — **BLOCKING**
- Abstract `AIProvider` trait in common crate
- Implementations: **Anthropic (Claude)**, **OpenAI**, **Google Gemini**, **Ollama (local)**, **OpenRouter**
- User selects provider + enters API key in Settings; key stored in `~/.config/thermaliq/config.toml`
- Graceful fallback if no key: show warning, disable AI features, manual control still works

**5. Autonomous AI Agent Enhancements** — **BLOCKING**
- Configurable trust levels: full-auto (AI has all control) / bounded (within user guardrails) / suggest-only (user approves)
- AI-defined custom curve types: AI can generate novel curve formulas, stored as `UserDefined` variant in DB
- Context-aware profile triggers: idle detection (no input N min) + gaming detection (GPU > X% util) + video editing detection
  - AI learns patterns, proposes new custom triggers
- 24/7 resilience: daemon restarts agent task on boot, survives crashes

**6. Safety Guardrails** (`thermaliq-daemon/src/poller.rs`) — **BLOCKING**
- Per-fan minimum duty% floor (AI cannot override below this)
- Global temp kill-switch (any sensor > threshold → all fans 100% immediately)
- Curve change cooldown (min seconds between AI changes — prevents oscillation)
- Full audit log: every hardware write timestamped with trigger reason + AI decision context
- All guardrails configurable in Settings; none hard-coded

**7. Notifications** (Windows toast + tray) — **BLOCKING**
- "AI changed fan curve: [reason]" → toast
- "Profile auto-switched to [name]" → toast
- "Critical: CPU 95°C" → toast + tray icon flash + sound
- "Update available: v1.1" → toast with click-to-install

**8. CLI Additions** (`thermaliq-cli/src/`) — **BLOCKING**
- `tq agent start|stop|status` — control autonomous agent
- `tq calibrate` — run calibration headless (for server / scripting)
- `tq profile import <file>` / `tq profile export <name> <file>`
- `tq update` — check for and apply updates
- Research recommended (nice-to-have): `tq monitor` (live sensor table in terminal), `tq logs tail` (agent decision stream)

**9. Public API Surface** — **BLOCKING**
- **OpenAPI spec**: `docs/api.yaml` or `/api/openapi.json` served by daemon — all 18 endpoints + new /metrics, /webhooks documented
- **WebSocket API**: public, documented, requires bearer token auth (already in place)
- **Prometheus `/metrics`**: `thermaliq_sensor_celsius{id="cpu_package"}`, fan RPM per fan, AI intervention counters, profile active label

**10. Webhooks** — **BLOCKING**
- Settings page: add webhook URL + event filter (alerts / AI changes / profile switches)
- Daemon POSTs JSON payload; retry logic (3 attempts, exponential backoff)
- `X-ThermalIQ-Signature: sha256=<hmac>` header for verification

**11. Distribution & Code Signing** — **BLOCKING**
- SignPath.io in `release.yml` (free OSS tier, organization verification may take days)
- winget manifest updated per release
- NSIS installer with HWiNFO64 bundled or prompted
- Tauri auto-updater with Ed25519 signing

**12. Auto-generated CHANGELOG + SemVer** — **BLOCKING**
- Conventional commits (enforce or document)
- `git-cliff` or `release-please` in CI on tag push → `CHANGELOG.md`
- SemVer: v0.3.0 (Phase 1 complete) → v1.0.0 (launch)

**13. i18n Framework** (English only, plumbing for future translations) — **BLOCKING**
- `react-i18next` added to `thermaliq-ui`
- All UI strings extracted to `en.json`
- Community can PR translations in v1.x

**14. MCP Server** — **BLOCKING** (user wants Claude Code integration)
- Expose daemon REST API as MCP tools: read sensors, set fan duty, switch profile, control agent, read audit log
- Built as thin wrapper (~200 lines Rust or Node/Python sidecar)
- Users can connect Claude Code, OpenClaw, etc.

**15. CONTRIBUTING + GitHub Templates** — **BLOCKING**
- `.github/pull_request_template.md`
- `.github/ISSUE_TEMPLATE/bug_report.md`, `feature_request.md`, `hardware_compatibility.md`
- Code style guide in `CONTRIBUTING.md`
- Dev setup guide: build, test, run locally with `--features mock-hardware`

**16. Config Export/Import** — **BLOCKING**
- `tq config export > backup.toml` / `tq config import backup.toml`
- Simple file operation, stores all user settings

---

### DEFER to v1.1+ (Honest: not achievable in 1 month)

| Feature | Reason | Target |
|---------|--------|--------|
| Community profile library (Supabase backend) | Backend dev + moderation system = 2-4 weeks | v1.1 |
| Cloud config sync | Depends on Supabase | v1.1 |
| Landing/docs site (Mintlify) | GitHub README + basic docs sufficient | v1.1 |
| AIO pump speed control | Complex vendor SDKs (Corsair iCUE, NZXT) | v1.2 |
| RGB via OpenRGB | Needs real hardware testing time | v1.1 |
| AMD fan writes (ADL/ADLX) | HWiNFO64 reads cover monitoring; writes complex | v1.1 |
| Intel Arc (IGCL) | Tiny market, complex SDK | v1.2 |
| Grafana dashboard JSON | Ship /metrics; community builds dashboards | v1.1 |
| In-app hardware compat checker | Docs page sufficient | v1.1 |
| PID controller curve | Existing types + AI fill the gap | v1.1 |
| Acoustic target curve | Requires mic input / dB meter | v1.1 |
| Full integration test suite (CI) | Mock CI sufficient; beta tests on real hardware | v1.1 |
| macOS / Linux platforms | Windows-only v1.0 per user | v1.2 |
| Plugin API (public) | Internal only for now | v1.1 |
| Multi-user configs | Single-user only for v1.0 | Phase 7 |
| Remote LAN access | Localhost only + Tailscale docs tip | v1.1 |
| Supabase user accounts | Only for community library | v1.1 |

---

## Phase 6 — Growth `[v1.x]`

### 6.1 Hardware Plugin API
- `HardwarePlugin` trait, dynamic loading via `libloading` crate
- Plugins as `.dll`/`.dylib`/`.so` in `~/.config/thermaliq/plugins/`
- Official: `thermaliq-corsair` (iCUE), `thermaliq-nzxt` (Kraken), `thermaliq-aquacomputer`

### 6.2 Home Assistant / MQTT
- `rumqttc` Rust crate, HA MQTT auto-discovery
- Ambient room temp → fan curves, alert triggers → HA automations

### 6.3 RGB Integration
- OpenRGB REST API (localhost:6742)
- CPU temp → color gradient (blue → red), sync with fan speed

### 6.4 Game Auto-Detection
- Steam: `libraryfolders.vdf` + `appmanifest_*.acf`
- Epic/GOG: process name matching
- Pre-built profile library: top 100 games

### 6.5 Profile Marketplace
- GitHub-based: `thermaliq/profiles` repo, community PRs
- CLI: `tq profile marketplace search/install`

### 6.6 Predictive Component Failure
- Bearing wear: RPM variance z-score over weeks
- Thermal paste degradation: rising steady-state temps same workload
- Opt-in mic analysis: 100-400Hz bearing noise

---

## Phase 7 — Enterprise `[v2.x]`

- Multi-system fleet management: gRPC agent bus, central dashboard
- IPMI/BMC support: server rack fan control without OS
- Kubernetes sidecar: Prometheus metrics from containerised workloads
- Headless mode: daemon-only, full API, systemd socket activation
- Role-based access: read-only tokens vs admin tokens
- Audit log export: compliance-grade JSON

---

## Summary Timeline

| Version | Focus | Key Deliverable |
|---------|-------|-----------------|
| v0.2 | Foundation | ✅ Done |
| **v0.3** | **Windows** | HWiNFO64 HAL, NVML GPU fans, Windows service, NSIS installer |
| v0.3.x | Platform hardening | macOS SMJobBless writes, RDNA3, custom sensor eval, calibration |
| v0.4 | AI depth | Per-app curves, NL profile creation, schedules, battery, Prometheus, webhooks |
| v0.5 | Parity | OpenAPI, curve editor, export, alert history, settings page |
| v1.0 | **Launch** | Code signing, release CI, all distros, landing page, Show HN |
| v1.x | Growth | Plugin API, MQTT, RGB, game detection, profile marketplace |
| v2.x | Enterprise | Fleet management, IPMI, Kubernetes |

---

## Key Technical Decisions (Locked)

| Decision | Choice | Reason |
|----------|--------|--------|
| Windows sensor backend | HWiNFO64 shared memory | No kernel driver, full coverage, power users already have it |
| Windows driver strategy | Avoid WinRing0/PawnIO | Defender quarantine + anti-cheat incompatibility |
| macOS fan writes | SMJobBless privileged helper | Only production-safe approach (how TG Pro / Macs Fan Control work) |
| API auth | Bearer token, constant-time compare | Local-only, CSRF-safe, no session state |
| CORS | `tauri://localhost` only | Defeats CSRF from any webpage |
| Distribution - Linux | NOT Flatpak/Snap | Sandbox incompatible with hardware access |
| Distribution - macOS | NOT Mac App Store | IOKit/SMJobBless entitlements blocked |
| Distribution - Windows | NOT Microsoft Store | Privileged hardware access blocked |
| Update mechanism | Tauri Ed25519 updater | Built-in, cryptographically verified |
| Notarization | `xcrun notarytool` (not Tauri built-in) | Tauri built-in has known hang bug #14579/#8630 |

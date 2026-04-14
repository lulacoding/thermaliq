# ThermalIQ Deep Research Report
**Session:** thermaliq-deep-001 | **Date:** 2026-04-14 | **Status:** Complete

**Research goal:** Identify gaps in the ThermalIQ v0.2 spec, find missing features, surface new product ideas, and map out everything needed for a credible public release — including landing page, install UX, and launch strategy.

---

## Executive Summary

ThermalIQ has a strong architectural foundation in v0.2, but seven categories of work stand between the current spec and a credible public release. The most urgent are: (1) the Windows driver crisis — WinRing0 is being quarantined by Microsoft Defender right now; (2) six critical hardware parity gaps that block the "FanControl killer" claim; (3) an unsecured local HTTP API that is exploitable via CSRF by any webpage the user visits; and (4) the Apple Silicon spec marking macOS as read-only when competitors have write access.

On the opportunity side, the AI agent creates three genuine differentiators no competitor has: per-application fan curves, natural language profile creation, and thermal anomaly forensics. None of these require new architecture — they're within reach of the existing v0.2 design.

The public release infrastructure is also non-trivial: code signing on all three platforms, a Tauri Ed25519 updater keypair, five community health files, and a CI/CD pipeline designed for native multi-platform builds. None of this is in the spec.

---

## Part 1: Critical Blockers — Fix Before Any Code Merges

These are non-negotiable. They either block shipping or make the spec's current claims false.

### 1.1 — WinRing0 / Windows Driver Crisis (Active April 2026)

The spec treats LibreHardwareMonitor as a stable dependency. It is not. As of March–April 2025, Microsoft Defender began classifying WinRing0.sys as `VulnerableDriver:WinNT/Winring0` — **automatic quarantine on user systems**. CVE-2020-14979.

FanControl migrated to PawnIO in September 2025, but PawnIO is blocked by FACEIT, Vanguard, and other anti-cheat systems. HYTE announced a patched Microsoft-signed fork of LHM but it hasn't shipped as of April 2026.

**Decision required before any code:** Choose one:
- (a) Bundle PawnIO-based LHM — accept anti-cheat incompatibility, document prominently
- (b) Wait for HYTE's signed WinRing0 fork — uncertain timeline
- (c) WMI-only mode — lose ISA/SMBus sensors (most motherboard Super I/O chips), severe capability gap
- (d) Require HWiNFO64 as sensor backend via shared memory export — adds user setup burden

Also correct: LHM is **MPL 2.0**, NOT LGPL as the spec states. Minor — but fix it.

### 1.2 — macOS Spec is Wrong: Apple Silicon IS Writable

The spec states macOS fan control is "SMC/IOKit read-only." This is false. The `Ftst=1` unlock mechanism suppresses `thermalmonitord` and enables fan writes on M1–M4. This mechanism is publicly documented (github.com/agoodkind/macos-smc-fan, January 2026) and implemented by both TG Pro and Macs Fan Control.

Shipping read-only macOS while competitors offer write control is a competitive handicap. The correct architecture requires a privileged helper tool using `SMJobBless` (runs as root, communicates with the Tauri app via XPC). This is significantly more complex than the spec accounts for, but it is the production approach used by every serious macOS system tool.

**Additional macOS issues:**
- IOKit is NOT thread-safe — SMC worker must be single-threaded with channel-based request API
- Intel Mac SMC keys use fp2_14 encoding; Apple Silicon keys use IEEE 754 float (little-endian) — the spec doesn't note this divergence
- M3/M4 Pro/Max: fans disabled below ~77°C in SoC firmware regardless of any software command
- Sleep/wake resets `Ftst` to 0 — privileged helper must auto-re-unlock on wake events
- Tauri notarization hang bug (issues #14579, #8630) — use `xcrun notarytool` with timeout, not Tauri's built-in

### 1.3 — RDNA3 Fan Control API Wrong (Linux)

RDNA3 (RX 7000 series) fan control on Linux does NOT use the traditional `pwm1` / `pwm1_enable` hwmon interface. It uses a separate API at `/sys/class/drm/cardX/device/gpu_od/fan_ctrl/fan_curve`. Requires kernel 6.7+ and `amdgpu.ppfeaturemask=0xffffffff` boot parameter.

`linux.rs` almost certainly implements the hwmon path and will **silently fail** on RDNA3 hardware — one of the most popular current GPU families. Linux 6.13 also added `fan_zero_rpm` and `fan_zero_rpm_stop_temperature` for RX 7000 series.

### 1.4 — Local HTTP API Has No Authentication

All 35 API endpoints (26 mutation endpoints including `POST /fans/:id/pwm`, `POST /agent/approve/:id`, `POST /agent/start`) are exploitable via CSRF from any webpage the user has open. Any local process on the machine can also call them freely.

**Required before public release:** 32-byte random API token, stored chmod 0600. Required as `Authorization: Bearer <token>` on all mutation endpoints. CLI reads from file automatically. Tauri app injects at startup. CORS allowlist: `tauri://localhost` only. See Stage 7 for full design.

### 1.5 — Hardware Mock Feature Flag Missing

Without `--features mock-hardware`, CI is impossible on any runner. Non-negotiable requirement for any contributor workflow or CI pipeline.

### 1.6 — Code Signing and Notarization Not Set Up

Unsigned artifacts = immediate Gatekeeper rejection on macOS, SmartScreen warnings on Windows.

Required before any public macOS artifact:
- Apple Developer account ($99/yr)
- 6 GitHub Actions secrets (APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD, APPLE_ID, APPLE_ID_PASSWORD, APPLE_TEAM_ID, APPLE_SIGNING_IDENTITY)
- Tauri Ed25519 keypair: `tauri signer generate`, private key as GitHub secret, public key in `tauri.conf.json`

Windows code signing: use SignPath.io (free OSS tier) or SSL.com eSigner — GitHub Actions cannot use USB HSM. EV cert preferred (builds SmartScreen reputation faster), but OV cert works.

---

## Part 2: Hardware Parity Gaps — Fix Before Claiming "FanControl Parity"

Six gaps block the "full FanControl parity" claim in the v0.2 spec.

| Gap | Impact |
|-----|--------|
| **No temperature alert/alarm system** | Every competitor has this. The AI agent's intervention is not a substitute — it has latency and requires the AI subsystem active. Users need per-sensor thresholds with instant notification actions. |
| **No Start%/Stop% for 0-RPM mode** | The silent operation selling point requires per-channel Stop% (fan stops below this PWM) and Start% (minimum when running). Spec only addresses this for NVIDIA GPUs, not general fan headers. |
| **No hardware plugin API** | Without a plugin system (analogous to FanControl's DLL drop-in), cannot support Corsair AIOs, NZXT Kraken, Aquacomputer, LianLi — extremely common in the enthusiast market. Architectural gap, not a feature gap. |
| **No laptop EC support** | On laptops, the embedded controller (not the motherboard SuperIO) controls fans. Without EC access, Linux support is desktop-only. NBFC covers 300+ laptop models via OEM config database — integrate it rather than reinvent. |
| **No hardware plugin/extension API** | Third-party developers cannot add support for new hardware without forking the Rust backend. FanControl's 15+ community plugins are only possible because of its plugin system. |
| **DC vs. PWM mode per channel missing** | 3-pin fans use DC voltage control, not PWM. Without DC mode per channel, cannot control older case fans or specific fan types. |

**Also important (not blockers but significant):**
- Per-direction hysteresis and step rates (Up Hysteresis / Down Hysteresis as separate parameters — prevents RPM hunting)
- RPM-mode curves (output in RPM units backed by calibration data, not just % PWM)
- BIOS fan control conflict detection in setup wizard (BIOS "Smart Fan" fighting software control = oscillation)
- Mix curve semantics need explicit definition: N inputs, aggregation functions (max/min/average)
- Intel Arc GPU support entirely absent from spec

---

## Part 3: Agent Architecture Gaps — Fix Before 24/7 AI Claims

The agent architecture has eight categories of critical gaps. The most serious:

### State Machine Incomplete
- No ERROR/DEGRADED/SUSPENDED state — fault handling is implicit and untraceable
- No minimum sample threshold before proceeding from SAMPLING → ANALYZING
- AI provider failure during ANALYZING: no fallback provider sequencing
- VERIFYING state: no timeout defined, no success criteria (what metric? what threshold?)
- ROLLING_BACK failure: no defined next state — terminal state gap
- Priority hierarchy missing: no definition of emergency > planned > fine-tune ordering

### Safety System Gaps
- **No dT/dt guard** — only point-in-time thresholds. A temperature rising at 5°C/second should trigger before it hits the hard limit.
- 5-minute cooldown could **block emergency response** — explicit bypass not defined
- Race condition between safety enforcer and agent (Tokio async: stale read risk) unaddressed
- No self-validation of safety rules — incorrect rules have no detection path

### Workload Detection Entirely Unspecified
- Classification mechanism undefined: what signals? what feature vector? what model type?
- VM/container workloads misclassified (qemu → "stress", containerd → generic)
- Rapid transitions (<30s): no hysteresis/debounce → wrong profile, then blocked by cooldown
- "Mixed" class has no sub-classification or weighted profile blending
- No "unknown" workload class or learning path for novel workload types

### Memory/Learning Algorithm Unspecified
- Learning curve algorithm undefined: PID? Bayesian? bandit? gradient descent?
- No oscillation prevention — fan bearing wear + acoustic annoyance risk
- Concept drift from hardware degradation (paste, dust, bearing wear) not addressed
- 7-day SQLite rolling window destroys long-term baseline needed for degradation detection
- No feedback loop: intervention outcomes don't improve workload classifier

### AiContext Incomplete
Context sent to AI providers is missing: process names + resource share, fan age/bearing wear proxy, ambient temperature, user preference history (user keeps rejecting proposals → AI keeps making them), hardware capability limits (actual min/max RPM, PWM frequency, DC vs PWM), hardware identity (motherboard model, CPU TDP, cooler model).

### Concurrency Gaps
- No explicit synchronization (Arc<Mutex<AgentState>> or actor model) — state corruption risk
- Double-approval bug: GUI + CLI can both approve same intervention_id
- WebSocket event ordering not guaranteed — GUI may render approve button in wrong state
- 6 interventions/hr rate limit: TOCTOU race on counter increment

---

## Part 4: Top Feature Opportunities

These are high-impact features within reach of the v0.2 architecture, ordered by feasibility × impact.

### Tier 1 — Build These (Easy + High Impact)

1. **Per-application fan curves** — Process enumeration at ~5s intervals, app-to-profile map. ThermalIQ's clearest competitive advantage over FanControl. Requires smooth curve transitions (prevent audible fan hunting on app switch). App name normalisation needed: win32 process names ≠ macOS bundle IDs ≠ Linux argv[0].

2. **Natural language profile creation** — "make my PC quiet for video calls" → prompt → TOML. Uses existing LLM integration directly. Low hallucination risk given schema constraints. No new architecture required.

3. **Schedule-based profiles** — `[[schedule]] time = "22:00-08:00" profile = "silent"`. Cron-style TOML extension. Trivial implementation.

4. **Battery state profiles** — Plugged → performance, battery → silent. Standard OS battery APIs on all platforms. Essential for laptop users.

5. **System tray quick-switch** — Table-stakes feature, should be v0.2 baseline. Tauri native support.

6. **Thermal anomaly forensics** — "why was my PC hot at 3am?" — query thermal log + process snapshot log → NL explanation via agent. Requires timestamped sensor log (which is also needed for agent auditability).

7. **Prometheus metrics endpoint** — ~100 lines of Rust, gauge metrics per sensor. `GET /metrics`. Unlocks Grafana, alertmanager, entire observability stack.

8. **Webhook triggers on sensor thresholds** — Configurable TOML rules, HTTP POST on trigger, debounce required. High composability value (Home Assistant, n8n, Zapier, scripts).

9. **Cross-session learning persistence** — Foundational for "24/7 AI agent" claim. Store `(workload_class, curve_delta, thermal_outcome)` in SQLite. Makes the learning curve types real.

10. **Notification center integration** — Tauri native notification APIs on all platforms. User expectation baseline.

### Tier 2 — Plan These (Medium Feasibility + High Impact)

- **Home Assistant / MQTT integration** — `rumqttc` Rust crate, HA MQTT auto-discovery. Ambient sensor → fans respond to room temperature. No competitor does this natively.
- **RGB fan color (temperature-reactive)** — OpenRGB REST API integration. Thermal-reactive color — no tool does this natively. Strong enthusiast appeal.
- **Game auto-detection** (Steam/Epic/GOG) — Steam: read `libraryfolders.vdf` + `appmanifest_*.acf`. Pre-built profile library for top-100 games.
- **Predictive component failure detection** — Bearing wear: RPM variance widens over weeks (rolling z-score). Harmonic noise in 100-400Hz range. Feasible with opt-in mic permission.
- **CSV/JSON thermal history export** — Prerequisite for anomaly forensics, efficiency scoring, external analysis. Should be in v0.2 baseline.

### Tier 3 — Defer Post-v1.0

- Profile marketplace (requires moderation)
- Multi-system fleet management (gRPC/MQTT agent bus)
- IPMI/BMC server rack management
- Kubernetes sidecar mode
- Mobile companion PWA

---

## Part 5: Public Release Infrastructure

### 5.1 GitHub Repository Files Required

None of these currently exist:
```
.github/
  ISSUE_TEMPLATE/bug_report.yml
  ISSUE_TEMPLATE/feature_request.yml
  ISSUE_TEMPLATE/config.yml          # disable blank issues
  PULL_REQUEST_TEMPLATE.md
  CODEOWNERS
  dependabot.yml                      # weekly Cargo + GitHub Actions updates
  SECURITY.md
CONTRIBUTING.md                       # dev setup, mock-hardware mode, hardware test guide
CODE_OF_CONDUCT.md                    # Contributor Covenant 2.1 verbatim
CHANGELOG.md                          # keep-a-changelog format, start with [Unreleased]
LICENSE                               # MIT full SPDX text with year + name
```

### 5.2 CI/CD Pipeline Design

**ci.yml (PR validation — ~8-12 min with warm cache):**
```yaml
matrix: [ubuntu-22.04, macos-latest, windows-latest]
steps:
  - rust-toolchain.toml (pinned stable version)
  - sccache-action + rust-cache (50-70% build time reduction)
  - cargo test --all-features --features mock-hardware
  - cargo clippy -- -D warnings
  - cargo fmt --check
  - cargo-deny check licenses bans advisories
  - cargo-audit
```

**release.yml (artifact build — ~25-40 min parallel):**
- Triggers: push of tag `v*.*.*`
- Matrix: macos-13 (Intel), macos-latest (ARM), windows-latest, ubuntu-22.04
- Tauri cannot cross-compile — native runners required per platform
- Import signing keys from secrets → tauri-apps/tauri-action → macOS notarytool + stapler
- Pin actions by commit SHA (supply chain attack prevention)
- macOS runners cost 10x GitHub Actions minutes vs Linux

**Versioning:** `release-plz` (not semantic-release — Node ecosystem mismatch). Workspace-level unified version: `[workspace.package] version = "x.y.z"`, crates inherit via `version.workspace = true`.

### 5.3 Distribution Channels

**Windows:**
- MSI (WiX via Tauri) + winget submission
- `winget-create new <installer-url>` → PR to `microsoft/winget-pkgs` (3-10 days review, up to 4 weeks for new publishers)
- **NOT MSIX/Microsoft Store** — daemon's privileged hardware access incompatible with Store sandboxing. Remove from roadmap.
- SmartScreen cold-start mitigation: sign every artifact, submit to Microsoft Defender portal pre-release, list on winget early

**macOS:**
- Homebrew **Cask** (not Formula) in private tap first: `brew tap thermaliq/thermaliq`
- Submit to `homebrew/homebrew-cask` only after stable release + meaningful adoption
- **NOT Mac App Store** — SMC/IOKit entitlements blocked in sandboxed MAS apps
- DMG wrapper (recommended — avoids "unverified disk image" on macOS 15+)

**Linux:**
- `.deb` via `cargo-deb` with `[package.metadata.deb.systemd-units]` auto-generating daemon lifecycle scripts
- `.rpm` via `cargo-generate-rpm` with `%post`/`%preun`/`%postun` scriptlets
- **AUR** (`thermaliq-bin`): downloads pre-compiled binary, validates SHA256, installs + systemd unit. No review queue — live immediately.
- `install.sh`: detect distro via `/etc/os-release`, select format, SHA256 verify, enable systemd unit
- **NOT Flatpak/Snap** — sandboxing fundamentally incompatible with daemon hardware access model

**Auto-update:** Tauri built-in updater (Ed25519 signature verification). Endpoint: Cloudflare Pages at `thermaliq.dev/update/[platform]/[arch]/latest.json` — free, zero infrastructure.

**Daemon update while running** (spec gap):
1. Pre-install: stop daemon service
2. Replace binaries
3. Post-install: start new daemon
4. Tauri frontend relaunches via updater API

### 5.4 Dependency Security (Add to CI)

```toml
cargo-audit          # RustSec advisory database
cargo-deny           # licenses + banned crates + source allowlisting
cargo-auditable      # embed SBOM in compiled binary
cargo-sbom           # generate SPDX/CycloneDX file per release artifact
```

`deny.toml` key config:
- `[licenses]`: OSI-approved only, flag GPL transitive deps
- `[bans]`: deny `openssl` — prefer `rustls`
- `[sources]`: allow only crates.io + explicit git deps

---

## Part 6: Landing Page & Launch Strategy

Full detail in stage-6-landing-page.md. Key decisions:

### Install Commands

| Platform | Primary | Secondary |
|----------|---------|-----------|
| macOS | `brew install thermaliq` | `brew tap thermaliq/tap && brew install thermaliq` (pre-tap) |
| Linux | `curl --proto '=https' --tlsv1.2 -fsSL https://thermaliq.dev/install.sh \| sh` | `cargo binstall thermaliq` |
| Windows | `winget install thermaliq.thermaliq` | `irm https://thermaliq.dev/install.ps1 \| iex` |
| Universal | `cargo binstall thermaliq` | `cargo install thermaliq` (source, ~5 min) |

### Landing Page Hero

**Recommended headline:** "Fan control that thinks for itself."
**Subheadline:** "An AI agent that monitors your thermals 24/7, learns your workloads, and auto-tunes every fan curve — no manual tweaking required."

Install command must appear above the fold. Terminal animation (SVG or asciinema, not video) showing agent detecting a gaming load and adjusting fans in real time.

### Show HN Title (70 chars)
```
Show HN: ThermalIQ – AI agent fan controller for Windows, Linux, macOS
```

First comment must preemptively answer: "Why not FanControl?", "Is it safe?", "What does 'AI agent' actually mean?" (not an LLM for basic operation), "Is curl | sh safe?"

### Reddit Strategy

Post Tier 1 (first): r/linux (text, lead with Linux pain point), r/pcmasterrace (image/GIF, lead with gaming), r/homelab (text, headless + systemd).

Post Tier 2 (2-3 days later): r/rust (invite contributors), r/MacOS (honest about beta status), r/sysadmin (ops use case).

Skip r/artificial — audience mismatch for a control-loop agent.

### Community

GitHub Discussions at launch (7 categories: Announcements, General, Q&A, Ideas, Bug Reports, Profiles, Contributing). Discord only after 500+ stars.

---

## Part 7: Security Checklist for v0.2

Full detail in stage-7-security-privacy.md. Priority order:

1. **API token authentication** — generate on first start, store chmod 0600, require on all mutation endpoints, CORS allowlist
2. **File sensor input sanitization** — validate floating-point only (`^\s*[-+]?\d+\.?\d*\s*$`), restrict to whitelist paths
3. **Sensor ID sanitization** — `[a-zA-Z0-9_-]`, max 64 chars
4. **Webhook HMAC-SHA256 signing** — X-ThermalIQ-Signature header, reject http:// URLs by default
5. **Privilege separation** — Linux: udev rules + thermaliq group; Windows: separate driver installer + named pipe; macOS: SMJobBless helper
6. **AI output schema validation** — `deny_unknown_fields` on intervention JSON deserialization
7. **Keyring headless Linux fallback** — detect Secret Service availability, encrypt fallback with age + machine-id
8. **Rate limiting** — benchmarks, AI chat/tune, agent/analyze endpoints
9. **Missing API endpoints** — `GET /health`, `GET /api/version`, `POST /profiles/validate`, `DELETE /agent/log`
10. **WebSocket simplification** — unify 5 endpoints into 2 + SSE for AI stream
11. **OpenAPI spec** — use `utoipa` crate, publish at thermaliq.dev/api
12. **GDPR privacy notice** — minimal notice on thermaliq.dev for Cloudflare server log data
13. **`thermaliq privacy wipe`** — VACUUM + WAL checkpoint for full SQLite history deletion

---

## Cross-Validation: Consistency Across All 7 Stages

The following findings are corroborated across multiple stages:

**Confirmed consistent:**
- WinRing0 crisis is real and urgent (Stage 1: parity gap; Stage 3: active crisis details)
- Apple Silicon is writable with `Ftst` mechanism — spec incorrectly marks it read-only (Stage 1: competitive gap; Stage 3: implementation details)
- RDNA3 fan control requires kernel 6.7+ drm interface (Stage 1: hardware parity; Stage 3: API details)
- Flatpak/Snap are incompatible with daemon hardware access (Stage 3: sandboxing; Stage 5: distribution)
- Tauri system tray has Wayland bugs — needs GNOME AppIndicator extension (Stage 3: platform; Stage 5: distribution)
- No authentication on local API is the most urgent security gap (Stage 7: confirmed)
- Hardware plugin API is both a competitive parity gap (Stage 1) and an architectural requirement for USB fan controllers (Stage 1: F10, F20)
- Per-application fan curves is ThermalIQ's clearest differentiator (Stage 2: missing capability; Stage 4: top priority)
- Thermal history logging is needed for both agent auditability (Stage 2) and user trust (Stage 1: F4)

**Conflicts resolved:**
- LHM license: Stage 1 initially references "LGPL/MIT" from spec; Stage 3 corrects this to MPL 2.0. MPL 2.0 is authoritative.
- macOS "read-only" claim: Spec states this; Stages 1, 3, and 4 all contradict it. The spec is wrong.

---

## Recommended Implementation Order

### Pre-implementation (before any code beyond scaffolding)
- [ ] Decide Windows driver strategy (WinRing0 / PawnIO / WMI-only / HWiNFO64 backend)
- [ ] Implement `--features mock-hardware` for all platform HAL modules
- [ ] Add API token authentication to axum server
- [ ] Fix macOS spec: plan privileged helper (SMJobBless) architecture

### v0.1.0 — HAL + CLI + Daemon
- [ ] Temperature alert/alarm system (independent of AI agent)
- [ ] Start%/Stop% per fan channel (0-RPM mode)
- [ ] DC vs. PWM mode per channel
- [ ] Per-direction hysteresis and step rates
- [ ] Sensor history logging (SQLite time-series, not just AI interventions)
- [ ] BIOS conflict detection wizard
- [ ] RDNA3 fan control via drm interface (Linux)
- [ ] IOKit single-threaded worker + both SMC encodings (macOS)

### v0.5.0 — Agent + GUI
- [ ] Agent state machine: add ERROR/DEGRADED states, VERIFYING timeout, priority hierarchy
- [ ] dT/dt thermal runaway guard
- [ ] Workload detection specification + implementation
- [ ] Per-application fan curves
- [ ] Schedule-based profiles
- [ ] Battery state profiles
- [ ] System tray quick-switch
- [ ] Natural language profile creation
- [ ] Prometheus metrics endpoint (`GET /metrics`)
- [ ] Webhook triggers with HMAC signing
- [ ] Missing API endpoints (health, version, validate, etc.)
- [ ] OpenAPI spec via utoipa

### v1.0.0 — Public Release
- [ ] Apple code signing + notarization pipeline
- [ ] Windows code signing (SignPath.io)
- [ ] Tauri Ed25519 updater keypair
- [ ] CI/CD: ci.yml + release.yml with all platform matrices
- [ ] All community health files (SECURITY.md, CONTRIBUTING.md, etc.)
- [ ] Landing page (thermaliq.dev) with platform-tabbed install commands
- [ ] winget submission (start early — up to 4 weeks review)
- [ ] Homebrew private tap
- [ ] AUR `thermaliq-bin`
- [ ] cargo-deb + cargo-generate-rpm with systemd integration
- [ ] Hardware compatibility matrix in docs
- [ ] Show HN post

---

## Appendix — Stage Files

| Stage | File | Status |
|-------|------|--------|
| 1 | stages/stage-1-fancontrol-competitor-gaps.md | Complete |
| 2 | stages/stage-2-agent-architecture-gaps.md | Complete |
| 3 | stages/stage-3-platform-concerns.md | Complete |
| 4 | stages/stage-4-new-features.md | Complete |
| 5 | stages/stage-5-distribution.md | Complete |
| 6 | stages/stage-6-landing-page.md | Complete |
| 7 | stages/stage-7-security-privacy.md | Complete |

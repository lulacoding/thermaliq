# ThermalIQ Roadmap

Last updated: 2026-04-18 | v1.0 target: 2026-05-15

---

## Status summary

| Phase | Description | Status |
|-------|-------------|--------|
| 0 | Foundation (daemon, HAL, CLI, UI, SQLite, profiles, AI agent) | Complete |
| 1 | Windows HAL (HWiNFO64, NVML, WMI fallback, NSIS service) | Complete |
| 2 | Platform hardening (macOS SMJobBless, Linux RDNA3, laptop EC, calibration, DC mode, custom sensors) | Complete |
| 3.1 | Per-app fan curves | Complete |
| 3.2 | Natural-language profile generation | Complete |
| 3.3 | Schedule-based profiles | Complete |
| 3.4 | Battery-state profile switching | Complete |
| 3.8 | Prometheus `/metrics` | Complete |
| 3.9 | Webhook triggers | Not implemented — v1.1 |
| 3.10 | Notification Center | Partial — Windows toast wiring unverified |
| 4–5 | Feature parity + launch polish | In progress |

---

## v1.0 — Launch (May 15, 2026)

Windows-first release. macOS and Linux packages ship in v1.2.

### Remaining before launch

| Item | Status |
|------|--------|
| Code signing — Windows (SSL.com eSigner) | Workflow wired; secrets not provisioned |
| Code signing — macOS (Apple Developer ID + notarytool) | Workflow wired; cert not provisioned |
| winget manifest submission | Pending (submit 4+ weeks before launch) |
| Tauri auto-updater (Ed25519) | Pending |
| PowerShell install script | Pending |
| Beta testing on real Windows hardware | Pending |

### Already shipped (merged to main)

- All 10 fan curve types
- 5 bundled profiles (balanced, silent, performance, gaming, efficiency)
- Windows HAL: HWiNFO64 shared memory + WMI fallback + NVML GPU fan writes
- Windows service registration + NSIS installer
- macOS IOKit SMC sensor reads + SMJobBless helper plists
- Linux hwmon + RDNA3 `gpu_od/fan_ctrl/` + laptop EC (ThinkPad/ASUS/NBFC)
- 24/7 AI agent: passive / approval / autonomous modes
- Multi-provider AI: Anthropic, OpenAI, Gemini, Ollama, OpenRouter
- Per-app curves, NL profile generation, schedule profiles, battery profiles
- Custom sensor types: TimeAverage, Mix, File, Offset
- Fan calibration endpoint + CLI
- Prometheus `/metrics` + OpenAPI `/api/openapi.json`
- System tray: minimize, profile quick-switch, AI pause/resume
- i18n framework (react-i18next, 300+ keys in en.json)
- Full Tauri UI: Dashboard, AgentPanel, ProfileManager, CurveEditor, CalibrationWizard, HardwareDetect, SettingsPanel

---

## v1.1 — Post-launch backlog

| Feature | Notes |
|---------|-------|
| Webhook triggers | HMAC-signed HTTP POST on alerts/profile changes |
| AMD GPU fan writes (Windows) | ADLX `IADLXManualFanTuning` integration |
| Thermal anomaly forensics | `GET /api/agent/forensics` — LLM explains past thermal events |
| Cross-session learning | Outcome feedback loop for Learning curve type |
| dT/dt thermal runaway guard | `DtDtGuard` is implemented; wire into poll loop |
| Community profile library | GitHub-based, `tq profile marketplace search/install` |
| RGB via OpenRGB | CPU temp → color gradient |
| AUR package (`thermaliq-bin`) | Linux binary package |
| Homebrew tap | macOS distribution |

---

## v1.2 — macOS + Linux packages

- Signed `.dmg` + Homebrew tap for macOS (requires Apple Developer ID provisioning)
- `.deb` + `.rpm` packages for Linux
- Full macOS fan writes via signed SMJobBless helper
- Intel Arc fan writes (IGCL SDK)

---

## v1.x — Growth

- Hardware plugin API (`HardwarePlugin` trait, `libloading`)
- Home Assistant / MQTT integration (`rumqttc`)
- Game auto-detection (Steam `libraryfolders.vdf`, Epic/GOG process matching)
- Profile marketplace (community PRs to `thermaliq/profiles` repo)
- Predictive component failure detection (bearing wear, thermal paste degradation)

---

## v2.x — Enterprise

- Multi-system fleet management (gRPC agent bus)
- IPMI/BMC support (server rack fan control)
- Kubernetes sidecar (Prometheus metrics from containerised workloads)
- Role-based access tokens (read-only vs admin)
- Audit log export for compliance

---

## Key technical decisions

| Decision | Choice | Reason |
|----------|--------|--------|
| Windows sensor backend | HWiNFO64 shared memory | No kernel driver, full coverage, ~80% of target users already have it |
| Windows driver strategy | Avoid WinRing0 / PawnIO | Defender quarantine (CVE-2020-14979) + anti-cheat incompatibility |
| macOS fan writes | SMJobBless privileged helper | Only production-safe approach (same as TG Pro, Macs Fan Control) |
| macOS notarization | `xcrun notarytool` (not Tauri built-in) | Tauri built-in hangs — upstream bugs #14579 / #8630 |
| API auth | Bearer token, constant-time compare | Local-only, CSRF-safe, no session state |
| CORS | `tauri://localhost` only | Defeats CSRF from any webpage |
| Linux distribution | Not Flatpak / Snap | Sandbox incompatible with hardware access |
| Windows distribution | Not Microsoft Store | Privileged hardware access blocked by Store policy |
| Update mechanism | Tauri Ed25519 auto-updater | Built-in, cryptographically verified |

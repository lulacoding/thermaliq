---
name: ThermalIQ v1.0 Launch Testing & Release Plan
description: Session-by-session plan to validate real-hardware code paths on Windows desktop, procure signing materials, and ship v1.0 by May 15 2026
type: project
confidence: 0.9
last_validated: 2026-04-19
decay_half_life_days: 3
---

# ThermalIQ v1.0 Launch Plan

**Context:** v1.0 target is May 15, 2026 (26 days). The app has been built against `mock-hardware` features only — the Windows HAL, NVML writes, HWiNFO64 parsing, service install, Tauri UI on Windows, auto-updater, and signing workflows have never been exercised on real hardware. This plan fixes that.

**Format:** Each session is ~60–90 min, self-contained, with a "Done when" checkpoint. Check off items as you go. Pick up in the next session at the first unchecked box.

**Rule:** Don't move to the next session until the current one's checkpoint is hit. Skip ahead only for truly unblocked work (e.g. async cert procurement).

---

## Session 0 — Async Kickoff (10 min, do before Session 1)

These have multi-day lead times. Start them now so they're ready when needed.

- [ ] **SSL.com eSigner EV code signing cert** — visit https://www.ssl.com/code-signing/, choose EV, start identity verification. Lead time: 3–5 business days.
- [ ] **Apple Developer Program** (only if shipping macOS) — $99/year, identity verification takes 24–48h if not already enrolled.
- [ ] **Buy a USB storage key for eSigner cert** (some EV providers ship physical; SSL.com eSigner is cloud-based, no key needed — skip if confirmed cloud).
- [ ] **Inventory the test desktop:** CPU model, GPU vendor/model, motherboard chipset, OS version. Write into `.claude/context/test-hardware.md` (create if missing).

**Done when:** Cert order is placed, Apple enrollment started (or decided to skip macOS for v1.0), test hardware documented.

---

## Session 1 — Windows Dev Environment (~90 min)

**Goal:** Get the repo building on your actual Windows desktop with `mock-hardware` first, then without. This proves the toolchain is set up before you touch real hardware logic.

### 1.1 Toolchain install

- [ ] Install Rust via rustup: https://rustup.rs — verify with `rustc --version` (expect 1.80+).
- [ ] Install Node.js 20 LTS from nodejs.org. Verify: `node --version`.
- [ ] Install Tauri prerequisites for Windows: `winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools"` (or use full VS Community if preferred). Need: MSVC C++, Windows SDK.
- [ ] Install `git` if missing: `winget install Git.Git`.
- [ ] Install `gh` CLI: `winget install GitHub.cli`, then `gh auth login`.

### 1.2 Clone + mock build

- [ ] `git clone https://github.com/lulacoding/thermaliq.git && cd thermaliq`
- [ ] `cargo build --features mock-hardware` — should complete in ~5 min on fresh machine.
- [ ] `cargo test --features mock-hardware` — all tests should pass.
- [ ] `cd thermaliq-ui && npm ci && cd ..`
- [ ] Confirm the `thermaliq-ui/src-tauri/icons/` dir has all required sizes (Tauri build will fail loudly if not).

### 1.3 Native (real HAL) build

- [ ] `cargo build` (no features — real HAL path).
- [ ] Note any compile errors. The Windows HAL uses HWiNFO64 shared memory + NVML + ADL; all should link without HWiNFO64 installed (they only fail at runtime).
- [ ] `cd thermaliq-ui && npm run tauri build` — produces NSIS installer at `src-tauri/target/release/bundle/nsis/*.exe`. **First full build takes 15–20 min.**

### 1.4 Smoke test

- [ ] Run the daemon: `cargo run -p thermaliq-daemon --features mock-hardware`. Expect it to listen on `127.0.0.1:7272` and print a startup line.
- [ ] `curl http://127.0.0.1:7272/api/health` (in a new terminal) — expect 401 without auth header.
- [ ] Read `%APPDATA%\thermaliq\api.token`, then `curl -H "Authorization: Bearer <token>" http://127.0.0.1:7272/api/health` — expect 200 + JSON.

**Done when:** Mock build + test run, real build produces an NSIS installer, daemon responds to authenticated `/api/health`.

**Common failure modes:**
- MSVC `link.exe` missing → install VS Build Tools with C++ workload
- Tauri bundle fails with icon errors → regenerate via `npm run tauri icon <path-to-png>`
- `cargo build` fails on `wmi` crate → needs MSVC, not MinGW

---

## Session 2 — HWiNFO64 + Real Sensor Read (~60 min)

**Goal:** Prove the daemon actually reads real temps/fans from your hardware via HWiNFO64 shared memory.

### 2.1 HWiNFO64 setup

- [ ] Download HWiNFO64 from https://www.hwinfo.com/download/ (free version is fine).
- [ ] Install, launch, **tick "Sensors-only" mode**, click Start.
- [ ] Confirm HWiNFO64's Sensors window shows real temps (CPU, GPU, motherboard) and fan RPMs. Keep it running.
- [ ] Open Task Manager → Details → verify `HWiNFO64.exe` is running.

### 2.2 Daemon against real HWiNFO

- [ ] Stop the mock daemon if running.
- [ ] `cargo run -p thermaliq-daemon` (no mock features — real HAL).
- [ ] Watch the startup logs: expect `hwinfo` backend messages. If you see `HwinfoError::NotRunning`, HWiNFO64 isn't in Sensors-only mode — fix and restart.
- [ ] `curl -H "Authorization: Bearer <token>" http://127.0.0.1:7272/api/sensors` — compare values to what HWiNFO64's own UI shows. They should match within ±1°C.
- [ ] `curl .../api/fans` — expect RPM values matching HWiNFO64's fan readings.

### 2.3 WMI fallback path

- [ ] Close HWiNFO64 entirely.
- [ ] Restart the daemon. It should now fall back to WMI.
- [ ] `GET /api/sensors` — expect fewer sensors (WMI can only see ACPI thermal zones + BIOS temp probes, not ISA/SMBus). This is expected.
- [ ] Restart HWiNFO64 and verify the daemon's next poll cycle reopens the shared memory (check logs for re-open messages).

### 2.4 Prometheus metrics

- [ ] `curl http://127.0.0.1:7272/metrics` — expect Prometheus-format output with `thermaliq_cpu_temp_celsius`, `thermaliq_fan_rpm`, etc.

**Done when:** Real sensor values observed matching HWiNFO64, WMI fallback works, `/metrics` returns Prometheus output.

**Regression to file as bug if seen:** any sensor value wildly off (e.g. -273°C, NaN), duplicate fans from multiple backends, crash on HWiNFO64 restart.

---

## Session 3 — Fan Writes (Real GPU) (~60 min)

**Goal:** Prove the daemon can actually control fans. **This is the most dangerous session — do small, short-duration writes and revert.**

**Prerequisites:**
- NVIDIA GPU with driver 520+ (for NVML `nvmlDeviceSetFanSpeed_v2`) OR AMD GPU with Adrenalin installed (for ADL Overdrive5).
- **Daemon running elevated (Administrator)** — NVML/ADL fan writes require admin.

### 3.1 Read current fan state

- [ ] Launch PowerShell as Administrator.
- [ ] `cargo run -p thermaliq-daemon` (real HAL, running elevated).
- [ ] `GET /api/fans` — note each fan's current duty %.
- [ ] Write these down. If the daemon crashes, you'll need to know the "normal" values to restore.

### 3.2 Manual fan write test

Start at a safe duty % that's higher than current (more cooling = safe). For a typical 30% idle, try 50%.

- [ ] Find a GPU fan id (e.g. `nvml_gpu0_fan0`).
- [ ] `POST /api/fans/nvml_gpu0_fan0/duty` with body `{"duty_percent": 50}`.
- [ ] **Listen.** The fan should audibly spin up within 2 seconds.
- [ ] Confirm with HWiNFO64 — fan RPM should have jumped.
- [ ] Revert: `POST /api/fans/nvml_gpu0_fan0/duty` with `{"duty_percent": <original>}` OR set a curve that ramps based on temp.

### 3.3 Curve application

- [ ] Load the "balanced" profile: `POST /api/profiles/activate` with `{"name": "balanced"}`.
- [ ] Watch `/api/fans` over ~60 seconds — duty % should track the curve based on temp.
- [ ] Apply a custom flat curve at 70% → observe fans spin up → revert to balanced.

### 3.4 Safety limits

- [ ] Verify emergency limits are enforced by testing at the boundary. If your CPU is idle (~40°C), the 95°C emergency won't trigger. **Do not induce heat.** Instead, check the code path via unit test review or log injection.

**Done when:** You've observed a real fan spin up / down in response to daemon API calls, applied a profile, and reverted safely.

**If anything goes wrong:** Kill the daemon (`Ctrl-C` or Task Manager). Most hardware defaults to BIOS-controlled fans within 30 seconds of the daemon exiting, so risk is low, but don't leave a 0% write sitting.

---

## Session 4 — Tauri UI on Windows (~75 min)

**Goal:** Run the full Tauri UI against the real daemon on Windows. Exercise every wired component.

### 4.1 Launch UI

- [ ] Daemon running (real HAL, elevated, HWiNFO64 on).
- [ ] `cd thermaliq-ui && npm run tauri dev` — opens a dev window with hot reload.
- [ ] Confirm the Dashboard shows real sensor values streaming via WebSocket.
- [ ] Check system tray — expect ThermalIQ icon with profile quick-switch.

### 4.2 Component walk-through

Go through each, click everything, note what breaks:

- [ ] **Dashboard** — temps update in real time? Fan RPMs match HWiNFO64?
- [ ] **ProfileManager** — switch between silent/balanced/performance. Fans should audibly respond.
- [ ] **CurveEditor** — drag curve points, save. Verify curve is actually persisted (restart daemon, reload — does it survive?). **Known issue:** `save_curve` may be in-memory only.
- [ ] **CalibrationWizard** — run full 0→100% ramp on ONE fan. Takes ~45 sec. Confirm calibration table is saved.
- [ ] **HardwareDetect** — enumerates all HAL-reported devices. Compare to HWiNFO64.
- [ ] **SettingsPanel** — configure AI provider (use a throwaway OpenAI or Anthropic key), test connection.
- [ ] **AgentPanel** — send a prompt to the AI agent. Expect a response. Try Passive → Approval → Autonomous mode transitions.

### 4.3 System tray

- [ ] Click tray icon — menu should show profiles + agent pause/resume.
- [ ] Quick-switch profile from tray — verify UI updates.

### 4.4 Toast notifications

- [ ] Force an AI decision (trigger via `POST /api/agent/evaluate` with a contrived state). Expect a Windows toast in Action Center.
- [ ] Check rate-limit: send 10 rapid events of the same type → expect only 1 toast within cooldown window.

**Done when:** Every component tested against real data. Bug list written to `.claude/context/lessons.md` (add a `## Found During Real-Hardware Testing (YYYY-MM-DD)` section).

---

## Session 5 — Service Install + Autostart (~45 min)

**Goal:** Validate the Windows service registration path used by the NSIS installer.

### 5.1 Install via NSIS

- [ ] Uninstall any dev daemon still running.
- [ ] Find the NSIS installer at `thermaliq-ui/src-tauri/target/release/bundle/nsis/ThermalIQ_*.exe`.
- [ ] Run it (elevated). Walk through the installer.
- [ ] Verify:
  - [ ] `services.msc` shows "ThermalIQ" service, Running, Automatic start.
  - [ ] `%APPDATA%\thermaliq\api.token` exists.
  - [ ] Start Menu shortcut works.
  - [ ] Firewall rule for port 7272 is created (or not needed — it's loopback).

### 5.2 Service lifecycle

- [ ] Stop service via `services.msc` → daemon exits cleanly (check Event Viewer).
- [ ] Start → sensors populate again.
- [ ] Reboot the machine → service auto-starts → UI launches → Dashboard shows live data.

### 5.3 Uninstall

- [ ] Run uninstaller.
- [ ] Verify service removed, `%APPDATA%\thermaliq` either preserved or cleaned per installer choice.
- [ ] Reinstall fresh.

**Done when:** NSIS install → reboot → UI works without manual daemon start, and uninstall is clean.

---

## Session 6 — Auto-Updater End-to-End (~60 min)

**Goal:** Prove the auto-updater actually downloads + installs a new version.

### 6.1 Generate real Ed25519 keypair

- [ ] `npx @tauri-apps/cli signer generate -w ~/thermaliq-updater.key`
- [ ] Output: pubkey (base64) + private key file.
- [ ] Add secrets to GitHub repo settings:
  - [ ] `TAURI_SIGNING_PUBLIC_KEY` = pubkey string
  - [ ] `TAURI_SIGNING_PRIVATE_KEY` = full file contents
- [ ] Delete local private key file.
- [ ] Push a trivial commit (bump version to `1.0.0-test.1` in `Cargo.toml` and `tauri.conf.json`) to a test branch.

### 6.2 Trigger release workflow (beta)

- [ ] Tag: `git tag v1.0.0-test.1 && git push origin v1.0.0-test.1`
- [ ] Watch `release.yml` run in GitHub Actions. Expected failures: no eSigner secrets yet → workflow emits unsigned artifact (pre-release, so allowed per PR #12 logic).
- [ ] Download `latest.json` from the release → verify it contains `signature` (non-empty) + platform URLs.

### 6.3 Install older version, trigger update

- [ ] Install `v1.0.0-test.0` (build one manually first if needed).
- [ ] Bump to `v1.0.0-test.1`, tag, wait for release.
- [ ] Launch the installed app. Within 2 seconds, the updater should check, show an update banner.
- [ ] Click install → new version downloads, installs, app restarts.
- [ ] Verify new version is running.

**Done when:** Updater UI flow works end-to-end, signature verifies, app restarts on new version.

**Failure modes:**
- "Update check failed" → check `tauri.conf.json` endpoint URL, verify `latest.json` exists at that URL
- "Signature invalid" → pubkey in config doesn't match key used to sign
- Install silently succeeds but app doesn't restart → missing `app.restart()` call

---

## Session 7 — Signing Procurement & Production Secrets (~30 min active, days elapsed)

**Goal:** Complete the async items from Session 0 and wire them into CI.

- [ ] SSL.com eSigner cert should be ready by now (3–5 days after Session 0).
- [ ] Add GitHub repo secrets:
  - [ ] `ESIGNER_USERNAME`
  - [ ] `ESIGNER_PASSWORD`
  - [ ] `ESIGNER_CREDENTIAL_ID`
  - [ ] `ESIGNER_TOTP_SECRET`
- [ ] Download the real `CodeSignTool-v1.3.2-windows.zip`, compute SHA256, replace the placeholder in `.github/workflows/release.yml`.
- [ ] If shipping macOS:
  - [ ] Apple Developer Team ID known.
  - [ ] Replace `REPLACE_WITH_TEAM_ID` sentinel in `thermaliq-helper/Info.plist` and any host app Info.plist.
  - [ ] Export Apple Developer ID cert as `.p12`, base64-encode, add as `APPLE_CERTIFICATE` secret.
  - [ ] Add `APPLE_ID`, `APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID`.
- [ ] Push a `v1.0.0-test.2` tag — workflow should now sign Windows artifact successfully.
- [ ] Download signed installer, right-click → Properties → Digital Signatures → verify "ThermalIQ" / SSL.com certificate present.

**Done when:** Signed Windows installer verifies in Explorer; (optional) signed+notarized macOS app verifies via `spctl --assess`.

---

## Session 8 — Beta Release (v1.0.0-beta.1) (~45 min)

**Goal:** Cut the first public beta.

- [ ] Bump version to `1.0.0-beta.1` in `Cargo.toml`, `thermaliq-ui/src-tauri/tauri.conf.json`, `thermaliq-ui/package.json`.
- [ ] Update `CHANGELOG.md` with v1.0 highlights.
- [ ] Commit, push, tag `v1.0.0-beta.1`.
- [ ] Watch `release.yml`:
  - [ ] Windows NSIS signed
  - [ ] `latest.json` generated with valid signatures
  - [ ] Winget manifest generated (not submitted — beta filter should skip)
  - [ ] Release created as draft
- [ ] Promote the draft release to "Pre-release" (not "Latest"). Add release notes.
- [ ] Post announcement link (Discord/Reddit/forum of choice).

**Done when:** Beta release is live at `https://github.com/lulacoding/thermaliq/releases/tag/v1.0.0-beta.1` with signed artifacts.

---

## Session 9 — Beta Triage Loop (repeat as needed)

**Goal:** Fix what breaks when real users try it. This will span 3–5 days wall-clock.

For each reported issue:

- [ ] Reproduce on your Windows desktop.
- [ ] Write a failing test if possible.
- [ ] Fix, commit on a branch, PR, review, merge.
- [ ] Cut `v1.0.0-beta.N+1` tag. Updater pushes fix to beta testers.

**Common beta issues anticipated:**
- HWiNFO64 not installed / wrong mode — add a clearer first-run dialog
- NVML fan write requires admin — installer should run daemon as LocalSystem, not per-user
- Intel integrated graphics don't expose fan control at all → make sure UI doesn't claim they do
- Laptop EC controllers on unsupported models → graceful "no writable fans" state

**Done when:** No P0/P1 bugs in the tracker, beta has been stable for 48h.

---

## Session 10 — v1.0.0 Launch (~30 min)

**Goal:** Ship it.

- [ ] Final smoke test on your desktop — install signed NSIS from beta-N, verify everything works.
- [ ] Bump version to `1.0.0` (no suffix).
- [ ] Update `CHANGELOG.md`, `README.md` with launch date.
- [ ] Tag `v1.0.0`, push. Release workflow runs.
- [ ] Promote release from draft → Latest.
- [ ] Winget manifest should auto-generate and upload (check if PAT submission is wired — if not, manually submit the generated manifest via `winget-create submit --token <PAT> <manifest-tarball>`).
- [ ] Post launch announcement.
- [ ] **Celebrate.**

---

## Cross-Session Checklist

These items need attention regardless of which session you're in.

- [ ] After each session, commit any test notes to `.claude/context/lessons.md` (new section: `## Real-Hardware Testing Findings (YYYY-MM-DD)`).
- [ ] Update `.claude/context/status.md` "Phase Dashboard" and "Launch Target" rows as milestones complete.
- [ ] If you find a real bug, open a GitHub issue BEFORE fixing it (so it's in the public changelog).
- [ ] Keep the beta testers informed — at least one announcement per beta bump.

---

## Known Gaps That Won't Block v1.0

Document these in the launch notes so beta testers don't flag them:

- **macOS fan writes** remain advisory until SMJobBless helper signing is complete (tracked post-v1.0).
- **RDNA3 AMD cards** require `amdgpu.ppfeaturemask=0xffffffff` boot param (Linux) or latest Adrenalin (Windows).
- **No Corsair iCUE / NZXT CAM integration** — deferred to v1.1.
- **Frontend has zero unit tests** — tracked as tech debt.
- **Profile/curve AI generation** is Anthropic-only despite multi-provider settings (documented in `docs/AI_AGENT.md`).

---

## Quick Reference — Env & Commands

```powershell
# Mock daemon (dev only — no real hardware)
cargo run -p thermaliq-daemon --features mock-hardware

# Real daemon (Windows, needs admin for fan writes)
Start-Process -Verb RunAs cargo -ArgumentList "run -p thermaliq-daemon"

# Tauri UI dev
cd thermaliq-ui; npm run tauri dev

# Full release build
cd thermaliq-ui; npm run tauri build

# Tests
cargo test --features mock-hardware

# Token
type %APPDATA%\thermaliq\api.token

# Authenticated curl
$env:TOK = (Get-Content "$env:APPDATA\thermaliq\api.token")
curl -H "Authorization: Bearer $env:TOK" http://127.0.0.1:7272/api/health
```

---

## Timeline Target

| Session | Target Date | Elapsed |
|---------|-------------|---------|
| 0 (async kickoff) | 2026-04-19 | — |
| 1 (Windows env) | by 2026-04-22 | ≤3 days |
| 2 (HWiNFO) | by 2026-04-24 | ≤5 days |
| 3 (fan writes) | by 2026-04-26 | ≤7 days |
| 4 (UI walk-through) | by 2026-04-28 | ≤9 days |
| 5 (service install) | by 2026-04-30 | ≤11 days |
| 6 (updater e2e) | by 2026-05-02 | ≤13 days |
| 7 (signing live) | by 2026-05-04 | ≤15 days |
| 8 (beta) | 2026-05-06 | ≤17 days |
| 9 (triage) | 2026-05-07 → 12 | days 17–23 |
| 10 (v1.0) | 2026-05-15 | day 26 |

If you fall behind, cut Session 6 (auto-updater) from the v1.0 critical path and ship it in v1.0.1. Everything else is blocking.

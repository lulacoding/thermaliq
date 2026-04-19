---
name: Session 1 — Windows Dev Environment
description: Get the repo building on the Windows test desktop with mock and real HAL features, daemon responding to authenticated API calls
session: 1
target_date: 2026-04-22
duration_minutes: 90
risk: medium
---

# Session 1 — Windows Dev Environment

## Why this session exists

Until you've built ThermalIQ from source on your actual Windows desktop, every Windows-only assumption in the codebase is unverified. This session installs the toolchain, builds in both mock and real-HAL modes, and proves the daemon answers authenticated API calls. **No real hardware control yet** — that's S3.

## Prerequisites

- S0 complete (`test-hardware.md` filled)
- Windows 11 (or 10 22H2+) test desktop with admin access
- ~30 GB free disk space (toolchain + Cargo cache + Tauri build artifacts)
- Stable internet (toolchain downloads ~5 GB)
- 2+ hours available — first build is slow

## Step-by-step

### 1.1 Toolchain install (PowerShell, Run as Administrator)

Open PowerShell as Administrator. Confirm with `whoami /groups | findstr S-1-16-12288` (returns a line if admin).

#### Rust
- [ ] Download rustup-init: https://rustup.rs → "Download rustup-init.exe"
- [ ] Run it. Choose default install (option 1). Toolchain: `stable-x86_64-pc-windows-msvc` (NOT gnu).
- [ ] Close + reopen PowerShell. Verify:
  ```powershell
  rustc --version    # expect: rustc 1.80+ (or whatever current stable is)
  cargo --version
  ```

#### Visual Studio Build Tools (MSVC linker)
- [ ] Run:
  ```powershell
  winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
  ```
- [ ] Takes 10–15 min. Watch for completion (returns to prompt).
- [ ] Verify: `Get-Command link.exe` should resolve.
- [ ] **Restart PowerShell** so PATH updates pick up MSVC.

#### Node.js 20 LTS
- [ ] `winget install OpenJS.NodeJS.LTS`
- [ ] Verify: `node --version` (expect v20.x), `npm --version` (expect 10.x)

#### Git
- [ ] `winget install Git.Git`
- [ ] Verify: `git --version`
- [ ] Configure:
  ```powershell
  git config --global user.name "Your Name"
  git config --global user.email "you@example.com"
  ```

#### GitHub CLI
- [ ] `winget install GitHub.cli`
- [ ] `gh auth login` → GitHub.com → HTTPS → "Login with a web browser" → paste code → authorize
- [ ] Verify: `gh repo list lulacoding`

### 1.2 Clone + mock build

- [ ] Pick a build dir. Recommend `C:\dev\` (short paths matter for Tauri's bundle scripts).
  ```powershell
  mkdir C:\dev
  cd C:\dev
  ```
- [ ] Clone:
  ```powershell
  git clone https://github.com/lulacoding/thermaliq.git
  cd thermaliq
  ```
- [ ] Mock build:
  ```powershell
  cargo build --features mock-hardware
  ```
  - First build: ~5–10 min (downloads ~400 deps).
  - Watch for: `error[E0XXX]` (real failure) vs `warning:` (cosmetic, ignore for now).
- [ ] Mock test:
  ```powershell
  cargo test --features mock-hardware
  ```
  - All tests should pass. If any fail: capture full output, file as a regression — does not block progress on mocked tests, but DOES indicate the codebase isn't green.
- [ ] UI deps:
  ```powershell
  cd thermaliq-ui
  npm ci
  cd ..
  ```
  - `npm ci` (not `npm install`) — uses lockfile exactly. ~3 min.
- [ ] Confirm icon assets present:
  ```powershell
  dir thermaliq-ui\src-tauri\icons
  ```
  - Expect: `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.ico`, `icon.icns`. If missing, Tauri build will fail loudly later.

### 1.3 Native (real HAL) build

This is the build that actually targets Windows hardware code paths. Mock features are off.

- [ ] Native build:
  ```powershell
  cargo build
  ```
  - Watch for crate-level errors. Likely suspects:
    - `wmi` crate fails to link → confirm MSVC is on PATH
    - `nvml-wrapper` errors → harmless if no NVIDIA driver yet (links dynamically at runtime)
    - `windows-sys` version mismatch → `cargo update -p windows-sys` and retry
- [ ] Tauri release build:
  ```powershell
  cd thermaliq-ui
  npm run tauri build
  ```
  - **First build: 15–25 min.** Most time is in `cargo build --release`.
  - Output: `src-tauri\target\release\bundle\nsis\ThermalIQ_<version>_x64-setup.exe`
  - If build fails on icons: `npx @tauri-apps/cli icon path\to\1024x1024.png`
- [ ] Locate installer:
  ```powershell
  dir src-tauri\target\release\bundle\nsis\
  cd ..
  ```

### 1.4 Smoke test

- [ ] Run mock daemon (separate terminal, leave it running):
  ```powershell
  cargo run -p thermaliq-daemon --features mock-hardware
  ```
  - Expect startup line: `daemon listening on 127.0.0.1:7272` (or similar)
- [ ] Test unauthenticated request (should fail):
  ```powershell
  curl http://127.0.0.1:7272/api/health
  ```
  - Expect: HTTP 401 + JSON like `{"error":"unauthorized"}`
- [ ] Get token:
  ```powershell
  $tok = Get-Content "$env:APPDATA\thermaliq\api.token"
  echo $tok    # 64-char hex
  ```
- [ ] Authenticated request:
  ```powershell
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/health
  ```
  - Expect: HTTP 200 + JSON `{"status":"ok","version":"...","uptime_seconds":N}`
- [ ] Quick endpoint sweep:
  ```powershell
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/sensors
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/fans
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/profiles
  ```
  - Mock data — values are fake but structure must be valid JSON.
- [ ] Stop daemon: Ctrl-C in its terminal.

## Mac-side prep (Claude does)

Ahead of this session I'll prepare:
- [ ] `scripts/win-bootstrap.ps1` — single PowerShell script that runs 1.1 + 1.2 + 1.3 + 1.4 with progress logging and error trapping. You paste-run instead of clicking through 20 commands.
- [ ] `scripts/win-smoke.ps1` — just the 1.4 smoke test, reusable for later sessions.

Both scripts:
- Use `$ErrorActionPreference = 'Stop'` to abort on first error
- Log to `C:\dev\thermaliq-bootstrap-<timestamp>.log` for paste-back if something fails
- Print a clear "✓ STAGE N COMPLETE" between stages

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `link.exe not found` | MSVC PATH not loaded | Restart PowerShell, or run `& "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"` |
| `cargo build` slow / hangs | Antivirus scanning every .o file | Add `C:\Users\<you>\.cargo` and `C:\dev\thermaliq` to Defender exclusions |
| Tauri build "icon error" | Missing/corrupt PNG sizes | `npx @tauri-apps/cli icon` against a 1024x1024 source |
| `npm ci` fails | Wrong Node version | `nvm use 20` (install nvm-windows if missing) |
| `winget` not found | Old Windows version | Update to 22H2+, or install App Installer from Store |
| `gh auth login` browser doesn't open | Headless / WSL session | Use `--web` flag or device-code flow |

## Done when

- [ ] `cargo build --features mock-hardware` succeeds clean
- [ ] `cargo test --features mock-hardware` passes
- [ ] `cargo build` (real HAL) succeeds clean
- [ ] `npm run tauri build` produces NSIS installer
- [ ] Daemon `/api/health` returns 200 with bearer token
- [ ] All mock endpoints return valid JSON
- [ ] Bootstrap log saved (in case of future debugging)

## Session log

```
Date executed: 
Time taken (vs 90 min estimate): 
NSIS installer file size: 
Failures encountered: 
Defender exclusions added: yes / no
```

## What unblocks next

- S2 (HWiNFO real reads) — needs working real-HAL daemon from this session
- S4 (UI walkthrough) — needs working Tauri build artifact
- S5 (service install) — needs the NSIS installer this session produces

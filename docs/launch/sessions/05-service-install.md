---
name: Session 5 — Service Install + Autostart
description: Validate the Windows service registration path used by the NSIS installer; reboot survives; uninstall is clean
session: 5
target_date: 2026-04-30
duration_minutes: 45
risk: medium
---

# Session 5 — Service Install + Autostart

## Why this session exists

End users won't run `cargo run` — they'll double-click an installer. This session validates the production path:
- NSIS installer registers the daemon as a Windows service
- Service starts on boot
- UI launches on login (or via tray icon)
- `api.token`, firewall rules, file ACLs all set correctly
- Uninstall removes everything cleanly

Bugs here are **first-impression bugs** — they hit every user.

## Prerequisites

- S1 produced an NSIS installer at `thermaliq-ui/src-tauri/target/release/bundle/nsis/ThermalIQ_*.exe`
- S4 confirmed UI works against dev-mode daemon
- Dev daemon stopped (no `cargo run` daemon listening)
- Test desktop OK to reboot mid-session
- Snapshot/restore point optional but recommended (`Checkpoint-Computer -Description "Pre-ThermalIQ install"`)

## Step-by-step

### 5.1 Pre-install cleanup

- [ ] Stop dev daemon: Ctrl-C in its terminal
- [ ] Verify nothing on port 7272:
  ```powershell
  Get-NetTCPConnection -LocalPort 7272 -ErrorAction SilentlyContinue
  ```
  - Empty = good. Otherwise find PID, stop it.
- [ ] Backup `%APPDATA%\thermaliq` (in case install overwrites):
  ```powershell
  Copy-Item -Recurse "$env:APPDATA\thermaliq" "$env:APPDATA\thermaliq.s5backup" -ErrorAction SilentlyContinue
  ```
- [ ] Note current state of:
  ```powershell
  Get-Service ThermalIQ -ErrorAction SilentlyContinue   # expect: nothing
  Get-NetFirewallRule -DisplayName "ThermalIQ*" -ErrorAction SilentlyContinue   # expect: nothing
  ```

### 5.2 Run NSIS installer

- [ ] Locate installer:
  ```powershell
  $installer = Get-ChildItem C:\dev\thermaliq\thermaliq-ui\src-tauri\target\release\bundle\nsis\ThermalIQ_*.exe | Select-Object -First 1
  echo $installer.FullName
  ```
- [ ] Right-click → Run as Administrator (or `Start-Process $installer.FullName -Verb RunAs`)
- [ ] Walk through installer:
  - License agreement (if present)
  - Install location (accept default `C:\Program Files\ThermalIQ` or similar)
  - Component selection (default: all)
  - Click Install
- [ ] On completion, confirm "Launch ThermalIQ" checkbox behavior — leave unchecked for now (we'll verify pieces individually)

### 5.3 Post-install verification

Run the verify script (Mac-side prep) OR manually check each:

- [ ] **Service registered:**
  ```powershell
  Get-Service ThermalIQ
  ```
  - Expect: Status=Running, StartType=Automatic
- [ ] **Service is actually responding:**
  ```powershell
  Start-Sleep 5
  $tok = Get-Content "$env:APPDATA\thermaliq\api.token"
  Invoke-RestMethod -Uri http://127.0.0.1:7272/api/health -Headers @{Authorization="Bearer $tok"}
  ```
  - Expect: 200 OK with version JSON
- [ ] **Token file:**
  ```powershell
  Test-Path "$env:APPDATA\thermaliq\api.token"
  Get-Acl "$env:APPDATA\thermaliq\api.token" | Format-List
  ```
  - Expect: file exists, readable by current user, ideally not world-readable
- [ ] **Start menu shortcut:**
  ```powershell
  Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\ThermalIQ.lnk"
  ```
- [ ] **Desktop shortcut (if installer creates one):**
  ```powershell
  Test-Path "$env:USERPROFILE\Desktop\ThermalIQ.lnk"
  ```
- [ ] **Firewall rule:** loopback usually doesn't need one, but check:
  ```powershell
  Get-NetFirewallRule -DisplayName "ThermalIQ*" -ErrorAction SilentlyContinue
  ```
  - If installer created one, verify it's loopback-only (not 0.0.0.0)

### 5.4 Launch UI from installed shortcut

- [ ] Double-click Start menu shortcut OR:
  ```powershell
  Start-Process "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\ThermalIQ.lnk"
  ```
- [ ] UI window appears within 5s
- [ ] Dashboard shows live data (proves UI → installed-service connection works)
- [ ] System tray icon present
- [ ] Profile switch from tray works

### 5.5 Service lifecycle

- [ ] Stop service:
  ```powershell
  Stop-Service ThermalIQ
  ```
- [ ] UI Dashboard should show "Disconnected" or similar within 10s
- [ ] Check Event Viewer for clean shutdown:
  ```powershell
  Get-EventLog -LogName Application -Source ThermalIQ -Newest 5 -ErrorAction SilentlyContinue
  ```
- [ ] Start service:
  ```powershell
  Start-Service ThermalIQ
  ```
- [ ] UI reconnects within 10s
- [ ] Sensors populate again

### 5.6 Reboot test (THE big one)

- [ ] Note current uptime: `(Get-CimInstance Win32_OperatingSystem).LastBootUpTime`
- [ ] Restart Windows (full reboot, not sleep):
  ```powershell
  Restart-Computer -Force
  ```
- [ ] After login, do NOT manually start anything
- [ ] Wait 30 seconds for boot to settle
- [ ] Check service:
  ```powershell
  Get-Service ThermalIQ
  ```
  - Expect: Running (auto-started)
- [ ] Daemon responding:
  ```powershell
  $tok = Get-Content "$env:APPDATA\thermaliq\api.token"
  Invoke-RestMethod -Uri http://127.0.0.1:7272/api/health -Headers @{Authorization="Bearer $tok"}
  ```
- [ ] If UI is configured to autostart, it should appear in tray. If not, launch manually from Start menu.

### 5.7 Uninstall

- [ ] Find uninstaller:
  ```powershell
  Get-ItemProperty HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\* | Where-Object DisplayName -like "ThermalIQ*"
  ```
- [ ] Or via Settings → Apps → ThermalIQ → Uninstall
- [ ] Walk through uninstaller. Note any prompts about preserving config.
- [ ] Post-uninstall verification:
  ```powershell
  Get-Service ThermalIQ -ErrorAction SilentlyContinue              # expect: nothing
  Get-NetFirewallRule -DisplayName "ThermalIQ*" -ErrorAction SilentlyContinue   # expect: nothing
  Test-Path "C:\Program Files\ThermalIQ"                            # expect: false
  Test-Path "$env:APPDATA\thermaliq"                                # depends on uninstaller choice
  ```
- [ ] If config dir preserved: document. If wiped: confirm that's intended (some users want to keep their profiles).

### 5.8 Reinstall

- [ ] Re-run the installer
- [ ] If config dir was preserved from 5.7: profiles/curves/calibration should reload
- [ ] If wiped: fresh install behavior — verify defaults populate

## Mac-side prep (Claude does)

- [ ] `scripts/service-verify.ps1` — runs all 5.3 checks in one command, prints pass/fail table:
  ```
  ✓ Service registered
  ✓ Service running
  ✓ /api/health responds
  ✓ Token file exists
  ✓ Start menu shortcut
  ✗ Firewall rule (not present — may be intentional)
  ```
- [ ] `scripts/service-uninstall-verify.ps1` — runs 5.7 cleanup checks
- [ ] Code-read of installer config (`tauri.conf.json` bundle section + NSIS template) to document expected behavior

## Bugs to log if seen

- Service installs but doesn't auto-start on reboot → P0
- Token file world-readable → P0 (security)
- UI can't connect to installed service (works in dev) → P0 (likely a port/auth mismatch)
- Uninstall leaves orphan service entry → P1
- Reinstall over existing prompts unexpectedly → P2
- Service runs as wrong account (e.g. user instead of LocalSystem) → blocks fan writes for unprivileged users → P0

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Service not registered | NSIS service script missing/broken | Check `tauri.conf.json` `bundle.windows.nsis.installerScript` |
| Service registered but won't start | Daemon binary missing dependency DLL | Check Event Viewer; usually MSVC redist or VC++ runtime missing — bundle it in installer |
| `/api/health` 500 from installed service | Service running as wrong account, can't read config | Run service as LocalSystem or NetworkService |
| UI launches but Dashboard blank | Token mismatch (UI uses old token) | Restart UI; if still broken, check token regeneration logic |

## Done when

- [ ] NSIS install succeeds without errors
- [ ] Service auto-starts after fresh reboot
- [ ] UI from Start menu connects to installed service
- [ ] Service stop/start lifecycle works
- [ ] Uninstall removes service + binaries
- [ ] Reinstall works (with or without config preservation, per design)
- [ ] All bugs logged with severity

## Session log

```
Date executed: 2026-04-20
Time taken: ~15 min
NSIS installer version: not tested (direct install-service path used instead)
Service runs as account: LocalSystem (default)
Reboot survival: yes - service auto-started on boot, /health responded immediately
Uninstall preserved config: not tested (uninstall-service exists, NSIS uninstaller not exercised)
Bugs filed: none
Notes:
  - Used `thermaliq-daemon.exe install-service` directly instead of NSIS installer
  - HWiNFO64 needs manual start on boot (known: it's not a service, user must start it)
  - Start menu shortcut only created by NSIS installer (expected gap for direct install path)
  - NSIS installer end-to-end test deferred to S8 (beta cut)
```

## What unblocks next

- S6 (auto-updater) — needs installed-service baseline to upgrade FROM
- S8 (beta cut) — beta installer is the same NSIS path; bugs here block beta

---
name: Session 3 — Real Fan Writes (HIGH RISK)
description: First time the daemon actually controls fan duty on real hardware; small writes, auto-revert wrapper, GPU-only for v1.0
session: 3
target_date: 2026-04-26
duration_minutes: 60
risk: HIGH
---

# Session 3 — Real Fan Writes ⚠️

## Why this session exists

Every prior session was read-only. This is the first time the daemon writes to hardware. A buggy write can:
- Drop fan speed to 0% and silently overheat the GPU (worst case: thermal throttle, eventually crash)
- Set fans to 100% indefinitely (annoying but safe)
- Trigger driver-level panics on some NVIDIA cards if NVML calls are malformed

**Mitigations baked into this session:**
1. GPU fan only — never CPU (motherboard EC writes are the most failure-prone path)
2. Always write *higher* duty first (more cooling = safer)
3. Use the auto-revert wrapper (Mac-side prep)
4. Daemon stays in foreground so Ctrl-C kills it instantly
5. BIOS/driver defaults take over within ~30s of daemon exit

## Prerequisites

- S1, S2 complete and green
- HWiNFO64 running in Sensors-only mode (for visual confirmation of fan response)
- NVIDIA GPU with driver 520+ (run `nvidia-smi` to check) **OR** AMD GPU with current Adrenalin
- PowerShell **as Administrator** (NVML/ADL writes require admin)
- Note current ambient temp (so you can compare baseline post-test)
- Optional: a phone-camera to film the test in case of crash (hardware response is hard to recall under pressure)

## Pre-flight safety check

- [ ] Identify which physical fan corresponds to which fan ID. Easiest method:
  1. Read `/api/fans` → note IDs and current duties
  2. Run a 100% write to ONE fan ID
  3. Listen — which fan sped up?
  4. Confirm via HWiNFO64 RPM jump
  5. Map ID → physical location in `test-hardware.md`
- [ ] Confirm room ambient ≤25°C (cold room is safer; hot room reduces margin)
- [ ] Close all GPU-heavy apps (browsers with hardware acceleration off, no games, no video playback)
- [ ] **Have HWiNFO64 visible on a second monitor** so you can see temp + RPM in real time

## Step-by-step

### 3.1 Read & record current fan state

- [ ] Daemon running elevated:
  ```powershell
  cd C:\dev\thermaliq
  cargo run -p thermaliq-daemon
  ```
- [ ] Token:
  ```powershell
  $tok = Get-Content "$env:APPDATA\thermaliq\api.token"
  $hdr = @{ Authorization = "Bearer $tok" }
  ```
- [ ] Read fans:
  ```powershell
  $fans = Invoke-RestMethod -Uri http://127.0.0.1:7272/api/fans -Headers $hdr
  $fans | Format-Table id, current_duty_percent, current_rpm, capabilities
  ```
- [ ] **Write down on paper:** every fan ID + its current duty %. If anything goes wrong, you need to manually restore these.
- [ ] Identify a GPU fan (`id` should contain `nvml` for NVIDIA, `adl` for AMD).
- [ ] Confirm `capabilities` includes `write` (read-only fans cannot be controlled — that's BIOS-fixed).

### 3.2 Single GPU fan write (auto-reverted)

Use the auto-revert wrapper script (Mac-side prep). It does the equivalent of:

```
1. Snapshot current duty
2. POST new duty
3. Sleep 5 seconds
4. Print observed RPM change
5. Restore original duty
```

- [ ] Pick a GPU fan ID, e.g. `nvml_gpu0_fan0`. Current duty: e.g. 30%.
- [ ] Run wrapper:
  ```powershell
  .\scripts\fan-test-safe.ps1 -FanId "nvml_gpu0_fan0" -TargetDuty 50 -HoldSeconds 5
  ```
- [ ] Watch & listen:
  - Fan should audibly spin up within 2 seconds
  - HWiNFO64 RPM should jump (e.g. 800 → 1400 RPM)
  - GPU temp should drop 1–3°C over the 5s hold
  - Wrapper auto-reverts to 30% — fan should slow down again
- [ ] **If fan does NOT spin up:**
  - Check daemon logs for write errors (NVML error codes: 1=Uninitialized, 6=NoPermission, 17=NotSupported)
  - Try elevated PowerShell if not already
  - If `NotSupported`: GPU model doesn't allow user fan control (mostly older laptops). Document and move on.

### 3.3 Manual write (no auto-revert) — only if 3.2 succeeded

This proves you can do real writes without the wrapper.

- [ ] Set fan to 60%:
  ```powershell
  $body = @{ duty_percent = 60 } | ConvertTo-Json
  Invoke-RestMethod -Uri "http://127.0.0.1:7272/api/fans/nvml_gpu0_fan0/duty" -Method POST -Headers $hdr -ContentType "application/json" -Body $body
  ```
- [ ] Confirm fan jumped via HWiNFO64.
- [ ] Restore to original:
  ```powershell
  $body = @{ duty_percent = 30 } | ConvertTo-Json
  Invoke-RestMethod -Uri "http://127.0.0.1:7272/api/fans/nvml_gpu0_fan0/duty" -Method POST -Headers $hdr -ContentType "application/json" -Body $body
  ```
- [ ] **Always restore.** Never leave a manual write in place.

### 3.4 Curve / profile application

This tests that profile-driven fan control works end-to-end.

- [ ] List profiles:
  ```powershell
  Invoke-RestMethod -Uri http://127.0.0.1:7272/api/profiles -Headers $hdr
  ```
- [ ] Activate `balanced`:
  ```powershell
  $body = @{ name = "balanced" } | ConvertTo-Json
  Invoke-RestMethod -Uri http://127.0.0.1:7272/api/profiles/activate -Method POST -Headers $hdr -ContentType "application/json" -Body $body
  ```
- [ ] Watch over 60 seconds:
  ```powershell
  for ($i=0; $i -lt 12; $i++) {
    $f = Invoke-RestMethod -Uri http://127.0.0.1:7272/api/fans -Headers $hdr
    $f | Select id, current_duty_percent, current_rpm | Format-Table
    Start-Sleep 5
  }
  ```
  - Duty % should track the balanced curve relative to current temps.
- [ ] Switch to `silent`:
  ```powershell
  $body = @{ name = "silent" } | ConvertTo-Json
  Invoke-RestMethod -Uri http://127.0.0.1:7272/api/profiles/activate -Method POST -Headers $hdr -ContentType "application/json" -Body $body
  ```
  - Fans should slow noticeably. Temps may rise 2–5°C — expected.
- [ ] **Restore to balanced** before ending the session.

### 3.5 Safety limits review (no test, code review only)

The 95°C CPU / 90°C GPU emergency limits are hardcoded. **Do not induce heat** to test them — risk far outweighs reward.

- [ ] Mac-side: I'll grep the codebase for the emergency-limit constants and review the trigger path. Report findings in this session log.
- [ ] On Windows: skip live test of emergency limits.

## Mac-side prep (Claude does)

- [ ] `scripts/fan-test-safe.ps1` — auto-revert wrapper (snapshot + write + sleep + restore + log)
- [ ] `scripts/fan-watch.ps1` — polls `/api/fans` every 5s, prints RPM/duty table to terminal
- [ ] Code-read of emergency-limit logic in `thermaliq-daemon/src/`. Report:
  - File + line of constants
  - Trigger condition (every poll? on threshold cross?)
  - Override behavior (does it actually force 100%? Or just request?)
  - Whether it logs to audit DB

## Failure recovery

| Symptom | Action |
|---------|--------|
| Daemon crashes mid-write | Ctrl-C its terminal. Most fans default to BIOS-controlled within 30s. Confirm via HWiNFO64. |
| Fan stuck at written duty | Restart daemon (will re-apply last profile, OR will exit and BIOS takes over). If still stuck, reboot — BIOS resets PWM control on POST. |
| GPU temp climbing toward 90°C | Immediately set fan to 100%: `POST /api/fans/<id>/duty {"duty_percent":100}`. If no response, kill daemon (Ctrl-C). If still climbing, shut down PC. |
| NVML returns NotSupported | This GPU doesn't support user fan control. Document, skip rest of S3 for this fan. |
| AMD ADL Overdrive5 not available | Newer cards use Overdrive8 — file as a known gap. |

## Bugs to log if seen

- Write succeeds (HTTP 200) but fan doesn't change → silent write failure, P0
- Write to one fan affects another → cross-talk bug, P1
- `/api/fans` shows stale duty after write → cache invalidation bug, P2
- Curve doesn't recompute on temp change → polling/scheduler bug, P1

## Done when

- [ ] One GPU fan responded to a manual duty write (audible + RPM confirmed)
- [ ] Auto-revert wrapper restored original duty cleanly
- [ ] Profile activation drove curve-based duty changes
- [ ] All fans returned to original or profile-controlled state at session end
- [ ] Emergency limit code review captured in log

## Session log

```
Date executed: 2026-04-20
Time taken: ~30 min
GPU model under test: NVIDIA GeForce RTX 4060
NVML driver: nvml.dll in C:\Windows\System32 (driver dated 2026-03-07)
Fan IDs successfully written: nvml_gpu0_fan0 (31% -> 60% confirmed, gradual ramp via GPU slew rate)
Fan IDs that returned NotSupported: none
Profile transitions tested: balanced -> silent -> balanced (both activated successfully)
Final fan state restored: yes (auto-revert script + profile restore)
Bugs fixed:
  - nvml.dll not nvidia-ml.dll (Windows ships it as nvml.dll)
  - nvmlDeviceSetFanSpeed_v2 takes 3 params (device, fan, speed), not 4;
    extra "policy" arg caused speed=1% instead of target (commit 2122530)
  - fan write route handler swallowed errors silently; added tracing::error
Emergency limits code review:
  - Constants: MAX_CPU_TEMP=90.0, EMERGENCY_TEMP=95.0 in agent_task.rs:12-13
  - Trigger: check_safety() runs every agent tick (5s); CPU>=95 or GPU>=90 -> 100% all fans
  - Action: immediate emergency intervention, logged to audit DB
  - NOT live-tested (too risky for hardware)
Notes:
  - GPU BIOS enforces ~31% minimum duty; writes below this are clamped by hardware
  - HWiNFO fan reads show RPM; NVML reads show duty% but 0 RPM (NVML limitation on Windows)
  - Created 3 default profiles: balanced, silent, performance in %APPDATA%\thermaliq\profiles\
```

## What unblocks next

- S4 (UI walkthrough) — needs working fan writes (CurveEditor, CalibrationWizard exercise this)
- S9 (beta triage) — fan-write reliability is the #1 thing testers will hit first

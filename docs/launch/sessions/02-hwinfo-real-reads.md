---
name: Session 2 — HWiNFO64 + Real Sensor Read
description: Prove the daemon reads real temps/fans from hardware via HWiNFO64 shared memory; verify WMI fallback path
session: 2
target_date: 2026-04-24
duration_minutes: 60
risk: low
---

# Session 2 — HWiNFO64 + Real Sensor Read

## Why this session exists

S1 proved the daemon runs. This session proves the daemon's *Windows HAL* actually parses real sensor data. It exercises three independent paths:
1. **HWiNFO64 shared memory** (primary) — richest sensor surface
2. **WMI ACPI** (fallback) — works without HWiNFO64, fewer sensors
3. **Auto-recovery** — daemon must reopen shared memory if HWiNFO64 restarts

If sensor values are wildly wrong here, every higher-level feature (curves, AI, profiles) is built on garbage data.

## Prerequisites

- S1 complete (real-HAL daemon builds + runs)
- Test desktop has at least one fan visible to OS (most have ≥2: CPU + case)
- Internet access to download HWiNFO64 (free)
- ~15 min HWiNFO64 sensor warm-up if you want stable comparisons

## Step-by-step

### 2.1 HWiNFO64 install

- [ ] Download from https://www.hwinfo.com/download/ → **Installer** (free portable also works but installer registers Start menu shortcut)
- [ ] Install with defaults
- [ ] **First launch:** dialog asks "Sensors-only / Summary-only / Full". Choose **Sensors-only**, click Run.
- [ ] Wait ~10 seconds for the Sensors window to populate. Confirm:
  - CPU temp shown (e.g. "CPU Package")
  - GPU temp shown (e.g. "GPU Temperature")
  - Motherboard temp shown
  - At least 1 fan with a non-zero RPM
- [ ] **Critical:** the daemon talks to HWiNFO64 via shared memory which is ONLY exposed in Sensors-only mode. If you run Full mode, daemon will not see anything.
- [ ] Leave HWiNFO64 running. Minimize to tray.
- [ ] Verify process exists:
  ```powershell
  Get-Process HWiNFO64
  ```

### 2.2 Daemon against real HWiNFO

- [ ] Stop any mock daemon from S1.
- [ ] Run real daemon:
  ```powershell
  cd C:\dev\thermaliq
  cargo run -p thermaliq-daemon
  ```
  - **Watch startup logs carefully.** Expected lines:
    - `hal: backend=hwinfo` (good)
    - `hal: discovered N sensors` where N matches roughly what HWiNFO64 shows
  - **Bad signs:**
    - `HwinfoError::NotRunning` → HWiNFO64 not in Sensors-only mode (re-check 2.1)
    - `HwinfoError::SharedMemoryUnavailable` → permission issue, try running PowerShell elevated
    - `falling back to wmi` → HWiNFO64 wasn't detected; investigate before continuing
- [ ] Token refresh:
  ```powershell
  $tok = Get-Content "$env:APPDATA\thermaliq\api.token"
  ```
- [ ] Read sensors via API:
  ```powershell
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/sensors | ConvertFrom-Json | Format-Table
  ```
- [ ] **Side-by-side compare** with HWiNFO64's UI for at least:
  - CPU package temp (delta should be <1°C)
  - GPU temp (delta <1°C)
  - At least 2 fan RPMs (delta <50 RPM is fine — fans fluctuate)
- [ ] Read fans:
  ```powershell
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/fans | ConvertFrom-Json | Format-Table
  ```
- [ ] **Capture deltas** in a markdown table for the session log:
  | Sensor | API value | HWiNFO value | Delta |

### 2.3 WMI fallback path

The daemon must degrade gracefully when HWiNFO64 isn't running.

- [ ] Close HWiNFO64 entirely:
  ```powershell
  Stop-Process -Name HWiNFO64
  ```
- [ ] Restart daemon (Ctrl-C and re-run):
  ```powershell
  cargo run -p thermaliq-daemon
  ```
  - Expect log: `hal: backend=wmi` (or similar)
- [ ] Read sensors:
  ```powershell
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/sensors | ConvertFrom-Json | Format-Table
  ```
  - **Expected:** fewer sensors than 2.2. WMI exposes ACPI thermal zones + BIOS temp probes only — typically 2–4 sensors total. ISA/SMBus temps are invisible.
  - Values should still be within ±5°C of physical reality.
- [ ] If WMI returns *zero* sensors: this is a real bug. Capture log, file as P1.

### 2.4 Auto-reopen on HWiNFO restart

This tests the daemon's resilience.

- [ ] With daemon still running on WMI:
- [ ] Restart HWiNFO64 in Sensors-only mode again.
- [ ] Watch daemon logs for ~30 seconds. Expect a log line indicating HWiNFO64 was redetected and the shared memory was reopened. (Exact wording depends on implementation — search for `hwinfo`, `reopened`, `backend switched`.)
- [ ] Re-query `/api/sensors` — should now show the richer HWiNFO sensor list.
- [ ] **If daemon does NOT auto-switch:** record exact behavior. This is a P2 bug (workaround: restart daemon manually). Do not block S3 on it.

### 2.5 Prometheus metrics

The `/metrics` endpoint should expose Prometheus-format counters/gauges. This validates the metrics layer wires through correctly.

- [ ] Read metrics:
  ```powershell
  curl http://127.0.0.1:7272/metrics
  ```
  - Note: `/metrics` is typically unauthenticated for Prometheus scraping. If 401, that's a separate decision to capture.
- [ ] Look for keys:
  - `thermaliq_cpu_temp_celsius{sensor="..."}`
  - `thermaliq_gpu_temp_celsius{sensor="..."}`
  - `thermaliq_fan_rpm{fan="..."}`
  - `thermaliq_fan_duty_percent{fan="..."}`
- [ ] Confirm values match `/api/sensors` and `/api/fans`.

## Mac-side prep (Claude does)

- [ ] `scripts/sensor-diff.ps1` — automates the side-by-side compare:
  - Reads `/api/sensors` JSON
  - Reads HWiNFO64's CSV log export (HWiNFO can write CSV via Settings → Logging)
  - Prints a markdown table with deltas
  - Flags any sensor with >2°C delta as suspicious
- [ ] `.claude/context/sensor-baseline.md` — template for capturing the baseline sensor list per backend; useful for future regression detection

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Daemon never sees HWiNFO | Not in Sensors-only mode | Restart HWiNFO64, pick Sensors-only |
| `HwinfoError::Permission` | Shared memory ACL | Run daemon elevated |
| Sensor values are 0 / NaN | HWiNFO64 hasn't polled yet | Wait 10s after HWiNFO start, retry |
| Fan RPM = 0 but fan spinning | PWM-only fan, no tach signal | Real, expected — flag in hardware notes |
| WMI returns nothing | Modern hardware exposes via SMBus only | Document; not a daemon bug |
| Daemon crashes on HWiNFO restart | Race condition in shared memory reopen | File as P1, capture stack trace |

## Bugs to log if seen

Open a GitHub issue for any of:
- Sensor value <-100°C or >150°C (sentinel/garbage)
- Duplicate sensor IDs from different backends
- Daemon panic on backend switch
- WMI backend returning zero sensors when ACPI zones exist (verify with `Get-WmiObject -Namespace root\wmi -Class MSAcpi_ThermalZoneTemperature`)

## Done when

- [ ] Real HWiNFO sensor values match HWiNFO UI within ±1°C / ±50 RPM
- [ ] WMI fallback returns at least one sensor
- [ ] `/metrics` endpoint returns Prometheus-format text
- [ ] Auto-reopen behavior captured (working OR documented as bug)
- [ ] Sensor delta table saved to session log

## Session log

```
Date executed:
Time taken:
HWiNFO sensors discovered: N
WMI sensors discovered: N
Worst sensor delta: N°C on <sensor>
Auto-reopen works: yes / no / partial
Bugs filed: <issue numbers>
```

## What unblocks next

- S3 (fan writes) — needs validated sensor reads as the safety baseline
- S4 (UI) — Dashboard reads stream from these endpoints
- All future work depends on trustworthy sensor data

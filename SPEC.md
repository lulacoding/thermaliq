# ThermalIQ v2 — Spec

Fan control for Windows that is safe first, smart second. One page of truth: if code and this file disagree, this file wins until it is changed on purpose.

Reference: the v0 code is tagged `v0-archive` (see `PORTING.md`). The audit that led to this rebuild: "ThermalIQ Codebase Audit — Continue, Rewrite, or V2?" (2026-11-04).

## 1. What v2.0 is

A background service that reads temperatures and drives fans from user-defined curves, with a desktop UI, a CLI, and an optional AI agent that suggests or applies profile changes. Local-first: no cloud, no telemetry.

## 2. Scope

| In v2.0 | Later (not v2.0) |
| --- | --- |
| Windows 10/11 x64 | macOS (helper, SMC) |
| NVIDIA GPU fans (NVML) | AMD GPU fans (ADLX) |
| Motherboard fans via LHM bridge (LibreHardwareMonitorLib + PawnIO) | Laptop EC / NBFC |
| | Linux (hwmon) — v2.1 |
| | File custom sensor — v2.1 |
| | Battery-based switching |
| Curves: linear, flat, graph, mix, sync, trigger | Curves: auto, ai, predictive, learning |
| Custom sensors: time-average, mix, offset | Webhooks |
| Profiles, app rules, schedule | Community profile library |
| Alerts: log + notify + apply-profile | |
| Tauri UI, `tq` CLI, HTTP API + WebSocket | |
| LLM profile generation (one OpenAI-compatible provider) | Multiple LLM providers |
| Agent with a Jev-style decision provider | Predictive thermal model |

Anything not in the left column does not get code, stubs, enum variants, UI, or docs claiming it exists.

## 3. Safety invariants (non-negotiable)

Every one of these has an automated test before any code that writes to real fans is merged.

- **S1 One writer.** Only the engine calls `HardwareLayer::set_fan_duty`. Everything else submits requests to the engine.
- **S2 Emergency latch.** CPU >= 95 °C or GPU >= 90 °C → every controllable fan at 100%. The latch holds until CPU < 85 °C and GPU < 80 °C continuously for 30 s. Nothing can lower a fan while latched.
- **S3 Sensor loss.** If a fan's controlling sensor is missing or invalid (NaN, < -20 °C, > 150 °C) for 3 consecutive ticks, that fan goes to 100% until the sensor is back for 3 ticks.
- **S4 HAL failure.** If a snapshot fails 3 ticks in a row, every fan goes to 100% (best effort) and the daemon reports `degraded`.
- **S5 Hand back on exit.** On normal exit, service stop, Ctrl-C or panic, the engine calls `restore_auto()` for every fan it has touched (NVML default fan speed, original `pwm_enable`, bridge reset).
- **S6 Watchdog.** Any out-of-process helper (LHM bridge) restores auto control by itself if it gets no heartbeat from the daemon for 5 s.
- **S7 Clamp.** Every duty is clamped 0–100 before it reaches a HAL. Profile minimums and safety floors apply after curves, before ramping.
- **S8 No fake success.** No handler, setting or feature returns success without doing the thing.

## 4. Who controls a fan (priority, highest first)

1. Safety (S2–S4)
2. Calibration or manual override (always time-limited, auto-expires)
3. Agent intervention
4. App rule
5. Schedule
6. User's chosen profile

0-RPM: a running fan stops at or below `stop_percent`; a stopped fan restarts only above `start_percent`, jumping straight to at least `start_percent`.

The engine resolves this every tick. A lower source never overwrites a higher one. A profile change ramps using that fan's `step_up_pct` / `step_down_pct`; safety does not ramp.

## 5. Architecture

```
thermaliq-common   types, config + profile schema, validation (no IO beyond parsing)
thermaliq-hal      HardwareLayer trait, mock, platform backends
thermaliq-engine   control loop: curves, sensors, priority, safety, ramping (pure logic + HAL trait)
thermaliq-daemon   wiring: config/paths, history DB, HTTP API, WebSocket, service, agent
thermaliq-cli      `tq` — talks to the daemon over HTTP
bridges/lhm-bridge (later) .NET sidecar for Windows motherboard fans, JSON over a named pipe
thermaliq-ui       (later) Tauri 2 + React
```

`HardwareLayer` (minimum): `snapshot()`, `set_fan_duty(fan, duty)`, `restore_auto(fan)`, `capabilities()`.
It is synchronous; the engine runs on its own thread (no async in the HAL).

One snapshot per tick, cached and shared. API reads use the cache, never the hardware directly.

## 6. Data, paths, auth

- One shared data directory: `%ProgramData%\ThermalIQ` (overridable by env var for dev and CI). Config, profiles, token, history DB live there.
- `config.toml`, written atomically (temp file + rename). Settings changed through the API are saved there.
- API keys in the OS keychain, never in TOML.
- Every mutating endpoint needs `Authorization: Bearer <token>`. Read-only endpoints may be open. CORS lists the real production origins (including `http://tauri.localhost` on Windows).
- UI gets the token through a Tauri command, not by reading files from the webview.
- History: one transaction per tick. Records: every fan; every CPU/GPU sensor; and every sensor referenced by **any saved profile** (curve sources, custom-sensor sources, alert sensors), not just the active one. Raw readings are kept **24 h**, then downsampled to 1/min until 90 days. Expected size ~300–400 MB.

## 7. Agent

- Modes: **passive** (observe and log), **approval** (propose; you approve or reject each change), **autonomous** (apply within the rate limit).
- Every 5 s: builds a JSON state (5-min temperature window, slopes, foreground app, active profile) and asks a decision provider two typed questions: workload `[idle, gaming, compiling, encoding, stress]` and action `[none, propose, apply]`, with confidence.
- `DecisionProvider` speaks the Jev API format and reads `TYPESAFE_BASE_URL`. Default: a local runner (Ollaya + open decision model). Real Jev: optional, user's key. No endpoint → threshold fallback.
- Agent requests go through the engine at priority 3. Max 6 interventions per hour. The agent never runs inside the fan loop.

## 8. Gates (real hardware, run by Sam on the Windows PC)

- **G1 Safety:** emulated 97 °C holds every fan at 100% for 30 s; killing the daemon returns fans to automatic within 5 s.
- **G2 UI:** packaged (not dev) UI changes a curve and the AI key; both survive a reboot.
- **G3 Motherboard:** a case fan follows its curve through the LHM bridge; a 24 h soak passes with no stuck fans and no restarts.

## 9. Done means

A module is done when its tests pass in CI on Windows, macOS and Ubuntu runners (mock hardware), it is ticked in `PORTING.md`, and a separate reviewer (not the session that wrote it) has checked it against this spec.

## 10. Decisions

Decided 2026-11-04 by Sam:

- Agent modes: 3 — passive, approval, autonomous.
- Linux moves to v2.1. v2.0 is Windows-only. CI still builds and tests on Ubuntu and macOS with mock hardware.
- HWiNFO stays as an optional extra sensor source once the LHM bridge works.
- LLM for profile generation: OpenRouter by default; any OpenAI-compatible base URL (e.g. local Ollama) can be configured.

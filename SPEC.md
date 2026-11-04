# ThermalIQ v2 — Spec

An AI cooling agent for Windows that sets up, runs and improves your PC's cooling on its own, inside hard safety rules. One page of truth: if code and this file disagree, this file wins until it is changed on purpose.

Reference: the v0 code is tagged `v0-archive` (see `PORTING.md`). The audit that led to this rebuild: "ThermalIQ Codebase Audit — Continue, Rewrite, or V2?" (2026-11-04).

## 1. What v2.0 is

**The agent is the product** (decided by Sam, 2026-11-04). ThermalIQ is not a manual fan-control tool with an optional AI add-on. Instead of the user building curves and configuring every system fan, GPU fan, CPU cooler and AIO pump, the agent:

1. **Sets up:** finds every fan and pump, calibrates each one, and works out which fan cools what by spinning each and watching which temperatures change.
2. **Runs:** every second, a small learned model predicts temperatures and adjusts fans to the user's quiet ↔ cool goal, during gaming too. A Jev-style decision model (not an LLM) makes the bigger calls: which profile, whether to accept a learned curve.
3. **Improves over time:** learns from history which settings meet the user's temperature goals with the least noise, per workload, and keeps tuning.

All of this happens inside the SPEC §3 safety rules, which the agent can never override. Manual curves stay available as an advanced option. Underneath is a background service with a desktop UI, a CLI and a local API. Local-first: no cloud, no telemetry. A hosted decision model is only used if the user opts in.

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
| Agent (§7): setup, live optimiser with a learned thermal model, Jev-style decision provider, learning | Agent-only curve types in profiles (the agent writes normal curves) |
| Setup load generator, fan health alerts, "why is it loud?", tray pause, acoustic calibration (§7.6) | Weekly report, one-click hardware report, call mode (quiet when Discord/Zoom open) |

Anything not in the left column does not get code, stubs, enum variants, UI, or docs claiming it exists.

## 3. Safety invariants (non-negotiable)

Every one of these has an automated test before any code that writes to real fans is merged.

- **S1 One writer.** Only the engine calls `HardwareLayer::set_fan_duty`. Everything else submits requests to the engine.
- **S2 Emergency latch.** CPU >= 95 °C or GPU >= 90 °C → every controllable fan at 100%. The latch holds until CPU < 85 °C and GPU < 80 °C continuously for 30 s. Nothing can lower a fan while latched. The thresholds are for core temperature: GPU hot spot and memory junction readings are never classified `Gpu`, and limit/threshold readings are not sensors.
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
thermaliq-agent    agent logic (§7): thermal model, classifier, live optimiser, decision routing (no HAL; submits to the engine)
thermaliq-daemon   wiring: config/paths, history DB, HTTP API, WebSocket, service, agent thread
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

Decided in the agent brainstorm (Sam, 2026-11-04). Research: `docs/dev/research/decision-models.md`.

**Ground rules.** The agent is a separate thread, never inside the engine loop. It reads the cached snapshot and history and only *submits* to the engine (S1): duty requests at priority 3 with a short expiry, and profile or curve changes through the normal validated config path. If it stops or crashes, its requests expire and the active profile takes over. It cannot see or change the S2–S4 thresholds. It never stops a pump, and never sets a fan below the profile's minimum.

**Inputs.** Temperatures, CPU/GPU load %, power (W), clocks and GPU throttle reasons (Metrics module), fan duty and RPM, the foreground app's exe name (never window titles or file contents), and time of day.

**Goal.** A quiet ↔ cool slider (0–100) per workload. Quiet = the lowest noise that keeps temps under safe targets. Cool = keep the GPU and CPU cool enough to hold full boost (no thermal throttling). Defaults: gaming leans cool, browsing and idle lean quiet.

### 7.1 Setup (first run, and again when hardware changes)

1. Discover every fan and pump (M10–M13 backends).
2. Calibrate each fan at priority 2 (time-limited): start %, stop %, max RPM. Pumps are measured, never stopped.
3. Map fans to components: raise one fan at a time under a steady load (the built-in load generator, §7.6) and watch which temperatures drop. A clear result is applied. An unclear one is asked of the decision model, then of the user if the model isn't confident.
4. Write a starting profile per workload from the mapping, and record a baseline for the thermal model.

### 7.2 Runtime

**Fast layer: every tick (1 s), CPU only, microseconds, always on, including while gaming.**
- *Workload classifier:* small gradient-boosted trees over the inputs give idle / browsing / gaming / compiling / rendering / other. It is trained on history labelled by the slow layer and by the user's corrections.
- *Thermal model:* small, learned per component. It predicts each controlled temperature 10–30 s ahead from duty, load and power.
- *Live optimiser:* it adjusts each fan up or down from its curve output to meet the slider target. Ramping up early for a predicted spike counts as a **short boost**, which expires in ≤ 60 s. Limits: it may raise any fan freely. It may lower a fan by at most 15 points below the curve output, and only while every temperature that fan cools is under its target and predicted to stay there. Requests expire in 3 s and are renewed every tick.

**Slow layer: every 60 s and whenever the foreground app changes. Decision model, not an LLM.**
- Typed questions with confidence: the workload for an app not seen before (cached per exe afterwards); which profile to use; whether to accept a learned curve or a new profile.
- **Provider routing:** hosted Jev when the user has added a key (OS keychain), is online, and gets an answer within 2 s. Otherwise local: an open decision model (Strands Decider / Clef-flash class) behind an Ollaya-compatible endpoint (`TYPESAFE_BASE_URL`). The local model runs on one CPU core at background priority. It is loaded on demand and unloaded after use, so it isn't resident in VRAM. It may use the GPU only while GPU load is < 50%.
- **Big calls** (accept a learned curve, create a profile): when both providers are available, both answer. Agreement → apply. Disagreement → ask the user.
- Hosted requests contain numbers and exe names only. The user can turn hosted off at any time.
- No provider available: the fast layer keeps running, the slow layer's calls queue as questions for the user, and nothing pretends to have decided (S8).

### 7.3 Learning (when the PC is idle)

Retrain the classifier and the thermal models on history. Search curve changes per workload that meet the slider target with less noise, using the thermal model, and only within validated profile rules (monotonic, within 0–100, at or above minimums). The results are proposals for the slow layer.

### 7.4 Autonomy and reporting

- **Modes:** *observe* (log what it would do), *ask* (propose every change), *auto* (**default**). In auto, a change is applied when the decision's confidence is ≥ its threshold and asked otherwise. Thresholds are higher for bigger changes.
- **The agent may:** pick a profile per app, tune curves, create profiles, run short boosts and live adjustments. Slow-layer changes are limited to 6 per hour. Fast-layer adjustments are bounded by §7.2 instead.
- **Activity feed** in the UI and API: every applied change with its reason and confidence, one-click undo (restores the previous profile or curve exactly), and a Windows toast only when it needs the user to answer.

### 7.5 Evaluation (tests + M20 spike on Sam's PC)

- Decisions: Brier score and accuracy against labelled history and user accept/undo.
- Thermal model: mean error at 20 s ahead, ≤ 2 °C on held-out history.
- Live optimiser: in mock-hardware tests, it never lowers a fan whose temperature is over target or predicted over target, and never goes past the 15-point limit.
- Gaming cost: the local CPU decision model's FPS cost in a real game must be < 2% (average FPS, same scene, with and without it). If it fails, the local model doesn't run while a game is in the foreground, and its calls queue until the game closes.
- Latency: the fast layer takes < 1 ms per tick. Slow-layer timing is logged per provider.

### 7.6 Launch features (Sam, 2026-11-04)

- **Load generator (setup):** steady CPU and GPU load, for mapping and calibration only. Each run lasts at most 60 s, and the user can cancel it any time. It stops at once if the S2 latch fires or any temperature reaches its target + 5 °C. It never runs without the user starting setup.
- **Fan health and dust alerts:** from history, at the same duty and load. A fan's RPM dropping ≥ 15% over 30 days → "fan may be failing". A component running ≥ 5 °C hotter than its baseline over 30 days → "clean filters or dust". These are alerts only (M16). They never change control.
- **"Why is it loud?":** every fan shows its current reason, built from templates over the deciding source (latch, override, agent with reason and confidence, profile curve) and its inputs. No LLM. API: part of `/fans`.
- **Tray icon:** shows the state (safe / agent active / latched / degraded). One click pauses the agent: its requests are withdrawn, and the profile runs as if the agent were off. Pausing works even when the UI is closed.
- **Acoustic calibration (optional, needs a mic, user opt-in):** during setup, sweep each fan's RPM and measure the noise level in dB. The agent's "quiet" then uses the measured dB, and it avoids RPM bands with resonance or whine. Audio is processed in memory on the PC and never stored or sent. Only the RPM → dB table is saved.

## 8. Gates (real hardware, run by Sam on the Windows PC)

- **G1 Safety:** emulated 97 °C holds every fan at 100% for 30 s; killing the daemon returns fans to automatic within 5 s.
- **G2 UI:** packaged (not dev) UI changes a curve and the AI key; both survive a reboot.
- **G3 Motherboard:** a case fan follows its curve through the LHM bridge; a 24 h soak passes with no stuck fans and no restarts.

## 9. Done means

A module is done when its tests pass in CI on Windows, macOS and Ubuntu runners (mock hardware), it is ticked in `PORTING.md`, and a separate reviewer (not the session that wrote it) has checked it against this spec.

## 10. Decisions

Decided 2026-11-04 by Sam:

- Agent modes: 3 — observe, ask, auto. Auto is the default and applies above a confidence threshold, asking below it (agent brainstorm, §7).
- Agent decision provider: hosted Jev if a key is set and online, local open model otherwise. Both vote on big calls (§7.2).
- Linux moves to v2.1. v2.0 is Windows-only. CI still builds and tests on Ubuntu and macOS with mock hardware.
- HWiNFO stays as an optional extra sensor source once the LHM bridge works.
- LLM for profile generation: OpenRouter by default; any OpenAI-compatible base URL (e.g. local Ollama) can be configured.

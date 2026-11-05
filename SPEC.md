# ThermalIQ v2 — Spec

An AI cooling agent for Windows that sets up, runs and improves your PC's cooling on its own, inside hard safety rules. One page of truth: if code and this file disagree, this file wins until it is changed on purpose.

Reference: the v0 code is tagged `v0-archive` (see `PORTING.md`). The audit that led to this rebuild: "ThermalIQ Codebase Audit — Continue, Rewrite, or V2?" (2026-11-04).

## 1. What v2.0 is

**The agent is the product** (decided by Sam, 2026-11-04). ThermalIQ is not a manual fan-control tool with an optional AI add-on. Instead of the user building curves and configuring every system fan, GPU fan, CPU cooler and AIO pump, the agent:

1. **Sets up:** finds every fan and pump, calibrates each one, and works out which fan cools what by spinning each and watching which temperatures change.
2. **Runs:** every second, a small learned model predicts temperatures and adjusts fans to the user's quiet ↔ cool goal, during gaming too. A Jev-style decision model (not an LLM) makes the bigger calls: which workload a new app is, whether to accept a learned curve.
3. **Improves over time:** learns from history which settings meet the user's temperature goals with the least noise, per workload, and keeps tuning.

All of this happens inside the SPEC §3 safety rules, which the agent can never override. Manual curves stay available as an advanced option. Underneath is a background service with a desktop UI, a CLI and a local API. Fan control, safety and the fast layer run entirely on the PC, with no telemetry. The main route for the decision model is hosted Jev with the user's own key, which first-run setup asks for prominently (about 3 cents a day). No key is shipped and there is no ThermalIQ proxy. Without a key, the agent falls back as described in §7.2.

## 2. Scope

| In v2.0 | Later (not v2.0) |
| --- | --- |
| Windows 10/11 x64 | macOS (helper, SMC) |
| NVIDIA GPU fans (NVML) | AMD GPU fans (ADLX) |
| Motherboard fans via LHM bridge (LibreHardwareMonitorLib + PawnIO, optional: §5.1) | Laptop EC / NBFC |
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
- **S6 Watchdog.** Any out-of-process helper (LHM bridge) restores auto control by itself if it gets no heartbeat from the daemon for 5 s (the daemon sends one every 1 s, §5.1).
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
bridges/lhm-bridge (later) .NET sidecar for Windows motherboard fans (§5.1), JSON lines over a named pipe
thermaliq-ui       (later) Tauri 2 + React
```

`HardwareLayer` (minimum): `snapshot()`, `set_fan_duty(fan, duty)`, `restore_auto(fan)`, `capabilities()`.
It is synchronous; the engine runs on its own thread (no async in the HAL).

One snapshot per tick, cached and shared. API reads use the cache, never the hardware directly.

### 5.1 LHM bridge (M12 brainstorm, Sam, 2026-11-05)

**Why a sidecar.** A .NET sidecar on LibreHardwareMonitorLib, not LHM's web server. That server has no watchdog, and in LHM 0.9.6 anyone who can reach it can set fan values, because its login is off by default.

**Shipping.** Self-contained .NET 10, with LibreHardwareMonitorLib pinned exactly to `0.9.7-pre742`, the first build with the NCT6687D restore fix (LHM `dc51e75`, #2527). Stable 0.9.6 can't be used: on Sam's board it left a header taken from the BIOS in manual at a stale duty, which breaks S5 and S6. The bridge exe lives under Program Files and must pass the M20c ACL check (`acl::replaceable_by_others`) before the daemon starts it, because it runs as SYSTEM.

**PawnIO.** Offered at setup using the official signed installer, never installed silently. Motherboard fans are optional: without PawnIO, the GPU and the agent work as normal.

**Protocol.**

- A named pipe only SYSTEM can open, carrying JSON lines.
- The daemon sends a heartbeat every 1 s. After 5 s with none, the bridge hands the fans back to the BIOS by itself (S6).
- Every bridge call has a 2 s timeout and returns an error instead of blocking.
- The bridge's watchdog can't run while a request is stuck inside LHM or PawnIO, so the daemon covers that case. A request unanswered for 5 s gets the bridge process killed and reported as `degraded`, and a new bridge hands the fans back from the saved originals. A bridge that won't die stays `degraded`, and no second one starts.
- The daemon checks that the pipe's server is the bridge process it just started, and connects at identification level, so a process that took the pipe name first can't impersonate it.

**Hand-back (Sam, 2026-11-05).**

- After each restore, the bridge reads the mode register back (`0xA00` on NCT6687D). If the header is still manual after 3 attempts, the bridge writes 100% to that header, and the daemon reports `degraded` and names it in `degraded_reasons`. That is a fail-safe for one header, not S4: one stuck header doesn't send every fan to 100%.
- Before first taking over a header, the bridge saves its original mode and duty under `%ProgramData%\ThermalIQ`, so a restarted bridge can restore them.
- A restarted bridge restores by using reflection on the pinned LHM build: it sets the three private save fields of `Nct677X` (`_initialFanControlMode`, `_initialFanPwmCommand`, `_restoreDefaultFanControlRequired`) from the saved originals, then calls the public `SetDefault`. It never writes registers itself.
- A startup self-check refuses to run the bridge if those fields or types differ. Motherboard fans are then absent; the GPU and agent carry on.

**Anti-cheat mode.** FACEIT has blocked PawnIO. PawnIO's maintainer reports that release 2.1.0 fixes this with a new signing certificate, which we have not tested yet. On detecting FACEIT or another blocker, ThermalIQ hands motherboard fans to the BIOS, stops the bridge, and unloads PawnIO if that works without a reboot. NVML GPU control continues. PawnIO unloads by disabling its PnP device (`ROOT\PAWNIO\0000`), which worked with no reboot on Sam's PC; `sc stop` is refused with error 1052 (`docs/dev/research/m12-spike/README.md`). ThermalIQ writes a marker under `%ProgramData%\ThermalIQ` before disabling it. On service start, if the marker shows we disabled PawnIO and no anti-cheat is running, it re-enables PawnIO and clears the marker. If an anti-cheat is running at service start, the bridge doesn't start, and motherboard fans stay off until the next service restart. Detection covers FACEIT only so far: the `FACEITService` service in any state but stopped, or a `faceitclient.exe` process. `FACEIT.exe` is the desktop app, loads no driver and doesn't count. The mode ends once the service has been stopped and no client has been seen for 30 s, without a restart if the bridge was already running. Tested with a fake `faceitclient.exe` only; real FACEIT is a pre-release item (PORTING M21).

A backend that should have started and didn't (the bridge failing to start, or NVML failing while another backend carries on) is listed in `unavailable_sources` and named in `degraded_reasons`, so `degraded` is true. One left off on purpose (no bridge installed, no PawnIO, anti-cheat running at start) is only listed in `unavailable_sources`. A snapshot with no fan at all is `degraded` too, whatever the cause: an empty engine must never look healthy (S8). The service writes everything it prints to `service.log` in the data dir.

While the bridge is stopped, its sensors are gone. S3 still applies: a GPU fan whose curve reads a bridge sensor goes to 100%. S2 sees a CPU temperature only if HWiNFO supplies one, and the BIOS has the CPU fan. `/status` lists `lhm-bridge` under `unavailable_sources`.

### 5.2 Fan and sensor identity (M13 brainstorm, Sam, 2026-11-06)

**Stable IDs.** An ID must survive a reboot, a reorder and a driver update.

- **NVIDIA:** `nvml_<uuid>_<name>` (`nvml_GPU-1b2c..._fan0`, `..._temp`, `..._load`). The UUID is NVML's, not its enumeration index. A GPU whose UUID can't be read is not started; the backend is reported as failed.
- **Motherboard:** the bridge's chip and channel (`lhm_nct6687d_0_fan_1`). The board fingerprint (the bridge's `hello.board`: LHM's motherboard name and the chip) goes in the saved inventory, not in the ID. Same IDs on a different board count as `changed`.
- **HWiNFO:** `hwinfo_<group>_<sensor>`, from the group and sensor names slugged to `[a-z0-9-]`, never HWiNFO's numeric ids, which shift. Two with the same name get `_2`, `_3` in HWiNFO's order.

**Inventory.** `inventory.json` in the data dir: the board fingerprint and every fan (id, label, kind). Checked at every start against the first snapshot: `new`, `missing`, `changed` (label, kind, or the board) are logged and listed in `GET /identity`. The first run saves the baseline. After that it changes only when the user accepts it (`POST /inventory/accept`, or finishing `identify`), so a fan that stays missing is reported until someone decides.

**Roles.** Bundled profiles name roles, not hardware. Fan roles: `cpu_fan`, `pump`, `case_front`, `case_rear`, `case_top`, `gpu_fan`. Sensor roles: `cpu_temp`, `gpu_temp`. `roles.toml` in the data dir maps a role to a real ID. When a profile loads, each role becomes its mapped ID. An unmapped role is skipped and reported by name, never silently:

- a control on an unmapped fan role is dropped;
- a curve on an unmapped sensor role is dead, and so is every control that uses it, plus any curve or custom sensor built on it;
- an alert on an unmapped sensor role is dropped.

A dropped fan goes to the BIOS or driver, not to a guess.

**A profile fan that no backend lists** (after mapping) is reported by name:

- at load: logged, and `degraded` with `degraded_reasons` naming the fan and the profile. The daemon still starts, because S2–S4 and the other fans must keep working;
- on activation: refused (`POST /profiles/active` and the agent's own switches), with an error naming the fan. The engine stays on the previous profile.

**First-run identification** (`thermaliq-daemon identify`, over the API). It only ever raises a fan, never lowers one, and a pump only goes up:

1. **Auto:** NVML GPU fans become `gpu_fan`. A header's LHM label proposes a role (`CPU Fan` → `cpu_fan`, `Pump` → `pump`). `cpu_temp` and `gpu_temp` are the sensors of kind Cpu and Gpu when there is exactly one.
2. **RPM pairing** (fans with a tach): the probe puts the fan at 100% for 5 s and compares every RPM. The fan whose RPM rose is the one it drives. A rise on another fan's tach is reported, never guessed.
3. **Listen** (fans with no tach reading, or no rise): the probe puts the fan at 100% for 5 s and the user answers front, rear, top or none. `none` leaves the fan unmapped.

A probe adds one flat-100% control for that fan to the active profile for 5 s, then restores the profile, so the engine stays the only writer (S1) and S2–S4 still win. Nothing is saved until the user confirms, and `roles.toml` is written atomically.

M13 is identity only. Which fan cools which component is M20d.

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
5. On first run only, ask prominently for the user's OpenRouter or TypeSafe key, and show the cost (about 3 cents a day). Without a key, routing uses the later steps in §7.2.

### 7.2 Runtime

**Fast layer: every tick (1 s), CPU only, microseconds, always on, including while gaming.**
- *Workload classifier:* small gradient-boosted trees over the inputs give idle / browsing / gaming / compiling / rendering / other. It is trained on history labelled by the slow layer and by the user's corrections.
- *Thermal model:* small, learned per component. It predicts each controlled temperature 10–30 s ahead from duty, load and power.
- *Live optimiser:* it adjusts each fan up or down from its curve output to meet the slider target. Ramping up early for a predicted spike counts as a **short boost**, which expires in ≤ 60 s. Limits: it may raise any fan freely. It may lower a fan by at most 15 points below the curve output, and only while every temperature that fan cools is under its target and predicted to stay there. Requests expire in 3 s and are renewed every tick.
  - *Over target now* (a real reading above the target, not a prediction): the request goes up 2 points per tick above the curve, up to 100, with no time cap. It is held while any temperature is within 2 °C of the target. Once every temperature has been 2 °C under target for 10 s, it eases back 1 point per tick to the curve. An unknown reading holds it. Predicted-only boosts keep +20 and the 60 s cap, and being over target does not use that window up. The thermal model gives no prediction for inputs outside the range it was fitted on (a fan spinning up reads duty 0 and a linear fit would predict +40 °C).
  - *Targets from the slider* (0 = quiet, 100 = cool): a straight line between two targets per component. GPU 80 → 65 °C, CPU 85 → 70 °C. At 50: GPU 72.5, CPU 77.5. Safety (S2–S4) never sees them.
  - *Which fan cools what:* until setup mapping (M20d) exists, a fan cools the component whose temperature sensor its profile curve reads. If the GPU, its load or its power cannot be named unambiguously, or no fan follows it, the agent leaves those fans to their curves.

**Slow layer: every 60 s and whenever the foreground app changes. Decision model, not an LLM.**
- Typed questions with confidence: the workload for an app not seen before (cached per exe afterwards); whether to accept a learned curve or a new profile.
- **Profile choice is a fixed rule, not a model question:** idle → silent, browsing → efficiency, gaming → gaming, compiling and rendering → performance, other → balanced. A workload answer at or above its confidence threshold selects the profile, and applying it is a slow-layer change (§7.4). Below the threshold, nothing changes and the §7.4 ask applies.
- **State sent to the model:** centred on the foreground exe: its name plus recent averages of load, power and temperature, not one instant reading. The averages cover the last 60 s of this exe in the foreground. An exe not seen before is asked about after 10 s in the foreground, so an alt-tab costs no call. A reading that isn't available is left out, never sent as 0.
- **Foreground app:** a service in session 0 can't see the desktop, so a small read-only helper runs in the logged-on user's session and reports the exe name and whether it fills the screen, once a second. It inherits only its output pipe and gets the user's environment, never a handle or the environment of the service. If it goes quiet for 3 s, the foreground counts as unknown: nothing is decided, and the local model stays out.
- **A rule-selected profile must cover this PC.** It applies only if it controls every real fan the active profile controls, names no unknown fan, and reads only sensors that exist. Otherwise nothing changes and the log says why, because a profile switch hands back any fan it drops, and a missing sensor trips S3.
- **Provider routing, in order:** (1) hosted Jev, when a key is set, the PC is online and an answer arrives within 2 s; (2) the local model, when the PC is idle; (3) a built-in list of well-known exes, for the workload question only; (4) ask the user. A built-in answer has no model confidence: it is a default the user can correct, it shows as "built-in" in the log, and it isn't cached, so a model can replace it once one is available.
- **Keys:** each user brings their own OpenRouter or TypeSafe key, stored in the OS keychain. No key is shipped and there is no ThermalIQ proxy.
- **Local model:** an open decision model (Strands Decider class; Clef-flash is deferred) behind an Ollaya-compatible endpoint (`TYPESAFE_BASE_URL`). It never runs while a game is in the foreground: if one comes to the foreground during a call, the call stops and the model unloads. When the PC is idle it may use up to half the logical processors at below-normal priority. **Idle** means no fullscreen foreground app, CPU < 30% and GPU < 20%, unbroken for 5 minutes. **A game** doesn't have to be recognised: a fullscreen foreground app, GPU load > 50%, an exe known as a game, or a foreground or GPU load that can't be read all count. It is named in `local-model.toml` in the data directory, which the API can't set, because the service runs it as SYSTEM. The service only runs an exe under `%ProgramFiles%`, and only if nobody but SYSTEM, Administrators or TrustedInstaller can write or own `%ProgramFiles%` or anything in the app's folder, checked at startup and before every start. Models are read from `<data dir>\models`. It is loaded on demand and unloaded after use, so it isn't resident in VRAM. It may use the GPU only while GPU load is < 50%.
- **Big calls** (accept a learned curve, create a profile): when both providers are available, both answer. Agreement → apply. Disagreement → ask the user. With only one provider available, a big call applies only at or above the big-change threshold (§7.4); otherwise it asks.
- Hosted requests contain numbers and exe names only. The user can turn hosted off at any time.
- Nothing can answer: the fast layer keeps running, the slow layer's calls queue as questions for the user, and nothing pretends to have decided (S8).

### 7.3 Learning (when the PC is idle)

Retrain the classifier and the thermal models on history. Search curve changes per workload that meet the slider target with less noise, using the thermal model, and only within validated profile rules (monotonic, within 0–100, at or above minimums). The results are proposals for the slow layer.

### 7.4 Autonomy and reporting

- **Modes:** *observe* (log what it would do), *ask* (propose every change), *auto* (**default**). In auto, a change is applied when the decision's confidence is ≥ its threshold and asked otherwise. Thresholds are higher for bigger changes, and per provider, because providers' confidence scales differ. They start as uncalibrated values (hosted: workload 0.7, big 0.85; local: workload 0.6, big 0.85), and M20e calibrates them with a Brier score from accept/undo history.
- **The agent may:** pick a profile per app, tune curves, create profiles, run short boosts and live adjustments. Slow-layer changes are limited to 6 per hour. Fast-layer adjustments are bounded by §7.2 instead.
- **Activity feed** in the UI and API: every applied change with its reason and confidence, one-click undo (restores the previous profile or curve exactly), and a Windows toast only when it needs the user to answer.

### 7.5 Evaluation (tests + M20 spike on Sam's PC)

- Decisions: Brier score and accuracy against labelled history and user accept/undo.
- Thermal model: mean error at 20 s ahead, ≤ 2 °C on held-out history, over windows where no input (load, power, duty) steps during the window. No model can foresee an unmeasured step, like a game starting. Errors over windows with a step are reported separately, not gated. Fitted coefficients must be physically possible (more duty never heats, more power never cools), or the fit is rejected: history is recorded in closed loop, where the curve sets duty from temperature, and that biases a naive fit.
- Live optimiser: in mock-hardware tests, it never lowers a fan whose temperature is over target or predicted over target, and never goes past the 15-point limit.
- Gaming cost: no FPS test (Sam, 2026-11-05). The local model never runs while a game is in the foreground, because of its memory use and load time (M20c spike on 16 GB: 7.8 GB, 37 s to first answer). Tests must prove that it doesn't start during a game and stops when one comes to the foreground.
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
- Agent decision provider: changed on 2026-11-05, see below. Both vote on big calls when both are available (§7.2).
- Linux moves to v2.1. v2.0 is Windows-only. CI still builds and tests on Ubuntu and macOS with mock hardware.
- HWiNFO stays as an optional extra sensor source once the LHM bridge works.
- LLM for profile generation: OpenRouter by default; any OpenAI-compatible base URL (e.g. local Ollama) can be configured.

Decided 2026-11-05 by Sam:

- The decision model no longer picks the profile. Profile answers were unreliable in the M20c spike, so a fixed workload → profile rule replaces that question, and the state centres on the exe (§7.2, `docs/dev/research/decision-models.md`).
- Agent decision provider: hosted Jev is the main route. Order: Jev (key set and online) → local when idle → built-in list of well-known exes → ask the user (§7.2).
- Keys: each user brings their own OpenRouter or TypeSafe key, stored in the OS keychain. First-run setup asks for it prominently, with the cost (about 3 cents a day). No key is shipped and there is no ThermalIQ proxy.
- Local model: never runs while a game is in the foreground (7.8 GB, 37 s load in the spike), so the FPS test is skipped. When the PC is idle it may use up to half the cores at below-normal priority. Clef-flash is deferred.

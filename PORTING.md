# Porting ledger

Work top to bottom, one module per session. v0 paths are relative to the reference worktree `../thermaliq-v0` (tag `v0-archive`).

Actions: **port** = bring the v0 logic across, re-read and re-tested · **rewrite** = new code, v0 is only a reference for intent · **new** = nothing in v0.

Tick a box only when its tests pass in CI and a reviewer has signed off (SPEC §9).

## Brainstorm before starting

Stop and talk these through with Sam (in claude.ai) before any code for the module. Claude Code: if the next unticked module is listed here and its box below is unchecked, ask Sam first.

- [x] **M20 / Agent:** brainstormed with Sam on 2026-11-04. The result is SPEC §7 and modules M20a–M20e below.
- [x] **Metrics (small, after the agent brainstorm):** inputs settled in SPEC §7, so no separate brainstorm is needed. the snapshot currently has temperatures only. Add CPU/GPU load %, power (W) and maybe clocks: HWiNFO already exposes them, NVML has GPU load and power. Extend M10, M11 and history. M12 should report load from day one.
- [ ] **M12 (large):** sidecar .NET vs LHM's built-in web server; shipping .NET (self-contained vs runtime); PawnIO install flow and anti-cheat conflicts; protocol and watchdog. Run a spike on Sam's PC first: prove one case fan moves.
- [ ] **M13 (medium):** first-run fan identification UX (spin each header, "which fan moved?"); stable ID scheme across hardware changes.
- [ ] **M18 (medium):** which screens matter; onboarding; keep v0 design or restyle; how a standard-user account gets the token. **Design in Claude Design first, code second:**
    1. Make a Design System artifact (colours, type, components): restyle from v0's look (dark, neon-green accent) or a fresh direction.
    2. Mock about 8 screens in Claude Design: dashboard (live temps and fans, safety status, "why is it loud?"), curve editor, first-run setup (load generator, mapping, optional mic), agent activity feed with undo, the quiet ↔ cool slider per workload, settings, the tray menu, and the emergency/latched state.
    3. Feed the mocks the real API fields (`/status`: `engine_running`, `latched`, `degraded`, `waiting_on`, `snapshot_age`; override `in_effect` + `reason`), so stale, latched and degraded states are designed, not invented.
    4. Claude Code then builds Tauri + React from the mocks: port v0 screens where they fit, rewrite the data layer against the real API, and use the design tokens.
- [x] **M20 research requirements** (done in the brainstorm; notes updated): **first, fully research and understand Jev**: a non-LLM "System One" decision model (typed probabilistic decisions in one parallel pass, RLCD-calibrated, not text generation). Then research non-LLM alternatives. Start from `docs/dev/research/decision-models.md` and re-check it, since the field is weeks old. The agent must use a decision model, not a chat LLM. Then: what the agent decides; approval-mode UX; model choice (local Strands Decider / Clef-flash via Ollaya, real Jev, or a small trained classifier); GPU contention while gaming; privacy of what it sees; how to evaluate decisions (Brier, accuracy, latency on Sam's PC); offline behaviour.
- [ ] **Gaps (small), before Phase 4:** M15's exact profile-arbitration rules; M21 code-signing certificate (cost, SmartScreen).

## Phase 1 — Safety core

- [x] **M1 Common types + profile/config schema** — rewrite · v0: `thermaliq-common/src/{lib,config}.rs` · test: every bundled profile parses; invalid profiles give a readable error; no `ai/predictive/learning/auto` variants
- [x] **M2 HAL trait + mock** — rewrite · v0: `thermaliq-hal/src/{lib,mock}.rs` · test: mock temps can be scripted per tick; mock records every write and `restore_auto` call
- [x] **M3 Curves + custom sensors** — port linear/flat/graph/custom sensors, fix mix/sync, rewrite trigger · v0: `thermaliq-daemon/src/poller.rs` · test: mix of a CPU curve at 75 °C equals the CPU curve at 75 °C; trigger respects `response_ms` and hysteresis
- [x] **M4 Engine: priority, ramping, 0-RPM** — rewrite · v0: `poller.rs`, `agent_task.rs` · test: a lower-priority request never overwrites a higher one; overrides expire
- [x] **M5 Engine: safety S2–S5, S7** — new · v0: `agent_task.rs::check_safety` (intent only) · test: 97 °C holds 100% for 30 s; sensor missing 3 ticks → 100%; drop/panic calls `restore_auto` on every touched fan

> Gate check (mock): all S-tests green before Phase 2.

## Phase 2 — Daemon, paths, auth

- [x] **M6 Paths + config + token** — rewrite paths/config, port token · v0: `thermaliq-daemon/src/token.rs`, `thermaliq-common/src/config.rs` · test: atomic write; ProgramData path on Windows; env override for dev
- [x] **M7 History DB** — port schema, add prune + batching · v0: `thermaliq-common/src/history.rs` · test: one transaction per tick; prune keeps 24 h raw; recorded-sensor set covers all saved profiles
- [x] **M8 HTTP API + WebSocket** — port router/auth/ws, rewrite handlers · v0: `thermaliq-daemon/src/api/*` · test: every mutation rejects missing token; no handler returns success without effect; reads use the snapshot cache (M8a #38 + M8b #39, reviewed, CI-green)
- [x] **M9 Windows service** — port, add graceful stop · v0: `thermaliq-daemon/src/service.rs` · test (Windows CI): stop path calls engine shutdown, not `abort()`; data dir `ThermalIQ\` has inheritance broken (SYSTEM + Administrators full, Users read-only), so a standard user cannot drop a config.toml or profile the service loads; token.txt readable only by users allowed to control fans (M6 review); service stop must finish within the SCM timeout even if a HAL call is stuck (M8a review)
- [x] **M10 NVIDIA backend** — port + fix · v0: `thermaliq-hal/src/windows.rs` (`nvml_*`) · fix: load + init once; `restore_auto` = default fan speed; every HAL call has a timeout (~2 s) and returns an error instead of blocking, else a stuck `snapshot()`/`set_fan_duty()` hangs shutdown and S5 (M8a review)
- [x] **M11 HWiNFO sensor source (optional)** — port · v0: `thermaliq-hal/src/windows.rs` (`Hwinfo*`) · keep its tests (#42, hardware-tested and approved)

- [x] **G1 Safety gate** (SPEC §8) — passed on the Windows PC, RTX 4060, 2026-11-04: `scripts/g1.ps1`, all 9 steps; reviewed and approved on PR #43.

## Phase 2b — Agent core (after Metrics; works with the NVIDIA fan alone)

Order: Metrics → M20a → CPU load → M20b → M20c → Phase 3. New crate `thermaliq-agent`: pure logic, no HAL. The daemon wires it, and it only submits to the engine (S1).

- [x] **Metrics** — done, PR #45: CI green on Windows/macOS/Ubuntu, reviewed in a fresh session, `/metrics` captured on the 4060 PC (2026-11-05) · load %, power, clocks, GPU throttle reasons in M10/M11 and history (see the brainstorm list) · test: NaN/missing metrics are skipped, not zeroed · also add `FanReading.kind` (fan / pump; NVML fans = fan): the agent must never stop a pump (SPEC §7), and profile validation must reject 0-RPM on a pump
- [x] **M20a Thermal model + workload classifier** — done, PR #46: CI green on Windows/macOS/Ubuntu, reviewed in a fresh session (2026-11-05) · new · SPEC §7.2 fast layer · test: on recorded and mock history, 20 s-ahead error ≤ 2 °C; the classifier trains and predicts within the < 1 ms tick budget; stale or missing inputs → "unknown", never a guess
- [x] **CPU load (built-in, Windows)** — done, PR #48: CI green on Windows/macOS/Ubuntu, reviewed in a fresh session, `/metrics` captured on the 4060 PC (2026-11-05) · new, small · so the agent works without HWiNFO · `GetSystemTimes` busy time (idle vs kernel + user deltas), which is the same quantity as PDH `% Processor Time`. Not Task Manager's `% Processor Utility`: that one is frequency-scaled and goes over 100% under turbo, and clocks/power already carry frequency. Metric kind `cpu_load`, id `win_cpu_load`; HWiNFO's `cpu_load`, if present, is dropped in favour of it (one source per metric). CPU power still needs HWiNFO, or LHM in M12 · test: the delta math on fixed counter values, the first call (no previous sample) → no reading, the counter wrapping or not moving → no reading, never 0
- [x] **M20b Live optimiser + short boosts (logic)** — done, PR #49: CI green on Windows/macOS/Ubuntu, reviewed in a fresh session (2026-11-05); not wired into the daemon and not run on real fans, see M20b wiring below · new · SPEC §7.2 · test: never lowers a fan whose temperature is over target or predicted over it; never more than 15 points below the curve output; never below the profile minimum; never touches pumps; requests expire in 3 s and the profile resumes if the agent stops; boosts ≤ 60 s; the latch always wins · engine additions (M4/M5 code, tests first): expose each fan's profile-curve output to the agent; **the engine itself** clamps an Agent request to ≥ curve − 15 (defence in depth, not only the agent's own check); `Engine::withdraw_all(Source::Agent)` for pause and agent crash · **carry-overs from the M20a review:** (1) `ThermalModel::fit` accepts `w[duty] == 0`, so M20b may only *lower* a fan using a model whose duty effect is meaningfully negative (a 10-point duty drop must raise the 20 s prediction by ≥ 0.1 °C), otherwise the model counts as unknown and the fan is raise-only (add a test); (2) store `ahead` in the model and reject a prediction with a different `ahead`; (3) a fan that sat at one duty gives "no model" until it has varied, which is safe: expect it · the hardware run moved to M20b wiring
- [ ] **M20b wiring: agent thread in the daemon** — new · SPEC §7.2, §7.4 · `thermaliq-daemon`: an agent thread builds bindings (explicit ids; NVML ids, HWiNFO only if one GPU can be named), fits a `ThermalModel` per component from the last 24 h of history (split at gaps) and refits hourly, runs one `Optimiser` per fan each tick and submits to the engine (S1), and calls `withdraw_all(Source::Agent)` on pause and when the thread dies. `POST /agent/pause` and `/agent/resume` (token); agent state in `/status`. Slider → targets: straight line quiet → cool, GPU 80 → 65 °C, CPU 85 → 70 °C (SPEC §7.2) · test: the targets at slider 0/50/100; no model → raise-only; pause and a panicking agent thread withdraw its requests and the profile resumes; pause/resume need the token; `/status` shows the state; the engine keeps S2–S4 · **then, on Sam's PC:** a game run, and re-run `scripts/g1.ps1` with the agent active (G1 must still pass)
- [ ] **M20c DecisionProvider + slow layer** — new · v0: `ai_provider.rs` (intent only) · SPEC §7.2 routing (hosted Jev with a key, local otherwise, 2 s timeout), both vote on big calls, per-exe cache, 60 s / app-change cadence, 6 changes per hour · test: no provider → calls queue as questions, nothing faked (S8); disagreement → ask; hosted payload has numbers and exe names only · **spike first on Sam's PC:** local model latency on CPU, and FPS cost in a game < 2% (SPEC §7.5)

## Phase 3 — Windows motherboard fans

- [ ] **M12 LHM bridge** — new · .NET sidecar on LibreHardwareMonitorLib + PawnIO, JSON over a named pipe, 5 s watchdog (S6) · test: protocol tests; watchdog restores auto; every bridge call has a timeout (~2 s) and errors instead of blocking, else shutdown hangs (M8a review)
- [ ] **M13 Stable fan IDs + first-run mapping** — new · v0 lesson: HWiNFO IDs shift with sensor index · test: IDs survive reorder; bundled-profile role IDs (`cpu_package`, `chassis1`, ...) map to real hardware IDs; NVIDIA ids (`nvml_gpu{index}_...`, M10) depend on NVML enumeration order, so key them on the GPU UUID (M10 review); first-run mapping must make a bundled profile drive the real fan (G1 needed a temporary profile); switch g1.ps1 to the mapped profile

> **G3** on the Windows PC.

## Phase 4 — Features

- [ ] **M14 `tq` CLI** — port · v0: `thermaliq-cli/src/main.rs`
- [ ] **M15 Profiles, app rules, schedule** — port schedule, fix app rules (read from base profile/config, not the active one) · v0: `schedule_monitor.rs`, `app_monitor.rs` · test: also, which profile is active is priority-arbitrated (agent > app rule > schedule > user), not last-switch-wins (M5 re-review) · **carry-over from the M20b review:** the engine's agent floor is `profile curve − 15`. Once App rules and Schedule submit duties, the baseline under Agent may be one of those: the floor must be `max(lower-priority non-agent requests) − 15`, or the agent can undercut an app rule by more than 15 (add a test)
- [ ] **M16 Alerts** — port `AlertEvaluator` (has cooldowns) and run its actions · v0: `thermaliq-common/src/alert.rs` · add fan health and dust alerts (SPEC §7.6: RPM −15% at the same duty, +5 °C at the same load, over 30 days; alerts only, they never change control)
- [ ] **M18 UI** — port screens, rewrite data layer (token via Tauri command, real endpoints only) · v0: `thermaliq-ui/` · then **G2** · M9 review: `token.txt` is readable by the user who ran `install-service`; if that is a separate admin account (UAC with another user) and the PC is used as a standard user, the UI cannot read the token, so M18 needs a way to name the token user · tray icon (SPEC §7.6): state colour, one-click agent pause that works with the UI window closed
- [ ] **M19 LLM profile generation** — port prompt, rewrite provider (one OpenAI-compatible client, timeouts, keychain key) · v0: `profile_gen.rs`

## Phase 5 — Agent

- [ ] **M20d Setup: load generator, calibration and fan-to-component mapping** — new (replaces the "calibration module" gap) · SPEC §7.1, §7.6 · after M12/M13 · test: the load generator stops on the latch, on target + 5 °C, on cancel and at 60 s, and never starts without setup; calibration runs at priority 2 and expires; pumps are never stopped; a clear mapping is applied, an unclear one is asked; it writes valid starting profiles per workload
- [ ] **M20e Learning, autonomy modes, activity feed** — new · v0: `agent_task.rs` (intent only) · SPEC §7.3–7.4 · after M15/M18 · test: observe/ask/auto; confidence thresholds; undo restores the previous profile byte for byte; learned curves pass profile validation; toast only when it needs an answer; every fan in `/fans` has a "why" reason that matches the deciding source (SPEC §7.6), from templates, never generated text (needs an engine method that reports which source, or which safety rule, set each fan this tick)
- [ ] **M20f Acoustic calibration (optional mic)** — new · SPEC §7.6 · after M20d · test: off unless the user opts in; no audio stored or sent (only the RPM → dB table is saved); quiet mode prefers measured dB and skips flagged resonance bands; with no mic, it falls back to RPM-based quiet

## Phase 6 — Release

- [ ] **M21 Release pipeline** — port · v0: `.github/workflows/release.yml`, `packaging/`, Tauri updater config

## Later (v2.1)

- **Linux hwmon** — port reads, fix writes (`pwmN_mode` for DC, save/restore `pwm_enable`, match chips by path) · v0: `thermaliq-hal/src/linux.rs` · drop NBFC EC writes

## Dropped (do not port)

Curve types `auto`, `ai`, `predictive`, `learning` · fake settings handlers (trust level, guardrails, telemetry) · battery switching claims · macOS helper (v2.1) · AMD ADL Overdrive5 · NBFC EC writes · oh-my-claudecode config and `.omc/` state · generated Tauri schemas

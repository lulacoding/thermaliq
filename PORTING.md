# Porting ledger

Work top to bottom, one module per session. v0 paths are relative to the reference worktree `../thermaliq-v0` (tag `v0-archive`).

Actions: **port** = bring the v0 logic across, re-read and re-tested · **rewrite** = new code, v0 is only a reference for intent · **new** = nothing in v0.

Tick a box only when its tests pass in CI and a reviewer has signed off (SPEC §9).

## Brainstorm before starting

Stop and talk these through with Sam (in claude.ai) before any code for the module. Claude Code: if the next unticked module is listed here and its box below is unchecked, ask Sam first.

- [ ] **M12 (large):** sidecar .NET vs LHM's built-in web server; shipping .NET (self-contained vs runtime); PawnIO install flow and anti-cheat conflicts; protocol and watchdog. Run a spike on Sam's PC first: prove one case fan moves.
- [ ] **M13 (medium):** first-run fan identification UX (spin each header, "which fan moved?"); stable ID scheme across hardware changes.
- [ ] **M18 (medium):** which screens matter; onboarding; keep v0 design or restyle; how a standard-user account gets the token.
- [ ] **M20 (large):** **first, fully research and understand Jev**: a non-LLM "System One" decision model (typed probabilistic decisions in one parallel pass, RLCD-calibrated, not text generation). Then research non-LLM alternatives. Start from `docs/dev/research/decision-models.md` and re-check it, since the field is weeks old. The agent must use a decision model, not a chat LLM. Then: what the agent decides; approval-mode UX; model choice (local Strands Decider / Clef-flash via Ollaya, real Jev, or a small trained classifier); GPU contention while gaming; privacy of what it sees; how to evaluate decisions (Brier, accuracy, latency on Sam's PC); offline behaviour.
- [ ] **Gaps (small), before Phase 4:** a calibration module (not in this ledger yet); M15's exact profile-arbitration rules; M21 code-signing certificate (cost, SmartScreen).

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
- [ ] **M11 HWiNFO sensor source (optional)** — port · v0: `thermaliq-hal/src/windows.rs` (`Hwinfo*`) · keep its tests

> **G1** on the Windows PC (SPEC §8).

## Phase 3 — Windows motherboard fans

- [ ] **M12 LHM bridge** — new · .NET sidecar on LibreHardwareMonitorLib + PawnIO, JSON over a named pipe, 5 s watchdog (S6) · test: protocol tests; watchdog restores auto; every bridge call has a timeout (~2 s) and errors instead of blocking, else shutdown hangs (M8a review)
- [ ] **M13 Stable fan IDs + first-run mapping** — new · v0 lesson: HWiNFO IDs shift with sensor index · test: IDs survive reorder; bundled-profile role IDs (`cpu_package`, `chassis1`, ...) map to real hardware IDs; NVIDIA ids (`nvml_gpu{index}_...`, M10) depend on NVML enumeration order, so key them on the GPU UUID (M10 review)

> **G3** on the Windows PC.

## Phase 4 — Features

- [ ] **M14 `tq` CLI** — port · v0: `thermaliq-cli/src/main.rs`
- [ ] **M15 Profiles, app rules, schedule** — port schedule, fix app rules (read from base profile/config, not the active one) · v0: `schedule_monitor.rs`, `app_monitor.rs` · test: also, which profile is active is priority-arbitrated (agent > app rule > schedule > user), not last-switch-wins (M5 re-review)
- [ ] **M16 Alerts** — port `AlertEvaluator` (has cooldowns) and run its actions · v0: `thermaliq-common/src/alert.rs`
- [ ] **M18 UI** — port screens, rewrite data layer (token via Tauri command, real endpoints only) · v0: `thermaliq-ui/` · then **G2** · M9 review: `token.txt` is readable by the user who ran `install-service`; if that is a separate admin account (UAC with another user) and the PC is used as a standard user, the UI cannot read the token, so M18 needs a way to name the token user
- [ ] **M19 LLM profile generation** — port prompt, rewrite provider (one OpenAI-compatible client, timeouts, keychain key) · v0: `profile_gen.rs`

## Phase 5 — Agent

- [ ] **M20 Agent + DecisionProvider** — rewrite · v0: `agent_task.rs`, `ai_provider.rs` (intent only) · test: fallback when no endpoint; rate limit; requests go through engine at priority 3

## Phase 6 — Release

- [ ] **M21 Release pipeline** — port · v0: `.github/workflows/release.yml`, `packaging/`, Tauri updater config

## Later (v2.1)

- **Linux hwmon** — port reads, fix writes (`pwmN_mode` for DC, save/restore `pwm_enable`, match chips by path) · v0: `thermaliq-hal/src/linux.rs` · drop NBFC EC writes

## Dropped (do not port)

Curve types `auto`, `ai`, `predictive`, `learning` · fake settings handlers (trust level, guardrails, telemetry) · battery switching claims · macOS helper (v2.1) · AMD ADL Overdrive5 · NBFC EC writes · oh-my-claudecode config and `.omc/` state · generated Tauri schemas

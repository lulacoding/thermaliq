# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-05 — M20b built on branch `m20b-live-optimiser` (PR open, NOT ticked: needs CI green + fresh-session review, then Sam's hardware run). Not run on real fans.
- **Next:** review/merge M20b; then the M20b hardware run (game + `scripts/g1.ps1` with the agent active). Then M20c. M12 (LHM) is Phase 3 and must report load and pump `FanKind` from day one.
- **M20b engine:** `Engine::curve_output(fan)` (None before first tick, when latched/S3/S4); `Engine::withdraw_all(Source)`; Agent `submit` to a known pump is an `Err`; each tick the engine raises any Agent request to ≥ curve − `AGENT_MAX_LOWER_PCT` (15, in `thermaliq-common`), and drops Agent requests while the curve has no input. Latch/S3/S4 bypass requests entirely.
- **M20b agent:** `optimiser.rs`: `Optimiser::decide(now, curve, slider, pump, &[Cooled]) -> Option<Request{duty, expires_at = now+3s}>`, one `Optimiser` per fan. Lowers only if every cooled temp is known, under target, model present, `lowering_is_safe`, and still under target `AHEAD` (20) ticks after lowering. Boost = curve+20 when over/predicted over target, ≤ 60 s, re-arms after 60 s without need, no lowering right after a boost. `ThermalModel::predict(now, ahead)` now refuses a different `ahead`.
- **M20b gaps (daemon wiring, not done):** nobody calls the optimiser yet. The daemon must: build bindings, fit models per fan (split at gaps), pick `target` per temp from the slider (SPEC gives no formula; `shortcut:` in optimiser.rs), submit each tick with `expires_at`, call `withdraw_all(Source::Agent)` on pause/crash. Pump kind comes from the snapshot.
- **M20a shape:** crate `thermaliq-agent` (deps: common only). `inputs.rs` bindings pick readings by explicit id AND kind, missing/invalid → `None`. `model.rs` ridge fit on ONE gap-free run, rejects `w[duty] > 0`/`w[power] < 0`. `classifier.rs` boosted stumps, `None` below `MIN_CONFIDENCE` 0.6. HWiNFO ids don't say which GPU: daemon needs the group-to-GPU mapping (or NVML ids). Snapshot has no timestamp: don't pass a stale one.
- **Metrics:** `MetricReading{id,kind,value:f64}`; invalid → skipped, never 0. NVML ids `nvml_gpu{n}_{load,power,..}`. RTX 4060 NVML power is "not available", HWiNFO fills in. CPU load: built-in `win_cpu_load` (>64 logical processors reads one group).
- **Pumps:** engine never writes a pump below `PUMP_MIN_DUTY` (20, placeholder: M20d). Nothing reports a pump yet (NVML = fan).
- **G1 facts:** `gate-tests` feature (`inject.rs`). `g1.ps1` sets `THERMALIQ_INJECT_CPU_TEMP`, resets fans via NVML. Crash recovery: SCM Restart after 1 s x3.
- **Known gap (M13):** bundled profiles use role IDs (`gpu1`, `chassis1`), so v2 drives no real fan out of the box.
- **Shapes:** `app.rs` `hardware()`; `runtime.rs` `Daemon::start(...)`; `api.rs` `ApiState::new(...)`; `store.rs`. `PUT /config` and new profiles need a restart (M15).
- **Run in foreground on the PC:** `$env:THERMALIQ_DATA_DIR="$env:TEMP\tiq"; cargo run -p thermaliq-daemon` (stop the `ThermalIQ` service first; port 7272).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default. Dev: Windows PC (primary), Mac secondary.

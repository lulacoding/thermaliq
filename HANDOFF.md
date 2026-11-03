# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M3 ticked (PR #32). M4 (`FanControl`) on branch `m4-engine-priority-ramping`, PR into `v2`; not ticked: needs fresh-session review + CI.
- **Next:** review M4 PR, tick M4 if OK, then M5 (safety S2–S5, S7).
- **M4 shape:** `thermaliq-engine::FanControl::new(&Control)`, `submit(Source, duty, expires_at) -> Result`, `tick(now) -> Option<u8>` (`None` = no request; M5 decides). `Source` order: Override > Agent > AppRule > Schedule > Profile; Override/Agent must expire. Order per tick: priority → 0-RPM → min_pwm floor → clamp → ramp.
- **0-RPM (v0 semantics, PR #30):** running fan stops at duty `<= stop_percent`; stopped fan restarts only when duty `> start_percent`. Stopped output is 0, still ramped down by `step_down_pct`. First tick has no ramp history and jumps to target. Not yet written in SPEC.
- **Not done in M4:** the engine loop that owns one `FanControl` per fan, feeds `CurveEngine`, writes via the HAL (M5/M6). S3 3-tick counting, S2 latch (M5). Safety sits above `Source` and does not ramp.
- **M3 shape:** `SensorEngine::new(&Profile).evaluate(&Temps, now) -> Temps`; `CurveEngine::duty(curve_id, &Temps, now) -> Option<u8>` (`None` = input invalid → M5 applies S3).
- **M2 shape:** `HardwareLayer` is sync; `MockHardwareLayer::new(fan_ids, Vec<Tick>)` with `read_only`, `fail_writes`, `fail_restores`.
- **Open:** bundled profile IDs are role IDs; M13 maps them to real hardware.
- **Left out earlier:** schedule (M15), agent/LLM settings (M19/M20), paths (M6).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.

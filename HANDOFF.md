# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M4 review applied on PR #33 (branch `m4-engine-priority-ramping`); not ticked: needs re-review + CI.
- **Next:** re-review M4 PR, tick M4 if OK, then M5 (safety S2–S5, S7).
- **M4 shape:** `FanControl::new(&Control)`, `submit(Source, duty, expires_at) -> Result`, `withdraw(Source)`, `seed(duty, spinning)`, `tick(now) -> Option<u8>`. `Source`: Override > Agent > AppRule > Schedule > Profile; Override/Agent must expire. Per tick: priority → 0-RPM → min_pwm floor → clamp → ramp.
- **M5 loop must:** `seed` once from the first snapshot (`duty_percent`, `rpm > 0`); each tick `withdraw(source)` when that source's curve gives `None` (else its old duty stays in force); `tick() == None` = no request, apply S3. Safety sits above `Source` and does not ramp.
- **0-RPM (SPEC §4):** stop at duty `<= stop_percent`; restart only `> start_percent`, jumping to at least `start_percent`, then ramping. Stopped output is 0, ramped down by `step_down_pct`.
- **Later (calibration):** an Override below `min_pwm` is floored, so a calibration sweep can't measure low duties; decide when calibration gets a module.
- **M3 shape:** `SensorEngine::new(&Profile).evaluate(&Temps, now) -> Temps`; `CurveEngine::duty(curve_id, &Temps, now) -> Option<u8>` (`None` = input invalid).
- **M2 shape:** `HardwareLayer` is sync; `MockHardwareLayer::new(fan_ids, Vec<Tick>)` with `read_only`, `fail_writes`, `fail_restores`.
- **Open:** bundled profile IDs are role IDs (M13 maps them). Left out earlier: schedule (M15), agent/LLM (M19/M20), paths (M6).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.

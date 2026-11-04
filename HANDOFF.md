# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M4 ticked (PR #33). M5 written on `m5-engine-safety` (`engine.rs`); PR into v2, NOT ticked: needs fresh-session review + CI.
- **Next:** review the M5 PR, tick M5 if OK, then M6 (paths).
- **M4 shape:** `FanControl::new(&Control)`, `submit(Source, duty, expires_at) -> Result`, `withdraw(Source)`, `seed(duty, spinning)`, `tick(now) -> Option<u8>`. `Source`: Override > Agent > AppRule > Schedule > Profile; Override/Agent must expire. Per tick: priority → 0-RPM → min_pwm floor → clamp → ramp.
- **M5 shape:** `Engine::new(Arc<dyn HardwareLayer>, &Profile)`, `tick(now)`, `submit(fan, Source, duty, expires)`, `degraded()`, `latched()`. Drop = S5. `write()` is the only `set_fan_duty` call: skips read-only fans, clamps to 100. S3 = curve `None` 3 ticks, recover after 3 good. S2/S4 seed FanControl at 100 so release ramps down. NaN blocks latch release. Failed writes/restores are ignored (`shortcut:`, no logging yet).
- **0-RPM (SPEC §4):** stop at duty `<= stop_percent`; restart only `> start_percent`, jumping to at least `start_percent`, then ramping. Stopped output is 0, ramped down by `step_down_pct`.
- **Later (calibration):** an Override below `min_pwm` is floored, so a calibration sweep can't measure low duties; decide when calibration gets a module.
- **M3 shape:** `SensorEngine::new(&Profile).evaluate(&Temps, now) -> Temps`; `CurveEngine::duty(curve_id, &Temps, now) -> Option<u8>` (`None` = input invalid).
- **M2 shape:** `HardwareLayer` is sync; `MockHardwareLayer::new(fan_ids, Vec<Tick>)` with `read_only`, `fail_writes`, `fail_restores`.
- **Open:** bundled profile IDs are role IDs (M13 maps them). Left out earlier: schedule (M15), agent/LLM (M19/M20), paths (M6).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.

# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M5 ticked (PR #34). M6 built on `m6-paths-config-token`, PR into v2; NOT ticked: needs review + CI.
- **Next:** review the M6 PR, tick M6, then M7. Daemon: never call `std::process::exit` while an `Engine` is alive (skips Drop = S5); keep `panic = "unwind"`.
- **M6 shape:** `thermaliq-daemon/src/store.rs`: `data_dir()` (env `THERMALIQ_DATA_DIR`, else `%ProgramData%\ThermalIQ`), `write_atomic`, `load_config` (missing = default, invalid = error), `save_config`, `load_or_create_token` (64 hex, malformed is replaced). No token compare yet (M8).
- **M6 risk:** token file has no ACL. ProgramData is world-readable by default, so any local user can read the token. M9 (service install) must lock the directory down. No 0600 on unix since v2 is Windows-only.
- **M4/M5 shape:** `Engine::new(Arc<dyn HardwareLayer>, &Profile)`, `set_profile`, `tick(now)`, `submit(fan, Source, duty, expires)`, `degraded()`, `latched()`. Drop = S5. Source: Override > Agent > AppRule > Schedule > Profile.
- **0-RPM (SPEC §4):** stop at duty `<= stop_percent`; restart only `> start_percent`, then ramp.
- **Later (calibration):** an Override below `min_pwm` is floored, so a calibration sweep can't measure low duties.
- **M2/M3 shape:** `HardwareLayer` is sync; `MockHardwareLayer::new(fan_ids, Vec<Tick>)`. `SensorEngine::new(&Profile).evaluate(&Temps, now)`; `CurveEngine::duty(...)`.
- **Open:** bundled profile IDs are role IDs (M13 maps them). Schedule (M15), agent/LLM (M19/M20).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.

# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M6 ticked (PR #35). M7 history DB written on branch `m7-history`, PR into v2. NOT ticked: needs review + CI.
- **Next:** review the M7 PR, tick M7, then M8. Daemon: never call `std::process::exit` while an `Engine` is alive (skips Drop = S5); keep `panic = "unwind"`.
- **M7 shape:** `thermaliq-daemon/src/history.rs`: `HistoryDb::{open, in_memory}`, `record_tick(now, &[SensorReading], &[FanReading])` (one transaction), `prune(now)` (>90d dropped, >7d averaged to 1/min), `sensor_rows`/`fan_rows`. Timestamps are passed in (unix secs).
- **M7 risk:** nothing calls `prune` yet; M8 wiring must schedule it (hourly) and wrap `HistoryDb` in a Mutex (Connection is not Sync). v0 calibration table not ported: belongs to the calibration module.
- **M6 shape:** `thermaliq-daemon/src/store.rs`: `data_dir()` (env `THERMALIQ_DATA_DIR`, else `%ProgramData%\ThermalIQ`), `write_atomic`, `load_config`, `save_config`, `load_or_create_token`. No token compare yet (M8). Writes use a fixed `.tmp` name: serialize writes in M8. Token file has no ACL yet (M9).
- **M4/M5 shape:** `Engine::new(Arc<dyn HardwareLayer>, &Profile)`, `set_profile`, `tick(now)`, `submit(fan, Source, duty, expires)`, `degraded()`, `latched()`. Drop = S5. Source: Override > Agent > AppRule > Schedule > Profile.
- **0-RPM (SPEC §4):** stop at duty `<= stop_percent`; restart only `> start_percent`, then ramp.
- **Later (calibration):** an Override below `min_pwm` is floored, so a calibration sweep can't measure low duties.
- **Open:** bundled profile IDs are role IDs (M13 maps them). Schedule (M15), agent/LLM (M19/M20).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.

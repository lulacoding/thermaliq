# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M2 ticked (PR #31). M3 (curves + custom sensors) on branch `m3-curves-custom-sensors`, PR #32 into `v2`; review fixes applied, not ticked: needs re-review + CI.
- **Next:** review M3 PR, tick M3 if OK, then M4 (next unticked item in PORTING.md).
- **M3 shape:** `thermaliq-engine`: `SensorEngine::new(&Profile).evaluate(&Temps, now) -> Temps` (real + custom sensors; any missing/invalid input → sensor absent); `CurveEngine::new(&Profile).duty(curve_id, &Temps, now) -> Option<u8>` (`None` = input missing/invalid → M4 applies S3, never guess). `now` is `Duration` since start. Trigger is stateful: dead band between idle/load holds state; a switch needs `response_ms` of continuous want.
- **Decided this session:** `file` custom sensor removed from common and moved to v2.1 (SPEC §2).
- **Not done in M3 (later modules):** M4 = priority, ramping, min_pwm, 0-RPM; M5 = safety S2–S5, S7 (incl. S3 3-tick counting). Review fixes applied: custom sensor never overwrites a real reading; `time_s` capped 1..=3600.
- **M2 shape:** `HardwareLayer` is sync (`snapshot`, `set_fan_duty(&str, u8)`, `restore_auto(&str)`, `capabilities`); `MockHardwareLayer::new(fan_ids, Vec<Tick>)` with `read_only`, `fail_writes`, `fail_restores`; `writes()`/`restores()` record raw calls.
- **Open:** bundled profile IDs are role IDs; M13 maps them to real hardware.
- **Left out of M1 (add with their module):** schedule (M15), agent/LLM settings (M19/M20), paths (M6).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0 (Linux → v2.1), HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.
- **Reference code:** `git worktree add ../thermaliq-v0 v0-archive`

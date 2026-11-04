# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M11 ticked (#42). G1 passed on hardware (RTX 4060) and ticked; PR #43 reviewed, approved and squash-merged into v2.
- **Next:** agent brainstorm, in chat with Sam (M20 line in PORTING.md: three parts, decision model vs learning loop, inputs; research Jev fully first). Then Metrics. Then M12 (LHM).
- **G1 facts:** `gate-tests` feature (`inject.rs`, wraps `app::hardware()`, `compile_error!` in release). `THERMALIQ_INJECT_CPU_TEMP` = a number or a file path, read every snapshot; g1.ps1 puts the path in the service's registry `Environment` and rewrites the file. Armed with no CPU sensor = a cool (40 C) `inject_cpu` sensor, so a latch can release.
- **Crash recovery:** `service::install` sets SCM Restart after 1 s x3 (needs `CHANGE_CONFIG | START` on the handle). Only a process death restarts; a self-reported startup failure does not. `service-smoke.ps1` checks `sc qfailure` on Windows CI.
- **Known gap (M13):** bundled profiles use role IDs (`gpu1`, `chassis1`), so out of the box v2 drives no real fan. g1.ps1 installs a temporary flat `g1` profile on `nvml_gpu0_fan0` (hardcoded: other fans of a multi-fan GPU stay automatic, so the gate fails honestly). M13 must map a bundled profile to the real fan and g1.ps1 must switch to it.
- **g1.ps1:** PASS/FAIL/SKIP per step; `finally` uninstalls, removes the injection file and g1 profile, restores config.toml (only if it wrote it), and resets every fan to automatic via NVML. NVML duty readback lags: it waits up to 20 s for >= 98%.
- **Shapes (still true):** `app.rs` `hardware()` = `backend()` (NVIDIA required + HWiNFO optional) + gate wrapper; `start(dir, hal, Token)`, `App::stop(budget)`; `runtime.rs` `Daemon::start(hal, &active, saved, history, Options)`; `api.rs` `ApiState::new(...)`, `ApiServer::start(state, port)`; `store.rs` `data_dir`, `load_config`, `load_token`, `write_atomic`. `PUT /config` and new profiles need a restart (M15).
- **HWiNFO:** `hwinfo.rs` `parse` is safe code; `hwinfo_win.rs` is the only unsafe. Optional sources via `CombinedHardwareLayer::with_optional`, `unavailable_sources` in `/status`. HWiNFO fans not exposed.
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default. Dev machine: Windows PC (primary), Mac secondary; setup `scripts/dev-setup.ps1` / `.sh`.

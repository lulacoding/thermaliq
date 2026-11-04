# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M10 ticked (#41). M11 (HWiNFO) on branch `m11-hwinfo`, PR into v2. NOT ticked: needs CI on all 3 OSes, a fresh-session review, and the hardware test run on a PC with HWiNFO64 (it was NOT run: no HWiNFO on the dev PC).
- **Next:** review the M11 PR; on the PC run `cargo test -p thermaliq-hal -- --ignored --nocapture hwinfo` with HWiNFO64 running + "Shared Memory Support" on. Check the printed names/kinds, the unit, and that `poll_time` is Unix seconds (stale = older than 30 s). Add real names you see as fixtures. Tick M11 after both. Then M12 (LHM).
- **M11 shape:** `hwinfo.rs`: `parse(&[u8], now) -> HardwareSnapshot` is safe code on a byte copy (bounds-checked, runs on every OS); `Hwinfo::with_reader(fn)` for tests, `Hwinfo::new()` (Windows) re-opens the mapping every snapshot. `hwinfo_win.rs` is the only unsafe (windows-sys: OpenFileMapping, MapViewOfFile, VirtualQuery, copy out), SAFETY on every block.
- **Kinds:** from HWiNFO's group name: `CPU [#` -> Cpu, `GPU [#` -> Gpu, else Other. Temperatures only (no fans). "Distance to TjMAX" skipped. Fixture names are from memory of real HWiNFO output, not captured.
- **IDs:** `hwinfo_{sensor_id:08x}_{inst}_{reading_id:08x}` (stable across reorder, unlike v0's sensor index). Label `"{label} ({group})"`.
- **Optional sources:** `CombinedHardwareLayer::with_optional(name, backend)` (must own no fans). Its failure drops only its sensors; trait method `HardwareLayer::unavailable_sources()` -> `Status.unavailable_sources` -> `/status` and ws frames (`unavailable_sources`, also under `status`). Required backends still fail the snapshot (S4).
- **Gaps:** a profile that reads an `hwinfo_*` sensor sees it vanish while HWiNFO is down (engine's missing-sensor path). Units: C accepted, F converted, others skipped (review must-fix 1; fixtures only, real unit bytes unseen). HWiNFO fans not exposed.
- **M10/M9 (still true):** `app.rs` `hardware()` builds NVIDIA (required) + HWiNFO (optional); `start(dir, hal, Token)`, `App::stop(budget)`; NVML worker thread with 2 s call timeout, `nvml_gpu{G}_temp`, fan writes need an elevated process and NVML duty readback follows the fan's slew. Service ACLs in `acl.rs` (tests need elevation). `PUT /config` and new profiles still need a restart (M15).
- **Older shapes:** `runtime.rs` `Daemon::start(hal, &active, saved, history, Options)`; `api.rs` `ApiState::new(...)`, `ApiServer::start(state, port)`; `store.rs` `data_dir`, `load_config`, `load_token`, `write_atomic`.
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default. Dev machine: Windows PC (primary), Mac secondary; setup `scripts/dev-setup.ps1` / `.sh`.

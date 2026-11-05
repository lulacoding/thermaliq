# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-05 — M20a built on branch `m20a-thermal-classifier` (PR open into v2). NOT ticked in PORTING: needs CI green + fresh-session review.
- **Next:** merge M20a after review, tick it, then M20b (live optimiser, SPEC §7.2) per PORTING Phase 2b order. M12 (LHM) is Phase 3 and must report load and pump `FanKind` from day one.
- **M20a shape:** new crate `thermaliq-agent` (deps: common only). `inputs.rs`: `ComponentBinding{component,temp_id,load_id,power_id,fan_ids}.sample(&snap)` and `WorkloadBinding.features(&snap)` pick readings by explicit id AND metric kind (never "the first gpu_power"); anything missing/invalid gives `None`. `model.rs`: `ThermalModel::fit(&[Sample], ahead)` (ridge, dT = w·[1,temp,duty,load,power]) and `predict`. `classifier.rs`: `Classifier::train(&[([f64;4],Workload)])`, boosted stumps; `classify` returns `None` for unknown.
- **M20a gaps:** the daemon must build the bindings. HWiNFO metric ids (`hwinfo_{sid}_{inst}_{rid}`) don't say which GPU, so the daemon needs the HWiNFO group-to-GPU mapping (or NVML ids only) before binding. The snapshot has no timestamp: the caller must not pass a stale one. The ≤ 2 °C test runs on a synthetic plant with inputs held 120 s, scored on steady windows. Real-history accuracy is the M20 spike (SPEC §7.5).
- **Metrics:** `MetricReading{id,kind,value:f64}` in `HardwareSnapshot.metrics`; `MetricKind::is_valid` else skipped, never 0. NVML ids `nvml_gpu{n}_{load,power,..}`. RTX 4060 NVML power is "not available", so HWiNFO "GPU Power (Total)" fills in. HWiNFO fallback is all-or-nothing (`shortcut:` in `combined.rs`).
- **Pumps:** `FanReading.kind`. Engine never writes a pump below `PUMP_MIN_DUTY` (20, placeholder: M20d calibration), `check_profile` rejects `stop_percent` on a known pump. Nothing reports a pump yet (NVML = fan).
- **G1 facts:** `gate-tests` feature (`inject.rs`). `g1.ps1` sets `THERMALIQ_INJECT_CPU_TEMP` in the service registry, resets fans via NVML. Crash recovery: SCM Restart after 1 s x3.
- **Known gap (M13):** bundled profiles use role IDs (`gpu1`, `chassis1`), so v2 drives no real fan out of the box. M13 must map a bundled profile to the real fan and g1.ps1 must switch to it.
- **Shapes:** `app.rs` `hardware()`; `runtime.rs` `Daemon::start(...)`; `api.rs` `ApiState::new(...)`; `store.rs` `data_dir`, `load_config`, `write_atomic`. `PUT /config` and new profiles need a restart (M15).
- **Run in foreground on the PC:** `$env:THERMALIQ_DATA_DIR="$env:TEMP\tiq"; cargo run -p thermaliq-daemon` (stop the `ThermalIQ` service first; port 7272).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default. Dev: Windows PC (primary), Mac secondary.

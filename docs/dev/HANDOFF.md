# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M9 ticked (#40). M10 (NVIDIA backend) on branch `m10-nvidia-backend`, PR into v2. NOT ticked: needs CI on all 3 OSes, a fresh-session review, and a run of the hardware test on the Windows PC.
- **Next:** review the M10 PR; run `cargo test -p thermaliq-hal -- --ignored nvidia` in an elevated shell on the GPU PC (spins GPU fan 0 to 60 % for 5 s, then restores). Tick M10 after both. Then M11 (HWiNFO) / M12 (LHM): add each as one more `Arc<dyn HardwareLayer>` in `app::hardware()`.
- **M10 shape:** `thermaliq-hal/src/nvidia.rs`: `Nvidia::new()` (Windows) / `Nvidia::with_api(factory, timeout)` (any OS, fake `Nvml` trait in tests). One worker thread owns NVML (nvml.dll loaded + init once). Every call waits 2 s then errors. A write that waited past its deadline is dropped, a `restore_auto` still runs late. Dropping `Nvidia` never joins the worker.
- **IDs:** sensor `nvml_gpu{G}_temp` (Gpu), fans `nvml_gpu{G}_fan{F}` (rpm via `fan_speed_rpm`, 0 on old drivers).
- **Combined:** `combined.rs` `CombinedHardwareLayer::new(Vec<Arc<dyn HardwareLayer>>)`: merges snapshots (any backend error = snapshot error, S4), routes by `capabilities()`, refuses duplicate fan ids. `app::hardware()` on Windows builds it from NVIDIA; no backend up = refuses to start (CI has no GPU: use `--features mock-hardware` there).
- **unsafe:** none in this repo; NVML is called through `nvml-wrapper` 0.13 (Windows-only dep). Its own internal unsafe is third-party.
- **Gaps:** engine seeds a fan as running from `rpm > 0` (`engine.rs` ~248): a GPU on an old driver reports rpm 0, so it starts "stopped" until the first write. Hung NVML: the worker stays stuck, every later call errors (S4 then drives other fans to 100 %); a restart is the only recovery. Laptop GPUs with no fan report zero fans.
- **M9 (still true):** `app.rs` `start(dir, hal, Token)`, `App::stop(budget)`; service in `service.rs`, ACLs in `acl.rs` (tests need elevation, FAIL on CI if not); `scripts/service-smoke.ps1` uses mock hardware. Service refuses to start without `token.txt`. `PUT /config` and new profiles still need a restart (M15). DB/prune/HAL errors are not logged yet.
- **Older shapes:** `runtime.rs` `Daemon::start(hal, &active, saved, history, Options)`; `api.rs` `ApiState::new(...)`, `ApiServer::start(state, port)`; `store.rs` `data_dir`, `load_config`, `load_token`, `write_atomic`.
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default. Dev machine: Windows PC (primary), Mac secondary; setup `scripts/dev-setup.ps1` / `.sh`.

# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M8a (daemon wiring) on branch `m8a-daemon-wiring`, PR into v2. NOT ticked: needs fresh-session review + CI. M8 stays unticked until M8b too.
- **Next:** review the M8a PR, then M8b (HTTP API + WebSocket: port v0 router/auth/ws, rewrite handlers). Never call `std::process::exit` while an `Engine`/`Daemon` is alive (skips Drop = S5); keep `panic = "unwind"`.
- **M8a shape:** `thermaliq-daemon/src/runtime.rs`: `Daemon::start(hal, &active, saved_profiles, Arc<Mutex<HistoryDb>>, Options{tick, prune_every})`, `cache() -> Cache{snapshot, status: Status{engine_running, latched, degraded, waiting_on}, ticks, snapshot_age}` (age 0 = fresh; `engine_running=false` after stop/panic, then the rest is stale: M8b must expose both), `shutdown()` (joins engine thread, then history thread; `Drop` does the same). `load_profiles(dir) -> (profiles, skipped)` = bundled + `<dir>/profiles/*.toml`.
- **M8a design:** engine thread ticks, then publishes the cache and `try_send`s the tick to a history thread (queue 64, full = drop; engine never waits on the DB). History thread records and prunes every `prune_every`. Only fresh snapshots are recorded. Engine gained `last_snapshot`, `snapshot_fresh`, `latch_waiting_on`.
- **M8a gaps for M8b:** no command channel yet, so the API can't switch profile or `submit` overrides; add one into the engine thread loop. `saved` profile list is fixed at start (history won't pick up newly saved profiles' sensors until restarted). `main.rs` is still the skeleton (no real HAL until M10; no Ctrl-C/service until M9). DB/prune errors are not logged yet. HAL call timeouts are in PORTING M9/M10/M12 (a stuck call hangs shutdown).
- **M8b rules:** every mutation needs Bearer (constant-time compare, M6 left it out); reads from `Daemon::cache()`/history Mutex; `save_config` writes serialized by one lock (fixed `.tmp` name); S8: no success without effect.
- **M6/M7 shape:** `store.rs`: `data_dir()`, `write_atomic`, `load_config`, `save_config`, `load_or_create_token`. `history.rs`: `HistoryDb::{open, in_memory, record_tick, prune, sensor_rows, fan_rows}`, `recorded_sensor_ids`.
- **M4/M5 shape:** `Engine::new(Arc<dyn HardwareLayer>, &Profile)`, `set_profile`, `tick(now)`, `submit(fan, Source, duty, expires)`. Source: Override > Agent > AppRule > Schedule > Profile.
- **Later:** an Override below `min_pwm` is floored (calibration). Bundled profile IDs are role IDs (M13). Token file has no ACL yet (M9).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0, HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.

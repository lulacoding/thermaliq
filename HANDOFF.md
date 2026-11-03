# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M1 ticked (PR #30). M2 (HAL trait + mock) written on branch `m2-hal-trait-mock`, PR into `v2`. Not ticked: needs fresh-session review + CI.
- **Next:** review M2 PR, tick M2 if OK, then M3 (next unticked item in PORTING.md).
- **M2 shape:** `HardwareLayer` is sync (`snapshot`, `set_fan_duty(&str, u8)`, `restore_auto(&str)`, `capabilities`), errors are `HalError(String)`. `MockHardwareLayer::new(fan_ids, Vec<Tick>)`: one `Tick` consumed per snapshot, last repeats, `Tick::Fail` for S4 tests; `writes()`/`restores()` record every call raw (no clamping; the engine clamps, S7).
- **Open for M3:** `file` custom sensor reads any path as SYSTEM on Windows — allowlist it or move to v2.1. Bundled profile IDs are role IDs; M13 maps them to real hardware.
- **Left out of M1 (add with their module):** schedule (M15), agent/LLM settings (M19/M20), paths (M6).
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0 (Linux → v2.1), HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.
- **Reference code:** `git worktree add ../thermaliq-v0 v0-archive`

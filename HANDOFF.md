# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M1 review fixes applied (PR #30). Not ticked: re-review + CI first.
- **Next:** review M1 PR, tick M1 if OK, then M2 — HAL trait + mock.
- **M1 shape:** `Profile` (sensors/curves/controls/alerts/app_rules) and `DaemonConfig` (port, poll interval, log level, default profile). Strict TOML: unknown fields/curve types rejected; ranges, refs, cycles validated. 5 bundled profiles in `thermaliq-common/profiles/`.
- **Deliberately left out of M1 (add with their module):** schedule (M15), agent/LLM settings (M19/M20), paths (M6). v0 dead fields dropped: `offset_pct`, fan `mode`, `version`. (`start/stop_percent` are kept: 0-RPM hysteresis, needed by M4.)
- **Open for M3:** `file` custom sensor reads any path as SYSTEM on Windows — allowlist it or move to v2.1. Bundled profile IDs are role IDs; M13 maps them to real hardware.
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0 (Linux → v2.1), HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.
- **Reference code:** `git worktree add ../thermaliq-v0 v0-archive`

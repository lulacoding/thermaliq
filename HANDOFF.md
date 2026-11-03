# Handoff

Rewrite this file at the end of every session. Keep it under 15 lines.

- **Last session:** 2026-11-04 — M1 written (branch `m1-common-schema`, PR into `v2`). Not ticked: needs CI on 3 OSes + fresh-session review.
- **Next:** review M1 PR, tick M1 if OK, then M2 — HAL trait + mock.
- **M1 shape:** `Profile` (sensors/curves/controls/alerts/app_rules) and `DaemonConfig` (port, poll interval, log level, default profile). Strict TOML: unknown fields/curve types rejected; ranges, refs, cycles validated. 5 bundled profiles in `thermaliq-common/profiles/`.
- **Deliberately left out of M1 (add with their module):** schedule (M15), agent/LLM settings (M19/M20), paths (M6). v0 dead fields dropped: `offset_pct`, `start/stop_percent`, fan `mode`, `version`.
- **Open question for M2/M3:** `FanReading.duty_percent` is `u8` and `HardwareSnapshot` ids are `String` — revisit if the HAL needs more.
- **Decided (SPEC §10):** 3 agent modes, Windows-only v2.0 (Linux → v2.1), HWiNFO optional, OpenRouter default.
- **Dev machine:** Windows PC (primary). Mac secondary. Setup: `scripts/dev-setup.ps1` / `.sh`.
- **Reference code:** `git worktree add ../thermaliq-v0 v0-archive`

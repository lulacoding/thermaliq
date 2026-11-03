# ThermalIQ v2

Safe, local-first fan control for Windows, with an optional AI agent. Linux follows in v2.1.

**Status:** rebuild in progress. Nothing here controls fans yet. See `PORTING.md` for what is actually done.

- `SPEC.md` — what v2 is, scope, safety rules
- `PORTING.md` — module checklist (the only progress tracker)
- `HANDOFF.md` — where the last session stopped
- v0 code: tag `v0-archive`

## Dev setup

```bash
scripts/dev-setup.sh      # macOS / Linux
scripts/dev-setup.ps1     # Windows
cargo test --workspace
```

MIT licensed.

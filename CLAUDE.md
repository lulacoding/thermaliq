# ThermalIQ — AI-Native Thermal Management

**Stack:** Rust (Tokio), SQLite, Tauri 2 + React 18  
**Status:** v0.2.0 released, Phase 3.3 complete (PR #10 pending), v1.0 Windows launch May 15, 2026

## Session Start

Always read these in order:
1. **`.claude/context/status.md`** — current phase, blockers, next steps
2. **`.claude/context/decisions.md`** — architectural choices (why things are done a certain way)
3. **`.claude/context/lessons.md`** — code patterns Claude gets wrong (corrections)

Extended context: `~/dev/brain/projects/thermaliq/{overview,research-findings,implementation-notes}.md`

## Quick Commands

```bash
cargo build --features mock-hardware    # Build with mock HAL (no hardware control)
cargo test --features mock-hardware     # Run tests
cargo clippy                            # Lint
cargo doc --open                        # Generate docs
```

## Architecture Snapshot

- **5 crates:** thermaliq-{common,hal,daemon,cli,ui}
- **10 fan curves:** linear, flat, graph, mix, trigger, sync, auto, ai, predictive, learning
- **AI agent:** 3 modes (Passive/Approval/Autonomous), 6-intervention/hour rate limit
- **Emergency limits:** 95°C CPU / 90°C GPU → 100% fan (hardcoded)
- **18 REST endpoints** + WebSocket real-time streams
- **SQLite audit trail** of every AI intervention

## Critical Blockers

- PR #10 awaiting review (gates Phase 3.4 and v1.0)
- Windows HAL incomplete (HWiNFO64 shared memory parsing)
- SMJobBless macOS helper unimplemented (fan writes advisory-only on M-series)
- GUI components incomplete (CurveEditor, Settings, CalibrationWizard)
- Zero frontend unit tests

## Key Files

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace root |
| `thermaliq-daemon/src/main.rs` | Daemon entry, AppState setup |
| `thermaliq-hal/src/{macos,linux,windows}.rs` | Platform HALs |
| `thermaliq-daemon/src/ai_provider.rs` | Multi-provider AI abstraction |
| `docs/ROADMAP.md` | Full v1.0 roadmap (phases 0–5) |
| `docs/api.md` | REST/WebSocket API reference |

## Security

- Bearer token auth (32-byte hex, constant-time comparison)
- CORS locked to `tauri://localhost` (not `http://localhost:1420` in production)
- Never commit `.env`, API keys, or HWiNFO token paths

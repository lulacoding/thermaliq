# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — 2025-04-15

### Added
- **10 fan curve types**: linear, flat, graph, mix, trigger, sync, auto, ai, predictive, learning
- **TOML profile system**: version-controlled fan profiles with human-readable syntax
- **Five bundled profiles**: balanced, performance, silent, gaming, efficiency
- **AI agent**: 24/7 workload detection (idle, gaming, compiling, encoding, stress) with Passive / Approval / Autonomous modes
- **Agent memory database**: SQLite audit log for thermal samples, interventions, workload sessions, and profile stats
- **Safety enforcer**: hardcoded 95°C CPU / 90°C GPU emergency limit — fans forced to 100% unconditionally
- **Rate limiting**: maximum 6 agent interventions per hour to prevent thrashing
- **WebSocket streaming**: `/ws/sensors` (1 Hz sensor/fan data) and `/ws/agent` (agent state events)
- **CLI (`tq`)**: full command surface — `sensors`, `fans`, `status`, `profile`, `agent`, `history`, `alert`, `stop`
- **REST API**: complete CRUD for profiles, agent mode, interventions, fan control, history
- **Intervention approval flow**: propose → approve/reject with full audit trail
- **macOS HAL**: IOKit/SMC integration with Apple Silicon fallback via `sysctl`
- **Linux HAL**: hwmon sysfs reader/writer for temperature sensors and PWM fan control
- **Tauri 2 + React UI**: desktop app with Dashboard, Agent Panel, Profile Manager, Alert Center
- **History retention**: sensor and fan readings persisted to SQLite with pruning
- **Multi-platform CI**: Ubuntu + macOS build/test/clippy/fmt/audit matrix
- **Release CI**: automated multi-platform artifact packaging on git tags
- **docs/api.md**: full HTTP + WebSocket API reference
- **docs/profiles.md**: complete profile and curve type format guide

### Changed
- Workspace version bumped to `0.2.0`
- CLI binary renamed to `tq`
- API port is now `7272`

### Fixed
- `history_fans` endpoint was returning wrong data — now calls `query_fan_last_n`
- CLI agent commands now POST to correct API paths
- All 10 curve types now evaluate correctly in the poll loop (including Mix/Sync cross-referencing)
- Alert evaluation is now wired into the polling loop

## [0.1.0] — 2025-04-14

### Added
- Initial workspace structure: thermaliq-common, thermaliq-hal, thermaliq-daemon, thermaliq-cli
- Core data types: `SensorReading`, `FanReading`, `HardwareSnapshot`, `SetFanDuty`
- `HistoryDb`: SQLite-backed time-series for sensor and fan readings
- `AlertEvaluator`: threshold-based alert engine with cooldown
- `AgentStateMachine`: 9-state machine for thermal intelligence lifecycle
- `DtDtGuard`: thermal runaway detection (dT/dt)
- Bearer token authentication middleware for the HTTP API
- axum 0.7 HTTP server foundation with CORS restricted to `tauri://localhost`
- Mock hardware layer for CI and hardware-free development (`--features mock-hardware`)
- `mock-hardware` feature flag propagated from workspace root through HAL crate
- CI/CD pipeline with GitHub Actions (Linux, macOS matrix)

### Fixed
- macOS temperature sensor spec: corrected `Ftst` write mechanism documentation

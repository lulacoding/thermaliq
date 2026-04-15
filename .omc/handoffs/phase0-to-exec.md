## Handoff: team-plan → team-exec

- **Decided**: 3 parallel implementation workers after config foundation is laid; no Tauri UI this session (too large); focus on daemon correctness + CLI UX
- **Rejected**: Splitting daemon into two workers (file conflict risk on main.rs/auth.rs); full v0.2 Tauri UI (separate milestone)
- **Risks**: worker-daemon must not conflict with worker-cli on Cargo.toml — worker-config handles workspace root changes first
- **Files**: See individual worker tasks below
- **Remaining**: Tauri + React UI (v0.3 milestone)

### What exists (do NOT re-implement):
- `thermaliq-common/src/lib.rs` — SensorReading, FanReading, HardwareSnapshot, SetFanDuty
- `thermaliq-common/src/history.rs` — HistoryDb (SQLite, WAL, insert_sensor/fan, query_sensor_last_n)
- `thermaliq-common/src/alert.rs` — AlertAction, TemperatureAlert, AlertEvaluator, FiredAlert
- `thermaliq-common/src/curve.rs` — FanMode, CurvePoint, FanCurve, FanChannelConfig, FanController
- `thermaliq-common/src/agent.rs` — AgentState, AgentTransition, AgentStateMachine, DtDtGuard
- `thermaliq-hal/src/lib.rs` — HardwareLayer trait, platform_hal() factory
- `thermaliq-hal/src/mock.rs` — MockHardwareLayer (3 sensors, 2 fans, AtomicU8 duties)
- `thermaliq-daemon/src/token.rs` — load_or_create(), chmod 0600, ~/.config/thermaliq/api.token
- `thermaliq-daemon/src/api/auth.rs` — AppState { api_token, hal }, require_bearer_auth middleware
- `thermaliq-daemon/src/api/routes.rs` — /health /api/version /sensors /fans /fans/:id/pwm /agent/start stub
- `thermaliq-daemon/src/main.rs` — minimal startup (token + HAL + AppState + serve)

### Architecture source of truth:
`/Users/samalesci-bateman/dev/Thermaliq/ThermalIQ_Architecture_Report_v0.2.docx`
`/Users/samalesci-bateman/dev/Thermaliq/.omc/plans/v010-architecture.md`

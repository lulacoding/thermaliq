# ThermalIQ — Complete Feature Specification
# "Announcement Ready" Product Definition

Generated: 2026-04-14 | Author: Orchestrator (Autopilot Phase 0→1)
Source: Architecture doc v0.2 + full codebase audit

---

## 1. Product Vision

**ThermalIQ** is an AI-native, cross-platform fan control daemon. It achieves full feature parity
with FanControl (the Windows gold standard) while adding a defining innovation: a **24/7 autonomous
AI agent** that monitors thermal patterns, detects workloads, and intervenes within configurable
safety bounds — no user action required.

**Target audience:**
- Power users wanting fine-grained fan control (replacing FanControl/Mac Fan Control/lm-sensors)
- Developers/creators running sustained workloads (compiling, encoding, gaming)
- Overclockers who want AI-optimised cooling without constant manual tuning
- Server/workstation operators who need silent autonomous thermal management

**Platforms:** Linux (x86_64, ARM64), macOS Intel, macOS Apple Silicon (sensors read-only), Windows x86_64

---

## 2. Complete Feature Set

### 2.1 Hardware Interface (HAL)

| Feature | Status | Notes |
|---------|--------|-------|
| Linux hwmon sysfs (`/sys/class/hwmon/`) | ❌ Stub | Read temp* + fan* files, write pwm* |
| macOS SMC via IOKit | ❌ Stub | CPU/GPU temps, fan RPM + duty |
| macOS Apple Silicon | ❌ Stub | Read-only sensors via powermetrics |
| Windows LibreHardwareMonitor / WMI | ❌ Stub | Full read+write |
| Mock HAL (CI/dev) | ✅ Built | Simulates sensors + fans |
| NVIDIA GPU (NVML) | ❌ Missing | 30% PWM minimum enforced |
| AMD GPU (ADLX / WMI) | ❌ Missing | Windows; Linux via hwmon |
| Fan calibration (RPM sweep) | ❌ Missing | 0→100% sweep, detect start%/stop%/RPM range |
| Per-fan RPM→PWM mapping | ❌ Missing | Stored in calibration DB |

### 2.2 Fan Curve Types (10 total)

| Curve | Status | Description |
|-------|--------|-------------|
| `linear` | ✅ Built | Min/max temp → min/max PWM, interpolated |
| `flat` | ✅ Built | Constant duty |
| `graph` | ✅ Defined, eval fixed | Arbitrary breakpoints, interpolated |
| `mix` | ✅ Defined, eval fixed | Combine curves: Max/Min/Average/Sum |
| `trigger` | ✅ Built | Step function idle→load |
| `sync` | ✅ Defined, eval fixed | Mirror another fan ± offset |
| `auto` | ✅ Built | Adaptive linear between idle/load temps |
| `ai` | ✅ Defined | Agent-controlled, goal-based |
| `predictive` | ❌ Missing | Uses thermal history to anticipate spikes |
| `learning` | ❌ Missing | Adapts curve points from workload patterns |

### 2.3 Custom Sensor Types (4 total)

| Type | Status | Description |
|------|--------|-------------|
| `time_average` | ✅ Defined | Rolling N-second average |
| `mix` | ✅ Defined | Combine sensors: Max/Min/Average/Sum/Subtract |
| `file` | ✅ Defined | Read temp from arbitrary path |
| `offset` | ✅ Defined | Add constant offset to source sensor |
| Custom sensor **evaluation** in poll loop | ❌ Missing | Types defined but not computed |

### 2.4 Profile System

| Feature | Status |
|---------|--------|
| TOML format (human-readable, git-friendly) | ✅ Built |
| Profile: `balanced` | ✅ Built |
| Profile: `silent` | ✅ Built |
| Profile: `performance` | ✅ Built |
| Profile: `gaming` | ❌ Missing |
| Profile: `efficiency` | ❌ Missing |
| Live profile switching (no restart) | ✅ Built |
| Profile list/show via API | ✅ Built |
| Profile import/export via CLI | ❌ Missing |
| Profile version migration | ❌ Future |

### 2.5 Alert System

| Feature | Status |
|---------|--------|
| Temperature threshold alerts | ✅ Built (AlertEvaluator) |
| Severity: warn / crit | ✅ Built |
| Action: log | ✅ Built |
| Action: desktop notify (Tauri) | ❌ Missing (requires UI) |
| Action: apply profile | ✅ Built (in config types) |
| Action: send webhook | ❌ Missing |
| Action: shutdown | ❌ Missing (privilege escalation needed) |
| Cooldown (anti-spam) | ✅ Built |
| AlertEvaluator wired into poll loop | ✅ Fixed (Wave 1A) |
| Alert history | ❌ Missing |
| CLI alert management | ❌ Missing |

### 2.6 24/7 AI Agent

| Feature | Status |
|---------|--------|
| Persistent Tokio task in daemon | ✅ Built |
| Mode: Passive (observe only) | ✅ Built |
| Mode: Approval (queue interventions) | ✅ Built |
| Mode: Autonomous (apply within safety bounds) | ✅ Built |
| **AgentStateMachine** (9 states) | ✅ Built |
| **DtDtGuard** (thermal runaway dT/dt) | ✅ Built |
| **Agent memory DB** (agent.db) | ❌ Missing |
| thermal_samples table (7-day rolling) | ❌ Missing |
| interventions table (audit log) | ❌ Missing |
| workload_log table (session patterns) | ❌ Missing |
| profile_stats table (performance metrics) | ❌ Missing |
| **Workload detection**: idle | ❌ Missing |
| **Workload detection**: gaming (GPU spike + sustained CPU) | ❌ Missing |
| **Workload detection**: compiling (many-core sustained CPU) | ❌ Missing |
| **Workload detection**: encoding (sustained mixed CPU/GPU) | ❌ Missing |
| **Workload detection**: stress test | ❌ Missing |
| **Pre-cooling** (anticipate load based on patterns) | ❌ Missing |
| **Safety enforcer** hard limits | ❌ Missing |
| max_cpu_temp = 90°C | ❌ Missing |
| max_gpu_temp = 85°C | ❌ Missing |
| emergency_temp = 95°C (all fans 100%) | ❌ Missing |
| max_pwm_change_per_step = 20 | ❌ Missing |
| max_interventions_per_hr = 6 | ❌ Missing |
| **Intervention audit log** (before/after snapshots) | ❌ Missing |
| **Rollback** capability | ❌ Missing |
| Intervention approval via API | ✅ Built |
| Intervention approval via CLI | ✅ Built |
| Intervention **rejection** | ❌ Missing |
| Agent state machine wired into agent_task.rs | ❌ Partial (state machine in common but not used in task) |

### 2.7 REST API (port 7272)

| Endpoint | Status |
|----------|--------|
| GET /health | ✅ Built |
| GET /api/version | ✅ Built |
| GET /sensors | ✅ Built |
| GET /fans | ✅ Built |
| POST /fans/:id/pwm | ✅ Built |
| GET /api/profiles | ✅ Built |
| GET /api/profiles/active | ✅ Built |
| POST /api/profiles/active | ✅ Built |
| GET /api/agent/status | ✅ Built |
| POST /api/agent/mode | ✅ Built |
| GET /api/agent/interventions | ✅ Built |
| POST /api/agent/interventions/:id/approve | ✅ Built |
| POST /api/agent/interventions/:id/reject | ❌ Missing |
| GET /api/agent/interventions/:id | ❌ Missing (detail view) |
| GET /api/agent/history | ❌ Missing |
| GET /api/history/sensors/:id | ✅ Built |
| GET /api/history/fans/:id | ✅ Fixed (Wave 1A) |
| GET /api/history/sensors/:id?from=&to=&limit= | ❌ Missing (time range params) |
| GET /api/calibration/:fan_id | ❌ Missing |
| POST /api/calibration/:fan_id/run | ❌ Missing |
| POST /api/alerts | ❌ Missing (runtime alert management) |
| POST /api/shutdown | ✅ Built |
| **WebSocket /ws/agent** | ❌ Missing |
| **WebSocket /ws/sensors** | ❌ Missing |

### 2.8 CLI (tq binary)

| Command | Status |
|---------|--------|
| tq sensors | ✅ Built |
| tq fans | ✅ Built |
| tq status | ✅ Built |
| tq profile list | ✅ Built |
| tq profile show \<name\> | ✅ Built |
| tq profile set \<name\> | ❌ Missing |
| tq fan \<id\> --pwm \<n\> | ✅ Built |
| tq agent start | ✅ Fixed (Wave 1A) |
| tq agent approve \<id\> | ✅ Fixed (Wave 1A) |
| tq agent reject \<id\> | ❌ Missing |
| tq agent status | ❌ Missing |
| tq agent mode \<passive\|approval\|autonomous\> | ❌ Missing |
| tq agent interventions | ❌ Missing |
| tq agent history | ❌ Missing |
| tq history sensors \<id\> [--limit] | ❌ Missing |
| tq history fans \<id\> [--limit] | ❌ Missing |
| tq calibrate \<fan_id\> | ❌ Missing |
| tq alert list | ❌ Missing |
| tq alert add | ❌ Missing |
| tq alert remove | ❌ Missing |
| tq stop | ✅ Fixed (Wave 1A) |

### 2.9 Tauri GUI

| Feature | Status |
|---------|--------|
| Tauri 2 + React + TypeScript + Vite | ❌ Not started |
| Dashboard: real-time temp charts | ❌ Not started |
| Dashboard: fan speed gauges (RPM + duty%) | ❌ Not started |
| Fan control: manual duty override per fan | ❌ Not started |
| Profile manager: list + activate | ❌ Not started |
| Curve editor: graphical drag-and-drop | ❌ Not started |
| Agent panel: mode selector | ❌ Not started |
| Agent panel: live state (Sampling/Analyzing/etc.) | ❌ Not started |
| Agent panel: intervention log + approve/reject | ❌ Not started |
| Alert center: active alerts + history | ❌ Not started |
| Calibration wizard | ❌ Not started |
| Settings page (daemon config, startup) | ❌ Not started |
| System tray icon + quick-access | ❌ Not started |
| macOS launchd startup | ❌ Not started |
| Linux systemd service | ❌ Not started |
| Windows service | ❌ Not started |
| Desktop notifications (alert actions) | ❌ Not started |

### 2.10 Integrations

| Feature | Status |
|---------|--------|
| Claude Code skill (`tq agent` commands) | ❌ Missing |
| OpenClaw webhook on agent events | ❌ Missing |
| Homebrew formula (macOS) | ❌ Missing |
| AUR PKGBUILD (Arch Linux) | ❌ Missing |
| MSI installer (Windows) | ❌ Missing |

### 2.11 Documentation

| Document | Status |
|----------|--------|
| README.md (public-facing, with shields/demo) | ❌ Missing |
| API reference | ❌ Missing |
| Profile format guide | ❌ Missing |
| Installation guide (per platform) | ❌ Missing |
| Architecture doc (v010-architecture.md) | ✅ Built |
| CONTRIBUTING.md | ✅ Built |
| SECURITY.md | ✅ Built |
| CHANGELOG.md | ✅ Built |

### 2.12 CI/CD

| Feature | Status |
|---------|--------|
| GitHub Actions multi-platform build | ✅ Built |
| cargo-audit (dependency scanning) | ✅ Built |
| Release artifacts (pre-built binaries) | ❌ Missing (CI exists but no release job) |
| GitHub Release automation | ❌ Missing |

---

## 3. Gap Analysis: Current v0.2 → Announcement Ready

### Priority 1 — Makes it actually work (ship blockers)
1. **macOS HAL** — reads real sensors + controls fans on dev machine
2. **Linux HAL** — hwmon sysfs for Linux users
3. **Agent memory DB** — agent.db with full schema (thermaliq-daemon)
4. **Safety enforcer** — hard limits before autonomous mode is usable
5. **WebSocket streams** — needed for real-time UI
6. **Tauri UI** — the face of the product

### Priority 2 — Makes it complete (feature parity)
7. **Custom sensor evaluation** in poll loop
8. **Predictive + Learning curves** (requires agent DB)
9. **Fan calibration** (RPM sweep)
10. **Intervention audit log + rollback**
11. **Full CLI commands** (agent mode/status/history, tq history, tq alert)
12. **Profile: gaming + efficiency**
13. **Alert actions**: webhook, notify, apply-profile fully wired

### Priority 3 — Makes it shippable (distribution)
14. **README.md** with demo GIF, install instructions, badges
15. **Release CI** — produce binaries on GitHub tag
16. **Homebrew formula**
17. **API reference docs**

### Priority 4 — Nice to have (post-announcement)
18. Windows HAL (LHM/WMI)
19. GPU handling (NVML/ADLX)
20. Claude Code skill integration
21. OpenClaw webhooks
22. Curve editor UI (graphical)
23. Calibration wizard UI
24. AUR + MSI

---

## 4. Crate Architecture (Announcement-Ready)

```
thermaliq/
├── Cargo.toml                    # workspace, version 0.2.0
├── thermaliq-common/             # shared types + business logic
│   └── src/
│       ├── lib.rs                # SensorReading, FanReading, HardwareSnapshot, SetFanDuty
│       ├── agent.rs              # AgentStateMachine, DtDtGuard, WorkloadKind
│       ├── alert.rs              # AlertEvaluator, TemperatureAlert, FiredAlert
│       ├── config.rs             # DaemonConfig, Profile, all 10 ProfileCurve types, CustomSensor
│       ├── curve.rs              # FanCurve, FanController (low-level)
│       └── history.rs            # HistoryDb (sensor/fan history, prune)
├── thermaliq-hal/                # platform hardware abstraction
│   └── src/
│       ├── lib.rs                # HardwareLayer trait, platform_hal()
│       ├── mock.rs               # MockHardwareLayer (CI/dev)
│       ├── linux.rs              # LinuxHardwareLayer (hwmon sysfs)    ← implement
│       ├── macos.rs              # MacosHardwareLayer (SMC/IOKit)      ← implement
│       └── windows.rs            # WindowsHardwareLayer (LHM/WMI)      ← future
├── thermaliq-daemon/             # background service
│   └── src/
│       ├── main.rs               # startup: config, token, HAL, DB, tasks, API
│       ├── token.rs              # Bearer token: generate, load, save
│       ├── poller.rs             # 1Hz poll: read HAL → history → alerts → fan curves
│       ├── agent_task.rs         # 5s agent loop: workload detection → interventions
│       ├── agent_db.rs           # AgentDb: agent.db SQLite (samples/interventions/log/stats)  ← add
│       └── api/
│           ├── mod.rs            # build_router()
│           ├── auth.rs           # AppState, AgentMode, require_bearer_auth
│           ├── routes.rs         # all REST handlers
│           └── ws.rs             # WebSocket handlers (/ws/agent, /ws/sensors)  ← add
├── thermaliq-cli/                # tq binary
│   └── src/main.rs               # full CLI with all subcommands
├── thermaliq-ui/                 # Tauri + React + TypeScript GUI     ← add
│   ├── src-tauri/
│   │   ├── Cargo.toml
│   │   └── src/main.rs
│   ├── src/
│   │   ├── App.tsx
│   │   ├── components/
│   │   │   ├── Dashboard.tsx     # real-time charts
│   │   │   ├── AgentPanel.tsx    # mode, state, interventions
│   │   │   ├── ProfileManager.tsx
│   │   │   ├── AlertCenter.tsx
│   │   │   └── Settings.tsx
│   │   └── api/
│   │       ├── client.ts         # REST client
│   │       └── ws.ts             # WebSocket client
│   ├── package.json
│   └── vite.config.ts
├── profiles/                     # bundled TOML profiles
│   ├── balanced.toml             # ✅
│   ├── silent.toml               # ✅
│   ├── performance.toml          # ✅
│   ├── gaming.toml               # ❌ add
│   └── efficiency.toml           # ❌ add
└── docs/
    ├── api.md                    # REST API reference  ❌ add
    ├── profiles.md               # TOML profile format guide  ❌ add
    └── installation.md           # per-platform install  ❌ add
```

---

## 5. Agent Memory DB Schema (agent.db)

```sql
-- 7-day rolling thermal history (auto-pruned)
CREATE TABLE thermal_samples (
    id INTEGER PRIMARY KEY,
    sensor_id TEXT NOT NULL,
    temp_celsius REAL NOT NULL,
    workload TEXT,              -- 'idle'|'gaming'|'compiling'|'encoding'|'stress'|NULL
    sampled_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX idx_thermal_samples ON thermal_samples(sensor_id, sampled_at);

-- Full intervention audit log
CREATE TABLE interventions (
    id TEXT PRIMARY KEY,        -- hex timestamp ID
    trigger_reason TEXT NOT NULL,
    proposed_action TEXT NOT NULL,
    proposed_at INTEGER NOT NULL,
    approved_at INTEGER,        -- NULL if autonomous
    applied_at INTEGER,
    result TEXT,                -- 'applied'|'rejected'|'rolled_back'|'failed'
    before_snapshot TEXT,       -- JSON: {sensors:[...], fans:[...], profile:name}
    after_snapshot TEXT,        -- JSON: same structure
    rolled_back_at INTEGER
);

-- Workload session log
CREATE TABLE workload_log (
    id INTEGER PRIMARY KEY,
    workload TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    peak_cpu_temp REAL,
    peak_gpu_temp REAL,
    avg_cpu_temp REAL,
    interventions_count INTEGER DEFAULT 0
);

-- Profile performance statistics
CREATE TABLE profile_stats (
    id INTEGER PRIMARY KEY,
    profile_name TEXT NOT NULL,
    recorded_at INTEGER NOT NULL DEFAULT (unixepoch()),
    avg_cpu_temp REAL,
    avg_fan_duty REAL,
    noise_score REAL,           -- proxy: avg fan duty as noise proxy
    thermal_headroom REAL       -- (max_safe_temp - avg_cpu_temp)
);
```

---

## 6. WebSocket Frame Formats

### /ws/agent — Agent event stream
```json
{ "type": "agent_state", "state": "Sampling", "ts": 1776169657 }
{ "type": "intervention_proposed", "id": "abc123", "description": "...", "action": "switch_profile:performance" }
{ "type": "intervention_applied", "id": "abc123", "result": "applied" }
{ "type": "workload_detected", "workload": "compiling", "confidence": 0.87 }
{ "type": "safety_limit", "sensor": "cpu_package", "temp": 91.2, "limit": 90.0 }
```

### /ws/sensors — Real-time sensor stream (1Hz)
```json
{ "type": "snapshot", "ts": 1776169657, "sensors": [...], "fans": [...] }
```

---

## 7. Safety Enforcer Spec

Applied in agent_task.rs BEFORE any intervention is executed:

```
hard limits (never overrideable by user):
  max_cpu_temp_c = 90.0      → block any action that increases CPU temp
  max_gpu_temp_c = 85.0      → block any action that increases GPU temp
  emergency_temp_c = 95.0    → set ALL fans to 100% immediately, log CRIT alert
  max_pwm_change_per_step = 20   → fan duty cannot jump more than 20% per intervention
  max_interventions_per_hr = 6   → rate-limit autonomous actions
  min_fan_duty_under_load = 30   → never set fans below 30% when CPU > 70°C
```

---

## 8. Workload Detection Algorithm

Input: last N thermal_samples (5-minute sliding window)
Output: WorkloadKind enum { Idle, Gaming, Compiling, Encoding, Stress, Unknown }

```
Idle:      avg_cpu < 45°C && avg_cpu_load_proxy < 20%
Gaming:    gpu_temp > 65°C && cpu_temp > 55°C && sustained > 120s
Compiling: cpu_temp > 60°C && many-core sustained && gpu < 50°C
Encoding:  cpu_temp > 58°C && gpu_temp > 55°C (mixed sustained)
Stress:    cpu_temp > 75°C sustained > 300s
```

Since we don't have CPU load% from HAL (only temp), use temperature proxy:
- Temp delta (dT/dt) indicates workload ramp-up
- Sustained high temps indicate active workload
- GPU temp distinguishes gaming vs compile

---

## 9. Implementation Order (Dependency-Correct)

### Wave 0 (done ✅): Core types, state machine, config schema, basic API, mock HAL, CI

### Wave 1 (in progress 🔄): Bug fixes
- [x] HistoryDb.query_fan_last_n
- [x] CLI endpoint fixes
- [x] AlertEvaluator wired into poller
- [x] Version bump 0.2.0
- [ ] Graph/Mix/Sync curve evaluation
- [ ] Predictive + Learning curve types added to enum

### Wave 2 (next): Agent intelligence
- [ ] agent_db.rs with full schema + CRUD
- [ ] Safety enforcer in agent_task.rs
- [ ] Workload detection
- [ ] Pre-cooling logic
- [ ] Intervention audit (before/after snapshots)
- [ ] AgentStateMachine wired into agent_task.rs
- [ ] Reject intervention endpoint + CLI

### Wave 3 (parallel): Platform HALs
- [ ] macOS HAL (SMC/IOKit) — priority since dev machine is macOS
- [ ] Linux HAL (hwmon sysfs)
- [ ] Windows HAL (future)

### Wave 4 (parallel): API + WebSocket
- [ ] /ws/agent WebSocket endpoint
- [ ] /ws/sensors WebSocket endpoint  
- [ ] Intervention reject endpoint
- [ ] History time-range params
- [ ] Fan calibration API skeleton

### Wave 5 (parallel): CLI expansion
- [ ] tq agent mode/status/interventions/reject/history
- [ ] tq history sensors/fans
- [ ] tq profile set
- [ ] tq alert list/add/remove

### Wave 6: Tauri UI
- [ ] Project scaffold (Tauri 2 + Vite + React + TypeScript)
- [ ] API client (REST + WebSocket)
- [ ] Dashboard component
- [ ] AgentPanel component
- [ ] ProfileManager component
- [ ] AlertCenter component
- [ ] Settings component
- [ ] System tray

### Wave 7: Distribution
- [ ] Gaming + Efficiency profiles
- [ ] README.md (shields, demo, install guide)
- [ ] API docs
- [ ] Profile format docs
- [ ] GitHub Release CI job
- [ ] Homebrew formula

---

## 10. "Ready to Announce" Checklist

- [ ] `cargo build` succeeds on Linux + macOS without mock-hardware
- [ ] `cargo test` all pass
- [ ] Real sensors appear via `tq sensors` on macOS
- [ ] Fan control works via `tq fan <id> --pwm 60`
- [ ] Agent detects workload and proposes intervention
- [ ] WebSocket stream delivers real-time sensor data
- [ ] Tauri UI launches, shows live dashboard
- [ ] Tauri UI agent panel: approve/reject interventions
- [ ] All 5 profiles available
- [ ] README.md with badges + install instructions
- [ ] GitHub Release with macOS + Linux binaries

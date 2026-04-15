# ThermalIQ

**AI-native thermal management for macOS and Linux.**

ThermalIQ is a background daemon that monitors your system's temperatures and intelligently controls fan speeds — with an optional 24/7 AI agent that adapts cooling to your workload in real time.

[![Rust](https://img.shields.io/badge/rust-1.78%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)](#)

---

## Features

### Fan Control
- **10 curve types**: linear, flat, graph, mix, trigger, sync, auto, ai, predictive, learning
- **TOML profiles** — version-controlled, human-readable fan configurations
- **Hysteresis support** — prevents fan flutter at threshold boundaries
- **Per-sensor routing** — different curves for CPU, GPU, chassis fans

### AI Agent
- **3 modes**: Passive (monitor only), Approval (propose + wait), Autonomous (act independently)
- **Workload detection**: automatically identifies gaming, compiling, encoding, stress workloads
- **Safety enforcer**: hardcoded 95°C CPU / 90°C GPU emergency limits — fans to 100% unconditionally
- **Rate limiting**: maximum 6 interventions per hour to prevent thrashing
- **Audit log**: every proposed and applied intervention is recorded to SQLite

### Real-Time Streaming
- **WebSocket `/ws/sensors`** — live sensor + fan data at 1 Hz
- **WebSocket `/ws/agent`** — agent state changes and intervention events

### CLI (`tq`)
```
tq status                          # health, version, agent mode
tq watch                           # live terminal dashboard (Ctrl+C to stop)
tq sensors                         # snapshot temperature readings
tq fans                            # snapshot fan RPM + duty
tq profile list                    # available profiles
tq profile set <name>              # switch active profile
tq agent status                    # mode + pending interventions
tq agent mode autonomous           # switch to autonomous mode
tq agent interventions             # list pending interventions
tq agent approve <id>              # approve an intervention
tq agent reject <id>               # reject an intervention
tq history sensors <id>            # sensor temperature history
tq history fans <id>               # fan history
tq alert list                      # alerts in active profile
tq stop                            # graceful daemon shutdown
```

---

## Installation

### Requirements
- Rust 1.78+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- macOS 12+ or Linux kernel 4.14+ (hwmon support)

### macOS
```bash
git clone https://github.com/thermaliq/thermaliq
cd thermaliq
cargo build --release
sudo cp target/release/thermaliq-daemon /usr/local/bin/
sudo cp target/release/tq /usr/local/bin/
```

> **Note:** Full fan control on macOS requires a privileged SMC helper (SMJobBless). Without it, the daemon reads temperatures and logs intended fan changes — control is advisory only on Apple Silicon.

### Linux
```bash
cargo build --release
sudo cp target/release/thermaliq-daemon /usr/local/bin/
sudo cp target/release/tq /usr/local/bin/

# Allow fan control without root (optional)
sudo usermod -aG video $USER
```

The daemon reads sensors from `/sys/class/hwmon/` and writes PWM values for control.

---

## Getting Started

### 1. Install bundled profiles
```bash
mkdir -p ~/.config/thermaliq/profiles
cp profiles/*.toml ~/.config/thermaliq/profiles/
```

### 2. Start the daemon
```bash
thermaliq-daemon
# Logs to stderr; set RUST_LOG=debug for verbose output
```

The daemon creates its config at `~/.config/thermaliq/thermaliq.toml` on first run.

### 3. Check status
```bash
tq status
# health:  ok
# version: 0.2.0
# agent:   passive (pending: 0)
```

### 4. Load a profile
```bash
tq profile list
# balanced
# performance
# silent
# gaming
# efficiency

tq profile set balanced
```

### 5. Enable the AI agent
```bash
# Approval mode — agent proposes, you approve
tq agent mode approval

# Check for pending interventions
tq agent interventions

# Approve one
tq agent approve <id>

# Or go fully autonomous
tq agent mode autonomous
```

---

## Configuration

### Daemon config (`~/.config/thermaliq/thermaliq.toml`)
```toml
api_port = 7272
polling_interval_ms = 1000
default_profile = "balanced"
```

### Profile format (`~/.config/thermaliq/profiles/balanced.toml`)
```toml
name = "balanced"
description = "Balanced cooling — quiet at idle, responsive under load"

[[curves]]
type = "linear"
id = "cpu_linear"
source = "cpu_package"
min_temp = 40.0
max_temp = 80.0
min_pwm = 20
max_pwm = 85

[[controls]]
fan = "fan1"
curve = "cpu_linear"
min_pwm = 15

[[alerts]]
sensor = "cpu_package"
threshold = 90.0
severity = "warn"
action = "notify"

[[alerts]]
sensor = "cpu_package"
threshold = 95.0
severity = "crit"
action = { apply_profile = "performance" }
```

### Curve Types

| Type | Description |
|------|-------------|
| `linear` | Straight-line interpolation between min/max temp → PWM |
| `flat` | Fixed PWM regardless of temperature |
| `graph` | Multi-point curve — define exact temp→PWM breakpoints |
| `mix` | Combine multiple curves (max/min/average/sum) |
| `trigger` | Two-state: idle below threshold, load above |
| `sync` | Mirror another curve with a fixed offset |
| `auto` | Adaptive linear with configurable deadband |
| `ai` | Fully agent-controlled — set by the AI agent |
| `predictive` | Look-ahead cooling based on workload trend |
| `learning` | Learns your thermal patterns over time |

---

## API Reference

The daemon exposes a local HTTP API on port 7272.

All **mutation endpoints** require `Authorization: Bearer <token>` (token at `~/.config/thermaliq/api.token`).

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/health` | — | Health check |
| GET | `/api/version` | — | Daemon version |
| GET | `/sensors` | — | Live sensor readings |
| GET | `/fans` | — | Live fan readings |
| GET | `/api/profiles` | — | Available profile names |
| GET | `/api/profiles/active` | — | Active profile name |
| POST | `/api/profiles/active` | ✓ | Switch active profile |
| GET | `/api/agent/status` | — | Agent mode + pending count |
| POST | `/api/agent/mode` | ✓ | Set agent mode |
| GET | `/api/agent/interventions` | — | Pending interventions |
| POST | `/api/agent/interventions/:id/approve` | ✓ | Approve intervention |
| POST | `/api/agent/interventions/:id/reject` | ✓ | Reject intervention |
| GET | `/api/history/sensors/:id` | — | Sensor history |
| GET | `/api/history/fans/:id` | — | Fan history |
| POST | `/api/shutdown` | ✓ | Graceful shutdown |
| WS | `/ws/sensors` | — | Live sensor/fan stream |
| WS | `/ws/agent` | — | Agent event stream |

---

## Architecture

```
thermaliq-common    — shared types: config, history DB, curves, alerts, agent state machine
thermaliq-hal       — hardware abstraction: IOKit/SMC (macOS), hwmon sysfs (Linux), mock
thermaliq-daemon    — axum HTTP server, poll loop, AI agent task, WebSocket streams
thermaliq-cli       — tq CLI binary (reqwest blocking client)
thermaliq-ui        — Tauri 2 + React desktop app (optional GUI)
```

### Safety Design
- API token: 32-byte random, stored at `chmod 0600`, constant-time comparison
- CORS: restricted to `tauri://localhost` only
- Emergency limits: hardcoded in Rust, not overrideable via API
- No network egress: daemon is purely local

---

## Development

```bash
# Build with mock hardware (no real SMC/hwmon needed)
cargo build --features mock-hardware

# Run tests
cargo test --features mock-hardware

# Run daemon in dev mode
RUST_LOG=debug cargo run -p thermaliq-daemon --features mock-hardware

# Run CLI against dev daemon
cargo run -p thermaliq-cli -- status
```

---

## License

MIT — see [LICENSE](LICENSE).

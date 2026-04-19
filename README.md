# ThermalIQ

**AI-native thermal management for Windows, macOS, and Linux.**

Fan control that thinks for itself — 10 curve types, a 24/7 AI agent, and natural-language profile creation, all in a single local daemon.

[![Rust](https://img.shields.io/badge/rust-1.78%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](#installation)

---

## Why ThermalIQ

- **Cross-platform** — Windows (HWiNFO64 + NVML), macOS (IOKit SMC), Linux (hwmon/RDNA3)
- **10 curve types** — linear, flat, graph, mix, trigger, sync, auto, ai, predictive, learning
- **AI agent** — passive / approval / autonomous modes; detects gaming, compiling, encoding workloads
- **Natural-language profiles** — `tq profile create "quiet for video calls"` generates valid TOML via LLM
- **Schedule + battery + per-app switching** — profiles change automatically based on time, power state, or foreground app
- **Local-first** — no cloud, no telemetry by default; daemon runs entirely on `127.0.0.1:7272`
- **5 AI providers** — Anthropic (default), OpenAI, Gemini, Ollama, OpenRouter

---

## Installation

> v1.0 is Windows-first. macOS and Linux packages ship in v1.2.
> Build from source for macOS/Linux today (see [docs/QUICKSTART.md](docs/QUICKSTART.md)).

### Windows

```powershell
winget install thermaliq.ThermalIQ
```

Or download the NSIS installer from [Releases](https://github.com/lulacoding/thermaliq/releases).

**Prerequisite:** [HWiNFO64](https://www.hwinfo.com/download/) must be running for full sensor coverage. The installer will prompt you if it is not detected.

### macOS / Linux (build from source)

```bash
git clone https://github.com/lulacoding/thermaliq
cd thermaliq
cargo build --release
sudo cp target/release/thermaliq-daemon target/release/tq /usr/local/bin/
```

See [docs/QUICKSTART.md](docs/QUICKSTART.md) for platform prerequisites and first-run steps.

---

## 60-Second Quickstart

```bash
# 1. Copy bundled profiles
mkdir -p ~/.config/thermaliq/profiles
cp profiles/*.toml ~/.config/thermaliq/profiles/

# 2. Start the daemon
thermaliq-daemon

# 3. Check status
tq status
# health:  ok
# version: 1.0.0
# agent:   passive (pending: 0)

# 4. Read sensors and fans
tq sensors
tq fans

# 5. Switch to a profile
tq profile set balanced

# 6. (Optional) Enable the AI agent
tq agent mode approval
tq agent interventions     # review proposals
tq agent approve <id>
```

---

## CLI Reference (`tq`)

```
tq status                          # health, version, agent mode
tq watch                           # live terminal dashboard (Ctrl+C to stop)
tq sensors                         # snapshot temperature readings
tq fans                            # snapshot fan RPM + duty
tq profile list                    # available profiles
tq profile set <name>              # switch active profile
tq profile create "<description>"  # generate profile via AI
tq agent status                    # mode + pending interventions
tq agent mode <passive|approval|autonomous>
tq agent interventions             # list pending interventions
tq agent approve <id>
tq agent reject <id>
tq history sensors <id>            # sensor temperature history
tq history fans <id>               # fan history
tq alert list                      # alerts in active profile
tq schedule list                   # scheduled profile rules
tq stop                            # graceful daemon shutdown
```

---

## Configuration

### Daemon (`~/.config/thermaliq/thermaliq.toml`)

```toml
api_port = 7272
polling_interval_ms = 1000
default_profile = "balanced"

# Battery-based profile switching
[battery]
on_battery = "silent"
plugged_in  = "balanced"

# Per-app profile switching
[[app_rules]]
process = "csgo.exe"
profile = "gaming"

# Scheduled profiles
[[schedule]]
days    = ["Mon", "Tue", "Wed", "Thu", "Fri"]
time    = "22:00-08:00"
profile = "silent"
```

### Profile format (`~/.config/thermaliq/profiles/balanced.toml`)

```toml
name        = "balanced"
description = "Balanced cooling — quiet at idle, responsive under load"

[[curves]]
type     = "linear"
id       = "cpu_linear"
source   = "cpu_package"
min_temp = 40.0
max_temp = 80.0
min_pwm  = 20
max_pwm  = 85

[[controls]]
fan     = "fan1"
curve   = "cpu_linear"
min_pwm = 15

[[alerts]]
sensor    = "cpu_package"
threshold = 95.0
severity  = "crit"
action    = { apply_profile = "performance" }
```

See [docs/profiles.md](docs/profiles.md) for all 10 curve types and full profile syntax.

---

## Curve Types

| Type | Description |
|------|-------------|
| `linear` | Straight-line interpolation between min/max temp |
| `flat` | Fixed PWM regardless of temperature |
| `graph` | Multi-point breakpoint curve |
| `mix` | Combine curves (max/min/average/sum) |
| `trigger` | Two-state: idle below threshold, load above |
| `sync` | Mirror another curve with a fixed offset |
| `auto` | Adaptive linear with configurable deadband |
| `ai` | Fully agent-controlled |
| `predictive` | Look-ahead cooling based on temperature trend |
| `learning` | Adapts over time from observed thermal patterns |

---

## API

The daemon exposes a local REST API on `http://127.0.0.1:7272`.

Read-only endpoints are unauthenticated. Mutation endpoints require `Authorization: Bearer <token>` (token at `~/.config/thermaliq/api.token`).

```
GET  /health                                  # health check
GET  /sensors                                 # live sensor readings
GET  /fans                                    # live fan readings
POST /fans/:id/pwm                            # set fan duty (auth)
GET  /api/profiles                            # list profiles
POST /api/profiles/active                     # switch profile (auth)
GET  /api/agent/status                        # agent mode + pending count
POST /api/agent/mode                          # set agent mode (auth)
GET  /api/agent/interventions                 # pending interventions
POST /api/agent/interventions/:id/approve     # approve (auth)
POST /api/agent/interventions/:id/reject      # reject (auth)
GET  /metrics                                 # Prometheus metrics
GET  /api/openapi.json                        # OpenAPI 3.0 spec
WS   /ws/sensors                              # live sensor stream (1 Hz)
WS   /ws/agent                                # agent event stream
```

Full reference: [docs/api.md](docs/api.md)

---

## Architecture

```
thermaliq-common    shared types: config, history DB, curves, alerts, agent state machine
thermaliq-hal       hardware abstraction: IOKit/SMC (macOS), hwmon (Linux), HWiNFO64/NVML (Windows), mock
thermaliq-daemon    axum HTTP server, poll loop, AI agent task, WebSocket streams
thermaliq-cli       tq CLI binary (reqwest blocking client)
thermaliq-ui        Tauri 2 + React desktop app (optional GUI)
thermaliq-helper    macOS privileged helper for SMC fan writes (SMJobBless)
```

### Safety Design

- API token: 32-byte random, stored at `chmod 0600`, constant-time comparison
- CORS: restricted to `tauri://localhost` only
- Emergency limits: CPU 95°C / GPU 90°C — hardcoded in Rust, not overrideable via API
- No network egress: daemon is purely local

---

## Documentation

| Doc | Description |
|-----|-------------|
| [docs/QUICKSTART.md](docs/QUICKSTART.md) | Install, first sensor read, first profile |
| [docs/AI_AGENT.md](docs/AI_AGENT.md) | AI modes, providers, guardrails, NL profiles |
| [docs/HARDWARE_SUPPORT.md](docs/HARDWARE_SUPPORT.md) | Supported hardware matrix |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | Diagnostics, log paths, common fixes |
| [docs/profiles.md](docs/profiles.md) | Profile format and all curve types |
| [docs/api.md](docs/api.md) | Full REST + WebSocket API reference |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Build from source, dev setup |
| [CHANGELOG.md](CHANGELOG.md) | Release history |

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

# Stage 4: New Features & Product Ideas Beyond v0.2 Spec
**Status:** Complete | **Tier:** HIGH | **Model:** Sonnet

## Top 10 Quick Wins (Easy feasibility + High impact)

1. **Per-application fan curves** — Core differentiator from FanControl. Process enumeration + app-to-profile map. Smooth curve transitions needed.
2. **Natural language profile creation** — "make my PC quiet for video calls" → uses existing LLM integration directly. Prompt → TOML. Low hallucination risk given schema constraints.
3. **Schedule-based profiles** — `[[schedule]] time = "22:00-08:00" profile = "silent"`. Cron-style TOML extension.
4. **System tray quick-switch** — Table-stakes feature. Tauri native support. Should be v0.2 baseline.
5. **Thermal anomaly forensics** — "why was my PC hot at 3am?" — query thermal log + process snapshot log → NL explanation via agent.
6. **Cross-session learning persistence** — Foundational requirement, makes "24/7 AI agent" claim real. Store (workload_class, curve_delta, thermal_outcome) in SQLite.
7. **Notification center integration** — Tauri native notification APIs on all platforms. User expectation baseline.
8. **Prometheus metrics endpoint** — ~100 lines of Rust, unlocks entire Grafana ecosystem. GET /metrics.
9. **Webhook triggers on sensor thresholds** — HTTP POST on threshold breach. Composability primitive for HA, n8n, Zapier, scripts.
10. **Battery state profiles** — Essential for laptop users. Trivial OS API call. Plugged = performance, battery = silent.

## AI/Agent Intelligence Expansions

### Predictive Component Failure Detection
- Bearing wear: RPM variance widens over weeks (rolling z-score), harmonic noise in 100-400Hz range
- Audio FFT from mic input (opt-in), current draw anomalies if smart fan headers expose it
- Feasibility: Medium | Impact: High | Blocker: labelled failure dataset, opt-in mic permission, FFT CPU cost

### Thermal Fingerprinting
- Coarse workload classes separable from thermal signature alone (idle/gaming/encode/browse)
- App-level discrimination requires process names (already available)
- Best as corroborating signal, not primary detection
- Feasibility: Medium | Impact: Medium

### Natural Language Profile Creation
- NL → structured profile spec → TOML serialisation
- All within current architecture. Low hallucination risk given schema constraints.
- Feasibility: Easy | Impact: High | Blocker: none significant

### Thermal Anomaly Forensics
- Requires: timestamped thermal log + process snapshot log + network/storage activity
- Query on-demand, NL explanation via agent
- Feasibility: Easy-Medium | Impact: High

### Energy Efficiency Scoring
- Score = work_done / thermal_joules_expended (relative scores reliable even if absolute ±20-30%)
- Carbon footprint via Electricity Maps / WattTime API
- Feasibility: Medium | Impact: Medium

### Multi-System Fleet Management
- gRPC/MQTT agent bus + central dashboard
- Architecture: agent-to-agent mesh or central coordinator
- Feasibility: Hard | Impact: High (enterprise) | Defer post-v1.0

## Hardware Integrations

### Home Assistant / MQTT
- rumqttc Rust crate, HA MQTT auto-discovery protocol
- Ambient sensor → fans slow down when room is already cool; ramp pre-emptively as room heats
- Feasibility: Easy | Impact: High | Blocker: none

### RGB Fan Color (Temperature-Reactive)
- OpenRGB REST API/SDK integration — no need to own RGB stack
- Differentiation: thermal-reactive color (no tool does this natively)
- Feasibility: Easy | Impact: Medium (enthusiast segment)

### USB Ambient Temperature Sensors
- Temper1, DS18B20 via USB ($8-15 retail), libusb/hidapi
- Room-temperature-aware fan curves — no competitor does this natively
- Feasibility: Easy | Impact: Medium | Blocker: Windows unsigned HID driver signing

### Liquid Cooling Pump Control
- PWM pump = same API as fans. Safety: hard-floor pump duty with watchdog restart on agent crash.
- Feasibility: Medium | Impact: High (AIO/custom loop users) | Blocker: safety watchdog design

### UPS/Power Event Handling
- NUT (Linux/Mac), WMI (Windows) — throttle fans on UPS battery backup
- Feasibility: Easy | Impact: Low (consumer) / High (server)

## Community/Social Features

### Profile Marketplace
- Backend: Supabase (hardware tags, TOML blob, ratings)
- Moderation needed (malicious profiles: fans to 0%)
- Hardware normalisation for search (CPU/GPU model tags)
- Feasibility: Medium | Impact: High (growth mechanic)

### Anonymous Opt-in Telemetry
- Min useful: (hardware_class, workload_class, fan_curve_applied, thermal_outcome_delta)
- No PII. Differential privacy budget applicable.
- GDPR Article 13 disclosure required for EU users
- Feasibility: Medium | Impact: High (long-term model quality)

### Cross-Device Profile Sync
- TOML profiles encrypted client-side (age/libsodium), uploaded to user-controlled storage
- Hardware mismatch detection required (280W GPU profile → 150W GPU = dangerous)
- Feasibility: Medium | Impact: Medium

### Thermal Efficiency Leaderboard
- Viral potential in enthusiast community
- Requires standardised benchmark workload for comparable scores
- Anti-cheat: noise penalty in scoring (can't just max fans)
- Feasibility: Medium | Impact: Medium (enthusiast)

## Developer/Power User Features

### Prometheus Metrics Endpoint
- Plain HTTP GET /metrics, gauge metrics per sensor, ~100 lines of Rust
- Unlocks Grafana, alertmanager, entire observability stack
- Feasibility: Easy | Impact: High

### Webhook Triggers on Thresholds
- Configurable rules in TOML, HTTP POST on trigger, debounce required
- Feasibility: Easy | Impact: High

### Scripting with Rhai (recommended over Lua/WASM)
- Rhai: Rust-native, no FFI, sandboxed, already a Rust crate
- Custom curve logic via script
- Feasibility: Medium | Impact: Medium

### CSV/JSON Thermal History Export
- Prerequisite for anomaly forensics, efficiency scoring, external analysis
- Should be in v0.2 baseline
- Feasibility: Easy | Impact: Medium

### CI/CD REST API Integration
- POST /profile/activate → poll for thermal stabilisation → run benchmark → restore profile
- Hardware reviewers and automated test labs
- Feasibility: Easy | Impact: Medium (niche but high-value)

### Kubernetes Sidecar Mode
- Linux-only, DaemonSet, IPMI/BMC integration, Prometheus metrics
- No current OSS solution in this space
- Feasibility: Hard | Impact: High (enterprise)

## Enterprise Features

### IPMI/BMC Server Rack Management
- Server edition with AI fan curve management via IPMI
- Only tool of its kind in OSS space
- Feasibility: Hard | Impact: High (enterprise beachhead)

### Compliance Reporting
- PDF/HTML thermal history for warranty claims — dated log of max temps + timestamps
- Feasibility: Easy | Impact: Medium

## UX/Accessibility Features

### Mobile Companion App (PWA)
- PWA served by existing HTTP API, SSE/WebSocket for realtime
- No native app needed, works iOS+Android
- Feasibility: Medium (PWA) | Impact: Medium
- Blocker: security for exposing HTTP API on LAN, mDNS discovery

### Desktop Widget (Mini Thermal Monitor)
- Always-on-top Tauri window, transparent background, configurable size
- High-visibility in enthusiast market
- Feasibility: Easy | Impact: Medium
- Blocker: macOS always-on-top restrictions

### Voice Control
- OpenClaw/Claude Code already provides conversational control — voice adds input modality
- Windows Speech Platform, macOS SFSpeechRecognizer (no cloud dependency)
- Privacy concern: always-on mic requires explicit consent + visual indicator
- Feasibility: Medium | Impact: Low (general) / High (accessibility)

## Profile Intelligence

### Per-Application Fan Curves ★ TOP PRIORITY
- Process enumeration at ~5s intervals, app-to-profile map (user-configurable or AI-inferred)
- Smooth curve transitions required (prevent audible fan hunting on app switch)
- App name normalisation: win32 process names ≠ macOS bundle IDs ≠ Linux argv[0]
- ThermalIQ's clearest competitive advantage over FanControl
- Feasibility: Easy-Medium | Impact: High

### Game Auto-Detection (Steam/Epic/GOG)
- Steam: read libraryfolders.vdf + appmanifest_*.acf + Steam Web API (public, no auth)
- Pre-built profile library for top-100 games (noise-prioritised RPG/strategy vs. perf-prioritised FPS)
- Feasibility: Medium | Impact: High
- Blocker: ongoing profile library maintenance, Epic/GOG format differences

### Schedule-Based Profiles ★ QUICK WIN
- `[[schedule]] time = "22:00-08:00" profile = "silent"`
- Feasibility: Easy | Impact: High

### Location-Based Profiles
- WiFi SSID detection → profile switch (home = balanced, office = performance)
- macOS: requires location permission in Monterey+ for SSID — significant friction
- Feasibility: Easy (Win/Linux) / Medium (macOS) | Impact: Medium

### Battery State Profiles ★ QUICK WIN
- Plugged → performance, battery → silent
- Standard OS battery APIs on all platforms
- Feasibility: Easy | Impact: High (laptop users)

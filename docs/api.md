# ThermalIQ Daemon API Reference

The daemon exposes a local HTTP API on `http://127.0.0.1:7272` (port configurable via `thermaliq.toml`).

## Authentication

Read-only endpoints are unauthenticated. All mutation endpoints require:

```
Authorization: Bearer <token>
```

The token is a 32-byte random hex string generated on first run and stored at:
- **macOS/Linux**: `~/.config/thermaliq/api.token` (mode 0600)

CORS is restricted to `tauri://localhost` — the Tauri desktop app origin.

---

## Endpoints

### Health & Version

#### `GET /health`
```json
{ "status": "ok" }
```

#### `GET /api/version`
```json
{ "version": "0.2.0" }
```

---

### Sensors & Fans

#### `GET /sensors`
Returns current temperature readings from all detected sensors.

```json
{
  "sensors": [
    { "id": "cpu_package", "label": "CPU Package", "temp_celsius": 58.5 },
    { "id": "gpu_edge",    "label": "GPU Edge",     "temp_celsius": 42.0 }
  ]
}
```

#### `GET /fans`
Returns current fan RPM and duty cycle for all detected fans.

```json
{
  "fans": [
    { "id": "fan1", "label": "CPU Fan", "rpm": 1800, "duty_percent": 45 },
    { "id": "fan2", "label": "Chassis",  "rpm": 1200, "duty_percent": 35 }
  ]
}
```

#### `POST /fans/:id/pwm` — Auth required
Manually set a fan's duty cycle.

**Request:**
```json
{ "duty_percent": 60 }
```

**Response:**
```json
{ "fan_id": "fan1", "duty_percent": 60 }
```

---

### Profiles

#### `GET /api/profiles`
Returns names of all available profiles (reads TOML files from `~/.config/thermaliq/profiles/`).

```json
{ "profiles": ["balanced", "performance", "silent", "gaming", "efficiency"] }
```

#### `GET /api/profiles/active`
```json
{ "name": "balanced", "description": "Balanced cooling — quiet at idle, responsive under load" }
```
Returns `{"name": null, "description": null}` if no profile is active.

#### `POST /api/profiles/active` — Auth required
Switch the active fan control profile. Change takes effect immediately on the next poll tick.

**Request:**
```json
{ "name": "performance" }
```

**Response:**
```json
{ "active_profile": "performance" }
```

**Errors:**
- `400 Bad Request` — missing `name` field
- `404 Not Found` — profile file does not exist

---

### AI Agent

#### `GET /api/agent/status`
```json
{ "mode": "approval", "pending_count": 2 }
```

`mode` is one of: `passive`, `approval`, `autonomous`

#### `POST /api/agent/mode` — Auth required
**Request:**
```json
{ "mode": "autonomous" }
```

**Response:**
```json
{ "agent_mode": "autonomous" }
```

#### `GET /api/agent/interventions`
Returns pending interventions (only in `approval` mode).

```json
{
  "interventions": [
    {
      "id": "186a5f3200000000",
      "description": "CPU 91.2°C exceeds safety threshold",
      "proposed_action": "switch_profile:performance",
      "proposed_at": 1713123456
    }
  ]
}
```

#### `POST /api/agent/interventions/:id/approve` — Auth required
Apply the proposed action and remove from pending list.

```json
{ "intervention_id": "186a5f3200000000", "status": "applied" }
```

#### `POST /api/agent/interventions/:id/reject` — Auth required
Discard the intervention without applying it.

```json
{ "intervention_id": "186a5f3200000000", "status": "rejected" }
```

---

### History

All history endpoints support `?limit=N` (default 100, max determined by DB retention).

#### `GET /api/history/sensors/:sensor_id`
```json
{
  "sensor_id": "cpu_package",
  "records": [
    { "id": 1001, "sensor_id": "cpu_package", "temp_celsius": 58.5, "recorded_at": 1713123400 },
    { "id": 1002, "sensor_id": "cpu_package", "temp_celsius": 59.1, "recorded_at": 1713123401 }
  ]
}
```

#### `GET /api/history/fans/:fan_id`
```json
{
  "fan_id": "fan1",
  "records": [
    { "id": 201, "fan_id": "fan1", "rpm": 1800, "duty_percent": 45, "recorded_at": 1713123400 }
  ]
}
```

---

### Control

#### `POST /api/shutdown` — Auth required
Initiates graceful shutdown. The daemon will stop the HTTP server and background tasks.

```json
{ "status": "shutting_down" }
```

---

## WebSocket Streams

WebSocket connections do not require authentication.

### `WS /ws/sensors`
Broadcasts a JSON frame every poll interval (default 1 Hz):

```json
{
  "sensors": [
    { "id": "cpu_package", "label": "CPU Package", "temp_celsius": 58.5 }
  ],
  "fans": [
    { "id": "fan1", "label": "CPU Fan", "rpm": 1800, "duty_percent": 45 }
  ],
  "ts": 1713123456
}
```

### `WS /ws/agent`
Sends an initial status frame on connect, then broadcasts agent events:

**Initial frame (on connect):**
```json
{ "type": "agent_status", "mode": "approval", "pending_count": 1 }
```

**Intervention proposed:**
```json
{
  "type": "intervention_proposed",
  "id": "186a5f3200000000",
  "description": "CPU 91.2°C exceeds safety threshold",
  "proposed_action": "switch_profile:performance"
}
```

**Mode changed:**
```json
{ "type": "mode_changed", "mode": "autonomous" }
```

---

## Error Responses

All error responses return the appropriate HTTP status code. The response body for API errors is a JSON object when possible.

| Status | Meaning |
|--------|---------|
| 400 | Bad request — malformed body or missing required field |
| 401 | Unauthorized — missing or invalid Bearer token |
| 404 | Not found — resource doesn't exist |
| 500 | Internal server error — hardware or database error |

# ThermalIQ AI Agent

ThermalIQ includes a 24/7 AI agent that monitors your system's thermal state and adjusts fan curves in real time. This document covers the three operating modes, trust guardrails, natural-language profile creation, and supported AI providers.

---

## Agent modes

The agent has three modes. Switch with `tq agent mode <mode>` or via the Settings panel.

### Passive (default)

```bash
tq agent mode passive
```

The agent monitors temperatures and workloads, logs observations to `agent.db`, but does not change any fan settings. Safe for first-time setup and hardware validation.

### Approval

```bash
tq agent mode approval
```

The agent detects load changes and proposes interventions. Each proposal waits in a queue until you approve or reject it.

```bash
tq agent interventions
# 186a5f32  CPU 91.2°C exceeds safety threshold  →  switch_profile:performance

tq agent approve 186a5f32
tq agent reject 186a5f32
```

Pending interventions are also visible on the WebSocket stream (`/ws/agent`) and in the desktop UI AgentPanel.

### Autonomous

```bash
tq agent mode autonomous
```

The agent applies interventions immediately without waiting for approval. All changes are still written to the audit log in `agent.db`.

---

## What the agent monitors

- **Workload classes**: idle, gaming, compiling, encoding, stress
- **Thermal trend**: temperature rate-of-change (dT/dt) to pre-empt runaway
- **Per-app rules**: foreground process matched against `[[app_rules]]` in config
- **Schedule**: time-of-day profile rules
- **Battery state**: AC / battery profile switching

---

## Safety guardrails

These limits apply in all modes and cannot be overridden via the API or config file:

| Limit | Value |
|-------|-------|
| CPU emergency threshold | 95°C — all fans to 100% immediately |
| GPU emergency threshold | 90°C — all fans to 100% immediately |
| Maximum interventions | 6 per hour per fan (prevents oscillation) |

Configurable guardrails (set in the Settings panel or `thermaliq.toml`):

```toml
[agent.guardrails]
min_fan_percent     = 10    # fans never go below this %
killswitch_temp_c   = 90    # any sensor above this -> all fans 100%
change_cooldown_sec = 30    # minimum seconds between AI-driven changes
```

---

## Natural-language profile creation

The `tq profile create` command sends a natural-language description and a hardware snapshot to the configured AI provider. The provider returns a TOML profile, which ThermalIQ validates before saving.

```bash
tq profile create "quiet for video calls, keep CPU below 70 degrees"
# Generating profile...
# Saved: ~/.config/thermaliq/profiles/quiet-video-calls.toml

tq profile set quiet-video-calls
```

Via the API:

```bash
curl -s -X POST http://127.0.0.1:7272/api/profiles/generate \
  -H "Authorization: Bearer $(cat ~/.config/thermaliq/api.token)" \
  -H "Content-Type: application/json" \
  -d '{"prompt": "aggressive cooling for gaming, GPU priority"}' | jq .
```

If the generated TOML fails validation, ThermalIQ discards it and reports the error. No invalid profile is ever saved.

---

## AI providers

Configure the provider in `~/.config/thermaliq/thermaliq.toml`:

```toml
[ai]
enabled  = true
provider = "anthropic"   # anthropic | openai | gemini | ollama | openrouter
api_key  = "sk-ant-..."
```

| Provider | Value | Notes |
|----------|-------|-------|
| Anthropic (default) | `anthropic` | Claude models; best results in testing |
| OpenAI | `openai` | GPT-4o and later |
| Google Gemini | `gemini` | Gemini 1.5 and later |
| Ollama (local) | `ollama` | Runs locally; set `api_base` to your Ollama URL |
| OpenRouter | `openrouter` | Routes to any supported model |

For Ollama:

```toml
[ai]
provider = "ollama"
api_base = "http://127.0.0.1:11434"
model    = "llama3"
```

### Disabling AI features

If no API key is configured, or if you set `enabled = false`, the AI agent switches to passive mode and all AI-specific curve types fall back to a 60% fixed duty.

```toml
[ai]
enabled = false
```

Manual fan control, profile switching, scheduling, and battery rules all continue to work without an AI provider.

---

## Audit log

Every agent decision — proposed or applied — is written to `agent.db` with:

- Timestamp
- Intervention ID
- Trigger reason
- Proposed action
- Hardware snapshot at time of decision
- Outcome (approved / rejected / auto-applied)

Query the log:

```bash
tq agent interventions     # pending only
tq history sensors <id>    # per-sensor history
```

Or query directly:

```bash
sqlite3 ~/.config/thermaliq/agent.db \
  "SELECT * FROM interventions ORDER BY proposed_at DESC LIMIT 20;"
```

---
name: Session 4 — Tauri UI Walkthrough on Real Hardware
description: Run the full Tauri UI against a real-HAL daemon; click every component; capture every bug
session: 4
target_date: 2026-04-28
duration_minutes: 75
risk: low
---

# Session 4 — Tauri UI Walkthrough

## Why this session exists

The Tauri UI was built against a mocked daemon. Running it against real-HAL data exposes:
- Components that crash on null/missing fields the mock didn't have
- Streaming/WebSocket bugs that mock latency hides
- Persistence assumptions (does CurveEditor actually save?)
- System tray + Windows-specific UX (toast notifications, autostart)

This is the **most exploratory** session. Don't fix bugs — log them. Triage in S9.

## Prerequisites

- S1, S2, S3 complete and green
- Daemon running with real HAL, elevated, HWiNFO64 active
- Test desktop has a working AI provider key (OpenAI or Anthropic — use a throwaway key with $5 cap)
- Two monitors helpful (UI on one, HWiNFO64 on the other for ground truth)

## Step-by-step

### 4.1 Launch UI in dev mode

- [ ] Daemon already running from S3? Confirm:
  ```powershell
  curl -H "Authorization: Bearer $tok" http://127.0.0.1:7272/api/health
  ```
- [ ] In a NEW PowerShell:
  ```powershell
  cd C:\dev\thermaliq\thermaliq-ui
  npm run tauri dev
  ```
  - First launch: ~30s. Opens a Tauri window.
  - Hot reload is on — file changes refresh automatically.
- [ ] **First-load checks:**
  - Window opens with no console errors (open DevTools: Ctrl+Shift+I)
  - Dashboard renders within 3 seconds
  - WebSocket connection established (check Network tab → WS frames flowing)
- [ ] System tray:
  - Look at the Windows system tray (bottom-right, may be in overflow `^`)
  - Expect a ThermalIQ icon
  - Right-click → menu should show profiles + agent controls

### 4.2 Component walk-through

For each component below: **click everything, type test inputs, observe failures, log bugs.** Use the bug template at the bottom.

#### 4.2.1 Dashboard
- [ ] Sensor values match HWiNFO64 within ±1°C
- [ ] Sensor values update in real time (every ~1s — should be visibly animating)
- [ ] Fan RPMs match HWiNFO64
- [ ] Fan duty % shown
- [ ] Active profile name displayed
- [ ] No "loading" spinner stuck
- [ ] No console errors over 60s

#### 4.2.2 ProfileManager
- [ ] All 3 default profiles listed (silent, balanced, performance)
- [ ] Click "silent" → fans audibly slow within 5s
- [ ] Click "performance" → fans audibly speed up within 5s
- [ ] Active profile indicator updates immediately
- [ ] Active profile name on Dashboard updates

#### 4.2.3 CurveEditor ⚠️ KNOWN ISSUE TO VERIFY
- [ ] Open editor
- [ ] Drag a curve point — visual updates
- [ ] Click "Save"
- [ ] **Critical test:** restart the daemon (Ctrl-C in daemon terminal, re-run). Reload UI. Does the saved curve persist?
  - **If NO:** confirms the suspected "in-memory only" bug. File as P1, blocks v1.0 if curves are advertised as savable.
  - **If YES:** great, scratch that off the lessons list.
- [ ] Try invalid curve (point at 0% duty for 95°C temp) — should reject or warn
- [ ] Apply custom curve to a fan, observe behavior

#### 4.2.4 CalibrationWizard
- [ ] Pick ONE fan (NOT a CPU fan — test on case fan or GPU fan only)
- [ ] Run wizard — performs 0% → 100% sweep over ~45s
- [ ] Observe RPM curve in real time
- [ ] On completion, calibration table should be saved
- [ ] Restart daemon — calibration data should persist (different storage path than curves)
- [ ] Re-open wizard — should show "previously calibrated" indicator

#### 4.2.5 HardwareDetect
- [ ] Lists all sensors + fans
- [ ] Compare to HWiNFO64's list — flag any missing
- [ ] Compare to `/api/sensors` JSON — should be identical
- [ ] Vendor/model info populated where available

#### 4.2.6 SettingsPanel
- [ ] AI provider dropdown shows: Anthropic, OpenAI, Gemini, Ollama, OpenRouter
- [ ] Add throwaway OpenAI key
- [ ] Click "Test connection" → should hit OpenAI's models endpoint, show success
- [ ] Bad key → clear error message
- [ ] Theme switcher works (if present)
- [ ] Token reset button works (regenerates `api.token`)

#### 4.2.7 AgentPanel
- [ ] Mode selector: Passive / Approval / Autonomous
- [ ] Send a manual prompt: "Why is fan X at duty Y?"
- [ ] Expect AI response within ~5s (depends on provider)
- [ ] Switch to Approval mode
- [ ] Trigger an evaluation: `POST /api/agent/evaluate` from another terminal
  ```powershell
  Invoke-RestMethod -Uri http://127.0.0.1:7272/api/agent/evaluate -Method POST -Headers $hdr
  ```
- [ ] Approval prompt should appear in UI
- [ ] Approve → action executes
- [ ] Deny → action skipped, logged
- [ ] Switch to Autonomous, repeat — no prompt, action executes immediately
- [ ] Audit log entry visible (Recent Decisions panel)

### 4.3 System tray interactions

- [ ] Right-click tray icon
- [ ] Click each menu item:
  - Open dashboard → window comes to front
  - Switch profile → fans respond, UI updates
  - Pause agent → agent suspends; Resume → restarts
  - Quit → daemon stops cleanly (no orphan processes)
- [ ] Restart UI — tray icon comes back

### 4.4 Toast notifications

- [ ] Trigger AI decision (Approval or Autonomous mode)
- [ ] Expect Windows toast in Action Center (notifications panel)
- [ ] Toast should show: action description, profile/fan affected, optional Approve/Deny buttons
- [ ] Click Action Center → toast appears in history
- [ ] Trigger 10 rapid identical events:
  ```powershell
  1..10 | ForEach-Object { Invoke-RestMethod -Uri http://127.0.0.1:7272/api/agent/evaluate -Method POST -Headers $hdr; Start-Sleep -Milliseconds 200 }
  ```
- [ ] Expect ONLY 1 toast (rate-limit kicks in). If you see 10 toasts, file as P1 — Windows users will hate this.

### 4.5 Performance & resource use

- [ ] Open Task Manager → Details
- [ ] Find `thermaliq-daemon.exe` — note CPU% and memory
- [ ] Find `thermaliq-ui.exe` (or whatever Tauri names it) — same
- [ ] Acceptable baselines (idle, on dashboard):
  - Daemon: <2% CPU, <50 MB RAM
  - UI: <5% CPU, <200 MB RAM
- [ ] Run for 10 minutes idle. Re-check. Memory should not grow >10% (no obvious leak).

## Mac-side prep (Claude does)

- [ ] `.claude/context/s4-ui-test-results.md` — checklist mirror with bug-capture template per component
- [ ] **Code-read the `save_curve` path** to settle the persistence question BEFORE the session. Report:
  - File + function name
  - Whether it writes to SQLite, JSON, or only memory
  - Whether daemon restart restores
- [ ] Code-read the toast rate-limit logic — report cooldown window + scope (per-event-type? per-fan?)

## Bug template (use for every issue logged)

```markdown
### [P0/P1/P2] Component: <name>

**Repro:**
1. ...
2. ...

**Expected:** ...

**Observed:** ...

**Console errors:** ...

**Screenshot:** (path or description)

**Likely cause:** (if you have a guess)
```

Append to `.claude/context/lessons.md` under a new heading:
```
## Real-Hardware UI Testing — 2026-04-NN
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Dashboard blank | WebSocket not connecting | Check daemon logs, port 7272 firewall |
| "Connection refused" toast | Daemon not running | Start daemon |
| AI panel says "no key" | Settings didn't save | Check `%APPDATA%\thermaliq\config.json`, may be a save bug |
| Tray icon missing | Windows may have hidden it | Drag from overflow tray to visible area |
| `npm run tauri dev` fails | Port 1420 in use | Kill stray Vite, or change port in `tauri.conf.json` |
| Hot reload broken | Tauri caching | Stop, `rm -rf node_modules/.vite`, restart |

## Done when

- [ ] All 7 components clicked through end-to-end
- [ ] System tray menu fully exercised
- [ ] At least one toast notification observed
- [ ] Rate-limit verified (1 toast for 10 rapid events)
- [ ] CurveEditor persistence question answered
- [ ] All bugs logged to `lessons.md` with severity
- [ ] Resource use within baseline OR documented

## Session log

```
Date executed: 2026-04-20
Time taken: ~30 min
Bugs logged: 3, severities: P0:0 P1:2 P2:1
CurveEditor persistence: broken (save fails — frontend may not send correct body format)
Toast rate-limit: not tested (AI key not persisted, agent panel non-functional without it)
Memory growth over 10 min idle: not measured (session cut short)
Daemon CPU baseline: not measured
UI CPU baseline: not measured
```

### Bugs

### [P1] CurveEditor: save fails

**Repro:** Open CurveEditor, modify curve, click Save
**Expected:** Curve saved to profile TOML on disk
**Observed:** Save fails (error response from daemon)
**Likely cause:** Frontend may not be sending required fields (profile, fan_id, points)
  or body format mismatch with backend `save_curve` handler (routes.rs:534-577)

### [P1] Settings: API key not persisted to disk

**Repro:** Open Settings, enter API key, click Save. Shows "saved". Restart daemon.
  Every page says "no API key".
**Expected:** API key persists across daemon restarts
**Observed:** Key stored in-memory only (AppState.ai_provider). Never written to config file.
**Likely cause:** `set_ai_provider` handler (routes.rs:646-705) updates in-memory state only,
  has no file write logic. Needs to write to %APPDATA%\thermaliq\config.toml.

### [P2] WebSocket: reconnect spam in console

**Repro:** Open DevTools console on UI launch
**Expected:** Clean WS connection
**Observed:** "WebSocket closed before established" errors repeat every few seconds
  for both /ws/sensors and /ws/agent. Dashboard still works via REST polling.
**Likely cause:** WS connects before Tauri webview is ready, or initial connection
  races with daemon response. Functional impact: none (data still flows via REST).

## What unblocks next

- S5 (service install) — needs UI proven to work against running daemon
- S9 (beta triage) — bug list seeds the triage queue

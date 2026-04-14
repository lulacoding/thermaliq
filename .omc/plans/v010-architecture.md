# ThermalIQ v0.1.0 — Data Layer Architecture

**Author:** worker-arch | **Date:** 2026-04-14 | **Status:** Source of truth for v0.1.0 implementation

This document defines every type, schema, and state machine required for the v0.1.0 milestone.
Implementation workers MUST use these exact definitions. Any deviation requires updating this document first.

---

## 1. SQLite Schema — `thermaliq-history` crate

Database file: `$XDG_DATA_HOME/thermaliq/history.db` (Linux), `~/Library/Application Support/thermaliq/history.db` (macOS), `%LOCALAPPDATA%\thermaliq\history.db` (Windows).

WAL mode enabled at connection time for concurrent read access from the API while the daemon writes.

```sql
-- Applied on first open via rusqlite migrations
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sensor_readings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    sensor_id   TEXT    NOT NULL,   -- matches SensorReading.id from thermaliq-common
    label       TEXT    NOT NULL,
    temp_celsius REAL   NOT NULL,
    timestamp   INTEGER NOT NULL    -- unix epoch seconds (i64)
);

CREATE INDEX IF NOT EXISTS idx_sensor_readings_sensor_ts
    ON sensor_readings (sensor_id, timestamp DESC);

CREATE TABLE IF NOT EXISTS fan_readings (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    fan_id       TEXT    NOT NULL,   -- matches FanReading.id from thermaliq-common
    label        TEXT    NOT NULL,
    rpm          INTEGER NOT NULL,
    duty_percent INTEGER NOT NULL,   -- 0..100
    timestamp    INTEGER NOT NULL    -- unix epoch seconds (i64)
);

CREATE INDEX IF NOT EXISTS idx_fan_readings_fan_ts
    ON fan_readings (fan_id, timestamp DESC);
```

### Retention policy

A background task runs every 60 minutes and deletes rows older than 7 days:

```sql
DELETE FROM sensor_readings WHERE timestamp < ?;  -- now_epoch - 7*86400
DELETE FROM fan_readings    WHERE timestamp < ?;
```

---

## 2. SensorRecord / FanRecord — Rust structs with rusqlite mapping

These live in the `thermaliq-history` crate. They map 1:1 to the SQLite rows above.

```rust
// thermaliq-history/src/models.rs

use serde::{Deserialize, Serialize};

/// A single persisted sensor reading row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorRecord {
    pub id: i64,              // SQLite AUTOINCREMENT PK
    pub sensor_id: String,    // logical sensor identifier
    pub label: String,
    pub temp_celsius: f32,
    pub timestamp: i64,       // unix epoch seconds
}

/// A single persisted fan reading row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanRecord {
    pub id: i64,              // SQLite AUTOINCREMENT PK
    pub fan_id: String,       // logical fan identifier
    pub label: String,
    pub rpm: u32,
    pub duty_percent: u8,     // 0..100
    pub timestamp: i64,       // unix epoch seconds
}
```

### rusqlite row mapping

```rust
use rusqlite::Row;

impl SensorRecord {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            sensor_id: row.get("sensor_id")?,
            label: row.get("label")?,
            temp_celsius: row.get("temp_celsius")?,
            timestamp: row.get("timestamp")?,
        })
    }
}

impl FanRecord {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            fan_id: row.get("fan_id")?,
            label: row.get("label")?,
            rpm: row.get("rpm")?,
            duty_percent: row.get("duty_percent")?,
            timestamp: row.get("timestamp")?,
        })
    }
}
```

### HistoryDb handle

```rust
// thermaliq-history/src/lib.rs

use rusqlite::Connection;
use std::path::PathBuf;

pub struct HistoryDb {
    conn: Connection,
}

impl HistoryDb {
    /// Open (or create) the history database at the platform-appropriate path.
    /// Runs migrations and enables WAL mode.
    pub fn open(path: PathBuf) -> rusqlite::Result<Self> {
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    fn migrate(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(())
    }

    /// Insert a batch of sensor readings from one poll cycle.
    pub fn insert_sensor_batch(&self, readings: &[SensorRecord]) -> rusqlite::Result<()> { todo!() }

    /// Insert a batch of fan readings from one poll cycle.
    pub fn insert_fan_batch(&self, readings: &[FanRecord]) -> rusqlite::Result<()> { todo!() }

    /// Query sensor readings for a given sensor_id within a time range.
    pub fn query_sensor(
        &self,
        sensor_id: &str,
        from_ts: i64,
        to_ts: i64,
    ) -> rusqlite::Result<Vec<SensorRecord>> { todo!() }

    /// Query fan readings for a given fan_id within a time range.
    pub fn query_fan(
        &self,
        fan_id: &str,
        from_ts: i64,
        to_ts: i64,
    ) -> rusqlite::Result<Vec<FanRecord>> { todo!() }

    /// Delete readings older than `before_ts`. Called by retention task.
    pub fn prune(&self, before_ts: i64) -> rusqlite::Result<()> { todo!() }
}
```

---

## 3. TemperatureAlert — alert definitions and evaluation

These live in `thermaliq-common/src/alerts.rs`.

```rust
// thermaliq-common/src/alerts.rs

use serde::{Deserialize, Serialize};

/// What to do when an alert threshold is breached.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlertAction {
    /// Write a tracing::warn! log line.
    Log,
    /// Send a desktop notification via the Tauri notification API.
    Notify,
    /// Initiate a graceful system shutdown (requires privilege escalation).
    Shutdown,
}

/// A single temperature alert rule.
///
/// When `sensor_id`'s reading exceeds `threshold_celsius`, fire `action`.
/// After firing, suppress re-firing for `cooldown_secs` seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemperatureAlert {
    /// Which sensor this alert monitors — must match SensorReading.id.
    pub sensor_id: String,
    /// Celsius threshold — alert fires when reading >= this value.
    pub threshold_celsius: f32,
    /// What happens when the alert fires.
    pub action: AlertAction,
    /// Minimum seconds between consecutive firings for this alert.
    /// Prevents notification spam. Set to 0 for no cooldown.
    pub cooldown_secs: u64,
}
```

---

## 4. AlertEvaluator — cooldown tracking and evaluation

Lives in `thermaliq-common/src/alerts.rs` alongside the types above.

```rust
use std::collections::HashMap;

/// Evaluates sensor readings against a set of TemperatureAlert rules.
///
/// Tracks last-fired timestamps per alert to enforce cooldown periods.
/// Not thread-safe on its own — wrap in a Mutex or run on a single task.
pub struct AlertEvaluator {
    /// Alert rules, keyed by sensor_id for O(1) lookup per reading.
    /// Multiple alerts per sensor are supported (e.g., warn at 80C, shutdown at 95C).
    alerts: Vec<TemperatureAlert>,
    /// Maps a composite key `"{sensor_id}:{threshold}"` to the unix epoch
    /// timestamp when that specific alert last fired. Prevents re-firing
    /// within the cooldown window.
    last_fired: HashMap<String, u64>,
}

/// An alert that has been triggered and passed cooldown checks.
#[derive(Debug, Clone)]
pub struct FiredAlert {
    pub alert: TemperatureAlert,
    pub actual_celsius: f32,
    pub timestamp: u64,
}

impl AlertEvaluator {
    pub fn new(alerts: Vec<TemperatureAlert>) -> Self {
        Self {
            alerts,
            last_fired: HashMap::new(),
        }
    }

    /// Evaluate a single sensor reading against all matching alerts.
    ///
    /// Returns a list of alerts that fired (threshold breached AND cooldown expired).
    /// Updates internal cooldown state for each fired alert.
    ///
    /// `now_epoch` is the current unix timestamp in seconds — passed explicitly
    /// for testability.
    pub fn evaluate(
        &mut self,
        sensor_id: &str,
        temp_celsius: f32,
        now_epoch: u64,
    ) -> Vec<FiredAlert> {
        let mut fired = Vec::new();

        for alert in &self.alerts {
            if alert.sensor_id != sensor_id {
                continue;
            }
            if temp_celsius < alert.threshold_celsius {
                continue;
            }

            // Composite key for per-alert cooldown tracking.
            let key = format!("{}:{}", alert.sensor_id, alert.threshold_celsius);

            let last = self.last_fired.get(&key).copied().unwrap_or(0);
            if now_epoch.saturating_sub(last) < alert.cooldown_secs {
                continue; // still in cooldown
            }

            self.last_fired.insert(key, now_epoch);
            fired.push(FiredAlert {
                alert: alert.clone(),
                actual_celsius: temp_celsius,
                timestamp: now_epoch,
            });
        }

        fired
    }

    /// Replace the current alert rules (e.g., config reload).
    /// Preserves cooldown state for alerts that still exist.
    pub fn replace_alerts(&mut self, alerts: Vec<TemperatureAlert>) {
        self.alerts = alerts;
        // Cooldown map entries for removed alerts will be stale but harmless.
        // They'll be evicted naturally if memory pressure ever matters.
    }
}
```

---

## 5. Fan Curve, Channel Config, and Hysteresis Controller

These live in `thermaliq-common/src/fan.rs`.

### FanMode enum

```rust
// thermaliq-common/src/fan.rs

use serde::{Deserialize, Serialize};

/// Control mode for a fan channel.
///
/// PWM: 4-pin fans, modulates the PWM signal (25kHz typical).
/// DC: 3-pin fans, modulates supply voltage via motherboard VRM.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FanMode {
    Pwm,
    Dc,
}
```

### CurvePoint and FanCurve

```rust
/// A single point on a fan curve: at `temp_celsius`, output `duty_percent`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temp_celsius: f32,
    pub duty_percent: u8, // 0..100
}

/// A fan curve defined as a sorted list of (temp, duty) control points.
///
/// Interpolation: linear between adjacent points.
/// Below the first point: clamp to first point's duty.
/// Above the last point: clamp to last point's duty.
///
/// INVARIANT: `points` must be sorted by `temp_celsius` ascending and contain
/// at least 2 points. Validate at config load time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanCurve {
    pub points: Vec<CurvePoint>,
}

impl FanCurve {
    /// Evaluate the curve at the given temperature using linear interpolation.
    ///
    /// Returns duty_percent (0..100).
    pub fn evaluate(&self, temp_celsius: f32) -> u8 {
        // Below first point — clamp
        if temp_celsius <= self.points[0].temp_celsius {
            return self.points[0].duty_percent;
        }
        // Above last point — clamp
        if temp_celsius >= self.points.last().unwrap().temp_celsius {
            return self.points.last().unwrap().duty_percent;
        }
        // Linear interpolation between bracketing points
        for window in self.points.windows(2) {
            let lo = &window[0];
            let hi = &window[1];
            if temp_celsius >= lo.temp_celsius && temp_celsius <= hi.temp_celsius {
                let range_temp = hi.temp_celsius - lo.temp_celsius;
                if range_temp == 0.0 {
                    return lo.duty_percent;
                }
                let t = (temp_celsius - lo.temp_celsius) / range_temp;
                let duty = lo.duty_percent as f32 + t * (hi.duty_percent as f32 - lo.duty_percent as f32);
                return duty.round() as u8;
            }
        }
        // Fallback (should never reach due to clamp above)
        self.points.last().unwrap().duty_percent
    }
}
```

### FanChannelConfig

```rust
/// Full configuration for a single fan channel (header).
///
/// Combines the curve, mode, and hysteresis/start-stop parameters that
/// the FanController uses to compute and apply duty cycle changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanChannelConfig {
    /// Unique identifier — matches FanReading.id from the HAL.
    pub id: String,
    /// Human-readable label (e.g., "CPU Fan", "Rear Exhaust").
    pub label: String,
    /// PWM or DC control mode.
    pub mode: FanMode,
    /// Temperature-to-duty mapping curve.
    pub curve: FanCurve,
    /// Minimum PWM/DC % at which the fan reliably starts spinning.
    /// When transitioning from stopped -> running, jump directly to this value.
    /// Typical: 20-40% depending on fan model.
    pub start_percent: u8,
    /// Below this duty %, stop the fan entirely (set to 0%).
    /// Enables 0-RPM idle mode. Must be <= start_percent.
    /// Typical: 15-30%.
    pub stop_percent: u8,
    /// Temperature must rise this many degrees above the curve's computed
    /// duty change point before increasing fan speed. Prevents rapid oscillation
    /// when temperature hovers near a curve transition.
    pub hysteresis_up_celsius: f32,
    /// Temperature must fall this many degrees below the curve's computed
    /// duty change point before decreasing fan speed. Asymmetric from up
    /// to allow more aggressive cooling ramp-up than ramp-down.
    pub hysteresis_down_celsius: f32,
    /// Hard upper limit on duty output. Overrides curve if curve returns higher.
    /// Use 100 for no limit.
    pub max_duty: u8,
    /// Hard lower limit on duty output (when fan is running).
    /// Overrides curve if curve returns lower. Use 0 for no limit.
    /// Note: when fan is stopped (0-RPM mode), duty is 0 regardless of min_duty.
    pub min_duty: u8,
}
```

### FanController — hysteresis state machine

```rust
use std::collections::HashMap;
use thermaliq_common::{SetFanDuty};

/// Per-channel runtime state tracked by the FanController.
#[derive(Debug, Clone)]
struct ChannelState {
    /// The duty % currently being output to this fan.
    current_duty: u8,
    /// Whether the fan is currently spinning (vs. stopped in 0-RPM mode).
    running: bool,
    /// The temperature reading that produced the current duty (for hysteresis comparison).
    last_temp: f32,
}

/// Controls all fan channels, applying curves with hysteresis and start/stop logic.
///
/// Call `update()` each poll cycle with the latest sensor temperatures.
/// Returns a list of SetFanDuty commands for fans whose duty changed.
pub struct FanController {
    configs: Vec<FanChannelConfig>,
    state: HashMap<String, ChannelState>,
}

impl FanController {
    pub fn new(configs: Vec<FanChannelConfig>) -> Self {
        let mut state = HashMap::new();
        for cfg in &configs {
            state.insert(cfg.id.clone(), ChannelState {
                current_duty: 0,
                running: false,
                last_temp: 0.0,
            });
        }
        Self { configs, state }
    }

    /// Compute new duty cycles for all channels given the current temperature map.
    ///
    /// `temps` maps sensor_id -> current temp_celsius. Each FanChannelConfig
    /// is evaluated against the sensor it's bound to (by convention the first
    /// sensor, or a future `source_sensor_id` field).
    ///
    /// Returns only the SetFanDuty commands for channels whose duty actually changed.
    pub fn update(&mut self, temp_celsius: f32, channel_id: &str) -> Option<SetFanDuty> {
        let cfg = self.configs.iter().find(|c| c.id == channel_id)?;
        let st = self.state.get_mut(channel_id)?;

        // 1. Evaluate the raw curve output.
        let raw_duty = cfg.curve.evaluate(temp_celsius);

        // 2. Clamp to min/max bounds.
        let clamped = raw_duty.clamp(cfg.min_duty, cfg.max_duty);

        // 3. Apply hysteresis — only change if temp moved past the band.
        let new_duty = if clamped > st.current_duty {
            // Wants to go UP — require hysteresis_up clearance.
            if temp_celsius >= st.last_temp + cfg.hysteresis_up_celsius {
                clamped
            } else {
                st.current_duty // hold
            }
        } else if clamped < st.current_duty {
            // Wants to go DOWN — require hysteresis_down clearance.
            if temp_celsius <= st.last_temp - cfg.hysteresis_down_celsius {
                clamped
            } else {
                st.current_duty // hold
            }
        } else {
            st.current_duty // no change
        };

        // 4. Apply start%/stop% logic for 0-RPM mode.
        let final_duty = if new_duty == 0 || new_duty < cfg.stop_percent {
            // Below stop threshold — turn fan off.
            if st.running {
                st.running = false;
            }
            0
        } else if !st.running && new_duty >= cfg.start_percent {
            // Fan was stopped, duty now at or above start% — spin up.
            st.running = true;
            new_duty
        } else if !st.running && new_duty < cfg.start_percent {
            // Fan stopped, duty wants to be between stop% and start% — stay off.
            0
        } else {
            // Fan is running — allow any duty >= stop%.
            // If duty drops below stop% while running, the first branch catches it.
            new_duty
        };

        // 5. Emit command only if duty actually changed.
        if final_duty != st.current_duty {
            st.current_duty = final_duty;
            st.last_temp = temp_celsius;
            Some(SetFanDuty {
                fan_id: channel_id.to_string(),
                duty_percent: final_duty,
            })
        } else {
            st.last_temp = temp_celsius; // always track latest temp for hysteresis
            None
        }
    }

    /// Replace channel configs (e.g., on config reload). Resets state.
    pub fn replace_configs(&mut self, configs: Vec<FanChannelConfig>) {
        self.state.clear();
        for cfg in &configs {
            self.state.insert(cfg.id.clone(), ChannelState {
                current_duty: 0,
                running: false,
                last_temp: 0.0,
            });
        }
        self.configs = configs;
    }
}
```

---

## 6. AgentState enum

Lives in `thermaliq-common/src/agent.rs`.

```rust
// thermaliq-common/src/agent.rs

use serde::{Deserialize, Serialize};

/// The AI agent's current lifecycle state.
///
/// The agent is a state machine that progresses through a well-defined
/// sequence: sample sensors -> analyze with LLM -> propose change ->
/// await user approval -> execute -> verify outcome. Any state can
/// transition to Error or Degraded on failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Collecting sensor data. Minimum `min_samples` readings before
    /// transitioning to Analyzing.
    Sampling,

    /// Sending sensor context to the AI provider for analysis.
    Analyzing,

    /// AI has proposed a fan curve change. Awaiting internal validation
    /// before presenting to user.
    Proposing,

    /// Proposal validated and presented to user. Blocked until the user
    /// approves or rejects via the API (`POST /agent/approve/:id`).
    Awaiting {
        intervention_id: String,
    },

    /// User approved. Applying the proposed fan curve changes.
    Executing,

    /// Changes applied. Monitoring sensors to confirm the outcome matches
    /// the AI's prediction within tolerance. Times out after `verify_timeout_secs`.
    Verifying,

    /// Verification failed or user requested undo. Reverting to the
    /// previous fan curve configuration.
    RollingBack,

    /// A fatal error occurred. Agent will not auto-recover.
    /// Requires manual restart via `POST /agent/start`.
    Error {
        reason: String,
    },

    /// A non-fatal degradation (e.g., AI provider unreachable, single sensor
    /// offline). Agent continues operating with reduced capability.
    /// Falls back to static curves until the issue resolves.
    Degraded {
        reason: String,
    },

    /// Agent has been manually suspended by the user.
    /// Resume via `POST /agent/start`.
    Suspended,
}
```

---

## 7. AgentTransition enum and legal transition table

Lives in `thermaliq-common/src/agent.rs` alongside `AgentState`.

```rust
/// Named transitions between agent states.
///
/// Each variant represents a specific event that causes a state change.
/// The `is_legal` function enforces the transition table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTransition {
    /// Enough samples collected -> begin analysis.
    SamplesReady,
    /// AI analysis complete -> propose changes.
    AnalysisDone,
    /// Internal validation passed -> present to user.
    ProposalReady,
    /// User approved the intervention.
    Approved,
    /// User rejected the intervention.
    Rejected,
    /// Fan curve changes applied successfully.
    ExecutionDone,
    /// Verification confirms outcome matches prediction.
    VerificationPassed,
    /// Verification shows outcome diverged from prediction.
    VerificationFailed,
    /// Rollback completed successfully.
    RollbackDone,
    /// Rollback itself failed — terminal error.
    RollbackFailed,
    /// A recoverable error occurred (provider timeout, sensor flake).
    DegradedEvent { reason: String },
    /// A fatal error occurred.
    FatalError { reason: String },
    /// User manually suspended the agent.
    Suspend,
    /// User resumed the agent from Suspended or Error.
    Resume,
}

impl AgentState {
    /// Returns true if transitioning from `self` via `transition` is legal.
    ///
    /// Illegal transitions should be logged as warnings and ignored — never panic.
    pub fn is_legal(&self, transition: &AgentTransition) -> bool {
        use AgentState::*;
        use AgentTransition::*;

        matches!(
            (self, transition),
            // Happy path
            (Sampling,      SamplesReady)       |
            (Analyzing,     AnalysisDone)       |
            (Proposing,     ProposalReady)      |
            (Awaiting { .. }, Approved)          |
            (Awaiting { .. }, Rejected)          |
            (Executing,     ExecutionDone)       |
            (Verifying,     VerificationPassed)  |
            (Verifying,     VerificationFailed)  |
            (RollingBack,   RollbackDone)        |
            (RollingBack,   RollbackFailed)      |

            // Rejection loops back to sampling
            // (handled by apply — Rejected -> Sampling)

            // Degradation — any active state can degrade
            (Sampling,      DegradedEvent { .. })  |
            (Analyzing,     DegradedEvent { .. })  |
            (Proposing,     DegradedEvent { .. })  |
            (Executing,     DegradedEvent { .. })  |
            (Verifying,     DegradedEvent { .. })  |

            // Fatal error — any state can error
            (Sampling,      FatalError { .. })     |
            (Analyzing,     FatalError { .. })     |
            (Proposing,     FatalError { .. })     |
            (Awaiting { .. }, FatalError { .. })   |
            (Executing,     FatalError { .. })     |
            (Verifying,     FatalError { .. })     |
            (RollingBack,   FatalError { .. })     |
            (Degraded { .. }, FatalError { .. })   |

            // Suspend — any active state
            (Sampling,      Suspend)               |
            (Analyzing,     Suspend)               |
            (Degraded { .. }, Suspend)             |

            // Resume — from suspended or error
            (Suspended,     Resume)                |
            (Error { .. },  Resume)                |

            // Degraded recovery — back to sampling
            (Degraded { .. }, SamplesReady)
        )
    }

    /// Apply a legal transition, returning the new state.
    /// Returns None if the transition is illegal.
    pub fn apply(&self, transition: AgentTransition) -> Option<AgentState> {
        if !self.is_legal(&transition) {
            return None;
        }

        use AgentTransition::*;

        let next = match transition {
            SamplesReady        => AgentState::Analyzing,
            AnalysisDone        => AgentState::Proposing,
            ProposalReady       => AgentState::Awaiting {
                intervention_id: String::new(), // caller must set the real ID
            },
            Approved            => AgentState::Executing,
            Rejected            => AgentState::Sampling, // loop back
            ExecutionDone       => AgentState::Verifying,
            VerificationPassed  => AgentState::Sampling, // success -> next cycle
            VerificationFailed  => AgentState::RollingBack,
            RollbackDone        => AgentState::Sampling, // recovered -> next cycle
            RollbackFailed      => AgentState::Error {
                reason: "rollback failed — manual intervention required".into(),
            },
            DegradedEvent { reason } => AgentState::Degraded { reason },
            FatalError { reason }    => AgentState::Error { reason },
            Suspend             => AgentState::Suspended,
            Resume              => AgentState::Sampling,
        };

        Some(next)
    }
}
```

### Legal transition table (visual reference)

```
From \ Event         | SamplesReady | AnalysisDone | ProposalReady | Approved | Rejected | ExecutionDone | VerifPass | VerifFail | RollbackDone | RollbackFail | Degrade | Fatal | Suspend | Resume
---------------------|-------------|-------------|--------------|---------|---------|--------------|----------|----------|-------------|-------------|---------|-------|---------|-------
Sampling             | Analyzing   |             |              |         |         |              |          |          |             |             | Degraded| Error | Suspend |
Analyzing            |             | Proposing   |              |         |         |              |          |          |             |             | Degraded| Error | Suspend |
Proposing            |             |             | Awaiting     |         |         |              |          |          |             |             | Degraded| Error |         |
Awaiting{id}         |             |             |              | Exec    | Sampling|              |          |          |             |             |         | Error |         |
Executing            |             |             |              |         |         | Verifying    |          |          |             |             | Degraded| Error |         |
Verifying            |             |             |              |         |         |              | Sampling | Rolling  |             |             | Degraded| Error |         |
RollingBack          |             |             |              |         |         |              |          |          | Sampling    | Error       |         | Error |         |
Error{reason}        |             |             |              |         |         |              |          |          |             |             |         |       |         | Sampling
Degraded{reason}     | Analyzing   |             |              |         |         |              |          |          |             |             |         | Error | Suspend |
Suspended            |             |             |              |         |         |              |          |          |             |             |         |       |         | Sampling
```

---

## 8. DtDtGuard — rate-of-change thermal runaway detector

Lives in `thermaliq-common/src/safety.rs`.

```rust
// thermaliq-common/src/safety.rs

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Guards against thermal runaway by monitoring the rate of temperature change.
///
/// Maintains a sliding window of (timestamp, temperature) readings.
/// If the computed dT/dt exceeds `threshold_celsius_per_sec` at any point,
/// the guard trips — indicating a dangerous thermal acceleration that
/// point-in-time thresholds would catch too late.
///
/// Example: a threshold of 5.0 C/s over a 10s window would trip if
/// the temperature rose from 60C to 110C in 10 seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DtDtGuard {
    /// Maximum allowed rate of temperature change (degrees C per second).
    /// Typical safe values: 2.0-5.0 for CPU, 1.0-3.0 for GPU.
    pub threshold_celsius_per_sec: f32,
    /// Duration of the sliding window in seconds.
    /// Readings older than this are evicted.
    pub window_secs: u64,
    /// Sliding window of (unix_epoch_secs, temp_celsius) observations.
    /// Newest at back, oldest at front.
    #[serde(skip)]
    readings: VecDeque<(u64, f32)>,
}

/// Result of a dT/dt check.
#[derive(Debug, Clone)]
pub struct DtDtResult {
    /// Whether the rate exceeded the threshold.
    pub tripped: bool,
    /// The computed rate (degrees per second). Positive = rising.
    pub rate_celsius_per_sec: f32,
    /// Number of readings currently in the window.
    pub window_size: usize,
}

impl DtDtGuard {
    pub fn new(threshold_celsius_per_sec: f32, window_secs: u64) -> Self {
        Self {
            threshold_celsius_per_sec,
            window_secs,
            readings: VecDeque::new(),
        }
    }

    /// Push a new reading and evaluate the dT/dt rate.
    ///
    /// Returns a DtDtResult indicating whether the guard has tripped.
    /// The rate is computed as (newest_temp - oldest_temp) / (newest_ts - oldest_ts).
    ///
    /// Requires at least 2 readings in the window to produce a meaningful rate.
    pub fn push(&mut self, timestamp: u64, temp_celsius: f32) -> DtDtResult {
        // Add new reading.
        self.readings.push_back((timestamp, temp_celsius));

        // Evict readings outside the window.
        let cutoff = timestamp.saturating_sub(self.window_secs);
        while self.readings.front().map_or(false, |(ts, _)| *ts < cutoff) {
            self.readings.pop_front();
        }

        // Need at least 2 readings for a rate.
        if self.readings.len() < 2 {
            return DtDtResult {
                tripped: false,
                rate_celsius_per_sec: 0.0,
                window_size: self.readings.len(),
            };
        }

        let (oldest_ts, oldest_temp) = self.readings.front().unwrap();
        let (newest_ts, newest_temp) = self.readings.back().unwrap();

        let dt_secs = newest_ts.saturating_sub(*oldest_ts) as f32;
        if dt_secs == 0.0 {
            return DtDtResult {
                tripped: false,
                rate_celsius_per_sec: 0.0,
                window_size: self.readings.len(),
            };
        }

        let rate = (newest_temp - oldest_temp) / dt_secs;

        DtDtResult {
            tripped: rate >= self.threshold_celsius_per_sec,
            rate_celsius_per_sec: rate,
            window_size: self.readings.len(),
        }
    }

    /// Clear all readings. Use on config reload or sensor re-enumeration.
    pub fn reset(&mut self) {
        self.readings.clear();
    }
}
```

---

## 9. AppState changes in thermaliq-daemon

The current `AppState` in `thermaliq-daemon/src/api/auth.rs` must be extended to include the new subsystems.

### Updated AppState

```rust
// thermaliq-daemon/src/api/auth.rs — updated

use std::sync::Arc;
use tokio::sync::RwLock;

use thermaliq_common::alerts::AlertEvaluator;
use thermaliq_common::agent::AgentState;
use thermaliq_history::HistoryDb;

/// Shared state injected into every axum handler.
#[derive(Clone)]
pub struct AppState {
    /// Bearer token for API authentication.
    pub api_token: Arc<String>,
    /// Platform hardware abstraction layer.
    pub hal: Arc<dyn thermaliq_hal::HardwareLayer>,
    /// SQLite history database for sensor/fan time-series data.
    /// Behind Arc<Mutex> because rusqlite::Connection is not Send+Sync.
    /// Use tokio::task::spawn_blocking for all DB operations.
    pub history: Arc<tokio::sync::Mutex<HistoryDb>>,
    /// Temperature alert evaluator. RwLock allows concurrent reads
    /// (API queries for alert status) with exclusive writes (evaluation + config reload).
    pub alerts: Arc<RwLock<AlertEvaluator>>,
    /// Current AI agent state. RwLock for the same read/write pattern as alerts.
    pub agent_state: Arc<RwLock<AgentState>>,
}
```

### Updated main.rs initialization

```rust
// thermaliq-daemon/src/main.rs — updated initialization

use thermaliq_common::alerts::{AlertEvaluator, TemperatureAlert};
use thermaliq_common::agent::AgentState;
use thermaliq_history::HistoryDb;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let api_token = Arc::new(token::load_or_create()?);

    let hal: Arc<dyn thermaliq_hal::HardwareLayer> =
        Arc::from(thermaliq_hal::platform_hal());

    // Open history database at platform-appropriate path.
    let history_path = thermaliq_history::default_db_path()?;
    let history = HistoryDb::open(history_path)?;
    let history = Arc::new(tokio::sync::Mutex::new(history));

    // Load alert rules from config (empty for now — populated by config loader).
    let alerts: Vec<TemperatureAlert> = vec![]; // TODO: load from config
    let alerts = Arc::new(RwLock::new(AlertEvaluator::new(alerts)));

    // Agent starts in Suspended state — user must explicitly start it.
    let agent_state = Arc::new(RwLock::new(AgentState::Suspended));

    let state = AppState {
        api_token,
        hal,
        history,
        alerts,
        agent_state,
    };

    let app = api::build_router(state);

    let bind_addr = "127.0.0.1:7272";
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = bind_addr, "thermaliq-daemon listening");

    axum::serve(listener, app).await?;
    Ok(())
}
```

---

## Crate structure

```
thermaliq-common/
  src/
    lib.rs          -- existing types (SensorReading, FanReading, etc.)
    alerts.rs       -- TemperatureAlert, AlertAction, AlertEvaluator, FiredAlert
    fan.rs          -- FanMode, CurvePoint, FanCurve, FanChannelConfig, FanController
    agent.rs        -- AgentState, AgentTransition
    safety.rs       -- DtDtGuard, DtDtResult

thermaliq-history/  (NEW crate)
  Cargo.toml        -- depends on rusqlite, serde, thermaliq-common
  src/
    lib.rs          -- HistoryDb, default_db_path()
    models.rs       -- SensorRecord, FanRecord
    schema.sql      -- CREATE TABLE statements (embedded via include_str!)

thermaliq-daemon/
  src/
    main.rs         -- updated to init history, alerts, agent_state
    api/
      auth.rs       -- AppState gains history, alerts, agent_state fields
      routes.rs     -- new endpoints for history queries, alert management
      mod.rs        -- unchanged
```

### New Cargo.toml for thermaliq-history

```toml
[package]
name = "thermaliq-history"
version = "0.1.0"
edition = "2021"

[dependencies]
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
thermaliq-common = { path = "../thermaliq-common" }
dirs = "5"
anyhow = "1"
```

---

## Derive summary

All public types derive the following traits where appropriate:

| Type | Debug | Clone | Serialize | Deserialize | PartialEq | Eq | Copy |
|------|-------|-------|-----------|-------------|-----------|-----|------|
| SensorRecord | Y | Y | Y | Y | | | |
| FanRecord | Y | Y | Y | Y | | | |
| AlertAction | Y | Y | Y | Y | Y | Y | |
| TemperatureAlert | Y | Y | Y | Y | | | |
| FiredAlert | Y | Y | | | | | |
| FanMode | Y | Y | Y | Y | Y | Y | Y |
| CurvePoint | Y | Y | Y | Y | | | Y |
| FanCurve | Y | Y | Y | Y | | | |
| FanChannelConfig | Y | Y | Y | Y | | | |
| AgentState | Y | Y | Y | Y | Y | Y | |
| AgentTransition | Y | Y | Y | Y | Y | Y | |
| DtDtGuard | Y | Y | Y | Y | | | |
| DtDtResult | Y | Y | | | | | |

---

## Implementation worker assignments

| Worker | Scope | Key types |
|--------|-------|-----------|
| **worker-history** | `thermaliq-history` crate, schema.sql, SensorRecord, FanRecord, HistoryDb | Sections 1, 2 |
| **worker-alerts** | alerts.rs (TemperatureAlert, AlertAction, AlertEvaluator, FiredAlert) | Sections 3, 4 |
| **worker-curve** | fan.rs (FanMode, CurvePoint, FanCurve, FanChannelConfig, FanController) + safety.rs (DtDtGuard) | Sections 5, 8 |
| **worker-agent** | agent.rs (AgentState, AgentTransition, transition table) + AppState updates | Sections 6, 7, 9 |

Each worker must write unit tests for their types. Minimum coverage:
- FanCurve.evaluate: test interpolation, below-first, above-last, equal-to-point
- FanController.update: test hysteresis hold, start/stop transitions, min/max clamping
- AlertEvaluator.evaluate: test threshold breach, cooldown enforcement, multiple alerts per sensor
- AgentState.is_legal + apply: test every row of the transition table
- DtDtGuard.push: test trip on fast rise, no trip on slow rise, window eviction

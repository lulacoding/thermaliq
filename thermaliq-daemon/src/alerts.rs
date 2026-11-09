//! Runs the alerts (SPEC §6, M16). The state machine is `thermaliq_common::alert`; this keeps the
//! tracker the engine thread feeds every fresh tick, the bounded record of what happened, and
//! what `GET /alerts` and `tq alerts` show. `notify` is a record: services run in session 0 and
//! cannot show UI, so the tray (M18) shows the toast. `apply_profile` is a request to the
//! arbiter (`wanted_profile`), never a call to the engine.

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use thermaliq_common::alert::{AlertTracker, Event, EventKind};
use thermaliq_common::{AlertAction, AlertEntry, HardwareSnapshot, Severity};

/// Events kept for `GET /alerts`; the oldest go first.
pub const LOG_MAX: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub at: i64,
    pub id: String,
    pub event: &'static str,
    pub severity: Severity,
    pub action: &'static str,
    pub sensor: String,
    pub value: f32,
    /// A `notify` alert that fired or repeated: the tray shows it.
    pub notify: bool,
}

#[derive(Default)]
pub struct AlertHub {
    entries: Vec<AlertEntry>,
    tracker: Option<AlertTracker>,
    log: VecDeque<Record>,
    fired_at: HashMap<String, i64>,
}

impl AlertHub {
    /// Replaces the alerts (sensor roles already resolved). Their state starts again.
    pub fn configure(&mut self, entries: Vec<AlertEntry>) {
        self.tracker = Some(AlertTracker::new(&entries));
        self.entries = entries;
        self.fired_at.clear();
    }

    /// One fresh tick. `now` is the engine's clock, `unix` the wall clock for the record.
    pub fn step(&mut self, now: Duration, unix: i64, snapshot: &HardwareSnapshot) {
        let Some(tracker) = self.tracker.as_mut() else {
            return;
        };
        let read = |id: &str| {
            snapshot
                .sensors
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.temp_celsius)
        };
        for e in tracker.step(now, read) {
            let r = record(unix, &e);
            match e.kind {
                EventKind::Fired => {
                    self.fired_at.insert(e.id.clone(), unix);
                }
                EventKind::Cleared => {
                    self.fired_at.remove(&e.id);
                }
                EventKind::Repeated => {}
            }
            eprintln!(
                "thermaliq: alert '{}' {}: {} reads {:.1} C",
                r.id, r.event, r.sensor, r.value
            );
            if self.log.len() == LOG_MAX {
                self.log.pop_front();
            }
            self.log.push_back(r);
        }
    }

    /// The sensor ids the alerts read, for history.
    pub fn sensors(&self) -> Vec<String> {
        self.entries.iter().map(|a| a.sensor.clone()).collect()
    }

    /// `(alert id, profile)` an active `apply_profile` alert asks for.
    pub fn wanted_profile(&self) -> Option<(String, String)> {
        self.tracker
            .as_ref()?
            .wanted_profile()
            .map(|(id, p)| (id.to_owned(), p.to_owned()))
    }

    /// Ids of the alerts active now, for `/status`.
    pub fn active(&self) -> Vec<String> {
        self.tracker.as_ref().map_or_else(Vec::new, |t| {
            t.states()
                .into_iter()
                .filter(|s| s.active)
                .map(|s| s.id)
                .collect()
        })
    }

    pub fn log(&self) -> Vec<Record> {
        self.log.iter().cloned().collect()
    }

    /// What `GET /alerts` answers.
    pub fn describe(&self) -> Value {
        let states = self.tracker.as_ref().map(AlertTracker::states);
        let alerts: Vec<Value> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let st = states.as_ref().and_then(|s| s.get(i));
                let state = if !a.enabled {
                    "disabled"
                } else if st.is_some_and(|s| s.active) {
                    "active"
                } else {
                    "idle"
                };
                json!({
                    "id": a.id,
                    "sensor": a.sensor,
                    "threshold": a.threshold,
                    "severity": a.severity,
                    "action": a.action,
                    "state": state,
                    "since": self.fired_at.get(&a.id),
                    "value": st.and_then(|s| s.last_value),
                    "no_reading": st.is_some_and(|s| s.missing),
                })
            })
            .collect();
        let events: Vec<Value> = self
            .log
            .iter()
            .map(|r| {
                json!({
                    "at": r.at, "id": r.id, "event": r.event, "severity": r.severity,
                    "action": r.action, "sensor": r.sensor, "value": r.value, "notify": r.notify,
                })
            })
            .collect();
        json!({ "alerts": alerts, "events": events })
    }
}

fn record(unix: i64, e: &Event) -> Record {
    let action = match e.action {
        AlertAction::Log => "log",
        AlertAction::Notify => "notify",
        AlertAction::ApplyProfile { .. } => "apply_profile",
    };
    Record {
        at: unix,
        id: e.id.clone(),
        event: match e.kind {
            EventKind::Fired => "fired",
            EventKind::Repeated => "repeated",
            EventKind::Cleared => "cleared",
        },
        severity: e.severity,
        action,
        sensor: e.sensor.clone(),
        value: e.value,
        notify: matches!(e.action, AlertAction::Notify) && e.kind != EventKind::Cleared,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use thermaliq_common::{SensorKind, SensorReading};

    fn entry(id: &str, action: AlertAction) -> AlertEntry {
        AlertEntry {
            id: id.into(),
            enabled: true,
            sensor: "cpu".into(),
            threshold: 85.0,
            severity: Severity::Warn,
            action,
        }
    }

    fn snap(t: f32) -> HardwareSnapshot {
        HardwareSnapshot {
            sensors: vec![SensorReading {
                id: "cpu".into(),
                label: "cpu".into(),
                kind: SensorKind::Cpu,
                temp_celsius: t,
            }],
            ..Default::default()
        }
    }

    fn run(h: &mut AlertHub, from: u64, n: u64, t: f32) {
        for i in from..from + n {
            h.step(Duration::from_secs(i), 1_000 + i as i64, &snap(t));
        }
    }

    #[test]
    fn a_notify_alert_is_recorded_once_for_the_tray_and_clears_with_a_record() {
        let mut h = AlertHub::default();
        h.configure(vec![entry("hot", AlertAction::Notify)]);
        run(&mut h, 0, 2, 90.0);
        assert!(h.log().is_empty(), "two hot ticks are not enough");
        run(&mut h, 2, 5, 90.0);
        let log = h.log();
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!(
            (log[0].event, log[0].action, log[0].notify),
            ("fired", "notify", true)
        );
        assert_eq!(h.active(), ["hot"]);
        assert_eq!(h.describe()["alerts"][0]["state"], "active");
        assert_eq!(h.describe()["alerts"][0]["since"], 1_002);

        run(&mut h, 7, 1, 70.0);
        let log = h.log();
        assert_eq!((log[1].event, log[1].notify), ("cleared", false));
        assert!(h.active().is_empty());
        assert_eq!(h.describe()["alerts"][0]["state"], "idle");
        assert!(h.describe()["alerts"][0]["since"].is_null());
    }

    #[test]
    fn a_log_alert_is_not_a_notification() {
        let mut h = AlertHub::default();
        h.configure(vec![entry("hot", AlertAction::Log)]);
        run(&mut h, 0, 3, 90.0);
        assert_eq!(h.log()[0].action, "log");
        assert!(!h.log()[0].notify);
    }

    #[test]
    fn the_record_keeps_the_last_hundred_events() {
        let mut h = AlertHub::default();
        h.configure(vec![entry("hot", AlertAction::Log)]);
        for round in 0..LOG_MAX as u64 {
            run(&mut h, round * 10, 3, 90.0); // fires
            run(&mut h, round * 10 + 3, 1, 70.0); // clears
        }
        let log = h.log();
        assert_eq!(log.len(), LOG_MAX);
        assert_eq!(
            log[LOG_MAX - 1].at,
            1_000 + 99 * 10 + 3,
            "the newest is kept"
        );
    }

    #[test]
    fn apply_profile_is_a_wish_for_the_arbiter_and_a_disabled_alert_is_shown_as_disabled() {
        let mut off = entry("off", AlertAction::Log);
        off.enabled = false;
        let mut h = AlertHub::default();
        h.configure(vec![
            entry(
                "cool",
                AlertAction::ApplyProfile {
                    profile: "performance".into(),
                },
            ),
            off,
        ]);
        assert_eq!(h.wanted_profile(), None);
        run(&mut h, 0, 3, 90.0);
        assert_eq!(
            h.wanted_profile(),
            Some(("cool".into(), "performance".into()))
        );
        assert_eq!(h.describe()["alerts"][1]["state"], "disabled");
    }

    #[test]
    fn no_alerts_configured_is_quiet() {
        let mut h = AlertHub::default();
        run(&mut h, 0, 5, 99.0);
        assert!(h.log().is_empty() && h.wanted_profile().is_none() && h.active().is_empty());
        assert_eq!(h.describe()["alerts"], json!([]));
    }
}

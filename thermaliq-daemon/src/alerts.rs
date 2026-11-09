//! Runs the alerts (SPEC §6, M16). The state machine is `thermaliq_common::alert`; this keeps the
//! tracker the engine thread feeds every fresh tick, the bounded record of what happened, and
//! what `GET /alerts` and `tq alerts` show. `notify` is a record: services run in session 0 and
//! cannot show UI, so the tray (M18) shows the toast. `apply_profile` is a request to the
//! arbiter (`wanted_profile`), never a call to the engine.

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Duration;
use thermaliq_common::alert::{AlertTracker, Event, EventKind};
use thermaliq_common::health::StallTracker;
use thermaliq_common::{AlertAction, AlertEntry, HardwareSnapshot, Severity};

/// Events kept for `GET /alerts`; the oldest go first.
pub const LOG_MAX: usize = 100;
/// A fan-health or dust condition that stays true is notified again after a week.
pub const CONDITION_REPEAT_SECS: i64 = 7 * 86_400;

/// A fan-health or dust finding (SPEC §7.6): one notify, a weekly repeat, a cleared record.
/// It never changes control.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    /// `fan-stalled:<fan>`, `fan-health:<fan>` or `dust:<sensor>`.
    pub key: String,
    pub message: String,
    pub severity: Severity,
    pub sensor: String,
    pub value: f32,
}

struct Active {
    since: i64,
    last_notified: i64,
    cond: Condition,
}

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
    /// Health and dust findings explain themselves; temperature alerts do not need to.
    pub message: Option<String>,
}

#[derive(Default)]
pub struct AlertHub {
    entries: Vec<AlertEntry>,
    tracker: Option<AlertTracker>,
    log: VecDeque<Record>,
    fired_at: HashMap<String, i64>,
    conds: BTreeMap<String, Active>,
    stall: StallTracker,
    /// Per fan, the 10-point duty buckets it has reported RPM at (history, then live).
    spun: HashMap<String, HashSet<u8>>,
    fan_health: HashMap<String, String>,
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
        let read = |id: &str| {
            snapshot
                .sensors
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.temp_celsius)
        };
        let events = self.tracker.as_mut().map_or_else(Vec::new, |t| t.step(now, read));
        for e in events {
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
            self.push(r);
        }
        self.step_stalls(now, unix, snapshot);
    }

    fn push(&mut self, r: Record) {
        if self.log.len() == LOG_MAX {
            self.log.pop_front();
        }
        self.log.push_back(r);
    }

    /// A fan reading 0 RPM for 30 s at a duty it has spun at is stalled (critical). A fan that
    /// never spun there is unmonitored, not stalled.
    fn step_stalls(&mut self, now: Duration, unix: i64, snapshot: &HardwareSnapshot) {
        let fans: Vec<(String, u32, u8)> = snapshot
            .fans
            .iter()
            .map(|f| (f.id.clone(), f.rpm, f.duty_percent))
            .collect();
        for (id, rpm, duty) in &fans {
            if *rpm > 0 {
                self.spun.entry(id.clone()).or_default().insert(duty / 10);
            }
        }
        let spun = &self.spun;
        let stalled = self.stall.step(now, &fans, |id, duty| {
            let b = duty / 10;
            spun.get(id)
                .is_some_and(|s| (b.saturating_sub(1)..=b + 1).any(|x| s.contains(&x)))
        });
        let active = stalled
            .into_iter()
            .map(|fan| Condition {
                key: format!("fan-stalled:{fan}"),
                message: format!("fan '{fan}' reads 0 RPM at a duty it has spun at: it may have stalled"),
                severity: Severity::Crit,
                sensor: fan,
                value: 0.0,
            })
            .collect();
        self.update_conditions("fan-stalled:", unix, active);
    }

    /// The conditions with this key prefix are now `active`. A new one fires (one notify), a
    /// standing one repeats after a week, one that ended clears.
    pub fn update_conditions(&mut self, prefix: &str, unix: i64, active: Vec<Condition>) {
        for c in &active {
            match self.conds.get_mut(&c.key) {
                None => {
                    self.conds.insert(
                        c.key.clone(),
                        Active { since: unix, last_notified: unix, cond: c.clone() },
                    );
                    self.condition_record(unix, c, "fired");
                }
                Some(a) => {
                    a.cond = c.clone();
                    if unix - a.last_notified >= CONDITION_REPEAT_SECS {
                        a.last_notified = unix;
                        self.condition_record(unix, c, "repeated");
                    }
                }
            }
        }
        let gone: Vec<String> = self
            .conds
            .keys()
            .filter(|k| k.starts_with(prefix) && !active.iter().any(|c| &c.key == *k))
            .cloned()
            .collect();
        for k in gone {
            if let Some(a) = self.conds.remove(&k) {
                self.condition_record(unix, &a.cond, "cleared");
            }
        }
    }

    fn condition_record(&mut self, unix: i64, c: &Condition, event: &'static str) {
        eprintln!("thermaliq: {} {event}: {}", c.key, c.message);
        self.push(Record {
            at: unix,
            id: c.key.clone(),
            event,
            severity: c.severity,
            action: "notify",
            sensor: c.sensor.clone(),
            value: c.value,
            notify: event != "cleared",
            message: Some(c.message.clone()),
        });
    }

    /// History's view of the fans: the duty buckets each has spun at, learned at start and hourly.
    pub fn set_spun(&mut self, spun: HashMap<String, HashSet<u8>>) {
        for (fan, buckets) in spun {
            self.spun.entry(fan).or_default().extend(buckets);
        }
    }

    /// `ok`, `declining`, `unmonitored` or `insufficient_history` per fan, from history.
    pub fn set_fan_health(&mut self, health: HashMap<String, String>) {
        self.fan_health = health;
    }

    /// What `/fans` shows: a live stall wins over the history verdict.
    pub fn fan_health(&self, fan: &str) -> String {
        if self.conds.contains_key(&format!("fan-stalled:{fan}")) {
            return "stalled".into();
        }
        self.fan_health.get(fan).cloned().unwrap_or_else(|| "unknown".into())
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
                    "message": r.message,
                })
            })
            .collect();
        let conditions: Vec<Value> = self
            .conds
            .values()
            .map(|a| {
                json!({
                    "id": a.cond.key, "severity": a.cond.severity, "since": a.since,
                    "message": a.cond.message,
                })
            })
            .collect();
        json!({ "alerts": alerts, "conditions": conditions, "events": events })
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
        message: None,
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

    fn cond(key: &str) -> Condition {
        Condition {
            key: key.into(),
            message: format!("{key} may be failing"),
            severity: Severity::Warn,
            sensor: "f".into(),
            value: 16.0,
        }
    }

    /// M16: fan health and dust are conditions: one notify, a weekly repeat, a cleared record.
    #[test]
    fn a_condition_is_notified_once_repeated_weekly_and_cleared_when_it_ends() {
        let mut h = AlertHub::default();
        let day = 86_400;
        h.update_conditions("fan-health:", 0, vec![cond("fan-health:f")]);
        h.update_conditions("fan-health:", day, vec![cond("fan-health:f")]);
        h.update_conditions("fan-health:", 6 * day, vec![cond("fan-health:f")]);
        assert_eq!(h.log().len(), 1, "once, not daily");
        assert_eq!((h.log()[0].event, h.log()[0].notify), ("fired", true));
        assert_eq!(h.log()[0].message.as_deref(), Some("fan-health:f may be failing"));
        h.update_conditions("fan-health:", 7 * day, vec![cond("fan-health:f")]);
        assert_eq!(h.log()[1].event, "repeated");
        assert_eq!(h.describe()["conditions"][0]["id"], "fan-health:f");
        // Another prefix's conditions are left alone; the same prefix with none clears it.
        h.update_conditions("dust:", 8 * day, vec![]);
        assert_eq!(h.log().len(), 2);
        h.update_conditions("fan-health:", 8 * day, vec![]);
        assert_eq!((h.log()[2].event, h.log()[2].notify), ("cleared", false));
        assert_eq!(h.describe()["conditions"], json!([]));
    }

    fn fan_tick(h: &mut AlertHub, at: u64, rpm: u32, duty: u8) {
        let mut s = snap(40.0);
        s.fans = vec![thermaliq_common::FanReading {
            id: "f".into(),
            label: "f".into(),
            rpm,
            duty_percent: duty,
            kind: thermaliq_common::FanKind::Fan,
        }];
        h.step(Duration::from_secs(at), 1_000 + at as i64, &s);
    }

    #[test]
    fn a_stall_is_critical_after_30_seconds_at_a_duty_the_fan_has_spun_at_and_clears() {
        let mut h = AlertHub::default();
        h.set_spun([("f".to_owned(), [6u8].into())].into());
        fan_tick(&mut h, 0, 0, 60);
        fan_tick(&mut h, 29, 0, 60);
        assert!(h.log().is_empty());
        fan_tick(&mut h, 30, 0, 60);
        let log = h.log();
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!((log[0].id.as_str(), log[0].severity, log[0].notify), ("fan-stalled:f", Severity::Crit, true));
        assert_eq!(h.fan_health("f"), "stalled");
        fan_tick(&mut h, 31, 900, 60);
        assert_eq!(h.log()[1].event, "cleared");
        assert_eq!(h.fan_health("f"), "unknown", "no history verdict was set");
    }

    #[test]
    fn a_fan_with_no_history_is_not_stalled_until_it_has_been_seen_spinning() {
        let mut h = AlertHub::default();
        fan_tick(&mut h, 0, 0, 60);
        fan_tick(&mut h, 40, 0, 60);
        assert!(h.log().is_empty(), "never spun: unmonitored, not stalled");
        fan_tick(&mut h, 41, 900, 60); // now it has
        fan_tick(&mut h, 42, 0, 60);
        fan_tick(&mut h, 80, 0, 60);
        assert_eq!(h.log().len(), 1);
    }

    #[test]
    fn per_fan_health_from_history_is_kept_for_the_fans_list() {
        let mut h = AlertHub::default();
        h.set_fan_health([("f".to_owned(), "declining".to_owned())].into());
        assert_eq!(h.fan_health("f"), "declining");
        assert_eq!(h.fan_health("other"), "unknown");
    }

    #[test]
    fn no_alerts_configured_is_quiet() {
        let mut h = AlertHub::default();
        run(&mut h, 0, 5, 99.0);
        assert!(h.log().is_empty() && h.wanted_profile().is_none() && h.active().is_empty());
        assert_eq!(h.describe()["alerts"], json!([]));
    }
}

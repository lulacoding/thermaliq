//! Global temperature alerts (SPEC §6, M16): a pure state machine. Time and readings are passed
//! in, nothing here reads a clock, a sensor or a file, and nothing runs an action. The daemon
//! feeds it every tick and carries out what it reports.

use crate::{AlertAction, AlertEntry, Severity, MAX_VALID_TEMP_C, MIN_VALID_TEMP_C};
use std::time::Duration;

/// Consecutive ticks at or above the threshold before an alert fires.
pub const FIRE_TICKS: u32 = 3;
/// An active alert clears only below `threshold - HYSTERESIS_C`.
pub const HYSTERESIS_C: f32 = 2.0;
/// While active, a warning repeats at most this often.
pub const REPEAT_WARN: Duration = Duration::from_secs(10 * 60);
/// While active, a critical alert repeats at most this often.
pub const REPEAT_CRIT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Fired,
    /// Still active after the repeat interval.
    Repeated,
    Cleared,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: String,
    pub kind: EventKind,
    pub severity: Severity,
    pub action: AlertAction,
    pub sensor: String,
    pub value: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlertState {
    pub id: String,
    pub active: bool,
    /// The sensor gave no usable reading on the latest tick.
    pub missing: bool,
    /// When it fired, on the clock passed to `step`.
    pub since: Option<Duration>,
    pub last_value: Option<f32>,
}

enum Phase {
    Idle {
        hot: u32,
    },
    Active {
        since: Duration,
        last_notified: Duration,
    },
}

struct Slot {
    entry: AlertEntry,
    phase: Phase,
    missing: bool,
    last_value: Option<f32>,
}

pub struct AlertTracker {
    slots: Vec<Slot>,
}

impl AlertTracker {
    pub fn new(alerts: &[AlertEntry]) -> Self {
        Self {
            slots: alerts
                .iter()
                .map(|a| Slot {
                    entry: a.clone(),
                    phase: Phase::Idle { hot: 0 },
                    missing: false,
                    last_value: None,
                })
                .collect(),
        }
    }

    /// One tick. `read` gives the latest temperature of a sensor id. A missing, non-finite or
    /// out-of-range reading changes nothing: it is never a reason to fire or to clear.
    pub fn step(&mut self, now: Duration, read: impl Fn(&str) -> Option<f32>) -> Vec<Event> {
        let mut events = Vec::new();
        for s in self.slots.iter_mut().filter(|s| s.entry.enabled) {
            let value = read(&s.entry.sensor)
                .filter(|v| v.is_finite() && (MIN_VALID_TEMP_C..=MAX_VALID_TEMP_C).contains(v));
            s.missing = value.is_none();
            let Some(v) = value else { continue };
            s.last_value = Some(v);
            let hot = v >= s.entry.threshold;
            let repeat = match s.entry.severity {
                Severity::Warn => REPEAT_WARN,
                Severity::Crit => REPEAT_CRIT,
            };
            let kind = match &mut s.phase {
                Phase::Idle { hot: n } if hot => {
                    *n += 1;
                    (*n >= FIRE_TICKS).then(|| {
                        s.phase = Phase::Active {
                            since: now,
                            last_notified: now,
                        };
                        EventKind::Fired
                    })
                }
                Phase::Idle { hot: n } => {
                    *n = 0;
                    None
                }
                Phase::Active { .. } if v < s.entry.threshold - HYSTERESIS_C => {
                    s.phase = Phase::Idle { hot: 0 };
                    Some(EventKind::Cleared)
                }
                Phase::Active { last_notified, .. } => {
                    (now.saturating_sub(*last_notified) >= repeat).then(|| {
                        *last_notified = now;
                        EventKind::Repeated
                    })
                }
            };
            if let Some(kind) = kind {
                events.push(Event {
                    id: s.entry.id.clone(),
                    kind,
                    severity: s.entry.severity,
                    action: s.entry.action.clone(),
                    sensor: s.entry.sensor.clone(),
                    value: v,
                });
            }
        }
        events
    }

    pub fn states(&self) -> Vec<AlertState> {
        self.slots
            .iter()
            .map(|s| AlertState {
                id: s.entry.id.clone(),
                active: matches!(s.phase, Phase::Active { .. }),
                missing: s.missing,
                since: match s.phase {
                    Phase::Active { since, .. } => Some(since),
                    Phase::Idle { .. } => None,
                },
                last_value: s.last_value,
            })
            .collect()
    }

    /// The profile an active `apply_profile` alert asks for: the highest severity wins, then
    /// the earlier alert in `config.toml`. `(alert id, profile)`.
    pub fn wanted_profile(&self) -> Option<(&str, &str)> {
        let mut best: Option<(&Slot, &str)> = None;
        for s in &self.slots {
            let AlertAction::ApplyProfile { profile } = &s.entry.action else {
                continue;
            };
            if !s.entry.enabled || !matches!(s.phase, Phase::Active { .. }) {
                continue;
            }
            if best.is_none_or(|(b, _)| {
                s.entry.severity == Severity::Crit && b.entry.severity == Severity::Warn
            }) {
                best = Some((s, profile));
            }
        }
        best.map(|(s, p)| (s.entry.id.as_str(), p))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{AlertAction, AlertEntry, Severity};
    use std::time::Duration;

    fn alert(id: &str, threshold: f32, severity: Severity, action: AlertAction) -> AlertEntry {
        AlertEntry {
            id: id.into(),
            enabled: true,
            sensor: "cpu".into(),
            threshold,
            severity,
            action,
        }
    }

    fn warn() -> AlertEntry {
        alert("hot", 85.0, Severity::Warn, AlertAction::Notify)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// One tick at `t` seconds with the sensor reading `v` (`None`: no reading).
    fn tick(t: &mut AlertTracker, at: u64, v: Option<f32>) -> Vec<(String, EventKind)> {
        t.step(secs(at), |_| v)
            .into_iter()
            .map(|e| (e.id, e.kind))
            .collect()
    }

    fn fired(id: &str) -> (String, EventKind) {
        (id.into(), EventKind::Fired)
    }

    #[test]
    fn it_fires_on_the_third_hot_tick_in_a_row_and_a_dip_starts_the_count_again() {
        let mut t = AlertTracker::new(&[warn()]);
        assert!(tick(&mut t, 0, Some(90.0)).is_empty());
        assert!(tick(&mut t, 1, Some(90.0)).is_empty());
        assert!(
            tick(&mut t, 2, Some(84.0)).is_empty(),
            "a dip resets the count"
        );
        assert!(tick(&mut t, 3, Some(90.0)).is_empty());
        assert!(tick(&mut t, 4, Some(90.0)).is_empty());
        assert_eq!(tick(&mut t, 5, Some(90.0)), [fired("hot")]);
        assert!(tick(&mut t, 6, Some(90.0)).is_empty(), "once, not per tick");
    }

    #[test]
    fn a_reading_at_the_threshold_counts_as_hot() {
        let mut t = AlertTracker::new(&[warn()]);
        for at in 0..2 {
            tick(&mut t, at, Some(85.0));
        }
        assert_eq!(tick(&mut t, 2, Some(85.0)), [fired("hot")]);
    }

    #[test]
    fn it_clears_only_below_threshold_minus_two_degrees_and_says_so_once() {
        let mut t = AlertTracker::new(&[warn()]);
        for at in 0..3 {
            tick(&mut t, at, Some(90.0));
        }
        assert!(
            tick(&mut t, 3, Some(83.0)).is_empty(),
            "83 is still inside the band"
        );
        assert!(tick(&mut t, 4, Some(85.0)).is_empty());
        assert_eq!(
            tick(&mut t, 5, Some(82.9)),
            [("hot".to_owned(), EventKind::Cleared)]
        );
        assert!(tick(&mut t, 6, Some(82.0)).is_empty());
        // It can fire again afterwards, after three more hot ticks.
        for at in 7..9 {
            assert!(tick(&mut t, at, Some(90.0)).is_empty());
        }
        assert_eq!(tick(&mut t, 9, Some(90.0)), [fired("hot")]);
    }

    #[test]
    fn it_repeats_while_active_every_ten_minutes_for_warn_and_five_for_crit() {
        let crit = alert("melt", 92.0, Severity::Crit, AlertAction::Log);
        let mut t = AlertTracker::new(&[warn(), crit]);
        for at in 0..3 {
            tick(&mut t, at, Some(95.0));
        }
        // Both fired at t=2. Nothing until the shorter cooldown has passed.
        assert!(tick(&mut t, 2 + 299, Some(95.0)).is_empty());
        assert_eq!(
            tick(&mut t, 2 + 300, Some(95.0)),
            [("melt".to_owned(), EventKind::Repeated)]
        );
        assert!(tick(&mut t, 2 + 599, Some(95.0))
            .iter()
            .all(|e| e.0 != "hot"));
        assert!(
            tick(&mut t, 2 + 600, Some(95.0)).contains(&("hot".to_owned(), EventKind::Repeated))
        );
    }

    #[test]
    fn no_reading_changes_nothing_in_either_state() {
        let mut t = AlertTracker::new(&[warn()]);
        tick(&mut t, 0, Some(90.0));
        tick(&mut t, 1, Some(90.0));
        for bad in [None, Some(f32::NAN), Some(999.0), Some(-100.0)] {
            assert!(tick(&mut t, 2, bad).is_empty());
        }
        assert_eq!(
            tick(&mut t, 3, Some(90.0)),
            [fired("hot")],
            "the count was kept"
        );
        for bad in [None, Some(f32::NAN), Some(999.0)] {
            assert!(tick(&mut t, 4, bad).is_empty());
        }
        assert!(
            t.states()[0].active,
            "a sensor that goes away does not clear a fire"
        );
        assert!(t.states()[0].missing);
    }

    #[test]
    fn a_disabled_alert_never_fires() {
        let mut off = warn();
        off.enabled = false;
        let mut t = AlertTracker::new(&[off]);
        for at in 0..10 {
            assert!(tick(&mut t, at, Some(99.0)).is_empty());
        }
        assert!(!t.states()[0].active);
    }

    #[test]
    fn the_profile_to_apply_is_the_highest_severity_active_alert_then_config_order() {
        let ap = |p: &str| AlertAction::ApplyProfile { profile: p.into() };
        let list = [
            alert("a", 80.0, Severity::Warn, ap("cool")),
            alert("b", 80.0, Severity::Crit, ap("max")),
            alert("c", 80.0, Severity::Crit, ap("other")),
            alert("d", 80.0, Severity::Crit, AlertAction::Log),
        ];
        let mut t = AlertTracker::new(&list);
        assert_eq!(t.wanted_profile(), None);
        for at in 0..3 {
            tick(&mut t, at, Some(90.0));
        }
        assert_eq!(t.wanted_profile(), Some(("b", "max")));
        // Cooling below the band releases it.
        tick(&mut t, 3, Some(70.0));
        assert_eq!(t.wanted_profile(), None);
    }

    #[test]
    fn each_alert_reads_its_own_sensor() {
        let mut gpu = warn();
        gpu.id = "gpu-hot".into();
        gpu.sensor = "gpu".into();
        let mut t = AlertTracker::new(&[warn(), gpu]);
        let read = |s: &str| Some(if s == "gpu" { 90.0 } else { 40.0 });
        for at in 0..2 {
            assert!(t.step(secs(at), read).is_empty());
        }
        let e = t.step(secs(2), read);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, "gpu-hot");
        assert_eq!(e[0].value, 90.0);
    }
}

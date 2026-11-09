//! Fan health and dust (SPEC §7.6, M16): reads history, runs the pure verdicts in
//! `thermaliq_common::health`, and reports findings to the alert hub. Alerts only: nothing here
//! can change a fan.

use crate::alerts::Condition;
use crate::history::HistoryDb;
use crate::runtime::Handle;
use crate::store::write_atomic;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use thermaliq_common::health::{dust, fan_history, Aligned, DustVerdict, FanHealth};
use thermaliq_common::{HardwareSnapshot, MetricKind, SensorKind, Severity};

const DAY: i64 = 86_400;
const FILE: &str = "health.json";

/// What one pass over history found.
#[derive(Debug, Default)]
pub struct Evaluation {
    /// `ok`, `declining`, `unmonitored` or `insufficient_history` per fan.
    pub fan_health: HashMap<String, String>,
    /// Per fan, the 10-point duty buckets it has reported RPM at in the last 30 days.
    pub spun: HashMap<String, HashSet<u8>>,
    /// `fan-health:` and `dust:` findings that hold now.
    pub conditions: Vec<Condition>,
}

/// Where the dust comparison starts: the last `tq alerts ack`, else the first history row.
/// A missing or bad file is never an ack.
pub fn baseline_from(dir: &Path, db: &HistoryDb) -> i64 {
    let acked = std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["dust_baseline_from"].as_i64());
    acked
        .or_else(|| db.first_recorded_at().ok().flatten())
        .unwrap_or(i64::MAX)
}

/// `tq alerts ack`: the filters were cleaned, so the next week of readings is the new baseline.
pub fn ack(dir: &Path, now: i64) -> Result<(), String> {
    write_atomic(
        &dir.join(FILE),
        serde_json::json!({ "dust_baseline_from": now })
            .to_string()
            .as_bytes(),
    )
    .map_err(|e| format!("{FILE}: {e}"))
}

fn minute_means<T: Copy + Into<f64>>(rows: &[(i64, T)]) -> BTreeMap<i64, f32> {
    let mut sums: BTreeMap<i64, (f64, u32)> = BTreeMap::new();
    for (t, v) in rows {
        let s = sums.entry(t / 60).or_default();
        s.0 += (*v).into();
        s.1 += 1;
    }
    sums.into_iter()
        .map(|(m, (s, n))| (m, (s / f64::from(n)) as f32))
        .collect()
}

pub fn evaluate(
    db: &HistoryDb,
    snapshot: &HardwareSnapshot,
    now: i64,
    baseline_from: i64,
) -> rusqlite::Result<Evaluation> {
    let mut out = Evaluation::default();
    let mut duty_at: BTreeMap<i64, Vec<f32>> = BTreeMap::new();
    for f in &snapshot.fans {
        let rows = db.fan_rows_since(&f.id, now - 30 * DAY)?;
        let verdict = fan_history(&rows, now);
        let spun: HashSet<u8> = rows.iter().filter(|r| r.1 > 0).map(|r| r.2 / 10).collect();
        if !spun.is_empty() {
            out.spun.insert(f.id.clone(), spun);
        }
        let name = match verdict {
            FanHealth::Ok => "ok",
            FanHealth::Declining { drop_pct, duty } => {
                out.conditions.push(Condition {
                    key: format!("fan-health:{}", f.id),
                    message: format!(
                        "fan '{}' may be failing: its RPM at {duty}% duty is down {drop_pct}% over 30 days",
                        f.label
                    ),
                    severity: Severity::Warn,
                    sensor: f.id.clone(),
                    value: drop_pct as f32,
                });
                "declining"
            }
            FanHealth::Unmonitored => "unmonitored",
            FanHealth::InsufficientHistory => "insufficient_history",
        };
        out.fan_health.insert(f.id.clone(), name.into());
        for (m, d) in minute_means(
            &db.fan_rows_since(&f.id, baseline_from)?
                .iter()
                .map(|r| (r.0, f64::from(r.2)))
                .collect::<Vec<_>>(),
        ) {
            duty_at.entry(m).or_default().push(d);
        }
    }
    let duty_at: BTreeMap<i64, f32> = duty_at
        .into_iter()
        .map(|(m, mut v)| {
            v.sort_by(f32::total_cmp);
            (m, v[v.len() / 2])
        })
        .collect();

    // A week of baseline, then a week of present, never overlapping.
    let recent_from = (now - 7 * DAY).max(baseline_from.saturating_add(7 * DAY));
    if now >= baseline_from.saturating_add(8 * DAY) {
        for s in snapshot
            .sensors
            .iter()
            .filter(|s| s.kind != SensorKind::Other)
        {
            let want = if s.kind == SensorKind::Cpu {
                MetricKind::CpuLoad
            } else {
                MetricKind::GpuLoad
            };
            let Some(load_id) = snapshot
                .metrics
                .iter()
                .find(|m| m.kind == want)
                .map(|m| m.id.as_str())
            else {
                continue;
            };
            let temps = minute_means(
                &db.sensor_rows_since(&s.id, baseline_from)?
                    .iter()
                    .map(|r| (r.0, f64::from(r.1)))
                    .collect::<Vec<_>>(),
            );
            let loads = minute_means(&db.metric_rows_since(load_id, baseline_from)?);
            let (mut then, mut recent) = (Vec::new(), Vec::new());
            for (m, temp) in temps {
                let (Some(load), Some(duty)) = (loads.get(&m), duty_at.get(&m)) else {
                    continue;
                };
                let a = Aligned {
                    temp,
                    load: *load,
                    duty: *duty,
                };
                if m * 60 < baseline_from + 7 * DAY {
                    then.push(a);
                } else if m * 60 >= recent_from {
                    recent.push(a);
                }
            }
            if let DustVerdict::Dusty {
                load_pct,
                temp_rise,
                duty_rise,
            } = dust(&then, &recent)
            {
                out.conditions.push(Condition {
                    key: format!("dust:{}", s.id),
                    message: format!(
                        "{} runs {temp_rise:.0} °C hotter at about {load_pct}% load than its baseline from {} with the fans {duty_rise:.0} points faster: check the filters and fans",
                        s.label,
                        &crate::lifecycle::stamp(baseline_from.max(0) as u64)[..10],
                    ),
                    severity: Severity::Warn,
                    sensor: s.id.clone(),
                    value: temp_rise,
                });
            }
        }
    }
    Ok(out)
}

/// Hourly, from the first minute after start: reads history, tells the hub.
pub fn spawn(
    handle: Handle,
    history: Arc<Mutex<HistoryDb>>,
    dir: std::path::PathBuf,
    first: std::time::Duration,
    every: std::time::Duration,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        std::thread::sleep(first);
        loop {
            let snap = handle.cache().snapshot;
            if !snap.fans.is_empty() {
                let now = crate::runtime::unix_now();
                let found = {
                    let db = history.lock().unwrap_or_else(PoisonError::into_inner);
                    let from = baseline_from(&dir, &db);
                    evaluate(&db, &snap, now, from)
                };
                match found {
                    Ok(e) => handle.apply_health(e, now),
                    Err(e) => eprintln!("thermaliq: health check failed: {e}"),
                }
            }
            std::thread::sleep(every);
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use thermaliq_common::{
        FanKind, FanReading, MetricKind, MetricReading, SensorKind, SensorReading,
    };

    const DAY: i64 = 86_400;
    const NOW: i64 = 100 * DAY;

    fn fan(id: &str, rpm: u32, duty: u8) -> FanReading {
        FanReading {
            id: id.into(),
            label: format!("{id} label"),
            rpm,
            duty_percent: duty,
            kind: FanKind::Fan,
        }
    }

    fn cpu(temp: f32) -> SensorReading {
        SensorReading {
            id: "cpu".into(),
            label: "CPU".into(),
            kind: SensorKind::Cpu,
            temp_celsius: temp,
        }
    }

    fn load(v: f64) -> MetricReading {
        MetricReading {
            id: "load".into(),
            kind: MetricKind::CpuLoad,
            value: v,
        }
    }

    /// Hourly rows for `days` days ending at NOW; `at(age_days)` gives that hour's readings.
    fn history(
        days: i64,
        at: impl Fn(i64) -> (Vec<SensorReading>, Vec<FanReading>, Vec<MetricReading>),
    ) -> HistoryDb {
        let mut db = HistoryDb::in_memory().unwrap();
        for h in 0..days * 24 {
            let t = NOW - days * DAY + h * 3600;
            let (s, f, m) = at((NOW - t) / DAY);
            db.record_tick(t, &s, &f, &m).unwrap();
        }
        db
    }

    fn now_snapshot() -> HardwareSnapshot {
        HardwareSnapshot {
            sensors: vec![cpu(60.0)],
            fans: vec![fan("tach", 840, 60), fan("case", 0, 60)],
            metrics: vec![load(55.0)],
        }
    }

    #[test]
    fn a_declining_fan_is_reported_and_one_without_a_tach_is_unmonitored() {
        let db = history(30, |age| {
            let rpm = if age >= 23 {
                1000
            } else if age < 7 {
                840
            } else {
                920
            };
            (
                vec![],
                vec![fan("tach", rpm, 60), fan("case", 0, 60)],
                vec![],
            )
        });
        let e = evaluate(&db, &now_snapshot(), NOW, NOW - 30 * DAY).unwrap();
        assert_eq!(e.fan_health["tach"], "declining");
        assert_eq!(e.fan_health["case"], "unmonitored");
        assert_eq!(e.conditions.len(), 1, "{:?}", e.conditions);
        let c = &e.conditions[0];
        assert_eq!(c.key, "fan-health:tach");
        assert!(
            c.message.contains("tach label") && c.message.contains("16%"),
            "{}",
            c.message
        );
        assert!(e.spun["tach"].contains(&6) && !e.spun.contains_key("case"));
    }

    #[test]
    fn a_healthy_fan_has_no_condition() {
        let db = history(30, |_| (vec![], vec![fan("tach", 1000, 60)], vec![]));
        let e = evaluate(&db, &now_snapshot(), NOW, NOW - 30 * DAY).unwrap();
        assert_eq!(e.fan_health["tach"], "ok");
        assert!(e.conditions.is_empty());
    }

    fn dusty_history() -> HistoryDb {
        history(30, |age| {
            // The baseline is the first week (age 23..30); the last week is hotter, fans faster.
            let (temp, duty) = if age >= 23 {
                (60.0, 40)
            } else if age < 7 {
                (66.0, 52)
            } else {
                (62.0, 45)
            };
            (
                vec![cpu(temp)],
                vec![fan("tach", 1000, duty)],
                vec![load(55.0)],
            )
        })
    }

    #[test]
    fn dust_is_reported_with_the_baseline_date_when_it_is_hotter_and_the_fans_are_faster() {
        let e = evaluate(&dusty_history(), &now_snapshot(), NOW, NOW - 30 * DAY).unwrap();
        let c = e
            .conditions
            .iter()
            .find(|c| c.key == "dust:cpu")
            .expect("dust condition");
        assert!(
            c.message.contains("CPU") && c.message.contains("filters"),
            "{}",
            c.message
        );
        assert!(
            c.message.contains("baseline from"),
            "the date explains a summer false alarm: {}",
            c.message
        );
    }

    #[test]
    fn a_baseline_that_is_not_a_week_old_says_nothing() {
        // Acknowledged 5 days ago: not enough since then to compare.
        let e = evaluate(&dusty_history(), &now_snapshot(), NOW, NOW - 5 * DAY).unwrap();
        assert!(e.conditions.iter().all(|c| c.key != "dust:cpu"));
    }

    #[test]
    fn the_baseline_defaults_to_the_first_history_row_and_an_ack_moves_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = dusty_history();
        assert_eq!(baseline_from(dir.path(), &db), NOW - 30 * DAY);
        ack(dir.path(), 777).unwrap();
        assert_eq!(baseline_from(dir.path(), &db), 777);
        std::fs::write(dir.path().join("health.json"), "not json").unwrap();
        assert_eq!(
            baseline_from(dir.path(), &db),
            NOW - 30 * DAY,
            "a bad file is not an ack"
        );
    }
}

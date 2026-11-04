//! History DB (SPEC §6): one transaction per tick; raw readings kept 7 days,
//! then downsampled to 1/min until 90 days. Timestamps are unix seconds, passed in.

use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::path::Path;
use thermaliq_common::{
    Curve, CustomSensor, FanReading, HardwareSnapshot, MetricReading, Profile, SensorKind,
    SensorReading, MAX_VALID_TEMP_C, MIN_VALID_TEMP_C,
};

const DAY: i64 = 86_400;
const KEEP_DAYS: i64 = 90;

/// Ids of the snapshot sensors to record (SPEC §6): every CPU/GPU sensor plus every
/// sensor any saved profile references. Ids that name custom sensors aren't in the
/// snapshot, so they drop out here.
pub fn recorded_sensor_ids(profiles: &[Profile], snapshot: &HardwareSnapshot) -> HashSet<String> {
    let mut refs: HashSet<&str> = HashSet::new();
    for p in profiles {
        for c in &p.curves {
            match c {
                Curve::Linear { source, .. }
                | Curve::Graph { source, .. }
                | Curve::Trigger { source, .. } => {
                    refs.insert(source);
                }
                Curve::Flat { .. } | Curve::Mix { .. } | Curve::Sync { .. } => {}
            }
        }
        for s in &p.sensors {
            match s {
                CustomSensor::TimeAverage { source, .. } | CustomSensor::Offset { source, .. } => {
                    refs.insert(source);
                }
                CustomSensor::Mix { sources, .. } => {
                    refs.extend(sources.iter().map(String::as_str))
                }
            }
        }
        refs.extend(p.alerts.iter().map(|a| a.sensor.as_str()));
    }
    snapshot
        .sensors
        .iter()
        .filter(|s| s.kind != SensorKind::Other || refs.contains(s.id.as_str()))
        .map(|s| s.id.clone())
        .collect()
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sensor_readings (
    id INTEGER PRIMARY KEY,
    sensor_id TEXT NOT NULL,
    temp_celsius REAL NOT NULL,
    recorded_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sensor_readings ON sensor_readings(sensor_id, recorded_at);
CREATE INDEX IF NOT EXISTS idx_sensor_time ON sensor_readings(recorded_at);
CREATE TABLE IF NOT EXISTS fan_readings (
    id INTEGER PRIMARY KEY,
    fan_id TEXT NOT NULL,
    rpm INTEGER NOT NULL,
    duty_percent INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fan_readings ON fan_readings(fan_id, recorded_at);
CREATE INDEX IF NOT EXISTS idx_fan_time ON fan_readings(recorded_at);
CREATE TABLE IF NOT EXISTS metric_readings (
    id INTEGER PRIMARY KEY,
    metric_id TEXT NOT NULL,
    value REAL NOT NULL,
    recorded_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_metric_readings ON metric_readings(metric_id, recorded_at);
CREATE INDEX IF NOT EXISTS idx_metric_time ON metric_readings(recorded_at);";

pub struct HistoryDb(Connection);

impl HistoryDb {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self(conn))
    }

    /// Writes every reading of one tick atomically: all rows land or none do.
    /// Sensor readings that aren't a finite, plausible temperature are skipped so one
    /// broken sensor can't cost the tick. Metrics that fail `MetricKind::is_valid` are skipped
    /// the same way (never stored as 0).
    pub fn record_tick(
        &mut self,
        now: i64,
        sensors: &[SensorReading],
        fans: &[FanReading],
        metrics: &[MetricReading],
    ) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO sensor_readings(sensor_id, temp_celsius, recorded_at) VALUES (?1, ?2, ?3)",
            )?;
            for r in sensors {
                if !(MIN_VALID_TEMP_C..=MAX_VALID_TEMP_C).contains(&r.temp_celsius) {
                    continue;
                }
                ins.execute(params![r.id, r.temp_celsius, now])?;
            }
            let mut ins = tx.prepare_cached(
                "INSERT INTO fan_readings(fan_id, rpm, duty_percent, recorded_at) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for r in fans {
                ins.execute(params![r.id, r.rpm, r.duty_percent, now])?;
            }
            let mut ins = tx.prepare_cached(
                "INSERT INTO metric_readings(metric_id, value, recorded_at) VALUES (?1, ?2, ?3)",
            )?;
            for r in metrics.iter().filter(|r| r.kind.is_valid(r.value)) {
                ins.execute(params![r.id, r.value, now])?;
            }
        }
        tx.commit()
    }

    /// Drops rows past 90 days and averages rows past 24 h into one per minute.
    /// The raw cutoff is minute-aligned so a bucket is never averaged in two runs.
    /// Idempotent; call it periodically (e.g. hourly).
    pub fn prune(&mut self, now: i64) -> rusqlite::Result<()> {
        let (raw, keep) = ((now - DAY) / 60 * 60, now - KEEP_DAYS * DAY);
        let tx = self.0.transaction()?;
        for (table, key, cols) in [
            ("sensor_readings", "sensor_id", "AVG(temp_celsius) AS temp_celsius"),
            (
                "fan_readings",
                "fan_id",
                "CAST(ROUND(AVG(rpm)) AS INTEGER) AS rpm, CAST(ROUND(AVG(duty_percent)) AS INTEGER) AS duty_percent",
            ),
            ("metric_readings", "metric_id", "AVG(value) AS value"),
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE recorded_at < ?1"), [keep])?;
            tx.execute_batch(&format!(
                "CREATE TEMP TABLE down AS SELECT {key}, {cols}, recorded_at / 60 * 60 AS recorded_at
                   FROM {table} WHERE recorded_at < {raw} GROUP BY {key}, recorded_at / 60 * 60;
                 DELETE FROM {table} WHERE recorded_at < {raw};
                 INSERT INTO {table} SELECT NULL, * FROM down;
                 DROP TABLE down;"
            ))?;
        }
        tx.commit()
    }

    /// `(recorded_at, temp_celsius)` oldest first.
    pub fn sensor_rows(&self, id: &str) -> rusqlite::Result<Vec<(i64, f32)>> {
        let mut q = self.0.prepare(
            "SELECT recorded_at, temp_celsius FROM sensor_readings WHERE sensor_id = ?1 ORDER BY recorded_at, id",
        )?;
        let rows = q.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// `(recorded_at, value)` oldest first.
    pub fn metric_rows(&self, id: &str) -> rusqlite::Result<Vec<(i64, f64)>> {
        let mut q = self.0.prepare(
            "SELECT recorded_at, value FROM metric_readings WHERE metric_id = ?1 ORDER BY recorded_at, id",
        )?;
        let rows = q.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// `(recorded_at, rpm, duty_percent)` oldest first.
    pub fn fan_rows(&self, id: &str) -> rusqlite::Result<Vec<(i64, u32, u8)>> {
        let mut q = self.0.prepare(
            "SELECT recorded_at, rpm, duty_percent FROM fan_readings WHERE fan_id = ?1 ORDER BY recorded_at, id",
        )?;
        let rows = q.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use thermaliq_common::{HardwareSnapshot, MetricKind, Profile, SensorKind};

    const NOW: i64 = 100 * DAY;

    fn s(id: &str, t: f32) -> SensorReading {
        SensorReading {
            id: id.into(),
            label: id.into(),
            kind: SensorKind::Cpu,
            temp_celsius: t,
        }
    }

    fn f(id: &str, rpm: u32, duty: u8) -> FanReading {
        FanReading {
            id: id.into(),
            label: id.into(),
            rpm,
            duty_percent: duty,
            kind: thermaliq_common::FanKind::Fan,
        }
    }

    fn m(id: &str, kind: MetricKind, value: f64) -> MetricReading {
        MetricReading {
            id: id.into(),
            kind,
            value,
        }
    }

    #[test]
    fn metrics_round_trip_skip_invalid_ones_and_downsample() {
        let mut db = HistoryDb::in_memory().unwrap();
        let bad = [f64::NAN, f64::INFINITY, -3.0];
        let tick = |t: i64, load: f64| {
            let mut v = vec![m("load", MetricKind::GpuLoad, load)];
            v.extend(bad.iter().map(|&b| m("power", MetricKind::GpuPower, b)));
            v.push(m("over", MetricKind::CpuLoad, 101.0));
            (t, v)
        };
        let (t, v) = tick(NOW, 50.0);
        db.record_tick(t, &[], &[], &v).unwrap();
        assert_eq!(db.metric_rows("load").unwrap(), vec![(NOW, 50.0)]);
        assert!(
            db.metric_rows("power").unwrap().is_empty(),
            "never stored as 0"
        );
        assert!(db.metric_rows("over").unwrap().is_empty());
        let base = NOW - 10 * DAY;
        for i in 0..60 {
            db.record_tick(
                base + i,
                &[],
                &[],
                &[m("old", MetricKind::GpuLoad, (i % 2) as f64 * 10.0)],
            )
            .unwrap();
        }
        db.prune(NOW).unwrap();
        assert_eq!(db.metric_rows("old").unwrap(), vec![(base, 5.0)]);
        assert_eq!(db.metric_rows("load").unwrap().len(), 1);
    }

    #[test]
    fn a_failed_tick_stores_no_metrics_either() {
        let mut db = HistoryDb::in_memory().unwrap();
        db.0.execute("DROP TABLE fan_readings", []).unwrap();
        let load = [m("load", MetricKind::GpuLoad, 1.0)];
        assert!(db.record_tick(NOW, &[], &[f("a", 1, 1)], &load).is_err());
        assert!(db.metric_rows("load").unwrap().is_empty());
    }

    #[test]
    fn tick_round_trips_sensors_and_fans() {
        let mut db = HistoryDb::in_memory().unwrap();
        db.record_tick(NOW, &[s("cpu", 50.5)], &[f("fan1", 1200, 40)], &[])
            .unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap(), vec![(NOW, 50.5)]);
        assert_eq!(db.fan_rows("fan1").unwrap(), vec![(NOW, 1200, 40)]);
    }

    #[test]
    fn tick_is_one_transaction() {
        let mut db = HistoryDb::in_memory().unwrap();
        db.0.execute("DROP TABLE fan_readings", []).unwrap();
        assert!(db
            .record_tick(NOW, &[s("a", 40.0)], &[f("fan1", 1000, 30)], &[])
            .is_err());
        assert!(db.sensor_rows("a").unwrap().is_empty());
    }

    #[test]
    fn bad_sensor_readings_are_skipped_and_the_rest_saved() {
        let mut db = HistoryDb::in_memory().unwrap();
        let ticks = [
            s("a", 40.0),
            s("nan", f32::NAN),
            s("inf", f32::INFINITY),
            s("hot", 500.0),
            s("b", 41.0),
        ];
        db.record_tick(NOW, &ticks, &[f("fan1", 1000, 30)], &[])
            .unwrap();
        assert_eq!(db.sensor_rows("a").unwrap(), vec![(NOW, 40.0)]);
        assert_eq!(db.sensor_rows("b").unwrap(), vec![(NOW, 41.0)]);
        for id in ["nan", "inf", "hot"] {
            assert!(db.sensor_rows(id).unwrap().is_empty());
        }
        assert_eq!(db.fan_rows("fan1").unwrap().len(), 1);
    }

    #[test]
    fn prune_keeps_24h_raw() {
        let mut db = HistoryDb::in_memory().unwrap();
        for i in 0..5 {
            db.record_tick(
                NOW - 23 * 3600 + i,
                &[s("cpu", 50.0)],
                &[f("fan1", 1000, 30)],
                &[],
            )
            .unwrap();
        }
        db.prune(NOW).unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap().len(), 5);
        assert_eq!(db.fan_rows("fan1").unwrap().len(), 5);
    }

    #[test]
    fn prune_cutoff_is_minute_aligned_so_a_bucket_is_averaged_once() {
        let mut db = HistoryDb::in_memory().unwrap();
        let bucket = NOW - DAY; // minute-aligned
        for (off, t) in [(10, 30.0), (20, 30.0), (40, 60.0)] {
            db.record_tick(bucket + off, &[s("cpu", t)], &[], &[])
                .unwrap();
        }
        db.prune(NOW + 30).unwrap(); // cutoff falls mid-bucket
        db.prune(NOW + 120).unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap(), vec![(bucket, 40.0)]);
    }

    #[test]
    fn recorded_set_is_cpu_gpu_plus_everything_any_profile_references() {
        let p = |toml: &str| Profile::from_toml(toml).unwrap();
        let active = p(r#"name = "active"
            [[curves]]
            type = "linear"
            id = "c"
            source = "mb_curve"
            min_temp = 30.0
            max_temp = 70.0
            min_pwm = 20
            max_pwm = 100"#);
        let other = p(r#"name = "other"
            [[sensors]]
            type = "offset"
            id = "custom"
            source = "mb_custom"
            offset = 1.0
            [[alerts]]
            sensor = "mb_alert"
            threshold = 80.0
            severity = "warn"
            action = { type = "log" }"#);
        let mut unused = s("mb_unused", 30.0);
        unused.kind = SensorKind::Other;
        let named = |id: &str| {
            let mut r = s(id, 30.0);
            r.kind = SensorKind::Other;
            r
        };
        let mut gpu = s("gpu0", 40.0);
        gpu.kind = SensorKind::Gpu;
        let snap = HardwareSnapshot {
            sensors: vec![
                s("cpu0", 40.0),
                gpu,
                unused,
                named("mb_curve"),
                named("mb_custom"),
                named("mb_alert"),
            ],
            fans: vec![],
            ..Default::default()
        };
        let got = recorded_sensor_ids(&[active, other], &snap);
        let want: HashSet<String> = ["cpu0", "gpu0", "mb_curve", "mb_custom", "mb_alert"]
            .map(String::from)
            .into();
        assert_eq!(got, want);
    }

    #[test]
    fn recorded_set_with_no_profiles_is_just_cpu_and_gpu() {
        let mut other = s("mb", 30.0);
        other.kind = SensorKind::Other;
        let snap = HardwareSnapshot {
            sensors: vec![s("cpu0", 40.0), other],
            fans: vec![],
            ..Default::default()
        };
        let got = recorded_sensor_ids(&[], &snap);
        assert_eq!(got, HashSet::from(["cpu0".to_string()]));
    }

    #[test]
    fn recorded_set_includes_every_cpu_and_gpu_sensor() {
        let mut gpu = s("gpu0", 40.0);
        gpu.kind = SensorKind::Gpu;
        let snap = HardwareSnapshot {
            sensors: vec![s("cpu0", 40.0), s("cpu1", 41.0), gpu],
            fans: vec![],
            ..Default::default()
        };
        assert_eq!(recorded_sensor_ids(&[], &snap).len(), 3);
    }

    #[test]
    fn prune_has_recorded_at_indexes() {
        let db = HistoryDb::in_memory().unwrap();
        let n: i64 = db
            .0
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('idx_sensor_time', 'idx_fan_time')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn file_db_uses_wal_and_keeps_data_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        {
            let mut db = HistoryDb::open(&path).unwrap();
            let mode: String =
                db.0.query_row("PRAGMA journal_mode", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(mode, "wal");
            db.record_tick(NOW, &[s("cpu", 50.0)], &[f("fan1", 1000, 30)], &[])
                .unwrap();
        }
        let db = HistoryDb::open(&path).unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap(), vec![(NOW, 50.0)]);
        assert_eq!(db.fan_rows("fan1").unwrap(), vec![(NOW, 1000, 30)]);
    }

    #[test]
    fn prune_downsamples_older_than_24h_to_one_per_minute() {
        let mut db = HistoryDb::in_memory().unwrap();
        let base = NOW - 10 * DAY; // minute-aligned
        for i in 0..60 {
            db.record_tick(
                base + i,
                &[s("cpu", 40.0 + (i % 2) as f32 * 10.0)],
                &[f("fan1", 1000 + i as u32, 30)],
                &[],
            )
            .unwrap();
        }
        db.prune(NOW).unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap(), vec![(base, 45.0)]);
        assert_eq!(db.fan_rows("fan1").unwrap().len(), 1);
        db.prune(NOW).unwrap(); // idempotent
        assert_eq!(db.sensor_rows("cpu").unwrap().len(), 1);
    }

    #[test]
    fn prune_drops_older_than_ninety_days() {
        let mut db = HistoryDb::in_memory().unwrap();
        db.record_tick(
            NOW - 91 * DAY,
            &[s("cpu", 50.0)],
            &[f("fan1", 1000, 30)],
            &[],
        )
        .unwrap();
        db.record_tick(
            NOW - 89 * DAY,
            &[s("cpu", 51.0)],
            &[f("fan1", 1000, 30)],
            &[],
        )
        .unwrap();
        db.prune(NOW).unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap(), vec![(NOW - 89 * DAY, 51.0)]);
        assert_eq!(db.fan_rows("fan1").unwrap().len(), 1);
    }
}

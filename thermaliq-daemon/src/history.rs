//! History DB (SPEC §6): one transaction per tick; raw readings kept 7 days,
//! then downsampled to 1/min until 90 days. Timestamps are unix seconds, passed in.

use rusqlite::{params, Connection};
use std::path::Path;
use thermaliq_common::{FanReading, SensorReading};

const DAY: i64 = 86_400;
const RAW_DAYS: i64 = 7;
const KEEP_DAYS: i64 = 90;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sensor_readings (
    id INTEGER PRIMARY KEY,
    sensor_id TEXT NOT NULL,
    temp_celsius REAL NOT NULL,
    recorded_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sensor_readings ON sensor_readings(sensor_id, recorded_at);
CREATE TABLE IF NOT EXISTS fan_readings (
    id INTEGER PRIMARY KEY,
    fan_id TEXT NOT NULL,
    rpm INTEGER NOT NULL,
    duty_percent INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fan_readings ON fan_readings(fan_id, recorded_at);";

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
    pub fn record_tick(
        &mut self,
        now: i64,
        sensors: &[SensorReading],
        fans: &[FanReading],
    ) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO sensor_readings(sensor_id, temp_celsius, recorded_at) VALUES (?1, ?2, ?3)",
            )?;
            for r in sensors {
                ins.execute(params![r.id, r.temp_celsius, now])?;
            }
            let mut ins = tx.prepare_cached(
                "INSERT INTO fan_readings(fan_id, rpm, duty_percent, recorded_at) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for r in fans {
                ins.execute(params![r.id, r.rpm, r.duty_percent, now])?;
            }
        }
        tx.commit()
    }

    /// Drops rows past 90 days and averages rows past 7 days into one per minute.
    /// Idempotent; call it periodically (e.g. hourly).
    pub fn prune(&mut self, now: i64) -> rusqlite::Result<()> {
        let (raw, keep) = (now - RAW_DAYS * DAY, now - KEEP_DAYS * DAY);
        let tx = self.0.transaction()?;
        for (table, key, cols) in [
            ("sensor_readings", "sensor_id", "AVG(temp_celsius) AS temp_celsius"),
            (
                "fan_readings",
                "fan_id",
                "CAST(ROUND(AVG(rpm)) AS INTEGER) AS rpm, CAST(ROUND(AVG(duty_percent)) AS INTEGER) AS duty_percent",
            ),
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
    use thermaliq_common::{FanReading, SensorKind, SensorReading};

    const DAY: i64 = 86_400;
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
        }
    }

    #[test]
    fn tick_round_trips_sensors_and_fans() {
        let mut db = HistoryDb::in_memory().unwrap();
        db.record_tick(NOW, &[s("cpu", 50.5)], &[f("fan1", 1200, 40)])
            .unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap(), vec![(NOW, 50.5)]);
        assert_eq!(db.fan_rows("fan1").unwrap(), vec![(NOW, 1200, 40)]);
    }

    #[test]
    fn tick_is_one_transaction() {
        let mut db = HistoryDb::in_memory().unwrap();
        // NaN binds as NULL and violates NOT NULL on the second sensor.
        let bad = [s("a", 40.0), s("b", f32::NAN)];
        assert!(db.record_tick(NOW, &bad, &[f("fan1", 1000, 30)]).is_err());
        assert!(db.sensor_rows("a").unwrap().is_empty());
        assert!(db.fan_rows("fan1").unwrap().is_empty());
    }

    #[test]
    fn prune_keeps_seven_days_raw() {
        let mut db = HistoryDb::in_memory().unwrap();
        for i in 0..5 {
            db.record_tick(NOW - 6 * DAY + i, &[s("cpu", 50.0)], &[f("fan1", 1000, 30)])
                .unwrap();
        }
        db.prune(NOW).unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap().len(), 5);
        assert_eq!(db.fan_rows("fan1").unwrap().len(), 5);
    }

    #[test]
    fn prune_downsamples_older_than_seven_days_to_one_per_minute() {
        let mut db = HistoryDb::in_memory().unwrap();
        let base = NOW - 10 * DAY; // minute-aligned
        for i in 0..60 {
            db.record_tick(
                base + i,
                &[s("cpu", 40.0 + (i % 2) as f32 * 10.0)],
                &[f("fan1", 1000 + i as u32, 30)],
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
        db.record_tick(NOW - 91 * DAY, &[s("cpu", 50.0)], &[f("fan1", 1000, 30)])
            .unwrap();
        db.record_tick(NOW - 89 * DAY, &[s("cpu", 51.0)], &[f("fan1", 1000, 30)])
            .unwrap();
        db.prune(NOW).unwrap();
        assert_eq!(db.sensor_rows("cpu").unwrap(), vec![(NOW - 89 * DAY, 51.0)]);
        assert_eq!(db.fan_rows("fan1").unwrap().len(), 1);
    }
}

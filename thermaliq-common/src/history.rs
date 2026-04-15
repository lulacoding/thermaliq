use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorRecord {
    pub id: i64,
    pub sensor_id: String,
    pub temp_celsius: f32,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanRecord {
    pub id: i64,
    pub fan_id: String,
    pub rpm: u32,
    pub duty_percent: u8,
    pub recorded_at: i64,
}

/// One measured point from a fan calibration sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationStep {
    pub duty_percent: u8,
    pub rpm: u32,
}

/// Full result of a fan calibration run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationResult {
    pub fan_id: String,
    pub calibrated_at: i64,
    /// Lowest duty% at which the fan starts spinning (0 → starts immediately).
    pub start_percent: u8,
    /// Highest duty% at which the fan stops (0 → never stops in measured range).
    pub stop_percent: u8,
    pub min_rpm: u32,
    pub max_rpm: u32,
    /// Full RPM-vs-duty sweep (21 points: 0, 5, 10, …, 100 %).
    pub steps: Vec<CalibrationStep>,
}

pub struct HistoryDb {
    conn: Connection,
}

impl HistoryDb {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS sensor_readings (
                id INTEGER PRIMARY KEY,
                sensor_id TEXT NOT NULL,
                temp_celsius REAL NOT NULL,
                recorded_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_sensor_readings_sensor_id ON sensor_readings(sensor_id, recorded_at);
            CREATE TABLE IF NOT EXISTS fan_readings (
                id INTEGER PRIMARY KEY,
                fan_id TEXT NOT NULL,
                rpm INTEGER NOT NULL,
                duty_percent INTEGER NOT NULL,
                recorded_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_fan_readings_fan_id ON fan_readings(fan_id, recorded_at);
            CREATE TABLE IF NOT EXISTS fan_calibration (
                id INTEGER PRIMARY KEY,
                fan_id TEXT NOT NULL,
                calibrated_at INTEGER NOT NULL DEFAULT (unixepoch()),
                start_percent INTEGER NOT NULL,
                stop_percent INTEGER NOT NULL,
                min_rpm INTEGER NOT NULL,
                max_rpm INTEGER NOT NULL,
                steps_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_fan_calibration_fan_id ON fan_calibration(fan_id, calibrated_at);
        ")?;
        Ok(())
    }

    pub fn insert_sensor(&self, sensor_id: &str, temp_celsius: f32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sensor_readings (sensor_id, temp_celsius) VALUES (?1, ?2)",
            params![sensor_id, temp_celsius],
        )?;
        Ok(())
    }

    pub fn insert_fan(&self, fan_id: &str, rpm: u32, duty_percent: u8) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fan_readings (fan_id, rpm, duty_percent) VALUES (?1, ?2, ?3)",
            params![fan_id, rpm, duty_percent as i64],
        )?;
        Ok(())
    }

    pub fn query_sensor_last_n(&self, sensor_id: &str, n: usize) -> Result<Vec<SensorRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, sensor_id, temp_celsius, recorded_at FROM sensor_readings WHERE sensor_id = ?1 ORDER BY recorded_at DESC LIMIT ?2"
        )?;
        let records = stmt
            .query_map(params![sensor_id, n as i64], |row| {
                Ok(SensorRecord {
                    id: row.get(0)?,
                    sensor_id: row.get(1)?,
                    temp_celsius: row.get(2)?,
                    recorded_at: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn query_fan_last_n(&self, fan_id: &str, n: usize) -> Result<Vec<FanRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, fan_id, rpm, duty_percent, recorded_at FROM fan_readings WHERE fan_id = ?1 ORDER BY recorded_at DESC LIMIT ?2"
        )?;
        let records = stmt
            .query_map(params![fan_id, n as i64], |row| {
                Ok(FanRecord {
                    id: row.get(0)?,
                    fan_id: row.get(1)?,
                    rpm: row.get(2)?,
                    duty_percent: row.get(3)?,
                    recorded_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Average temperature for `sensor_id` over the last `seconds` of history.
    /// Returns `None` when no rows exist in that window.
    pub fn query_sensor_avg_last_seconds(
        &self,
        sensor_id: &str,
        seconds: u64,
    ) -> Result<Option<f32>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let cutoff = now - seconds as i64;
        let mut stmt = self.conn.prepare(
            "SELECT AVG(temp_celsius) FROM sensor_readings \
             WHERE sensor_id = ?1 AND recorded_at >= ?2",
        )?;
        let avg: Option<f64> =
            stmt.query_row(params![sensor_id, cutoff], |row| row.get(0))?;
        Ok(avg.map(|v| v as f32))
    }

    pub fn store_calibration(&self, result: &CalibrationResult) -> Result<()> {
        let steps_json = serde_json::to_string(&result.steps)?;
        self.conn.execute(
            "INSERT INTO fan_calibration \
             (fan_id, calibrated_at, start_percent, stop_percent, min_rpm, max_rpm, steps_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                result.fan_id,
                result.calibrated_at,
                result.start_percent as i64,
                result.stop_percent as i64,
                result.min_rpm as i64,
                result.max_rpm as i64,
                steps_json,
            ],
        )?;
        Ok(())
    }

    /// Retrieve the most recent calibration result for a fan, if any.
    pub fn get_calibration(&self, fan_id: &str) -> Result<Option<CalibrationResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT fan_id, calibrated_at, start_percent, stop_percent, min_rpm, max_rpm, steps_json \
             FROM fan_calibration WHERE fan_id = ?1 ORDER BY calibrated_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![fan_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let steps_json: String = row.get(6)?;
        let steps: Vec<CalibrationStep> = serde_json::from_str(&steps_json)?;
        Ok(Some(CalibrationResult {
            fan_id: row.get(0)?,
            calibrated_at: row.get(1)?,
            start_percent: row.get::<_, i64>(2)? as u8,
            stop_percent: row.get::<_, i64>(3)? as u8,
            min_rpm: row.get::<_, i64>(4)? as u32,
            max_rpm: row.get::<_, i64>(5)? as u32,
            steps,
        }))
    }

    pub fn prune_older_than(&self, cutoff_unix: i64) -> Result<usize> {
        let n1 = self.conn.execute(
            "DELETE FROM sensor_readings WHERE recorded_at < ?1",
            params![cutoff_unix],
        )?;
        let n2 = self.conn.execute(
            "DELETE FROM fan_readings WHERE recorded_at < ?1",
            params![cutoff_unix],
        )?;
        Ok(n1 + n2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_query_sensor() {
        let db = HistoryDb::in_memory().unwrap();
        db.insert_sensor("cpu_tctl", 45.0).unwrap();
        db.insert_sensor("cpu_tctl", 47.0).unwrap();
        let records = db.query_sensor_last_n("cpu_tctl", 10).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn insert_fan_record() {
        let db = HistoryDb::in_memory().unwrap();
        db.insert_fan("cpu_fan", 1200, 50).unwrap();
        // No panic = success
    }

    #[test]
    fn query_unknown_sensor_returns_empty() {
        let db = HistoryDb::in_memory().unwrap();
        let records = db.query_sensor_last_n("nonexistent", 10).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn query_fan_last_n_works() {
        let db = HistoryDb::in_memory().unwrap();
        db.insert_fan("cpu_fan", 1200, 50).unwrap();
        db.insert_fan("cpu_fan", 1300, 60).unwrap();
        let records = db.query_fan_last_n("cpu_fan", 10).unwrap();
        assert_eq!(records.len(), 2);
    }
}

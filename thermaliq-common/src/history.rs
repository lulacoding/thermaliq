use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

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

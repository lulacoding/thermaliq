//! Agent memory database — persistent SQLite store for thermal intelligence.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalSample {
    pub id: i64,
    pub sensor_id: String,
    pub temp_celsius: f32,
    pub workload: Option<String>,
    pub sampled_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterventionRecord {
    pub id: String,
    pub trigger_reason: String,
    pub proposed_action: String,
    pub proposed_at: i64,
    pub approved_at: Option<i64>,
    pub applied_at: Option<i64>,
    pub result: Option<String>,
    pub before_snapshot: Option<String>,
    pub after_snapshot: Option<String>,
    pub rolled_back_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSession {
    pub id: i64,
    pub workload: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub peak_cpu_temp: Option<f32>,
    pub avg_cpu_temp: Option<f32>,
    pub interventions_count: i64,
}

pub struct AgentDb {
    conn: Connection,
}

impl AgentDb {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
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
            CREATE TABLE IF NOT EXISTS thermal_samples (
                id INTEGER PRIMARY KEY,
                sensor_id TEXT NOT NULL,
                temp_celsius REAL NOT NULL,
                workload TEXT,
                sampled_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_thermal_samples ON thermal_samples(sensor_id, sampled_at);

            CREATE TABLE IF NOT EXISTS interventions (
                id TEXT PRIMARY KEY,
                trigger_reason TEXT NOT NULL,
                proposed_action TEXT NOT NULL,
                proposed_at INTEGER NOT NULL,
                approved_at INTEGER,
                applied_at INTEGER,
                result TEXT,
                before_snapshot TEXT,
                after_snapshot TEXT,
                rolled_back_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS workload_log (
                id INTEGER PRIMARY KEY,
                workload TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                peak_cpu_temp REAL,
                avg_cpu_temp REAL,
                interventions_count INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS profile_stats (
                id INTEGER PRIMARY KEY,
                profile_name TEXT NOT NULL,
                recorded_at INTEGER NOT NULL DEFAULT (unixepoch()),
                avg_cpu_temp REAL,
                avg_fan_duty REAL,
                thermal_headroom REAL
            );
        ")?;
        Ok(())
    }

    pub fn insert_sample(&self, sensor_id: &str, temp_celsius: f32, workload: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO thermal_samples (sensor_id, temp_celsius, workload) VALUES (?1, ?2, ?3)",
            params![sensor_id, temp_celsius, workload],
        )?;
        Ok(())
    }

    pub fn recent_cpu_temps(&self, sensor_id: &str, window_secs: i64) -> Result<Vec<f32>> {
        let cutoff = unix_now() - window_secs;
        let mut stmt = self.conn.prepare(
            "SELECT temp_celsius FROM thermal_samples
             WHERE sensor_id = ?1 AND sampled_at >= ?2 ORDER BY sampled_at ASC",
        )?;
        let temps = stmt
            .query_map(params![sensor_id, cutoff], |row| row.get(0))?
            .collect::<std::result::Result<Vec<f32>, _>>()?;
        Ok(temps)
    }

    pub fn record_profile_stat(&self, profile_name: &str, avg_cpu_temp: f32, avg_fan_duty: f32, thermal_headroom: f32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO profile_stats (profile_name, avg_cpu_temp, avg_fan_duty, thermal_headroom)
             VALUES (?1, ?2, ?3, ?4)",
            params![profile_name, avg_cpu_temp, avg_fan_duty, thermal_headroom],
        )?;
        Ok(())
    }

    pub fn recent_samples(&self, sensor_id: &str, window_secs: i64) -> Result<Vec<ThermalSample>> {
        let cutoff = unix_now() - window_secs;
        let mut stmt = self.conn.prepare(
            "SELECT id, sensor_id, temp_celsius, workload, sampled_at FROM thermal_samples
             WHERE sensor_id = ?1 AND sampled_at >= ?2 ORDER BY sampled_at ASC"
        )?;
        let rows = stmt.query_map(params![sensor_id, cutoff], |row| {
            Ok(ThermalSample {
                id: row.get(0)?,
                sensor_id: row.get(1)?,
                temp_celsius: row.get(2)?,
                workload: row.get(3)?,
                sampled_at: row.get(4)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn log_intervention(&self, rec: &InterventionRecord) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO interventions
             (id, trigger_reason, proposed_action, proposed_at, approved_at, applied_at, result, before_snapshot, after_snapshot, rolled_back_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                rec.id, rec.trigger_reason, rec.proposed_action, rec.proposed_at,
                rec.approved_at, rec.applied_at, rec.result,
                rec.before_snapshot, rec.after_snapshot, rec.rolled_back_at
            ],
        )?;
        Ok(())
    }

    pub fn update_intervention_result(&self, id: &str, result: &str, applied_at: i64, after_snapshot: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE interventions SET result=?1, applied_at=?2, after_snapshot=?3 WHERE id=?4",
            params![result, applied_at, after_snapshot, id],
        )?;
        Ok(())
    }

    pub fn recent_interventions_count(&self, window_secs: i64) -> Result<i64> {
        let cutoff = unix_now() - window_secs;
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM interventions WHERE proposed_at >= ?1 AND result = 'applied'",
            params![cutoff],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn prune_samples_older_than(&self, cutoff: i64) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM thermal_samples WHERE sampled_at < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }

    pub fn list_interventions(&self, limit: usize) -> Result<Vec<InterventionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, trigger_reason, proposed_action, proposed_at, approved_at, applied_at,
             result, before_snapshot, after_snapshot, rolled_back_at
             FROM interventions ORDER BY proposed_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(InterventionRecord {
                id: row.get(0)?,
                trigger_reason: row.get(1)?,
                proposed_action: row.get(2)?,
                proposed_at: row.get(3)?,
                approved_at: row.get(4)?,
                applied_at: row.get(5)?,
                result: row.get(6)?,
                before_snapshot: row.get(7)?,
                after_snapshot: row.get(8)?,
                rolled_back_at: row.get(9)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_query_samples() {
        let db = AgentDb::in_memory().unwrap();
        db.insert_sample("cpu", 65.0, Some("compiling")).unwrap();
        db.insert_sample("cpu", 67.0, Some("compiling")).unwrap();
        let samples = db.recent_samples("cpu", 3600).unwrap();
        assert_eq!(samples.len(), 2);
    }

    #[test]
    fn log_and_retrieve_intervention() {
        let db = AgentDb::in_memory().unwrap();
        let rec = InterventionRecord {
            id: "test1".into(),
            trigger_reason: "CPU > 85°C".into(),
            proposed_action: "switch_profile:performance".into(),
            proposed_at: 1000,
            approved_at: None,
            applied_at: None,
            result: None,
            before_snapshot: None,
            after_snapshot: None,
            rolled_back_at: None,
        };
        db.log_intervention(&rec).unwrap();
        let list = db.list_interventions(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "test1");
    }
}

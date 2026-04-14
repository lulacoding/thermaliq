//! Daemon configuration and fan profile types.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

// ── DaemonConfig ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub api_port: u16,
    pub polling_interval_ms: u64,
    pub db_path: Option<PathBuf>,
    pub log_level: String,
    pub default_profile: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            api_port: 7272,
            polling_interval_ms: 1000,
            db_path: None,
            log_level: "info".to_string(),
            default_profile: "balanced".to_string(),
        }
    }
}

pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thermaliq")
        .join("config.toml")
}

pub fn default_db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thermaliq")
        .join("history.db")
}

pub fn load_daemon_config(path: &std::path::Path) -> anyhow::Result<DaemonConfig> {
    if !path.exists() {
        return Ok(DaemonConfig::default());
    }
    let text = std::fs::read_to_string(path)?;
    let config: DaemonConfig = toml::from_str(&text)?;
    Ok(config)
}

// ── Profile ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub sensors: Vec<CustomSensor>,
    #[serde(default)]
    pub curves: Vec<ProfileCurve>,
    #[serde(default)]
    pub controls: Vec<FanControl>,
    #[serde(default)]
    pub alerts: Vec<ProfileAlert>,
}

fn default_version() -> u32 { 1 }

pub fn profiles_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thermaliq")
        .join("profiles")
}

pub fn load_profile(path: &std::path::Path) -> anyhow::Result<Profile> {
    let text = std::fs::read_to_string(path)?;
    let profile: Profile = toml::from_str(&text)?;
    Ok(profile)
}

pub fn list_profiles() -> anyhow::Result<Vec<String>> {
    let dir = profiles_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

// ── Custom sensors ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CustomSensor {
    TimeAverage { id: String, source: String, time_s: u64 },
    Mix { id: String, function: MixFunction, sources: Vec<String> },
    File { id: String, path: PathBuf },
    Offset { id: String, source: String, offset: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixFunction { Max, Min, Average, Sum }

// ── Fan curves ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileCurve {
    Linear {
        id: String,
        source: String,
        min_temp: f32,
        max_temp: f32,
        min_pwm: u8,
        max_pwm: u8,
    },
    Flat {
        id: String,
        pwm: u8,
    },
    Graph {
        id: String,
        source: String,
        points: Vec<GraphPoint>,
    },
    Mix {
        id: String,
        function: MixFunction,
        curves: Vec<String>,
    },
    Trigger {
        id: String,
        source: String,
        idle_temp: f32,
        idle_pwm: u8,
        load_temp: f32,
        load_pwm: u8,
        #[serde(default = "default_response_ms")]
        response_ms: u64,
    },
    Sync {
        id: String,
        target: String,
        #[serde(default)]
        offset: i8,
        #[serde(default)]
        proportional: bool,
    },
    Auto {
        id: String,
        source: String,
        idle_temp: f32,
        load_temp: f32,
        min_pwm: u8,
        max_pwm: u8,
        #[serde(default = "default_step")]
        step: u8,
        #[serde(default = "default_deadband")]
        deadband: f32,
    },
    Ai {
        id: String,
        #[serde(default = "default_goal")]
        goal: String,
    },
}

fn default_response_ms() -> u64 { 2000 }
fn default_step() -> u8 { 2 }
fn default_deadband() -> f32 { 3.0 }
fn default_goal() -> String { "balanced".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphPoint {
    pub temp: f32,
    pub pwm: u8,
}

// ── Fan controls ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FanMode { #[default] Pwm, Rpm }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanControl {
    pub fan: String,
    pub curve: String,
    #[serde(default)]
    pub mode: FanMode,
    #[serde(default = "default_step_up")]
    pub step_up_pct: u8,
    #[serde(default = "default_step_down")]
    pub step_down_pct: u8,
    #[serde(default)]
    pub offset_pct: i8,
    #[serde(default)]
    pub min_pwm: u8,
}

fn default_step_up() -> u8 { 10 }
fn default_step_down() -> u8 { 3 }

// ── Alerts ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity { Warn, Crit }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAlertAction {
    Log,
    Notify,
    ApplyProfile { profile: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAlert {
    pub sensor: String,
    pub threshold: f32,
    pub severity: AlertSeverity,
    pub action: ProfileAlertAction,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_config_defaults() {
        let cfg = DaemonConfig::default();
        assert_eq!(cfg.api_port, 7272);
        assert_eq!(cfg.polling_interval_ms, 1000);
        assert_eq!(cfg.default_profile, "balanced");
    }

    #[test]
    fn test_profile_round_trip() {
        let toml_str = r#"
name = "test"
description = "test profile"

[[curves]]
type = "linear"
id = "cpu_curve"
source = "cpu_package"
min_temp = 40.0
max_temp = 80.0
min_pwm = 20
max_pwm = 100

[[curves]]
type = "flat"
id = "chassis_flat"
pwm = 40

[[controls]]
fan = "cpu_fan_0"
curve = "cpu_curve"

[[alerts]]
sensor = "cpu_package"
threshold = 90.0
severity = "warn"
action = "notify"
"#;
        let profile: Profile = toml::from_str(toml_str).expect("parse failed");
        assert_eq!(profile.name, "test");
        assert_eq!(profile.curves.len(), 2);
        assert_eq!(profile.controls.len(), 1);
        assert_eq!(profile.alerts.len(), 1);

        // Round-trip
        let serialized = toml::to_string(&profile).expect("serialize failed");
        let profile2: Profile = toml::from_str(&serialized).expect("re-parse failed");
        assert_eq!(profile2.name, profile.name);
    }

    #[test]
    fn test_load_daemon_config_missing_file() {
        let result = load_daemon_config(std::path::Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().api_port, 7272);
    }
}

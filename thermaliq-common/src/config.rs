use crate::{invalid, Error};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// `config.toml`. No secrets: API keys live in the OS keychain (SPEC §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub api_port: u16,
    pub polling_interval_ms: u64,
    pub log_level: LogLevel,
    pub default_profile: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            api_port: 7272,
            polling_interval_ms: 1000,
            log_level: LogLevel::Info,
            default_profile: "balanced".into(),
        }
    }
}

impl DaemonConfig {
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        let c: Self = toml::from_str(s)?;
        if c.api_port == 0 {
            return invalid("api_port must not be 0".into());
        }
        // S3/S4 trip after 3 ticks, so a slow tick delays the failsafe
        if !(250..=2000).contains(&c.polling_interval_ms) {
            return invalid(format!(
                "polling_interval_ms {} is outside 250..2000",
                c.polling_interval_ms
            ));
        }
        if c.default_profile.is_empty() {
            return invalid("default_profile must not be empty".into());
        }
        Ok(c)
    }
}

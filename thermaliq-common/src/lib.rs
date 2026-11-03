//! Shared types, config and profile schema, validation. See SPEC.md §5.
//!
//! Parsing is strict: unknown fields and unknown curve types are errors, so
//! out-of-scope features (SPEC §2) cannot be configured by accident.

mod config;
mod profile;

pub use config::{DaemonConfig, LogLevel};
pub use profile::{
    Alert, AlertAction, AppRule, Control, Curve, CustomSensor, GraphPoint, MixFunction, Profile,
    Severity,
};

use serde::{Deserialize, Serialize};
use std::fmt;

/// Hard limits for any temperature value (SPEC S3).
pub const MIN_VALID_TEMP_C: f32 = -20.0;
pub const MAX_VALID_TEMP_C: f32 = 150.0;

#[derive(Debug)]
pub enum Error {
    Parse(toml::de::Error),
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => write!(f, "{}", e.message()),
            Error::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self {
        Error::Parse(e)
    }
}

pub(crate) fn invalid<T>(msg: String) -> Result<T, Error> {
    Err(Error::Invalid(msg))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorReading {
    pub id: String,
    pub label: String,
    pub temp_celsius: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanReading {
    pub id: String,
    pub label: String,
    pub rpm: u32,
    pub duty_percent: u8,
}

/// One reading of all hardware, taken once per tick (SPEC §5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HardwareSnapshot {
    pub sensors: Vec<SensorReading>,
    pub fans: Vec<FanReading>,
}

/// Profiles shipped with the app, as `(name, toml)`.
pub fn bundled_profiles() -> &'static [(&'static str, &'static str)] {
    &[
        ("silent", include_str!("../profiles/silent.toml")),
        ("balanced", include_str!("../profiles/balanced.toml")),
        ("efficiency", include_str!("../profiles/efficiency.toml")),
        ("performance", include_str!("../profiles/performance.toml")),
        ("gaming", include_str!("../profiles/gaming.toml")),
    ]
}

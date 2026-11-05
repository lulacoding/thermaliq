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

/// SPEC §7.2: the agent may lower a fan at most this many points below its curve output.
/// The agent checks it and the engine enforces it. The engine's baseline is the profile
/// curve today (M15 carry-over in PORTING).
pub const AGENT_MAX_LOWER_PCT: u8 = 15;

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

/// Which component a sensor measures, set by the backend (SPEC S2 needs CPU vs GPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    Cpu,
    Gpu,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorReading {
    pub id: String,
    pub label: String,
    pub kind: SensorKind,
    pub temp_celsius: f32,
}

/// A pump must never be stopped (SPEC §7); a fan may be (0-RPM).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanKind {
    #[default]
    Fan,
    Pump,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanReading {
    pub id: String,
    pub label: String,
    pub rpm: u32,
    pub duty_percent: u8,
    #[serde(default)]
    pub kind: FanKind,
}

/// What a `MetricReading` measures (SPEC §7 inputs). Units: load %, power W, clock MHz;
/// `GpuThrottle` is NVML's throttle-reason bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    CpuLoad,
    GpuLoad,
    CpuPower,
    GpuPower,
    CpuClock,
    GpuClock,
    GpuThrottle,
}

impl MetricKind {
    /// Finite, not negative, and a load is at most 100. A backend skips a reading that
    /// fails this rather than report 0: a missing metric must not look like an idle PC.
    pub fn is_valid(self, value: f64) -> bool {
        let max = if matches!(self, Self::CpuLoad | Self::GpuLoad) {
            100.0
        } else {
            f64::MAX
        };
        (0.0..=max).contains(&value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricReading {
    pub id: String,
    pub kind: MetricKind,
    pub value: f64,
}

/// One reading of all hardware, taken once per tick (SPEC §5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HardwareSnapshot {
    pub sensors: Vec<SensorReading>,
    pub fans: Vec<FanReading>,
    #[serde(default)]
    pub metrics: Vec<MetricReading>,
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn metric_validity_rejects_nan_inf_negative_and_load_over_100() {
        use MetricKind::*;
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1] {
            assert!(!CpuPower.is_valid(bad), "{bad}");
        }
        assert!(!GpuLoad.is_valid(100.1));
        assert!(GpuLoad.is_valid(100.0) && CpuLoad.is_valid(0.0));
        assert!(GpuClock.is_valid(2850.0) && GpuThrottle.is_valid(256.0));
    }

    #[test]
    fn a_fan_without_kind_is_a_fan_and_a_pump_round_trips() {
        let old = "id = \"f\"\nlabel = \"f\"\nrpm = 1\nduty_percent = 2";
        assert_eq!(
            toml::from_str::<FanReading>(old).unwrap().kind,
            FanKind::Fan
        );
        let pump = format!("{old}\nkind = \"pump\"");
        assert_eq!(
            toml::from_str::<FanReading>(&pump).unwrap().kind,
            FanKind::Pump
        );
    }
}

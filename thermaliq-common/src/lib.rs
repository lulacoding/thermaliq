pub mod agent;
pub mod alert;
pub mod config;
pub mod curve;
pub mod history;

use serde::{Deserialize, Serialize};

/// Celsius temperature reading from a single sensor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub id: String,
    pub label: String,
    pub temp_celsius: f32,
}

/// Fan speed and control state for a single fan header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanReading {
    pub id: String,
    pub label: String,
    pub rpm: u32,
    /// Current duty cycle 0–100.
    pub duty_percent: u8,
}

/// Hardware snapshot returned by the HAL on each poll cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareSnapshot {
    pub sensors: Vec<SensorReading>,
    pub fans: Vec<FanReading>,
}

/// Command to set a fan's duty cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetFanDuty {
    pub fan_id: String,
    /// Target duty cycle 0–100.
    pub duty_percent: u8,
}

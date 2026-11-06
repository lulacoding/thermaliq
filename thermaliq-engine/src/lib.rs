//! The control loop: curves, sensors, priority, safety, ramping.
//! The only crate allowed to write to fans (SPEC.md §3, S1).
//!
//! Modules M3–M5 (PORTING.md): curves, sensors, priority/ramping, safety.

mod control;
mod curves;
mod engine;
mod sensors;

pub use control::{FanControl, Source};
pub use curves::CurveEngine;
pub use engine::{Engine, WRITE_FAILS_DEGRADED};
pub use sensors::SensorEngine;

use std::collections::HashMap;

/// SPEC S3: NaN, < -20 °C and > 150 °C are invalid.
pub(crate) fn valid_temp(t: f32) -> bool {
    (thermaliq_common::MIN_VALID_TEMP_C..=thermaliq_common::MAX_VALID_TEMP_C).contains(&t)
}

/// Valid temperatures by sensor id, for one tick.
pub type Temps = HashMap<String, f32>;

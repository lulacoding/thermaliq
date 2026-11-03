use crate::{valid_temp, Temps};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use thermaliq_common::{CustomSensor, MixFunction, Profile};

/// Adds a profile's custom sensors to the real readings each tick.
/// A custom sensor with any missing or invalid input is left out, so S3 sees it as lost.
pub struct SensorEngine {
    sensors: Vec<CustomSensor>,
    /// Samples per time-average sensor id.
    history: HashMap<String, VecDeque<(Duration, f32)>>,
}

impl SensorEngine {
    pub fn new(profile: &Profile) -> Self {
        Self {
            sensors: profile.sensors.clone(),
            history: HashMap::new(),
        }
    }

    /// Valid real readings plus custom sensors. Sensors are evaluated in
    /// definition order, so a later one can use an earlier one.
    pub fn evaluate(&mut self, real: &Temps, now: Duration) -> Temps {
        let mut out: Temps = real
            .iter()
            .filter(|(_, t)| valid_temp(**t))
            .map(|(k, t)| (k.clone(), *t))
            .collect();
        for s in &self.sensors {
            let value = match s {
                CustomSensor::Offset { source, offset, .. } => out.get(source).map(|t| t + offset),
                CustomSensor::Mix {
                    function, sources, ..
                } => sources
                    .iter()
                    .map(|s| out.get(s).copied())
                    .collect::<Option<Vec<f32>>>()
                    .and_then(|v| mix(*function, &v)),
                CustomSensor::TimeAverage {
                    id, source, time_s, ..
                } => out.get(source).map(|t| {
                    let h = self.history.entry(id.clone()).or_default();
                    h.push_back((now, *t));
                    let window = Duration::from_secs(*time_s);
                    while h
                        .front()
                        .is_some_and(|(at, _)| now.saturating_sub(*at) > window)
                    {
                        h.pop_front();
                    }
                    h.iter().map(|(_, t)| t).sum::<f32>() / h.len() as f32
                }),
            };
            // A real reading wins over a custom sensor with the same id.
            if real.contains_key(id_of(s)) {
                continue;
            }
            if let Some(v) = value.filter(|v| valid_temp(*v)) {
                out.insert(id_of(s).to_string(), v);
            }
        }
        out
    }
}

fn id_of(s: &CustomSensor) -> &str {
    match s {
        CustomSensor::TimeAverage { id, .. }
        | CustomSensor::Mix { id, .. }
        | CustomSensor::Offset { id, .. } => id,
    }
}

fn mix(f: MixFunction, v: &[f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    Some(match f {
        MixFunction::Max => v.iter().copied().fold(f32::MIN, f32::max),
        MixFunction::Min => v.iter().copied().fold(f32::MAX, f32::min),
        MixFunction::Average => v.iter().sum::<f32>() / v.len() as f32,
        MixFunction::Sum => v.iter().sum(),
    })
}

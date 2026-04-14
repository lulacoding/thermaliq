use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AlertAction {
    Log,
    Notify,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemperatureAlert {
    pub id: String,
    pub sensor_id: String,
    pub threshold_celsius: f32,
    pub action: AlertAction,
    pub cooldown_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FiredAlert {
    pub alert_id: String,
    pub sensor_id: String,
    pub action: AlertAction,
    pub current_temp: f32,
    pub threshold: f32,
}

pub struct AlertEvaluator {
    alerts: Vec<TemperatureAlert>,
    last_fired: HashMap<String, u64>, // key: "alert_id", value: unix timestamp
}

impl AlertEvaluator {
    pub fn new(alerts: Vec<TemperatureAlert>) -> Self {
        Self {
            alerts,
            last_fired: HashMap::new(),
        }
    }

    pub fn add_alert(&mut self, alert: TemperatureAlert) {
        self.alerts.push(alert);
    }

    pub fn alerts(&self) -> &[TemperatureAlert] {
        &self.alerts
    }

    /// Evaluate all alerts for a sensor reading. Returns alerts that fired.
    /// `now` is current unix timestamp (seconds). Respects cooldown.
    pub fn evaluate(&mut self, sensor_id: &str, current_temp: f32, now: u64) -> Vec<FiredAlert> {
        let mut fired = Vec::new();
        for alert in &self.alerts {
            if alert.sensor_id != sensor_id {
                continue;
            }
            if current_temp < alert.threshold_celsius {
                continue;
            }
            let last = self.last_fired.get(&alert.id).copied().unwrap_or(0);
            if now.saturating_sub(last) < alert.cooldown_secs {
                continue; // Still in cooldown
            }
            fired.push(FiredAlert {
                alert_id: alert.id.clone(),
                sensor_id: sensor_id.to_string(),
                action: alert.action.clone(),
                current_temp,
                threshold: alert.threshold_celsius,
            });
            self.last_fired.insert(alert.id.clone(), now);
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_alert(id: &str, sensor: &str, threshold: f32, cooldown: u64) -> TemperatureAlert {
        TemperatureAlert {
            id: id.to_string(),
            sensor_id: sensor.to_string(),
            threshold_celsius: threshold,
            action: AlertAction::Log,
            cooldown_secs: cooldown,
        }
    }

    #[test]
    fn fires_when_above_threshold() {
        let mut ev = AlertEvaluator::new(vec![make_alert("a1", "cpu", 80.0, 60)]);
        let fired = ev.evaluate("cpu", 85.0, 1000);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].alert_id, "a1");
    }

    #[test]
    fn does_not_fire_below_threshold() {
        let mut ev = AlertEvaluator::new(vec![make_alert("a1", "cpu", 80.0, 60)]);
        let fired = ev.evaluate("cpu", 75.0, 1000);
        assert!(fired.is_empty());
    }

    #[test]
    fn does_not_refire_within_cooldown() {
        let mut ev = AlertEvaluator::new(vec![make_alert("a1", "cpu", 80.0, 60)]);
        ev.evaluate("cpu", 85.0, 1000);
        let fired = ev.evaluate("cpu", 85.0, 1030); // 30s later, still in 60s cooldown
        assert!(fired.is_empty());
    }

    #[test]
    fn refires_after_cooldown() {
        let mut ev = AlertEvaluator::new(vec![make_alert("a1", "cpu", 80.0, 60)]);
        ev.evaluate("cpu", 85.0, 1000);
        let fired = ev.evaluate("cpu", 85.0, 1061); // 61s later, cooldown expired
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn does_not_fire_for_different_sensor() {
        let mut ev = AlertEvaluator::new(vec![make_alert("a1", "cpu", 80.0, 60)]);
        let fired = ev.evaluate("gpu", 95.0, 1000);
        assert!(fired.is_empty());
    }
}

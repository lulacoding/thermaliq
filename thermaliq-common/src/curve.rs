use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FanMode {
    Pwm,
    Dc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temp_celsius: f32,
    pub duty_percent: u8,
}

/// Fan curve mapping temperature to duty cycle via linear interpolation.
/// Points must be sorted ascending by temp_celsius.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanCurve {
    pub points: Vec<CurvePoint>,
}

impl FanCurve {
    /// Evaluate the curve at `temp`. Clamps to min/max duty at curve edges.
    pub fn evaluate(&self, temp: f32) -> u8 {
        if self.points.is_empty() {
            return 0;
        }
        if temp <= self.points.first().unwrap().temp_celsius {
            return self.points.first().unwrap().duty_percent;
        }
        if temp >= self.points.last().unwrap().temp_celsius {
            return self.points.last().unwrap().duty_percent;
        }
        // Find surrounding points and interpolate
        for window in self.points.windows(2) {
            let lo = &window[0];
            let hi = &window[1];
            if temp >= lo.temp_celsius && temp <= hi.temp_celsius {
                let t = (temp - lo.temp_celsius) / (hi.temp_celsius - lo.temp_celsius);
                let duty = lo.duty_percent as f32 + t * (hi.duty_percent as f32 - lo.duty_percent as f32);
                return duty.round().clamp(0.0, 100.0) as u8;
            }
        }
        self.points.last().unwrap().duty_percent
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanChannelConfig {
    pub id: String,
    pub label: String,
    pub mode: FanMode,
    pub curve: FanCurve,
    /// Minimum duty to run the fan (don't start below this, stop above this)
    pub start_percent: u8,
    /// Duty below which the fan is stopped (0-RPM mode)
    pub stop_percent: u8,
    pub hysteresis_up_celsius: f32,
    pub hysteresis_down_celsius: f32,
    pub max_duty: u8,
    pub min_duty: u8,
}

/// Stateful fan controller that applies hysteresis and start%/stop% logic.
pub struct FanController {
    pub config: FanChannelConfig,
    last_duty: u8,
    last_temp: f32,
    fan_running: bool,
}

impl FanController {
    pub fn new(config: FanChannelConfig) -> Self {
        Self {
            last_duty: 0,
            last_temp: 0.0,
            fan_running: false,
            config,
        }
    }

    /// Compute target duty for current temperature, applying hysteresis and start%/stop%.
    pub fn compute_duty(&mut self, current_temp: f32) -> u8 {
        let raw = self.config.curve.evaluate(current_temp);
        let clamped = raw.clamp(self.config.min_duty, self.config.max_duty);

        // Apply hysteresis: only update if temp moved enough
        let temp_delta = current_temp - self.last_temp;
        let moved_up = temp_delta >= self.config.hysteresis_up_celsius;
        let moved_down = (-temp_delta) >= self.config.hysteresis_down_celsius;

        let should_update = self.last_duty == 0 || moved_up || moved_down;
        if !should_update {
            return self.last_duty;
        }

        // Start%/Stop% logic
        let new_duty = if self.fan_running {
            if clamped <= self.config.stop_percent {
                self.fan_running = false;
                0
            } else {
                clamped
            }
        } else {
            if clamped >= self.config.start_percent {
                self.fan_running = true;
                clamped
            } else {
                0
            }
        };

        self.last_duty = new_duty;
        self.last_temp = current_temp;
        new_duty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_curve() -> FanCurve {
        FanCurve {
            points: vec![
                CurvePoint { temp_celsius: 30.0, duty_percent: 20 },
                CurvePoint { temp_celsius: 60.0, duty_percent: 50 },
                CurvePoint { temp_celsius: 90.0, duty_percent: 100 },
            ],
        }
    }

    #[test]
    fn interpolation_midpoint() {
        let curve = simple_curve();
        assert_eq!(curve.evaluate(45.0), 35); // midpoint 30-60 = (20+50)/2 = 35
    }

    #[test]
    fn clamps_below_min_temp() {
        let curve = simple_curve();
        assert_eq!(curve.evaluate(10.0), 20);
    }

    #[test]
    fn clamps_above_max_temp() {
        let curve = simple_curve();
        assert_eq!(curve.evaluate(100.0), 100);
    }

    fn default_controller() -> FanController {
        let config = FanChannelConfig {
            id: "test".into(),
            label: "Test Fan".into(),
            mode: FanMode::Pwm,
            curve: simple_curve(),
            start_percent: 30,
            stop_percent: 20,
            hysteresis_up_celsius: 2.0,
            hysteresis_down_celsius: 3.0,
            max_duty: 100,
            min_duty: 0,
        };
        FanController::new(config)
    }

    #[test]
    fn fan_starts_above_start_percent() {
        let mut ctrl = default_controller();
        // At 60°C curve gives 50%, which is >= start_percent(30)
        let duty = ctrl.compute_duty(60.0);
        assert!(duty > 0, "fan should start");
    }

    #[test]
    fn hysteresis_prevents_chattering() {
        let mut ctrl = default_controller();
        let d1 = ctrl.compute_duty(60.0); // initial read, updates
        let d2 = ctrl.compute_duty(61.0); // only 1°C up, below hysteresis_up(2.0)
        assert_eq!(d1, d2, "should not change within hysteresis band");
    }

    #[test]
    fn fan_stops_below_stop_percent() {
        let mut ctrl = default_controller();
        ctrl.compute_duty(60.0); // start fan
        // Curve at 30°C gives 20%, which equals stop_percent(20), so stops
        let duty = ctrl.compute_duty(30.0);
        assert_eq!(duty, 0, "fan should stop below stop_percent");
    }
}

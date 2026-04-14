//! Background sensor polling loop.

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{interval, Duration};

use crate::api::auth::AppState;
use thermaliq_common::config::ProfileCurve;

pub fn start_poll_loop(state: AppState, interval_ms: u64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_millis(interval_ms));

        loop {
            ticker.tick().await;

            let snap = match state.hal.snapshot().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("HAL snapshot error: {e}");
                    continue;
                }
            };

            let now_ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            // Persist readings — history.rs uses &str args, not structs
            {
                if let Ok(db) = state.history.lock() {
                    for sensor in &snap.sensors {
                        if let Err(e) = db.insert_sensor(&sensor.id, sensor.temp_celsius) {
                            tracing::warn!("insert_sensor failed: {e}");
                        }
                    }
                    for fan in &snap.fans {
                        if let Err(e) = db.insert_fan(&fan.id, fan.rpm, fan.duty_percent) {
                            tracing::warn!("insert_fan failed: {e}");
                        }
                    }
                }
            }

            // Suppress unused warning — recorded_at not used by insert methods
            let _ = now_ts;

            // Apply fan curves from active profile
            {
                let profile_guard = state.active_profile.read().await;
                if let Some(profile) = profile_guard.as_ref() {
                    for control in &profile.controls {
                        let Some(curve) = profile.curves.iter().find(|c| curve_id(c) == control.curve) else { continue };
                        let Some(src) = curve_source(curve) else { continue };
                        let Some(sensor) = snap.sensors.iter().find(|s| s.id == src) else { continue };
                        let duty = compute_duty(curve, sensor.temp_celsius);
                        let duty = duty.max(control.min_pwm);
                        let _ = state.hal.set_fan_duty(thermaliq_common::SetFanDuty {
                            fan_id: control.fan.clone(),
                            duty_percent: duty,
                        }).await;
                    }
                }
            }
        }
    })
}

fn curve_id(c: &ProfileCurve) -> &str {
    match c {
        ProfileCurve::Linear { id, .. } => id,
        ProfileCurve::Flat { id, .. } => id,
        ProfileCurve::Graph { id, .. } => id,
        ProfileCurve::Mix { id, .. } => id,
        ProfileCurve::Trigger { id, .. } => id,
        ProfileCurve::Sync { id, .. } => id,
        ProfileCurve::Auto { id, .. } => id,
        ProfileCurve::Ai { id, .. } => id,
    }
}

fn curve_source(c: &ProfileCurve) -> Option<&str> {
    match c {
        ProfileCurve::Linear { source, .. } => Some(source),
        ProfileCurve::Graph { source, .. } => Some(source),
        ProfileCurve::Auto { source, .. } => Some(source),
        ProfileCurve::Trigger { source, .. } => Some(source),
        _ => None,
    }
}

fn compute_duty(curve: &ProfileCurve, temp: f32) -> u8 {
    match curve {
        ProfileCurve::Linear { min_temp, max_temp, min_pwm, max_pwm, .. } => {
            if temp <= *min_temp { return *min_pwm; }
            if temp >= *max_temp { return *max_pwm; }
            let t = (temp - min_temp) / (max_temp - min_temp);
            (*min_pwm as f32 + t * (*max_pwm as f32 - *min_pwm as f32)).round() as u8
        }
        ProfileCurve::Flat { pwm, .. } => *pwm,
        ProfileCurve::Trigger { idle_temp, idle_pwm, load_temp, load_pwm, .. } => {
            if temp <= *idle_temp { *idle_pwm }
            else if temp >= *load_temp { *load_pwm }
            else { *idle_pwm }
        }
        ProfileCurve::Auto { idle_temp, load_temp, min_pwm, max_pwm, .. } => {
            if temp <= *idle_temp { return *min_pwm; }
            if temp >= *load_temp { return *max_pwm; }
            let t = (temp - idle_temp) / (load_temp - idle_temp);
            (*min_pwm as f32 + t * (*max_pwm as f32 - *min_pwm as f32)).round() as u8
        }
        _ => 50,
    }
}

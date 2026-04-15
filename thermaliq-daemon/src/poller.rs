//! Background sensor polling loop.

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{interval, Duration};

use crate::api::auth::AppState;
use thermaliq_common::config::{MixFunction, Profile, ProfileCurve};

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

            // Broadcast snapshot to WebSocket /ws/sensors subscribers
            {
                let snap_json = serde_json::to_string(&serde_json::json!({
                    "sensors": snap.sensors,
                    "fans": snap.fans,
                    "ts": now_ts,
                })).unwrap_or_default();
                let _ = state.sensor_tx.send(snap_json);
            }

            // Evaluate alerts from active profile
            {
                let profile_guard = state.active_profile.read().await;
                if let Some(profile) = profile_guard.as_ref() {
                    for sensor in &snap.sensors {
                        for alert in &profile.alerts {
                            if alert.sensor == sensor.id && sensor.temp_celsius >= alert.threshold {
                                tracing::warn!(
                                    sensor = %sensor.id,
                                    temp = sensor.temp_celsius,
                                    threshold = alert.threshold,
                                    severity = ?alert.severity,
                                    "alert fired"
                                );
                            }
                        }
                    }
                }
            }

            // Apply fan curves from active profile
            {
                let profile_guard = state.active_profile.read().await;
                if let Some(profile) = profile_guard.as_ref() {
                    for control in &profile.controls {
                        let Some(curve) = profile.curves.iter().find(|c| curve_id(c) == control.curve) else { continue };
                        // Source-independent curves (Flat, Mix, Sync, Ai) use temp=0.0;
                        // source-dependent curves look up the sensor reading.
                        let temp = match curve_source(curve) {
                            Some(src) => {
                                let Some(sensor) = snap.sensors.iter().find(|s| s.id == src) else { continue };
                                sensor.temp_celsius
                            }
                            None => 0.0,
                        };
                        let duty = compute_duty_for_curve(curve, temp, profile);
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
        ProfileCurve::Predictive { id, .. } => id,
        ProfileCurve::Learning { id, .. } => id,
    }
}

fn curve_source(c: &ProfileCurve) -> Option<&str> {
    match c {
        ProfileCurve::Linear { source, .. } => Some(source),
        ProfileCurve::Graph { source, .. } => Some(source),
        ProfileCurve::Auto { source, .. } => Some(source),
        ProfileCurve::Trigger { source, .. } => Some(source),
        ProfileCurve::Predictive { source, .. } => Some(source),
        ProfileCurve::Learning { source, .. } => Some(source),
        _ => None,
    }
}

fn compute_duty_for_curve(curve: &ProfileCurve, temp: f32, profile: &Profile) -> u8 {
    match curve {
        ProfileCurve::Linear { min_temp, max_temp, min_pwm, max_pwm, .. } => {
            if temp <= *min_temp { return *min_pwm; }
            if temp >= *max_temp { return *max_pwm; }
            let t = (temp - min_temp) / (max_temp - min_temp);
            (*min_pwm as f32 + t * (*max_pwm as f32 - *min_pwm as f32)).round() as u8
        }
        ProfileCurve::Flat { pwm, .. } => *pwm,
        ProfileCurve::Graph { points, .. } => {
            if points.is_empty() { return 50; }
            if temp <= points.first().unwrap().temp { return points.first().unwrap().pwm; }
            if temp >= points.last().unwrap().temp { return points.last().unwrap().pwm; }
            for window in points.windows(2) {
                let lo = &window[0];
                let hi = &window[1];
                if temp >= lo.temp && temp <= hi.temp {
                    let t = (temp - lo.temp) / (hi.temp - lo.temp);
                    let duty = lo.pwm as f32 + t * (hi.pwm as f32 - lo.pwm as f32);
                    return duty.round().clamp(0.0, 100.0) as u8;
                }
            }
            points.last().unwrap().pwm
        }
        ProfileCurve::Mix { function, curves, .. } => {
            let duties: Vec<u8> = curves.iter()
                .filter_map(|cid| profile.curves.iter().find(|c| curve_id(c) == cid))
                .map(|c| compute_duty_for_curve(c, temp, profile))
                .collect();
            if duties.is_empty() { return 50; }
            match function {
                MixFunction::Max => *duties.iter().max().unwrap(),
                MixFunction::Min => *duties.iter().min().unwrap(),
                MixFunction::Average => (duties.iter().map(|&d| d as u32).sum::<u32>() / duties.len() as u32) as u8,
                MixFunction::Sum => duties.iter().map(|&d| d as u32).sum::<u32>().min(100) as u8,
            }
        }
        ProfileCurve::Trigger { idle_temp, idle_pwm, load_temp, load_pwm, .. } => {
            if temp <= *idle_temp { *idle_pwm }
            else if temp >= *load_temp { *load_pwm }
            else { *idle_pwm }
        }
        ProfileCurve::Sync { target, offset, .. } => {
            if let Some(target_curve) = profile.curves.iter().find(|c| curve_id(c) == target) {
                let base = compute_duty_for_curve(target_curve, temp, profile) as i16;
                (base + *offset as i16).clamp(0, 100) as u8
            } else {
                50
            }
        }
        ProfileCurve::Auto { idle_temp, load_temp, min_pwm, max_pwm, .. } => {
            if temp <= *idle_temp { return *min_pwm; }
            if temp >= *load_temp { return *max_pwm; }
            let t = (temp - idle_temp) / (load_temp - idle_temp);
            (*min_pwm as f32 + t * (*max_pwm as f32 - *min_pwm as f32)).round() as u8
        }
        ProfileCurve::Ai { .. } => 60,
        ProfileCurve::Predictive { min_pwm, max_pwm, .. } => {
            let t = (temp - 40.0) / (80.0 - 40.0);
            (*min_pwm as f32 + t.clamp(0.0, 1.0) * (*max_pwm as f32 - *min_pwm as f32)).round() as u8
        }
        ProfileCurve::Learning { base_min_pwm, base_max_pwm, .. } => {
            let t = (temp - 40.0) / (80.0 - 40.0);
            (*base_min_pwm as f32 + t.clamp(0.0, 1.0) * (*base_max_pwm as f32 - *base_min_pwm as f32)).round() as u8
        }
    }
}

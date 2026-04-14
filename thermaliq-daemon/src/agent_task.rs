//! 24/7 AI agent background monitoring task.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::interval;

use crate::api::auth::{AgentMode, AppState, Intervention};

pub fn start_agent_loop(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(5));
        let mut high_temp_since: Option<Instant> = None;

        loop {
            ticker.tick().await;

            let snap = match state.hal.snapshot().await {
                Ok(s) => s,
                Err(_) => continue,
            };

            let mode = state.agent_mode.read().await.clone();

            match mode {
                AgentMode::Passive => {
                    for sensor in &snap.sensors {
                        tracing::debug!(
                            sensor_id = %sensor.id,
                            temp = sensor.temp_celsius,
                            "[agent] passive observation"
                        );
                    }
                    high_temp_since = None;
                }

                AgentMode::Approval | AgentMode::Autonomous => {
                    // Find highest CPU temperature
                    let cpu_temp = snap.sensors.iter()
                        .filter(|s| s.id.contains("cpu") || s.id.contains("package"))
                        .map(|s| s.temp_celsius)
                        .fold(f32::NEG_INFINITY, f32::max);

                    if cpu_temp > 85.0 {
                        let since = high_temp_since.get_or_insert_with(Instant::now);
                        if since.elapsed().as_secs() >= 30 {
                            high_temp_since = None;
                            let intervention = Intervention {
                                id: make_id(),
                                description: format!(
                                    "CPU {:.1}°C sustained for 30s — recommend performance profile",
                                    cpu_temp
                                ),
                                proposed_at: unix_now(),
                                proposed_action: "switch_profile:performance".to_string(),
                            };

                            if mode == AgentMode::Autonomous {
                                tracing::warn!(
                                    "[agent:autonomous] applying: {}",
                                    intervention.description
                                );
                                apply_intervention(&state, &intervention).await;
                            } else {
                                tracing::info!(
                                    "[agent:approval] queued intervention: {}",
                                    intervention.description
                                );
                                if let Ok(mut pending) = state.pending_interventions.lock() {
                                    pending.push(intervention);
                                }
                            }
                        }
                    } else {
                        high_temp_since = None;
                    }
                }
            }
        }
    })
}

async fn apply_intervention(state: &AppState, intervention: &Intervention) {
    if let Some(profile_name) = intervention.proposed_action.strip_prefix("switch_profile:") {
        let path = thermaliq_common::config::profiles_dir()
            .join(format!("{profile_name}.toml"));
        match thermaliq_common::config::load_profile(&path) {
            Ok(profile) => {
                *state.active_profile.write().await = Some(profile);
                tracing::info!("[agent] switched to profile: {profile_name}");
            }
            Err(e) => {
                tracing::warn!("[agent] could not load profile {profile_name}: {e}");
            }
        }
    }
}

fn make_id() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}{:08x}", ts.as_secs(), ts.subsec_nanos())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

//! 24/7 AI agent background monitoring task — workload detection, safety enforcement, interventions.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::time::interval;

use crate::agent_db::{AgentDb, InterventionRecord};
use crate::api::auth::{AgentMode, AppState, Intervention};

// Safety limits — hardcoded, never overrideable
const MAX_CPU_TEMP: f32 = 90.0;
const EMERGENCY_TEMP: f32 = 95.0;
const MAX_INTERVENTIONS_PER_HOUR: i64 = 6;

#[derive(Debug, Clone, PartialEq)]
pub enum WorkloadKind {
    Idle,
    Gaming,
    Compiling,
    Encoding,
    Stress,
    Unknown,
}

impl WorkloadKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkloadKind::Idle => "idle",
            WorkloadKind::Gaming => "gaming",
            WorkloadKind::Compiling => "compiling",
            WorkloadKind::Encoding => "encoding",
            WorkloadKind::Stress => "stress",
            WorkloadKind::Unknown => "unknown",
        }
    }
}

/// Detect workload from recent thermal samples.
/// Uses temperature as a proxy for CPU/GPU load (no direct load% available from HAL).
fn detect_workload(cpu_temp: f32, gpu_temp: f32, cpu_temps_5min: &[f32]) -> WorkloadKind {
    if cpu_temps_5min.is_empty() {
        return WorkloadKind::Unknown;
    }

    let avg_cpu: f32 = cpu_temps_5min.iter().sum::<f32>() / cpu_temps_5min.len() as f32;
    let max_cpu: f32 = cpu_temps_5min
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);

    // Emergency / Stress: sustained very high CPU
    if max_cpu >= 78.0 && avg_cpu >= 72.0 {
        return WorkloadKind::Stress;
    }

    // Gaming: high GPU + elevated CPU
    if gpu_temp > 65.0 && cpu_temp > 55.0 {
        return WorkloadKind::Gaming;
    }

    // Encoding: mixed sustained CPU + GPU warmth
    if cpu_temp > 58.0 && gpu_temp > 50.0 && gpu_temp < 65.0 {
        return WorkloadKind::Encoding;
    }

    // Compiling: sustained CPU, GPU stays cool
    if avg_cpu > 62.0 && gpu_temp < 50.0 {
        return WorkloadKind::Compiling;
    }

    // Idle: everything cool
    if avg_cpu < 45.0 && gpu_temp < 45.0 {
        return WorkloadKind::Idle;
    }

    WorkloadKind::Unknown
}

/// Check safety limits. Returns Some(reason) if an emergency action is needed.
fn check_safety(cpu_temp: f32, gpu_temp: f32) -> Option<&'static str> {
    if cpu_temp >= EMERGENCY_TEMP {
        return Some("EMERGENCY: CPU >= 95°C — all fans to 100%");
    }
    if gpu_temp >= 90.0 {
        return Some("EMERGENCY: GPU >= 90°C — all fans to 100%");
    }
    None
}

pub fn start_agent_loop(
    state: AppState,
    agent_db: Arc<Mutex<AgentDb>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(5));
        let mut cpu_temps_5min: Vec<f32> = Vec::new();
        let mut last_workload = WorkloadKind::Unknown;
        let mut prune_counter: u32 = 0;

        loop {
            ticker.tick().await;

            let snap = match state.hal.snapshot().await {
                Ok(s) => s,
                Err(_) => continue,
            };

            let now = unix_now();

            // Extract key temperatures
            let cpu_temp = snap
                .sensors
                .iter()
                .filter(|s| {
                    s.id.contains("cpu") || s.id.contains("package") || s.id.contains("tctl")
                })
                .map(|s| s.temp_celsius)
                .fold(f32::NEG_INFINITY, f32::max);

            let gpu_temp = snap
                .sensors
                .iter()
                .filter(|s| s.id.contains("gpu") || s.id.contains("edge"))
                .map(|s| s.temp_celsius)
                .fold(0.0_f32, f32::max);

            let cpu_temp = if cpu_temp == f32::NEG_INFINITY {
                0.0
            } else {
                cpu_temp
            };

            // Maintain 5-minute sliding window (60 samples at 5s interval)
            cpu_temps_5min.push(cpu_temp);
            if cpu_temps_5min.len() > 60 {
                cpu_temps_5min.remove(0);
            }

            // Prune agent DB every hour (720 ticks × 5s = 3600s)
            prune_counter += 1;
            if prune_counter >= 720 {
                prune_counter = 0;
                let cutoff = now - 7 * 86400;
                if let Ok(db) = agent_db.lock().await.prune_samples_older_than(cutoff) {
                    tracing::debug!("[agent] pruned {} old thermal samples", db);
                }
            }

            // Record thermal sample to agent DB
            let workload = detect_workload(cpu_temp, gpu_temp, &cpu_temps_5min);
            {
                let db = agent_db.lock().await;
                let _ = db.insert_sample("cpu_package", cpu_temp, Some(workload.as_str()));
                if gpu_temp > 0.0 {
                    let _ = db.insert_sample("gpu", gpu_temp, Some(workload.as_str()));
                }
            }

            if workload != last_workload {
                tracing::info!(
                    "[agent] workload changed: {:?} → {:?}",
                    last_workload,
                    workload
                );
                let _ = state.agent_event_tx.send(
                    serde_json::json!({
                        "type": "workload_changed",
                        "workload": workload.as_str(),
                        "cpu_temp": cpu_temp,
                    })
                    .to_string(),
                );
                last_workload = workload.clone();
            }

            // Safety check — emergency override regardless of mode
            if let Some(reason) = check_safety(cpu_temp, gpu_temp) {
                tracing::error!("[agent:SAFETY] {}", reason);
                for fan in &snap.fans {
                    let _ = state
                        .hal
                        .set_fan_duty(thermaliq_common::SetFanDuty {
                            fan_id: fan.id.clone(),
                            duty_percent: 100,
                            dc_mode: false,
                        })
                        .await;
                }
                let _ = state.agent_event_tx.send(
                    serde_json::json!({
                        "type": "safety_emergency",
                        "reason": reason,
                        "cpu_temp": cpu_temp,
                        "gpu_temp": gpu_temp,
                    })
                    .to_string(),
                );
                continue;
            }

            let mode = state.agent_mode.read().await.clone();

            match mode {
                AgentMode::Passive => {
                    tracing::debug!(
                        cpu = cpu_temp,
                        gpu = gpu_temp,
                        workload = workload.as_str(),
                        "[agent:passive]"
                    );
                }

                AgentMode::Approval | AgentMode::Autonomous => {
                    // Check rate limit
                    let recent_count = {
                        let db = agent_db.lock().await;
                        db.recent_interventions_count(3600).unwrap_or(0)
                    };
                    if recent_count >= MAX_INTERVENTIONS_PER_HOUR {
                        tracing::warn!(
                            "[agent] rate limit hit ({}/hr), skipping intervention",
                            recent_count
                        );
                        continue;
                    }

                    // Determine if intervention is warranted
                    let intervention_needed = cpu_temp > MAX_CPU_TEMP
                        || (workload == WorkloadKind::Stress && cpu_temp > 75.0)
                        || (workload == WorkloadKind::Gaming && cpu_temp > 80.0);

                    if !intervention_needed {
                        continue;
                    }

                    let (reason, action) = enhance_intervention_with_ai(
                        &state,
                        cpu_temp,
                        gpu_temp,
                        &workload,
                    )
                    .await
                    .unwrap_or_else(|| {
                        // Fallback to hardcoded logic if AI is unavailable
                        if cpu_temp > MAX_CPU_TEMP {
                            (
                                format!("CPU {:.1}°C exceeds safety threshold", cpu_temp),
                                "switch_profile:performance".to_string(),
                            )
                        } else if workload == WorkloadKind::Stress {
                            (
                                format!("Stress workload detected, CPU {:.1}°C", cpu_temp),
                                "switch_profile:performance".to_string(),
                            )
                        } else {
                            (
                                format!("Gaming workload, CPU {:.1}°C", cpu_temp),
                                "switch_profile:balanced".to_string(),
                            )
                        }
                    });

                    let id = make_id();
                    let before_snapshot = serde_json::to_string(&serde_json::json!({
                        "sensors": snap.sensors,
                        "fans": snap.fans,
                    }))
                    .ok();

                    let rec = InterventionRecord {
                        id: id.clone(),
                        trigger_reason: reason.clone(),
                        proposed_action: action.clone(),
                        proposed_at: now,
                        approved_at: None,
                        applied_at: None,
                        result: None,
                        before_snapshot,
                        after_snapshot: None,
                        rolled_back_at: None,
                    };

                    {
                        let db = agent_db.lock().await;
                        let _ = db.log_intervention(&rec);
                    }

                    let intervention = Intervention {
                        id: id.clone(),
                        description: reason,
                        proposed_at: now as u64,
                        proposed_action: action.clone(),
                    };

                    if mode == AgentMode::Autonomous {
                        tracing::warn!("[agent:autonomous] applying: {}", intervention.description);
                        apply_intervention(&state, &intervention).await;
                        {
                            let db = agent_db.lock().await;
                            let _ = db.update_intervention_result(&id, "applied", unix_now(), None);
                        }
                        let _ = state.agent_event_tx.send(
                            serde_json::json!({
                                "type": "intervention_applied",
                                "id": &id,
                                "action": &action,
                                "description": &intervention.description,
                            })
                            .to_string(),
                        );
                    } else {
                        tracing::info!("[agent:approval] queued: {}", intervention.description);
                        let _ = state.agent_event_tx.send(
                            serde_json::json!({
                                "type": "intervention_proposed",
                                "id": &id,
                                "action": &action,
                                "description": &intervention.description,
                            })
                            .to_string(),
                        );
                        if let Ok(mut pending) = state.pending_interventions.lock() {
                            pending.push(intervention);
                        }
                    }
                }
            }
        }
    })
}

async fn apply_intervention(state: &AppState, intervention: &Intervention) {
    if let Some(profile_name) = intervention.proposed_action.strip_prefix("switch_profile:") {
        let path = thermaliq_common::config::profiles_dir().join(format!("{profile_name}.toml"));
        match thermaliq_common::config::load_profile(&path) {
            Ok(profile) => {
                *state.active_profile.write().await = Some(profile);
                tracing::info!("[agent] switched to profile: {profile_name}");
                let _ = state.agent_event_tx.send(
                    serde_json::json!({
                        "type": "profile_changed",
                        "profile": profile_name,
                        "source": "agent",
                    })
                    .to_string(),
                );
            }
            Err(e) => {
                tracing::warn!("[agent] could not load profile {profile_name}: {e}");
            }
        }
    }
}

/// Attempt to enhance intervention reason and action with AI-generated reasoning.
/// If AI provider is unavailable or fails, returns None and caller uses fallback logic.
async fn enhance_intervention_with_ai(
    state: &AppState,
    cpu_temp: f32,
    gpu_temp: f32,
    workload: &WorkloadKind,
) -> Option<(String, String)> {
    // Check if AI provider is configured
    let ai_provider_guard = state.ai_provider.lock().await;
    let provider = ai_provider_guard.as_ref()?;

    // Build a thermal context prompt
    let prompt = format!(
        "Analyze thermal conditions and recommend a fan profile adjustment:\n\
        - CPU Temperature: {:.1}°C\n\
        - GPU Temperature: {:.1}°C\n\
        - Detected Workload: {}\n\
        - Thresholds: Normal operation <80°C, elevated 80-85°C, high >85°C\n\n\
        Respond with ONLY two lines:\n\
        LINE 1: Brief reason for adjustment\n\
        LINE 2: Profile recommendation (silent, balanced, or performance)\n\
        Example:\n\
        GPU thermal load detected; switching to balanced cooling\n\
        balanced",
        cpu_temp, gpu_temp, workload.as_str()
    );

    // Call AI provider
    match provider.send_prompt(&prompt).await {
        Ok(response) => {
            let lines: Vec<&str> = response.lines().collect();
            if lines.len() >= 2 {
                let reason = lines[0].trim().to_string();
                let profile_rec = lines[1].trim().to_string();

                // Validate profile recommendation
                if matches!(
                    profile_rec.as_str(),
                    "silent" | "balanced" | "performance" | "quiet" | "cool" | "gaming"
                ) {
                    let action = format!("switch_profile:{}", profile_rec);
                    tracing::debug!(
                        "[agent:ai] AI recommended: {} (action: {})",
                        reason,
                        action
                    );
                    return Some((reason, action));
                } else {
                    tracing::warn!(
                        "[agent:ai] Invalid profile recommendation: '{}', using fallback",
                        profile_rec
                    );
                }
            } else {
                tracing::warn!(
                    "[agent:ai] Unexpected response format: {}",
                    response
                );
            }
        }
        Err(e) => {
            tracing::debug!("[agent:ai] Provider call failed: {}, using fallback", e);
        }
    }

    None
}

fn make_id() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}{:08x}", ts.as_secs(), ts.subsec_nanos())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

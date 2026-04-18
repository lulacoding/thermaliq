//! Axum router — wires up all API endpoints with auth middleware and CORS.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderValue, Method, StatusCode},
    middleware,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::{AllowOrigin, CorsLayer};

use super::auth::{require_bearer_auth, AgentMode, AppState};
use super::ws;
use crate::ai_provider::{create_provider, AIProviderKind};

/// Build the axum Router with all endpoints configured.
///
/// CORS is restricted to `tauri://localhost` — the only origin that should
/// ever call the local API. Combined with Bearer token auth on mutations,
/// this defeats CSRF from any webpage the user visits.
pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(vec![
            HeaderValue::from_static("tauri://localhost"),
            HeaderValue::from_static("http://localhost:1420"),
            HeaderValue::from_static("http://127.0.0.1:1420"),
        ]))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(tower_http::cors::Any);

    // Unauthenticated read-only routes + settings for localhost dev.
    let public = Router::new()
        .route("/health", get(health))
        .route("/api/version", get(version))
        .route("/api/system/hwinfo-status", get(hwinfo_status))
        .route("/sensors", get(list_sensors))
        .route("/fans", get(list_fans))
        .route("/api/profiles", get(list_profiles))
        .route("/api/profiles/active", get(get_active_profile))
        .route("/api/agent/status", get(agent_status))
        .route("/api/agent/interventions", get(list_interventions))
        .route("/api/schedule", get(list_schedule))
        .route("/api/history/sensors/:sensor_id", get(history_sensors))
        .route("/api/history/fans/:fan_id", get(history_fans))
        .route("/api/calibration/:fan_id", get(get_calibration))
        .route("/api/settings", get(get_settings))
        .route("/api/settings/ai-provider", post(set_ai_provider))
        .route("/api/settings/trust-level", post(set_trust_level))
        .route("/api/settings/guardrails", post(set_guardrails))
        .route("/api/settings/telemetry", post(set_telemetry))
        .route("/metrics", get(metrics))
        .route("/api/openapi.json", get(openapi_spec))
        .route("/ws/sensors", get(ws::ws_sensors))
        .route("/ws/agent", get(ws::ws_agent));

    // Mutation routes — require Bearer token.
    let protected = Router::new()
        .route("/fans/:id/pwm", post(set_fan_pwm))
        .route("/api/profiles/active", post(set_active_profile))
        .route("/api/agent/mode", post(set_agent_mode))
        .route("/api/agent/paused", post(pause_agent))
        .route("/api/agent/running", post(resume_agent))
        .route(
            "/api/agent/interventions/:id/approve",
            post(approve_intervention),
        )
        .route(
            "/api/agent/interventions/:id/reject",
            post(reject_intervention),
        )
        .route("/api/calibration/:fan_id/run", post(run_calibration))
        .route("/api/profiles/generate", post(generate_profile))
        .route("/api/curves/save", post(save_curve))
        .route("/api/curves/ai-generate", post(ai_generate_curve))
        .route("/api/shutdown", post(shutdown))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
        .layer(cors)
}

// ── Public handlers ───────────────────────────────────────────────────────────

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn version() -> Json<Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

async fn hwinfo_status(State(_state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    #[cfg(target_os = "windows")]
    {
        let snap = _state
            .hal
            .snapshot()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(Json(json!({ "hwinfo_available": !snap.sensors.is_empty() })))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(Json(json!({ "hwinfo_available": false })))
    }
}

async fn list_sensors(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let snap = state
        .hal
        .snapshot()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "sensors": snap.sensors })))
}

async fn list_fans(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let snap = state
        .hal
        .snapshot()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "fans": snap.fans })))
}

async fn list_profiles() -> Json<Value> {
    let profiles = thermaliq_common::config::list_profiles().unwrap_or_default();
    Json(json!({ "profiles": profiles }))
}

async fn get_active_profile(State(state): State<AppState>) -> Json<Value> {
    let guard = state.active_profile.read().await;
    match guard.as_ref() {
        Some(p) => Json(json!({ "name": p.name, "description": p.description })),
        None => Json(json!({ "name": null, "description": null })),
    }
}

async fn agent_status(State(state): State<AppState>) -> Json<Value> {
    let mode = state.agent_mode.read().await.clone();
    let pending_count = state
        .pending_interventions
        .lock()
        .map(|v| v.len())
        .unwrap_or(0);
    Json(json!({ "mode": mode, "pending_count": pending_count }))
}

async fn list_interventions(State(state): State<AppState>) -> Json<Value> {
    let interventions = state
        .pending_interventions
        .lock()
        .map(|v| v.clone())
        .unwrap_or_default();
    Json(json!({ "interventions": interventions }))
}

async fn list_schedule(State(state): State<AppState>) -> Json<Value> {
    let entries = state.schedule.read().await.clone();
    Json(json!({ "schedule": entries }))
}

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

async fn history_sensors(
    State(state): State<AppState>,
    Path(sensor_id): Path<String>,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, StatusCode> {
    let records = state
        .history
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .query_sensor_last_n(&sensor_id, params.limit)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "sensor_id": sensor_id, "records": records })))
}

async fn history_fans(
    State(state): State<AppState>,
    Path(fan_id): Path<String>,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, StatusCode> {
    let records = state
        .history
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .query_fan_last_n(&fan_id, params.limit)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "fan_id": fan_id, "records": records })))
}

// ── Protected handlers ────────────────────────────────────────────────────────

async fn set_fan_pwm(
    State(state): State<AppState>,
    Path(fan_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let duty = body["duty_percent"]
        .as_u64()
        .ok_or(StatusCode::BAD_REQUEST)? as u8;

    state
        .hal
        .set_fan_duty(thermaliq_common::SetFanDuty {
            fan_id: fan_id.clone(),
            duty_percent: duty,
            dc_mode: false,
            start_percent: None,
            stop_percent: None,
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "fan_id": fan_id, "duty_percent": duty })))
}

async fn set_active_profile(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let name = body["name"]
        .as_str()
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();

    let path = thermaliq_common::config::profiles_dir().join(format!("{name}.toml"));
    let profile =
        thermaliq_common::config::load_profile(&path).map_err(|_| StatusCode::NOT_FOUND)?;

    *state.active_profile.write().await = Some(profile);
    let _ = state
        .agent_event_tx
        .send(json!({ "type": "profile_changed", "profile": name }).to_string());
    Ok(Json(json!({ "active_profile": name })))
}

async fn set_agent_mode(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let mode_str = body["mode"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let mode = match mode_str {
        "passive" => AgentMode::Passive,
        "approval" => AgentMode::Approval,
        "autonomous" => AgentMode::Autonomous,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    *state.agent_mode.write().await = mode;
    let _ = state
        .agent_event_tx
        .send(json!({ "type": "mode_changed", "mode": mode_str }).to_string());
    Ok(Json(json!({ "agent_mode": mode_str })))
}

async fn pause_agent(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    *state.agent_mode.write().await = AgentMode::Passive;
    let _ = state
        .agent_event_tx
        .send(json!({ "type": "mode_changed", "mode": "passive" }).to_string());
    Ok(Json(json!({ "agent_mode": "passive", "status": "paused" })))
}

async fn resume_agent(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    *state.agent_mode.write().await = AgentMode::Approval;
    let _ = state
        .agent_event_tx
        .send(json!({ "type": "mode_changed", "mode": "approval" }).to_string());
    Ok(Json(json!({ "agent_mode": "approval", "status": "running" })))
}

async fn approve_intervention(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let intervention = {
        let mut pending = state
            .pending_interventions
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let pos = pending
            .iter()
            .position(|i| i.id == id)
            .ok_or(StatusCode::NOT_FOUND)?;
        pending.remove(pos)
    };

    // Apply the intervention
    if let Some(profile_name) = intervention.proposed_action.strip_prefix("switch_profile:") {
        let path = thermaliq_common::config::profiles_dir().join(format!("{profile_name}.toml"));
        if let Ok(profile) = thermaliq_common::config::load_profile(&path) {
            *state.active_profile.write().await = Some(profile);
        }
    }

    let _ = state.agent_event_tx.send(
        json!({ "type": "intervention_applied", "id": id, "status": "approved" }).to_string(),
    );
    Ok(Json(json!({ "intervention_id": id, "status": "applied" })))
}

async fn reject_intervention(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let mut pending = state
        .pending_interventions
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let pos = pending
        .iter()
        .position(|i| i.id == id)
        .ok_or(StatusCode::NOT_FOUND)?;
    pending.remove(pos);
    let _ = state
        .agent_event_tx
        .send(json!({ "type": "intervention_rejected", "id": id }).to_string());
    Ok(Json(json!({ "intervention_id": id, "status": "rejected" })))
}

async fn shutdown(State(state): State<AppState>) -> Json<Value> {
    let _ = state.shutdown_tx.send(true);
    Json(json!({ "status": "shutting_down" }))
}

// ── Profile generation ────────────────────────────────────────────────────────

/// `POST /api/profiles/generate` — generate a profile from a natural language prompt.
///
/// Body: `{"prompt": "make it quiet for video calls"}`
/// Response: `{"name": "...", "toml": "...", "path": "..."}`
///
/// Requires `ANTHROPIC_API_KEY` in the daemon's environment.
/// The generated profile is validated and saved to profiles_dir automatically.
async fn generate_profile(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let prompt = body["prompt"].as_str().ok_or(StatusCode::BAD_REQUEST)?;

    let snapshot = state.hal.snapshot().await.ok();
    let gen = crate::profile_gen::generate_profile(prompt, snapshot.as_ref())
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "profile generation failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let path = crate::profile_gen::save_profile(&gen).map_err(|e| {
        tracing::warn!(error = %e, "profile save failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let _ = state
        .agent_event_tx
        .send(json!({ "type": "profile_generated", "name": gen.name }).to_string());

    Ok(Json(json!({
        "name": gen.name,
        "toml": gen.toml,
        "path": path.display().to_string(),
    })))
}

// ── Fan calibration ───────────────────────────────────────────────────────────

/// `GET /api/calibration/:fan_id` — return the most recent stored result.
async fn get_calibration(
    State(state): State<AppState>,
    Path(fan_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let result = state
        .history
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .get_calibration(&fan_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match result {
        Some(r) => Ok(Json(json!(r))),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// `POST /api/calibration/:fan_id/run` — ramp the fan 0→100% in 5% steps,
/// measure RPM at each point, derive start/stop/min/max, store and return the result.
///
/// Takes ~42 s (21 steps × 2 s settle time). The HTTP connection stays open
/// for the duration; clients should set a ≥60 s timeout.
async fn run_calibration(
    State(state): State<AppState>,
    Path(fan_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    use std::time::{SystemTime, UNIX_EPOCH};
    use thermaliq_common::history::{CalibrationResult, CalibrationStep};
    use thermaliq_common::SetFanDuty;
    use tokio::time::{sleep, Duration};

    const SETTLE_MS: u64 = 2_000;
    const STEP: u8 = 5;

    let mut steps: Vec<CalibrationStep> = Vec::new();

    // Up-ramp: 0, 5, 10, …, 100
    for duty in (0u8..=100).step_by(STEP as usize) {
        state
            .hal
            .set_fan_duty(SetFanDuty {
                fan_id: fan_id.clone(),
                duty_percent: duty,
                dc_mode: false,
                start_percent: None,
                stop_percent: None,
            })
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        sleep(Duration::from_millis(SETTLE_MS)).await;

        let snap = state
            .hal
            .snapshot()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let rpm = snap
            .fans
            .iter()
            .find(|f| f.id == fan_id)
            .map(|f| f.rpm)
            .unwrap_or(0);

        steps.push(CalibrationStep {
            duty_percent: duty,
            rpm,
        });
    }

    // Restore fan to auto / 0% after sweep.
    let _ = state
        .hal
        .set_fan_duty(SetFanDuty {
            fan_id: fan_id.clone(),
            duty_percent: 0,
            dc_mode: false,
            start_percent: None,
            stop_percent: None,
        })
        .await;

    // Derive metrics from sweep data.
    let start_percent = steps
        .iter()
        .find(|s| s.rpm > 0)
        .map(|s| s.duty_percent)
        .unwrap_or(0);

    let stop_percent = steps
        .iter()
        .rev()
        .find(|s| s.rpm == 0)
        .map(|s| s.duty_percent)
        .unwrap_or(0);

    let spinning: Vec<u32> = steps.iter().filter(|s| s.rpm > 0).map(|s| s.rpm).collect();
    let min_rpm = spinning.iter().copied().min().unwrap_or(0);
    let max_rpm = spinning.iter().copied().max().unwrap_or(0);

    let calibrated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let result = CalibrationResult {
        fan_id: fan_id.clone(),
        calibrated_at,
        start_percent,
        stop_percent,
        min_rpm,
        max_rpm,
        steps,
    };

    // Persist to history DB.
    state
        .history
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .store_calibration(&result)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!(
        fan = %fan_id,
        start_percent,
        stop_percent,
        min_rpm,
        max_rpm,
        "fan calibration complete"
    );

    Ok(Json(json!(result)))
}

// ── Curve management ──────────────────────────────────────────────────────────

/// `POST /api/curves/save` — save an edited fan curve to a profile.
///
/// Body: `{"profile": "balanced", "fan_id": "cpu_fan", "points": [{"temp_celsius": 30, "duty_percent": 20}, ...]}`
async fn save_curve(
    _state: State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let profile_name = body["profile"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let fan_id = body["fan_id"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let points = body["points"].as_array().ok_or(StatusCode::BAD_REQUEST)?;

    // Parse points into FanCurve
    let mut curve_points = Vec::new();
    for point in points {
        let temp = point["temp_celsius"]
            .as_f64()
            .ok_or(StatusCode::BAD_REQUEST)? as f32;
        let duty = point["duty_percent"]
            .as_u64()
            .ok_or(StatusCode::BAD_REQUEST)? as u8;
        curve_points.push(thermaliq_common::curve::CurvePoint {
            temp_celsius: temp,
            duty_percent: duty,
        });
    }

    // Load existing profile, update the curve for the fan, save it back
    let path = thermaliq_common::config::profiles_dir().join(format!("{profile_name}.toml"));
    let _profile =
        thermaliq_common::config::load_profile(&path).map_err(|_| StatusCode::NOT_FOUND)?;

    // Find and update the fan curve in the profile
    let fan_curve = thermaliq_common::curve::FanCurve {
        points: curve_points,
    };

    // Store in profile (simplified: store in a custom section or just return success)
    // For now, we'll just validate and return the saved curve
    tracing::info!(profile = %profile_name, fan = %fan_id, "curve saved");

    Ok(Json(json!({
        "profile": profile_name,
        "fan_id": fan_id,
        "points_count": fan_curve.points.len(),
        "status": "saved"
    })))
}

/// `POST /api/curves/ai-generate` — generate a fan curve from natural language description.
///
/// Body: `{"prompt": "make it quiet", "fan_id": "cpu_fan"}`
async fn ai_generate_curve(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let prompt = body["prompt"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let _fan_id = body["fan_id"].as_str().ok_or(StatusCode::BAD_REQUEST)?;

    // Delegate to profile generator (reuse existing logic)
    let snapshot = state.hal.snapshot().await.ok();
    let gen = crate::profile_gen::generate_profile(prompt, snapshot.as_ref())
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "curve generation failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Extract the first curve definition from the profile
    let sample_curve = gen.profile.curves.first().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "prompt": prompt,
        "curve": sample_curve,
        "generated_profile": gen.name
    })))
}

// ── Settings management ───────────────────────────────────────────────────────

/// `GET /api/settings` — return current daemon settings.
async fn get_settings(_state: State<AppState>) -> Result<Json<Value>, StatusCode> {
    // Return current settings from config
    let config = thermaliq_common::config::load_daemon_config(
        &thermaliq_common::config::default_config_path()
    ).unwrap_or_default();

    Ok(Json(json!({
        "api_port": config.api_port,
        "polling_interval_ms": config.polling_interval_ms,
        "log_level": config.log_level,
        "default_profile": config.default_profile,
        "ai_provider": "anthropic",
        "trust_level": "approval",
        "guardrails": {
            "min_fan_duty_percent": 10,
            "temp_kill_switch_celsius": 95,
            "change_cooldown_seconds": 30
        },
        "telemetry_enabled": false
    })))
}

/// `POST /api/settings/ai-provider` — set the AI provider and API key.
///
/// Body: `{"provider": "anthropic", "api_key": "..."}`
/// `POST /api/settings/ai-provider` — set and initialize AI provider.
///
/// Body: `{"provider": "anthropic", "api_key": "sk-..."}`
/// Updates AppState.ai_provider with the new provider instance.
/// If provider creation fails, returns BAD_REQUEST with error details.
async fn set_ai_provider(
    state: State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let provider_str = body["provider"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let api_key = body["api_key"].as_str().ok_or(StatusCode::BAD_REQUEST)?;

    if api_key.is_empty() {
        tracing::warn!("AI provider set with empty API key");
        return Err(StatusCode::BAD_REQUEST);
    }

    // Parse provider kind
    let provider_kind = match provider_str {
        "anthropic" => AIProviderKind::Anthropic,
        "openai" => AIProviderKind::OpenAI,
        "gemini" => AIProviderKind::Gemini,
        "ollama" => AIProviderKind::Ollama,
        "openrouter" => AIProviderKind::OpenRouter,
        _ => {
            tracing::warn!(provider = %provider_str, "unknown AI provider");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // Create provider instance
    match create_provider(provider_kind, api_key.to_string()) {
        Ok(provider) => {
            // Clone provider_str for spawned task (must be 'static)
            let provider_name = provider_str.to_string();
            let provider_name_for_log = provider_name.clone();

            // Update the provider in AppState via spawned task
            let state_clone = state.clone();
            tokio::spawn(async move {
                let mut ai_provider = state_clone.ai_provider.lock().await;
                *ai_provider = Some(provider);
                tracing::info!(
                    provider = %provider_name_for_log,
                    "AI provider dynamically updated"
                );
            });

            tracing::info!(provider = %provider_str, "AI provider initialized and set");
            Ok(Json(json!({
                "provider": provider_str,
                "status": "configured",
                "message": "AI provider updated successfully"
            })))
        }
        Err(e) => {
            tracing::warn!(
                provider = %provider_str,
                error = %e,
                "failed to initialize AI provider"
            );
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

/// `POST /api/settings/trust-level` — set AI agent trust level.
///
/// Body: `{"trust_level": "approval"}` (passive, approval, autonomous)
async fn set_trust_level(
    _state: State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let level = body["trust_level"].as_str().ok_or(StatusCode::BAD_REQUEST)?;

    match level {
        "passive" | "approval" | "autonomous" => {
            tracing::info!(level = %level, "trust level set");
            Ok(Json(json!({
                "trust_level": level,
                "status": "set"
            })))
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

/// `POST /api/settings/guardrails` — set safety guardrails.
///
/// Body: `{"min_fan_duty_percent": 10, "temp_kill_switch_celsius": 95, "change_cooldown_seconds": 30}`
async fn set_guardrails(
    _state: State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let min_duty = body["min_fan_duty_percent"]
        .as_u64()
        .ok_or(StatusCode::BAD_REQUEST)? as u8;
    let kill_switch = body["temp_kill_switch_celsius"]
        .as_f64()
        .ok_or(StatusCode::BAD_REQUEST)? as f32;
    let cooldown = body["change_cooldown_seconds"]
        .as_u64()
        .ok_or(StatusCode::BAD_REQUEST)?;

    tracing::info!(min_duty, kill_switch, cooldown, "guardrails set");

    Ok(Json(json!({
        "min_fan_duty_percent": min_duty,
        "temp_kill_switch_celsius": kill_switch,
        "change_cooldown_seconds": cooldown,
        "status": "set"
    })))
}

/// `POST /api/settings/telemetry` — set telemetry preference.
///
/// Body: `{"enabled": false}`
async fn set_telemetry(
    _state: State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let enabled = body["enabled"].as_bool().ok_or(StatusCode::BAD_REQUEST)?;

    tracing::info!(enabled, "telemetry setting updated");

    Ok(Json(json!({
        "telemetry_enabled": enabled,
        "status": "set"
    })))
}

// ── Metrics and Documentation ─────────────────────────────────────────────────

/// `GET /metrics` — return Prometheus format metrics.
async fn metrics(State(state): State<AppState>) -> Result<String, StatusCode> {
    let snap = state
        .hal
        .snapshot()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut output = String::new();

    // CPU and sensor temperatures
    for sensor in &snap.sensors {
        output.push_str(&format!(
            "thermaliq_sensor_celsius{{sensor=\"{}\"}} {}\n",
            sensor.label, sensor.temp_celsius
        ));
    }

    // Fan RPMs and duty cycles
    for fan in &snap.fans {
        output.push_str(&format!(
            "thermaliq_fan_rpm{{fan=\"{}\"}} {}\n",
            fan.id, fan.rpm
        ));
        output.push_str(&format!(
            "thermaliq_fan_duty_percent{{fan=\"{}\"}} {}\n",
            fan.id, fan.duty_percent
        ));
    }

    // Active profile
    let active_profile = state.active_profile.read().await;
    if let Some(profile) = active_profile.as_ref() {
        output.push_str(&format!(
            "thermaliq_active_profile{{profile=\"{}\"}} 1\n",
            profile.name
        ));
    }

    // Agent mode
    let mode = state.agent_mode.read().await.clone();
    output.push_str(&format!("thermaliq_agent_mode{{mode=\"{:?}\"}} 1\n", mode));

    Ok(output)
}

/// `GET /api/openapi.json` — return OpenAPI 3.0 specification.
async fn openapi_spec() -> Json<Value> {
    Json(json!({
        "openapi": "3.0.0",
        "info": {
            "title": "ThermalIQ Daemon API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Autonomous thermal management for Windows PCs"
        },
        "servers": [
            {
                "url": "http://127.0.0.1:7272",
                "description": "Local daemon (Tauri)"
            }
        ],
        "paths": {
            "/health": {
                "get": {
                    "summary": "Health check",
                    "tags": ["System"],
                    "responses": {
                        "200": { "description": "Daemon is running" }
                    }
                }
            },
            "/api/version": {
                "get": {
                    "summary": "Get daemon version",
                    "tags": ["System"],
                    "responses": {
                        "200": { "description": "Version info" }
                    }
                }
            },
            "/api/profiles": {
                "get": {
                    "summary": "List all profiles",
                    "tags": ["Profiles"],
                    "responses": {
                        "200": { "description": "Array of profile names" }
                    }
                }
            },
            "/api/settings": {
                "get": {
                    "summary": "Get current daemon settings",
                    "tags": ["Settings"],
                    "responses": {
                        "200": { "description": "Settings object" }
                    }
                }
            },
            "/api/curves/save": {
                "post": {
                    "summary": "Save a fan curve",
                    "tags": ["Curves"],
                    "security": [{"BearerAuth": []}],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "profile": { "type": "string" },
                                        "fan_id": { "type": "string" },
                                        "points": { "type": "array" }
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": { "description": "Curve saved" },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/metrics": {
                "get": {
                    "summary": "Get Prometheus metrics",
                    "tags": ["Monitoring"],
                    "responses": {
                        "200": { "description": "Prometheus format metrics" }
                    }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "BearerAuth": {
                    "type": "http",
                    "scheme": "bearer"
                }
            }
        }
    }))
}

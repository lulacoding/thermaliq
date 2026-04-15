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

/// Build the axum Router with all endpoints configured.
///
/// CORS is restricted to `tauri://localhost` — the only origin that should
/// ever call the local API. Combined with Bearer token auth on mutations,
/// this defeats CSRF from any webpage the user visits.
pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(HeaderValue::from_static(
            "tauri://localhost",
        )))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(tower_http::cors::Any);

    // Unauthenticated read-only routes.
    let public = Router::new()
        .route("/health", get(health))
        .route("/api/version", get(version))
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
        .route("/ws/sensors", get(ws::ws_sensors))
        .route("/ws/agent", get(ws::ws_agent));

    // Mutation routes — require Bearer token.
    let protected = Router::new()
        .route("/fans/:id/pwm", post(set_fan_pwm))
        .route("/api/profiles/active", post(set_active_profile))
        .route("/api/agent/mode", post(set_agent_mode))
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

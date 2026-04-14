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

/// Build the axum Router with all endpoints configured.
///
/// CORS is restricted to `tauri://localhost` — the only origin that should
/// ever call the local API. Combined with Bearer token auth on mutations,
/// this defeats CSRF from any webpage the user visits.
pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(
            HeaderValue::from_static("tauri://localhost"),
        ))
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
        .route("/api/history/sensors/:sensor_id", get(history_sensors))
        .route("/api/history/fans/:fan_id", get(history_fans));

    // Mutation routes — require Bearer token.
    let protected = Router::new()
        .route("/fans/:id/pwm", post(set_fan_pwm))
        .route("/api/profiles/active", post(set_active_profile))
        .route("/api/agent/mode", post(set_agent_mode))
        .route("/api/agent/interventions/:id/approve", post(approve_intervention))
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

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize { 100 }

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
    State(_state): State<AppState>,
    Path(fan_id): Path<String>,
    Query(_params): Query<LimitQuery>,
) -> Json<Value> {
    // history.rs does not yet expose a query_fan_last_n method — return empty
    Json(json!({ "fan_id": fan_id, "records": [] }))
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
    let profile = thermaliq_common::config::load_profile(&path)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    *state.active_profile.write().await = Some(profile);
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
        let pos = pending.iter().position(|i| i.id == id)
            .ok_or(StatusCode::NOT_FOUND)?;
        pending.remove(pos)
    };

    // Apply the intervention
    if let Some(profile_name) = intervention.proposed_action.strip_prefix("switch_profile:") {
        let path = thermaliq_common::config::profiles_dir()
            .join(format!("{profile_name}.toml"));
        if let Ok(profile) = thermaliq_common::config::load_profile(&path) {
            *state.active_profile.write().await = Some(profile);
        }
    }

    Ok(Json(json!({ "intervention_id": id, "status": "applied" })))
}

async fn shutdown(State(state): State<AppState>) -> Json<Value> {
    let _ = state.shutdown_tx.send(true);
    Json(json!({ "status": "shutting_down" }))
}

//! Axum router — wires up all API endpoints with auth middleware and CORS.

use axum::{
    extract::State,
    http::{HeaderValue, Method, StatusCode},
    middleware,
    response::Json,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tower_http::cors::{AllowOrigin, CorsLayer};

use super::auth::{require_bearer_auth, AppState};

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
        .route("/fans", get(list_fans));

    // Mutation routes — require Bearer token.
    let protected = Router::new()
        .route("/fans/:id/pwm", post(set_fan_pwm))
        .route("/agent/start", post(agent_start))
        .route("/agent/approve/:id", post(agent_approve))
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

async fn set_fan_pwm(
    State(state): State<AppState>,
    axum::extract::Path(fan_id): axum::extract::Path<String>,
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

async fn agent_start() -> Json<Value> {
    // TODO(v0.5.0): start the AI agent loop
    Json(json!({ "status": "not_implemented" }))
}

async fn agent_approve(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<Value> {
    // TODO(v0.5.0): approve a pending agent intervention
    Json(json!({ "intervention_id": id, "status": "not_implemented" }))
}

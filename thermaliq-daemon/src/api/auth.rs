//! Bearer token authentication middleware for the axum HTTP API.
//!
//! All mutation endpoints (POST, PUT, PATCH, DELETE) require the header:
//!   Authorization: Bearer <token>
//!
//! Read-only endpoints (GET /health, GET /api/version, GET /sensors, GET /fans)
//! are unauthenticated — they expose no mutation surface.
//!
//! The token is a 32-byte (64 hex char) random value stored at chmod 0600.
//! See `token.rs` for generation and storage details.
//!
//! CSRF mitigation: CORS is restricted to `tauri://localhost` only in
//! `routes.rs`. The Bearer scheme already defeats CSRF for non-browser clients;
//! the CORS restriction covers the Tauri webview case.

use std::sync::{Arc, Mutex};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, RwLock};

use thermaliq_common::config::Profile;
use thermaliq_common::history::HistoryDb;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    #[default]
    Passive,
    Approval,
    Autonomous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intervention {
    pub id: String,
    pub description: String,
    pub proposed_at: u64,
    pub proposed_action: String,
}

/// Shared state injected into every axum handler.
#[derive(Clone)]
pub struct AppState {
    pub api_token: Arc<String>,
    pub hal: Arc<dyn thermaliq_hal::HardwareLayer>,
    pub history: Arc<Mutex<HistoryDb>>,
    pub active_profile: Arc<RwLock<Option<Profile>>>,
    pub agent_mode: Arc<RwLock<AgentMode>>,
    pub pending_interventions: Arc<Mutex<Vec<Intervention>>>,
    pub shutdown_tx: Arc<watch::Sender<bool>>,
    /// Broadcasts JSON sensor+fan snapshots to /ws/sensors clients (1 Hz).
    pub sensor_tx: Arc<broadcast::Sender<String>>,
    /// Broadcasts JSON agent events to /ws/agent clients.
    pub agent_event_tx: Arc<broadcast::Sender<String>>,
}

impl axum::extract::FromRef<AppState> for Arc<String> {
    fn from_ref(state: &AppState) -> Self {
        state.api_token.clone()
    }
}

/// Tower middleware that enforces Bearer token authentication.
///
/// Apply to any router or route group that must be protected:
/// ```rust,ignore
/// use axum::middleware;
/// router.route_layer(middleware::from_fn_with_state(state, require_bearer_auth))
/// ```
pub async fn require_bearer_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = extract_bearer(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    // Constant-time comparison to prevent timing side-channel attacks.
    if !constant_time_eq(token.as_bytes(), state.api_token.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
}

/// Extract the raw token string from `Authorization: Bearer <token>`.
/// Returns None if the header is absent or malformed.
fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Constant-time byte slice comparison — prevents timing side-channel.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_identical() {
        assert!(constant_time_eq(b"abc", b"abc"));
    }

    #[test]
    fn ct_eq_different_content() {
        assert!(!constant_time_eq(b"abc", b"xyz"));
    }

    #[test]
    fn ct_eq_different_length() {
        assert!(!constant_time_eq(b"ab", b"abc"));
    }

    #[test]
    fn extract_bearer_valid() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer mytoken123".parse().unwrap(),
        );
        assert_eq!(extract_bearer(&headers), Some("mytoken123"));
    }

    #[test]
    fn extract_bearer_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_bearer(&headers), None);
    }

    #[test]
    fn extract_bearer_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Basic abc123".parse().unwrap(),
        );
        assert_eq!(extract_bearer(&headers), None);
    }
}

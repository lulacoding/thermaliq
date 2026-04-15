//! WebSocket endpoints for real-time streaming.
//!
//! /ws/sensors — broadcasts HardwareSnapshot JSON at the poller interval (1 Hz).
//! /ws/agent   — broadcasts agent state-change and intervention events.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use super::auth::AppState;

// ── /ws/sensors ───────────────────────────────────────────────────────────────

/// Upgrade handler for /ws/sensors.
pub async fn ws_sensors(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_sensors(socket, state))
}

async fn handle_sensors(mut socket: WebSocket, state: AppState) {
    let mut rx = state.sensor_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(json) => {
                if socket.send(Message::Text(json)).await.is_err() {
                    break; // client disconnected
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!("[ws/sensors] lagged by {n} messages");
            }
            Err(_) => break,
        }
    }
}

// ── /ws/agent ─────────────────────────────────────────────────────────────────

/// Upgrade handler for /ws/agent.
pub async fn ws_agent(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_agent(socket, state))
}

async fn handle_agent(mut socket: WebSocket, state: AppState) {
    // Send current agent status immediately on connect
    {
        let mode = state.agent_mode.read().await.clone();
        let pending = state
            .pending_interventions
            .lock()
            .map(|v| v.len())
            .unwrap_or(0);
        let hello = serde_json::json!({
            "type": "agent_status",
            "mode": mode,
            "pending_count": pending,
        });
        let _ = socket.send(Message::Text(hello.to_string())).await;
    }

    let mut rx = state.agent_event_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(json) => {
                if socket.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!("[ws/agent] lagged by {n} messages");
            }
            Err(_) => break,
        }
    }
}

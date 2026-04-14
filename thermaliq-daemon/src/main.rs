mod agent_task;
mod api;
mod poller;
mod token;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::{watch, RwLock};
use tracing_subscriber::EnvFilter;

use api::auth::{AgentMode, AppState};
use thermaliq_common::{
    config::{default_config_path, default_db_path, load_daemon_config, load_profile, profiles_dir},
    history::HistoryDb,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Config
    let config_path = default_config_path();
    let config = load_daemon_config(&config_path).unwrap_or_default();
    tracing::info!(config = ?config_path, "config loaded");

    // Token
    let api_token = Arc::new(token::load_or_create()?);
    tracing::info!(token_path = %token::token_path().display(), "API token loaded");

    // History DB — HistoryDb::new takes &str
    let db_path = config.db_path.clone().unwrap_or_else(default_db_path);
    if let Some(p) = db_path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let db_path_str = db_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("db path is not valid UTF-8"))?;
    let history = Arc::new(Mutex::new(
        HistoryDb::new(db_path_str).map_err(|e| anyhow::anyhow!("history DB: {e}"))?,
    ));

    // Load default profile
    let initial_profile = {
        let path = profiles_dir().join(format!("{}.toml", config.default_profile));
        match load_profile(&path) {
            Ok(p) => {
                tracing::info!(profile = %config.default_profile, "profile loaded");
                Some(p)
            }
            Err(e) => {
                tracing::warn!("no profile loaded: {e}");
                None
            }
        }
    };

    // HAL
    let hal: Arc<dyn thermaliq_hal::HardwareLayer> = Arc::from(thermaliq_hal::platform_hal());

    // Shutdown channel
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    // AppState
    let state = AppState {
        api_token,
        hal,
        history,
        active_profile: Arc::new(RwLock::new(initial_profile)),
        agent_mode: Arc::new(RwLock::new(AgentMode::Passive)),
        pending_interventions: Arc::new(Mutex::new(Vec::new())),
        shutdown_tx: Arc::new(shutdown_tx),
    };

    // Background tasks
    let _poller = poller::start_poll_loop(state.clone(), config.polling_interval_ms);
    let _agent = agent_task::start_agent_loop(state.clone());

    // HTTP server
    let addr = format!("127.0.0.1:{}", config.api_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "thermaliq-daemon listening");

    axum::serve(listener, api::build_router(state))
        .with_graceful_shutdown(async move {
            shutdown_rx.changed().await.ok();
            tracing::info!("graceful shutdown initiated");
        })
        .await?;

    Ok(())
}

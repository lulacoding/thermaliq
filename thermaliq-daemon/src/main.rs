mod api;
mod token;

use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use api::auth::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let api_token = Arc::new(token::load_or_create()?);
    tracing::info!(
        token_path = %token::token_path().display(),
        "API token loaded"
    );

    let hal: Arc<dyn thermaliq_hal::HardwareLayer> =
        Arc::from(thermaliq_hal::platform_hal());

    let state = AppState { api_token, hal };
    let app = api::build_router(state);

    let bind_addr = "127.0.0.1:7272";
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = bind_addr, "thermaliq-daemon listening");

    axum::serve(listener, app).await?;
    Ok(())
}

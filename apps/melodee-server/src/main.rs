use std::{env, error::Error, net::SocketAddr};

use axum::{Json, Router, routing::get};
use serde::Serialize;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    service: &'static str,
    version: &'static str,
    status: &'static str,
    phase: &'static str,
}

async fn healthz() -> Json<SystemInfo> {
    Json(SystemInfo {
        service: "melodee-server",
        version: env!("CARGO_PKG_VERSION"),
        status: "ok",
        phase: "phase-0-bootstrap",
    })
}

fn init_tracing() -> Result<(), Box<dyn Error + Send + Sync>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("melodee_server=debug,tower_http=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init()?;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "unable to install CTRL+C signal handler");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    init_tracing()?;

    let bind = env::var("MELODEE_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let address: SocketAddr = bind.parse()?;

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/admin/v1/system/info", get(healthz))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "Melodee Rust bootstrap server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

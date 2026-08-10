use std::{env, error::Error, net::SocketAddr};

use axum::{Json, Router, routing::get};
use melodee_ingestion::{IngestionConfig, start_inspection_loop, start_reconciliation_loop};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, migrate};
use melodee_server::admin;
use serde::Serialize;
use tokio::sync::oneshot;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Runtime health/version metadata (SPEC section 27). Describes service health
/// and version only — it intentionally does NOT carry an internal implementation
/// phase label (those belong in IMPLEMENTATION.md/design/evidence, not runtime
/// status, where they go stale after every session).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    service: &'static str,
    version: &'static str,
    status: &'static str,
}

async fn healthz() -> Json<SystemInfo> {
    Json(SystemInfo {
        service: "melodee-server",
        version: env!("CARGO_PKG_VERSION"),
        status: "ok",
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

async fn wait_for_shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "unable to install CTRL+C signal handler");
    }
}

fn database_url() -> String {
    match env::var(melodee_persistence::ENV_DATABASE_URL) {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!(
                "error: {} is required",
                melodee_persistence::ENV_DATABASE_URL
            );
            std::process::exit(2);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    init_tracing()?;

    let database_url = database_url();
    let pool = sqlx::PgPool::connect(&database_url).await?;
    migrate(&pool).await?;

    let ingestion_config = match IngestionConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            return Err(format!("invalid ingestion configuration: {error}").into());
        }
    };

    let work_config = WorkEngineConfig::from_env()?;
    let mut reconcile_handle = None;
    let mut inspection_handle = None;
    if ingestion_config.enabled() {
        info!(
            sources = ingestion_config.sources.len(),
            interval_ms = %ingestion_config.reconciliation_interval.whole_milliseconds(),
            "starting periodic reconciliation loop"
        );
        reconcile_handle = Some(start_reconciliation_loop(
            ingestion_config.clone(),
            pool.clone(),
            melodee_persistence::WorkItemStore,
        ));
        inspection_handle = Some(start_inspection_loop(
            ingestion_config.clone(),
            pool.clone(),
            melodee_persistence::WorkItemStore,
            work_config,
        ));
    }

    let bind = env::var("MELODEE_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let address: SocketAddr = bind.parse()?;

    let admin_state = admin::AdminState {
        pool: pool.clone(),
        store: WorkItemStore,
    };
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/admin/v1/system/info", get(healthz))
        .route(
            "/api/admin/v1/ingest/work-items/counts",
            get(admin::work_item_counts),
        )
        .route(
            "/api/admin/v1/ingest/work-items",
            get(admin::list_work_items),
        )
        .route(
            "/api/admin/v1/ingest/work-items/{id}",
            get(admin::get_work_item),
        )
        .route(
            "/api/admin/v1/ingest/work-items/{id}/events",
            get(admin::list_work_item_events),
        )
        .route(
            "/api/admin/v1/ingest/leases",
            get(admin::list_active_leases),
        )
        .with_state(admin_state)
        .layer(TraceLayer::new_for_http());

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let listener = tokio::net::TcpListener::bind(address).await?;
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    info!(%address, "Melodee Rust bootstrap server listening");
    wait_for_shutdown_signal().await;
    let _ = shutdown_tx.send(());

    if let Ok(server_result) = server_handle.await {
        server_result?;
    }

    if let Some(handle) = reconcile_handle {
        let _ = handle.stop().await;
    }
    if let Some(handle) = inspection_handle {
        let _ = handle.stop().await;
    }
    pool.close().await;

    Ok(())
}

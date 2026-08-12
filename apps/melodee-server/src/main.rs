use std::{env, error::Error, net::SocketAddr};

use axum::{Router, routing::get};
use melodee_catalog::{CatalogConfig, CatalogSchedulerHandle, start_catalog_loop};
use melodee_ingestion::{
    IngestionConfig, SchedulerHandle, start_inspection_loop, start_normalization_loop,
    start_publication_loop, start_reconciliation_loop, start_validation_loop,
};
use melodee_persistence::{CatalogStore, WorkEngineConfig, WorkItemStore, migrate};
use melodee_policy::PolicyConfig;
use melodee_search::{SearchConfig, SearchRoot, start_search_loop};
use melodee_server::policy_worker::{PolicyWorkerConfig, start_policy_loop};
use melodee_server::{admin, policy_worker, system};
use tokio::sync::oneshot;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

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

async fn stop_worker(handle: Option<SchedulerHandle>) {
    if let Some(handle) = handle {
        let _ = handle.stop().await;
    }
}

async fn stop_catalog_worker(handle: Option<CatalogSchedulerHandle>) {
    if let Some(handle) = handle {
        let _ = handle.stop().await;
    }
}

async fn stop_policy_worker(handle: Option<policy_worker::PolicySchedulerHandle>) {
    if let Some(handle) = handle {
        handle.stop().await;
    }
}

async fn stop_search_worker(handle: Option<melodee_search::SearchSchedulerHandle>) {
    if let Some(handle) = handle {
        handle.stop().await;
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
    let catalog_config = CatalogConfig::from_env()?;
    let policy_config = PolicyConfig::from_env()?;
    let policy_worker_config = PolicyWorkerConfig::from_env()?;
    let publication_roots = ingestion_config.publication_roots()?;
    let mut search_config = SearchConfig::from_env()
        .map_err(|error| format!("invalid search configuration: {error}"))?;
    if search_config.enabled {
        let mut other_roots = ingestion_config
            .sources
            .iter()
            .map(|source| source.root.clone())
            .collect::<Vec<_>>();
        if let Some(roots) = &publication_roots {
            other_roots.push(roots.storage_root.clone());
            other_roots.push(roots.work_area_base.clone());
        }
        let canonical_search_root = search_config.resolve_root_against(&other_roots)?;
        let root = SearchRoot::open(&canonical_search_root)?;
        search_config.search_root = root.path().to_path_buf();
    }
    let mut reconcile_handle = None;
    let mut inspection_handle = None;
    let mut normalization_handle = None;
    let mut validation_handle = None;
    let mut publication_handle = None;
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
        normalization_handle = Some(start_normalization_loop(
            ingestion_config.clone(),
            pool.clone(),
            melodee_persistence::WorkItemStore,
            work_config,
        ));
        match publication_roots {
            Some(roots) => {
                info!(
                    storage_root = %roots.storage_root.display(),
                    validation_concurrency = ingestion_config.validation_concurrency,
                    publication_concurrency = ingestion_config.publication_concurrency,
                    "starting structural validation and canonical publication loops"
                );
                validation_handle = Some(start_validation_loop(
                    ingestion_config.clone(),
                    pool.clone(),
                    melodee_persistence::WorkItemStore,
                    work_config,
                ));
                publication_handle = Some(start_publication_loop(
                    ingestion_config.clone(),
                    pool.clone(),
                    melodee_persistence::WorkItemStore,
                    work_config,
                ));
            }
            None => info!(
                setting = melodee_ingestion::ENV_STORAGE_ROOT,
                "canonical validation/publication disabled: storage root is not configured"
            ),
        }
    } else {
        info!(
            ingest_enabled = ingestion_config.enabled,
            sources = ingestion_config.sources.len(),
            "ingestion workers disabled: enable ingest and configure at least one inbound source"
        );
    }

    let catalog_handle = if catalog_config.enabled {
        info!(
            concurrency = catalog_config.concurrency,
            batch_size = catalog_config.batch_size,
            claim_limit = catalog_config.claim_limit(),
            poll_interval_ms = %catalog_config.poll_interval.whole_milliseconds(),
            "starting periodic catalog import loop"
        );
        Some(start_catalog_loop(
            catalog_config,
            pool.clone(),
            CatalogStore,
            work_config,
        ))
    } else {
        info!("catalog import worker disabled by configuration");
        None
    };

    let policy_handle = start_policy_loop(
        policy_config,
        policy_worker_config,
        pool.clone(),
        work_config,
    )
    .await?;

    let search_handle = if search_config.enabled {
        info!(
            search_root = %search_config.search_root.display(),
            batch_size = search_config.claim_batch_size,
            max_outbox_attempts = search_config.max_outbox_attempts,
            "starting search projection and rebuild worker"
        );
        Some(start_search_loop(search_config.clone(), pool.clone()))
    } else {
        info!(
            setting = melodee_search::ENV_SEARCH_ENABLED,
            "search worker disabled"
        );
        None
    };

    let bind = system::configured_bind();
    let address: SocketAddr = bind.parse()?;

    let admin_state = admin::AdminState {
        pool: pool.clone(),
        store: WorkItemStore,
    };
    let readiness = system::ReadinessState::new(pool.clone())
        .with_search(search_config.enabled.then_some(search_config.clone()));
    let app = Router::new()
        .route("/healthz", get(system::healthz))
        .route("/readyz", get(system::readyz).with_state(readiness))
        .route("/api/admin/v1/system/info", get(system::healthz))
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

    // Signal every worker immediately, then await server drain and worker
    // cancellation together while PostgreSQL is still available for safe
    // lease/error bookkeeping.
    let (server_result, (), (), (), (), (), (), (), ()) = tokio::join!(
        server_handle,
        stop_worker(reconcile_handle),
        stop_worker(inspection_handle),
        stop_worker(normalization_handle),
        stop_worker(validation_handle),
        stop_worker(publication_handle),
        stop_catalog_worker(catalog_handle),
        stop_policy_worker(policy_handle),
        stop_search_worker(search_handle),
    );
    server_result??;
    pool.close().await;

    Ok(())
}

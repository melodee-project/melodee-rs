use std::{env, error::Error, net::SocketAddr, sync::Arc};

use axum::{Router, routing::get};
use melodee_catalog::{CatalogConfig, CatalogSchedulerHandle, start_catalog_loop};
use melodee_ingestion::{
    IngestionConfig, SchedulerHandle, start_inspection_loop, start_normalization_loop,
    start_publication_loop, start_reconciliation_loop, start_validation_loop,
};
use melodee_persistence::{CatalogStore, WorkEngineConfig, WorkItemStore, migrate};
use melodee_policy::PolicyConfig;
use melodee_search::{SearchConfig, SearchRoot, start_search_loop};
use melodee_server::discovery_worker::{DiscoveryWorkerConfig, start_discovery_loop};
use melodee_server::policy_worker::{PolicyWorkerConfig, start_policy_loop};
use melodee_server::{
    admin, discovery_worker, jellyfin, native, native_stream, opensubsonic, podcast_worker,
    policy_worker, system,
};
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

async fn stop_podcast_worker(handle: Option<podcast_worker::PodcastSchedulerHandle>) {
    if let Some(handle) = handle {
        handle.stop().await;
    }
}

async fn stop_discovery_worker(handle: Option<discovery_worker::DiscoverySchedulerHandle>) {
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

const ENV_NATIVE_PUBLIC_ORIGIN: &str = "MELODEE__NATIVE__PUBLIC_ORIGIN";
const ENV_NATIVE_STREAM_CONCURRENCY: &str = "MELODEE__NATIVE__STREAM_CONCURRENCY";
const DEFAULT_NATIVE_STREAM_CONCURRENCY: usize = 32;
const MAX_NATIVE_STREAM_CONCURRENCY: usize = 4096;

fn native_public_origin(address: SocketAddr) -> Result<String, &'static str> {
    let value = env::var(ENV_NATIVE_PUBLIC_ORIGIN).unwrap_or_else(|_| format!("http://{address}"));
    validate_public_origin(&value)?;
    Ok(value.trim_end_matches('/').to_string())
}

fn validate_public_origin(value: &str) -> Result<(), &'static str> {
    let uri = value
        .parse::<axum::http::Uri>()
        .map_err(|_| "native public origin must be an absolute HTTP(S) origin")?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none()
        || uri.path() != "/"
        || uri.query().is_some()
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
    {
        return Err(
            "native public origin must be an absolute HTTP(S) origin without credentials, path, or query",
        );
    }
    Ok(())
}

fn parse_native_stream_concurrency(value: &str) -> Result<usize, &'static str> {
    let concurrency = value
        .parse::<usize>()
        .map_err(|_| "MELODEE__NATIVE__STREAM_CONCURRENCY must be an integer between 1 and 4096")?;
    if !(1..=MAX_NATIVE_STREAM_CONCURRENCY).contains(&concurrency) {
        return Err("MELODEE__NATIVE__STREAM_CONCURRENCY must be between 1 and 4096");
    }
    Ok(concurrency)
}

/// Jellyfin UserViews mirror the captured "unlocked Storage libraries" set.
/// The Rust catalog has no library table, so views are configuration-derived:
/// the canonical Storage view plus one view per ingestion source (the same
/// library set the OpenSubsonic adapter exposes as music folders).
fn jellyfin_libraries(
    ingestion_config: &IngestionConfig,
) -> Vec<std::sync::Arc<jellyfin::JfLibrary>> {
    let mut libraries: Vec<std::sync::Arc<jellyfin::JfLibrary>> = Vec::new();
    // The captured "Storage" library view has the Unix-epoch DateCreated quirk.
    let epoch = time::OffsetDateTime::UNIX_EPOCH;
    libraries.push(std::sync::Arc::new(jellyfin::JfLibrary {
        id: deterministic_jellyfin_library_id("storage"),
        name: "Storage".to_string(),
        created_at: epoch,
    }));
    let now = time::OffsetDateTime::now_utc();
    for source in &ingestion_config.sources {
        libraries.push(std::sync::Arc::new(jellyfin::JfLibrary {
            id: source
                .id
                .unwrap_or_else(|| deterministic_jellyfin_library_id(&source.name)),
            name: source.name.clone(),
            created_at: now,
        }));
    }
    // Captured UserViews order: SortOrder then Name (SortOrder is not
    // modeled; name ordering is the deterministic equivalent).
    libraries.sort_by(|left, right| left.name.cmp(&right.name));
    libraries
}

/// Deterministic per-name library id (stable across restarts so UserView ids
/// and their ETags do not churn).
fn deterministic_jellyfin_library_id(name: &str) -> uuid::Uuid {
    use md5::{Digest, Md5};
    let digest = Md5::digest(format!("melodee-jellyfin-library:{name}").as_bytes());
    uuid::Uuid::from_bytes(digest[..16].try_into().unwrap_or([0u8; 16]))
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
    let podcast_config = podcast_worker::PodcastConfig::from_env()?;
    let policy_config = PolicyConfig::from_env()?;
    let policy_worker_config = PolicyWorkerConfig::from_env()?;
    let discovery_worker_config = DiscoveryWorkerConfig::from_env()?;
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
    if podcast_config.enabled && podcast_config.worker_enabled {
        let mut other_roots = ingestion_config
            .sources
            .iter()
            .map(|source| source.root.clone())
            .collect::<Vec<_>>();
        other_roots.push(ingestion_config.work_area_base.clone());
        if let Some(roots) = &publication_roots {
            other_roots.push(roots.storage_root.clone());
        }
        if search_config.enabled {
            other_roots.push(search_config.search_root.clone());
        }
        podcast_config.prepare_owned_root(&other_roots)?;
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
        match &publication_roots {
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
    let public_origin = native_public_origin(address)?;
    let native_auth = Arc::new(native::auth::NativeAuthService::from_env()?);
    let native_cors = native::auth::cors_from_env()?;
    let trusted_proxies = env::var("MELODEE__NATIVE__TRUSTED_PROXIES")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let stream_concurrency = env::var(ENV_NATIVE_STREAM_CONCURRENCY)
        .map_or_else(
            |_| Ok(DEFAULT_NATIVE_STREAM_CONCURRENCY),
            |value| parse_native_stream_concurrency(&value),
        )
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error.into() })?;
    let direct_play = publication_roots
        .as_ref()
        .map(|roots| native_stream::CanonicalAssetRoot::open(&roots.storage_root))
        .transpose()?
        .and_then(|root| native_stream::DirectPlayService::new(root, stream_concurrency));
    let opensubsonic_state = opensubsonic::OsState::from_env(
        pool.clone(),
        public_origin.clone(),
        &ingestion_config,
        podcast_config.enabled,
        direct_play.clone(),
        native_stream::ClientAddressResolver::new(trusted_proxies.clone()),
        search_config.enabled.then_some(search_config.clone()),
    )?;
    let opensubsonic_app = opensubsonic::configured_router(opensubsonic_state);
    let jellyfin_state = jellyfin::JfState {
        pool: pool.clone(),
        server_name: "Melodee".to_string(),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        server_id: "93cd3cc3a120127ccfda124620290cae".to_string(),
        local_address: public_origin.clone(),
        public_origin: public_origin.clone(),
        token_pepper: jellyfin::SecretPepper::new(
            std::env::var("MELODEE__JELLYFIN__TOKEN_PEPPER")
                .unwrap_or_else(|_| {
                    "melodee-jellyfin-fixture-pepper-NOT-A-REAL-SECRET-32b!".to_string()
                })
                .into_bytes(),
        ),
        token_expiry_hours: 168,
        max_active_tokens: 10,
        direct_play: direct_play.clone(),
        client_addresses: native_stream::ClientAddressResolver::new(
            env::var("MELODEE__NATIVE__TRUSTED_PROXIES")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::parse)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        search: search_config.enabled.then_some(search_config.clone()),
        libraries: jellyfin_libraries(&ingestion_config),
    };
    let jellyfin_app = jellyfin::configured_router(jellyfin_state);
    let native_state = native::NativeState {
        pool: pool.clone(),
        search: search_config.enabled.then_some(search_config.clone()),
        public_origin: public_origin.clone(),
        podcast_config: Arc::new(podcast_config.clone()),
        podcast_discovery: native::podcasts::PodcastDiscoveryClient::new(
            &podcast_config.discovery_endpoint,
        )?,
        stream_url_signer: Arc::new(native::stream::DatabaseStreamUrlSigner::new(
            pool.clone(),
            public_origin,
        )),
        user_state: Arc::new(native::PostgresNativeUserStateReader::new(pool.clone())),
        direct_play,
        client_addresses: native_stream::ClientAddressResolver::new(trusted_proxies),
        auth: native_auth.clone(),
    };
    let native_app = native::configured_router(native_state).layer(native_cors);
    let podcast_handle = podcast_worker::start_podcast_loop(pool.clone(), podcast_config);
    let discovery_handle = start_discovery_loop(discovery_worker_config, pool.clone());

    let admin_state = admin::AdminState {
        pool: pool.clone(),
        store: WorkItemStore,
    };
    let admin_access_state = melodee_server::admin_access::AdminAccessState {
        pool: pool.clone(),
        auth: native_auth,
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
        .merge(melodee_server::admin_access::router().with_state(admin_access_state))
        .merge(native_app)
        .merge(opensubsonic_app)
        .merge(jellyfin_app)
        // The Jellyfin routing middleware must see unmatched paths (the
        // bare-root probe), so it is an outer layer, matching the captured
        // ASP.NET pipeline placement (ADR-0131).
        .layer(axum::middleware::from_fn(jellyfin::routing_middleware))
        .layer(TraceLayer::new_for_http().make_span_with(melodee_server::make_http_span));

    // Case-insensitive Jellyfin route matching (legacy ASP.NET matched route
    // templates case-insensitively; real clients like Feishin call lowercase
    // paths). Route-local layers never see unmatched paths, so the outer
    // fallback canonicalizes Jellyfin-owned roots and replays; everything
    // else keeps the captured empty-404 behavior.
    let replay_app = app.clone();
    let app = app.fallback_service(jellyfin::case_rewriting_fallback(replay_app));

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let listener = tokio::net::TcpListener::bind(address).await?;
    let server_handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
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
    let (server_result, (), (), (), (), (), (), (), (), (), ()) = tokio::join!(
        server_handle,
        stop_worker(reconcile_handle),
        stop_worker(inspection_handle),
        stop_worker(normalization_handle),
        stop_worker(validation_handle),
        stop_worker(publication_handle),
        stop_catalog_worker(catalog_handle),
        stop_policy_worker(policy_handle),
        stop_search_worker(search_handle),
        stop_podcast_worker(podcast_handle),
        stop_discovery_worker(discovery_handle),
    );
    server_result??;
    pool.close().await;

    Ok(())
}

#[cfg(test)]
mod native_configuration_tests {
    use super::{parse_native_stream_concurrency, validate_public_origin};

    #[test]
    fn native_stream_concurrency_is_strictly_bounded() {
        assert_eq!(parse_native_stream_concurrency("1"), Ok(1));
        assert_eq!(parse_native_stream_concurrency("4096"), Ok(4096));
        assert!(parse_native_stream_concurrency("0").is_err());
        assert!(parse_native_stream_concurrency("4097").is_err());
        assert!(parse_native_stream_concurrency(" 32").is_err());
        assert!(parse_native_stream_concurrency("many").is_err());
    }

    #[test]
    fn native_public_origin_rejects_ambiguous_or_credentialed_urls() {
        assert!(validate_public_origin("https://music.example.test").is_ok());
        assert!(validate_public_origin("http://127.0.0.1:8080").is_ok());
        assert!(validate_public_origin("music.example.test").is_err());
        assert!(validate_public_origin("https://user:secret@example.test").is_err());
        assert!(validate_public_origin("https://example.test/api").is_err());
        assert!(validate_public_origin("https://example.test/?query=1").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_public_origin_accepts_only_absolute_http_origins() {
        for valid in [
            "https://music.example",
            "http://127.0.0.1:8080",
            "https://music.example/",
        ] {
            assert!(validate_public_origin(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "music.example",
            "ftp://music.example",
            "https://user:secret@music.example",
            "https://music.example/api",
            "https://music.example?tenant=a",
        ] {
            assert!(validate_public_origin(invalid).is_err(), "{invalid}");
        }
    }
}

use std::{env, error::Error, net::SocketAddr, sync::Arc, time::Duration};

use axum::{Router, routing::get};
use melodee_catalog::{CatalogConfig, CatalogSchedulerHandle, start_catalog_loop};
use melodee_ingestion::{
    IngestionConfig, SchedulerHandle, start_inspection_loop, start_normalization_loop,
    start_publication_loop, start_reconciliation_loop, start_validation_loop,
};
use melodee_persistence::{CatalogStore, DatabasePoolConfig, WorkEngineConfig, migrate};
use melodee_policy::PolicyConfig;
use melodee_search::{SearchConfig, SearchRoot, start_search_loop};
use melodee_server::discovery_worker::{DiscoveryWorkerConfig, start_discovery_loop};
use melodee_server::enrichment_worker::{EnrichmentWorkerConfig, start_enrichment_loop};
use melodee_server::policy_worker::{PolicyWorkerConfig, start_policy_loop};
use melodee_server::{
    admin, admin_ui, discovery_worker, enrichment_worker, jellyfin, native, native_stream,
    opensubsonic, password_reset, podcast_worker, policy_worker, system,
};
use tokio::sync::oneshot;
use tower_http::trace::TraceLayer;
use tracing::info;

mod logging;

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    tracing::error!(%error, "unable to install SIGTERM signal handler");
                    return;
                }
            };

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::error!(%error, "unable to install CTRL+C signal handler");
                }
            }
            _ = terminate.recv() => {
                tracing::info!("received SIGTERM; beginning graceful shutdown");
            }
        }
    }

    #[cfg(not(unix))]
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

async fn stop_enrichment_worker(handle: Option<enrichment_worker::EnrichmentSchedulerHandle>) {
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
const ENV_SHUTDOWN_GRACE_SECONDS: &str = "MELODEE__SHUTDOWN__GRACE_SECONDS";
const DEFAULT_NATIVE_STREAM_CONCURRENCY: usize = 32;
const MAX_NATIVE_STREAM_CONCURRENCY: usize = 4096;
const DEFAULT_SHUTDOWN_GRACE_SECONDS: u64 = 30;
const MAX_SHUTDOWN_GRACE_SECONDS: u64 = 600;

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

fn parse_shutdown_grace_seconds(value: &str) -> Result<u64, &'static str> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| "MELODEE__SHUTDOWN__GRACE_SECONDS must be an integer between 1 and 600")?;
    if !(1..=MAX_SHUTDOWN_GRACE_SECONDS).contains(&seconds) {
        return Err("MELODEE__SHUTDOWN__GRACE_SECONDS must be between 1 and 600");
    }
    Ok(seconds)
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

/// Reads a boolean setting value with the documented default.
fn bool_setting_value(value: Option<&serde_json::Value>, default: bool) -> bool {
    value
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

/// `system.siteName` with the legacy default and empty-value fallback.
fn site_name_value(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Melodee")
        .to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _ = dotenvy::dotenv();
    logging::init()?;

    // install the process bounded-resource registry before any
    // instrumented service is constructed (several resolve the global
    // registry at construction). Purely observational; admission behavior
    // is unchanged.
    let resource_bounds = melodee_server::resource_bounds::ResourceBounds::new();
    melodee_server::resource_bounds::ResourceBounds::install_global(resource_bounds.clone());

    let database_url = database_url();
    let db_pool_config = DatabasePoolConfig::from_env()?;
    let pool = db_pool_config.pool_options().connect(&database_url).await?;
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
    let enrichment_worker_config = EnrichmentWorkerConfig::from_env()?;
    // Validate all configured storage roles even when canonical publication is
    // disabled. The inbound directory must never overlap staging, work, or
    // canonical roots.
    let storage_roots = ingestion_config.storage_roots()?;
    let publication_roots = ingestion_config.publication_roots()?;
    let mut search_config = SearchConfig::from_env()
        .map_err(|error| format!("invalid search configuration: {error}"))?;
    if search_config.enabled {
        let mut other_roots = ingestion_config
            .sources
            .iter()
            .map(|source| source.root.clone())
            .collect::<Vec<_>>();
        if let Some(staging_root) = &storage_roots.staging_root {
            other_roots.push(staging_root.clone());
        }
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
        if let Some(staging_root) = &storage_roots.staging_root {
            other_roots.push(staging_root.clone());
        }
        other_roots.push(ingestion_config.work_area_base.clone());
        if let Some(roots) = &publication_roots {
            other_roots.push(roots.storage_root.clone());
        }
        if search_config.enabled {
            other_roots.push(search_config.search_root.clone());
        }
        podcast_config.prepare_owned_root(&other_roots)?;
    }
    // sandboxed scripting hooks (ADR-0135): one shared service so the
    // process-wide leaked-thread bound stays accurate across the ingestion
    // gate and the admin test-run surface.
    let scripting = Arc::new(melodee_server::scripting::ScriptingService::new(
        pool.clone(),
    ));

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
            Some(Arc::new(
                melodee_server::scripting::ScriptingDirectoryGate::new(Arc::clone(&scripting)),
            )),
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

    // notification render/delivery service over layered settings resolution
    // settings (PRD-CONFIG-003): MELODEE__EMAIL__* env keys and the optional
    // config file win over DB-backed server_settings rows, which win over
    // the legacy seeded defaults. Delivery is fail-degraded and audited
    // (PRD-NOTIF-002).
    let settings_resolver = melodee_persistence::SettingsResolver::from_env(&pool)
        .map_err(|error| format!("invalid settings configuration: {error}"))?;
    let email_settings =
        melodee_server::notifications::EmailSettings::from_resolved(&settings_resolver)
            .await
            .map_err(|error| format!("invalid email configuration: {error}"))?;
    let notifications = Arc::new(
        melodee_server::notifications::NotificationService::new(pool.clone(), &email_settings)
            .map_err(|error| format!("invalid email configuration: {error}"))?,
    );

    let bind = system::configured_bind();
    let address: SocketAddr = bind.parse()?;
    let public_origin = native_public_origin(address)?;
    // Operator security policy resolved from the layered settings store:
    // password-reset token lifetime (legacy `UserPasswordResetService`) and
    // the configuration deny lists (legacy `BlacklistService`). Both were
    // registered in the Rust settings registry but previously had no runtime
    // consumer.
    let password_reset_ttl_minutes = {
        let resolved = settings_resolver
            .resolve("security.passwordResetTokenExpiryMinutes")
            .await
            .map_err(|error| format!("invalid security configuration: {error}"))?;
        resolved
            .value
            .as_ref()
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(60)
            .max(1)
    };
    let blacklist = Arc::new(melodee_server::security::BlacklistCache::new(pool.clone()));
    let native_auth = Arc::new(
        native::auth::NativeAuthService::from_env()?
            .with_security_settings(password_reset_ttl_minutes, blacklist.clone()),
    );
    let native_cors = native::auth::cors_from_env()?;
    let trusted_proxies = native_stream::configured_trusted_proxies(
        env::var("MELODEE__SERVER__TRUSTED_PROXIES").ok().as_deref(),
        env::var("MELODEE__NATIVE__TRUSTED_PROXIES").ok().as_deref(),
    )?;
    // Direct-play admission (`streaming.maxConcurrentStreams.global` /
    // `.perUser`, legacy `StreamingLimiter`). Legacy `0` means unlimited; the
    // Rust process keeps a bounded safety default instead of an unbounded
    // semaphore, and the environment override
    // (`MELODEE__NATIVE__STREAM_CONCURRENCY`) remains available for operators
    // who need a process-level cap below the setting.
    let env_stream_concurrency = env::var(ENV_NATIVE_STREAM_CONCURRENCY)
        .ok()
        .map(|value| parse_native_stream_concurrency(&value))
        .transpose()
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error.into() })?;
    let global_stream_setting = settings_resolver
        .resolve("streaming.maxConcurrentStreams.global")
        .await
        .map_err(|error| format!("invalid streaming configuration: {error}"))?
        .value
        .as_ref()
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let stream_concurrency = if global_stream_setting > 0 {
        usize::try_from(global_stream_setting)
            .map_err(|_| "streaming.maxConcurrentStreams.global exceeds platform usize")?
            .clamp(1, MAX_NATIVE_STREAM_CONCURRENCY)
    } else {
        env_stream_concurrency.unwrap_or(DEFAULT_NATIVE_STREAM_CONCURRENCY)
    };
    let per_user_stream_setting = settings_resolver
        .resolve("streaming.maxConcurrentStreams.perUser")
        .await
        .map_err(|error| format!("invalid streaming configuration: {error}"))?
        .value
        .as_ref()
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let per_user_stream_limit = if per_user_stream_setting > 0 {
        usize::try_from(per_user_stream_setting)
            .map_err(|_| "streaming.maxConcurrentStreams.perUser exceeds platform usize")?
            .clamp(1, MAX_NATIVE_STREAM_CONCURRENCY)
    } else {
        0
    };
    info!(
        global_stream_cap = stream_concurrency,
        per_user_stream_cap = per_user_stream_limit,
        "direct-play admission configured"
    );
    let shutdown_grace_seconds = env::var(ENV_SHUTDOWN_GRACE_SECONDS)
        .map_or_else(
            |_| Ok(DEFAULT_SHUTDOWN_GRACE_SECONDS),
            |value| parse_shutdown_grace_seconds(&value),
        )
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error.into() })?;
    let direct_play = publication_roots
        .as_ref()
        .map(|roots| native_stream::CanonicalAssetRoot::open(&roots.storage_root))
        .transpose()?
        .and_then(|root| {
            native_stream::DirectPlayService::new(root, stream_concurrency)
                .map(|service| service.with_per_user_limit(per_user_stream_limit))
        })
        .map(|service| {
            service.with_gauge(
                resource_bounds.register("streams.direct_play", stream_concurrency.max(1)),
            )
        });
    // ADR-0147 decision 5: transcoding capacity is bounded independently of
    // direct-play concurrency; 0 disables transcoding (fail closed on
    // transcode requests, direct play unaffected).
    let transcode_concurrency = {
        let resolved = settings_resolver
            .resolve("transcoding.maxConcurrent")
            .await
            .map_err(|error| format!("invalid transcoding configuration: {error}"))?;
        match resolved.value.as_ref().and_then(serde_json::Value::as_u64) {
            Some(value) => usize::try_from(value)
                .map_err(|_| "transcoding.maxConcurrent exceeds platform usize")?,
            None => 4,
        }
    };
    let transcoding = publication_roots
        .as_ref()
        .map(|roots| native_stream::CanonicalAssetRoot::open(&roots.storage_root))
        .transpose()?
        .and_then(|root| {
            melodee_server::transcode::TranscodingService::new(
                root,
                pool.clone(),
                transcode_concurrency,
            )
        })
        .map(|service| {
            service.with_gauge(
                resource_bounds.register("streams.transcoding", transcode_concurrency.max(1)),
            )
        });
    let direct_play_shutdown = direct_play.clone();
    let transcoding_shutdown = transcoding.clone();
    // Compatibility-surface settings (legacy `jellyfin.enabled`,
    // `system.siteName`, `system.isDownloadingEnabled`). Defaults preserve the
    // captured behavior: Jellyfin enabled, "Melodee" server name, downloads on.
    let jellyfin_enabled = bool_setting_value(
        settings_resolver
            .resolve("jellyfin.enabled")
            .await
            .map_err(|error| format!("invalid jellyfin configuration: {error}"))?
            .value
            .as_ref(),
        true,
    );
    let server_site_name = site_name_value(
        settings_resolver
            .resolve("system.siteName")
            .await
            .map_err(|error| format!("invalid system configuration: {error}"))?
            .value
            .as_ref(),
    );
    let downloads_enabled_setting = settings_resolver
        .resolve("system.isDownloadingEnabled")
        .await
        .map_err(|error| format!("invalid system configuration: {error}"))?
        .value
        .as_ref()
        .and_then(serde_json::Value::as_bool);
    if !jellyfin_enabled {
        info!("jellyfin compatibility routes disabled by settings (jellyfin.enabled=false)");
    }

    let mut opensubsonic_state = opensubsonic::OsState::from_env(
        pool.clone(),
        public_origin.clone(),
        &ingestion_config,
        podcast_config.enabled,
        direct_play.clone(),
        transcoding.clone(),
        native_stream::ClientAddressResolver::new(trusted_proxies.clone()),
        search_config.enabled.then_some(search_config.clone()),
    )?;
    if opensubsonic_state.legacy_auth_bypass {
        tracing::warn!(
            "legacy OpenSubsonic authentication bypass enabled: matching-host and loopback requests can access the catalog without credentials; do not expose through a public proxy"
        );
    }
    if let Some(enabled) = downloads_enabled_setting {
        opensubsonic_state.downloads_enabled = enabled;
    }
    let opensubsonic_app = opensubsonic::configured_router_with_cors(
        opensubsonic_state,
        &env::var(opensubsonic::cors::ENV_ALLOWED_ORIGINS).unwrap_or_default(),
    )?;
    let jellyfin_state = jellyfin::JfState {
        pool: pool.clone(),
        server_name: server_site_name.clone(),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        server_id: melodee_persistence::installation_id(&pool)
            .await?
            .simple()
            .to_string(),
        local_address: public_origin.clone(),
        public_origin: public_origin.clone(),
        token_pepper: jellyfin::token_pepper_from_env()?,
        token_expiry_hours: jellyfin::token_expiry_hours_from_env()?,
        max_active_tokens: 10,
        direct_play: direct_play.clone(),
        transcoding: transcoding.clone(),
        client_addresses: native_stream::ClientAddressResolver::new(trusted_proxies.clone()),
        search: search_config.enabled.then_some(search_config.clone()),
        libraries: jellyfin_libraries(&ingestion_config),
        lastfm_api_base_url: None,
    };
    let jellyfin_app = jellyfin::configured_router(jellyfin_state);
    let native_state = native::NativeState {
        pool: pool.clone(),
        notifications: notifications.clone(),
        search: search_config.enabled.then_some(search_config.clone()),
        public_origin: public_origin.clone(),
        podcast_config: Arc::new(podcast_config.clone()),
        podcast_discovery: native::podcasts::PodcastDiscoveryClient::new(
            &podcast_config.discovery_endpoint,
        )?,
        stream_url_signer: Arc::new(native::stream::DatabaseStreamUrlSigner::new(
            pool.clone(),
            public_origin.clone(),
        )),
        user_state: Arc::new(native::PostgresNativeUserStateReader::new(pool.clone())),
        direct_play: direct_play.clone(),
        client_addresses: native_stream::ClientAddressResolver::new(trusted_proxies.clone()),
        auth: native_auth.clone(),
        lastfm_api_base_url: None,
    };
    let native_app = native::configured_router(native_state).layer(native_cors);
    let podcast_handle = podcast_worker::start_podcast_loop(pool.clone(), podcast_config);
    let discovery_handle = start_discovery_loop(discovery_worker_config, pool.clone());
    let enrichment_handle = start_enrichment_loop(enrichment_worker_config, pool.clone());

    let admin_access_state = melodee_server::admin_access::AdminAccessState {
        pool: pool.clone(),
        auth: native_auth,
        notifications: notifications.clone(),
        scripting: scripting.clone(),
    };
    // review previews use Melodee-owned staging, never the transient inbound
    // directory. Containment is re-validated per request.
    let admin_review_state = melodee_server::admin_review::AdminReviewState::new(
        admin_access_state.clone(),
        storage_roots
            .staging_root
            .clone()
            .map(|root| vec![melodee_server::admin_review::PreviewSourceRoot { id: None, root }])
            .unwrap_or_default(),
        ingestion_config.work_area_base.clone(),
        storage_roots.storage_root.clone(),
    );
    // dashboard + system/workers/search surface: the configured
    // search projection handle lets health/status/rebuild report honestly
    // (search disabled → explicit `disabled`/`search_disabled`, never an
    // invented failure).
    let admin_system_state = melodee_server::admin_system::AdminSystemState::new(
        admin_access_state.clone(),
        search_config.enabled.then_some(search_config.clone()),
    )
    .with_resource_bounds(resource_bounds.clone());
    // catalog surface: the shared read-only signer issues admin preview
    // preview URLs on the frozen native stream route (PRD-UI-007); the
    // resolver mirrors the native trusted-proxy rules so issued tokens
    // validate for the requesting browser.
    let admin_purge_state = melodee_server::admin_purge::AdminPurgeState::new(
        admin_access_state.clone(),
        ingestion_config
            .sources
            .iter()
            .map(|source| melodee_server::admin_purge::PurgeSourceRoot {
                id: source.id,
                root: source.root.clone(),
            })
            .collect(),
        storage_roots.staging_root.clone(),
        ingestion_config.work_area_base.clone(),
        storage_roots.storage_root.clone(),
    );
    let admin_catalog_state = melodee_server::admin_catalog::AdminCatalogState::new(
        admin_access_state.clone(),
        Arc::new(native::stream::DatabaseStreamUrlSigner::new(
            pool.clone(),
            public_origin,
        )),
        native_stream::ClientAddressResolver::new(trusted_proxies),
    );
    let diagnostics_started_at = time::OffsetDateTime::now_utc();
    let admin_diagnostics_state = melodee_server::admin_diagnostics::AdminDiagnosticsState {
        admin: admin_access_state.clone(),
        search: search_config.enabled.then_some(search_config.clone()),
        direct_play: direct_play.clone(),
        global_stream_cap: stream_concurrency,
        per_user_stream_cap: per_user_stream_limit,
        started_at: diagnostics_started_at,
    };
    let readiness = system::ReadinessState::new(pool.clone())
        .with_search(search_config.enabled.then_some(search_config.clone()));
    let app = Router::new()
        .route("/healthz", get(system::healthz))
        .route("/readyz", get(system::readyz).with_state(readiness))
        // Admin-gated diagnostics (ADR-0139): every /api/admin/v1 route
        // requires AdminPrincipal; /healthz and /readyz stay anonymous.
        .merge(admin::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_access::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_radio::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_device_profiles::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_notifications::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_scripts::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_config::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_review::router().with_state(admin_review_state))
        .merge(melodee_server::admin_identity::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_duplicates::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_release_identity::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_purge::router().with_state(admin_purge_state))
        .merge(melodee_server::admin_metadata::router(ingestion_config.clone()).with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_catalog::router().with_state(admin_catalog_state))
        .merge(melodee_server::admin_charts::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_policies::router().with_state(admin_access_state.clone()))
        .merge(melodee_server::admin_system::router().with_state(admin_system_state.clone()))
        .merge(melodee_server::admin_stats::router().with_state(admin_system_state))
        .merge(melodee_server::admin_diagnostics::router().with_state(admin_diagnostics_state))
        .merge(melodee_server::admin_onboarding::router().with_state(admin_access_state))
        // ADR-0161 permits this one anonymous browser security page; it is
        // intentionally not a listener-facing web application.
        .merge(password_reset::router())
        .merge(native_app)
        .merge(opensubsonic_app);
    let app = if jellyfin_enabled {
        app.merge(jellyfin_app)
    } else {
        app
    };
    // Admin SPA static serving (ADR-0138): built assets at /admin with an
    // index.html fallback for client-side deep links. A missing dist is not
    // fatal — the APIs keep serving and /admin falls through to the 404
    // fallback.
    let app = admin_ui::nest_admin_spa(app, &admin_ui::configured_dist());
    let app = app
        // The Jellyfin routing middleware must see unmatched paths (the
        // bare-root probe), so it is an outer layer, matching the captured
        // ASP.NET pipeline placement (ADR-0131).
        .layer(axum::middleware::from_fn(
            melodee_server::diagnostics::request_logging,
        ))
        .layer(axum::middleware::from_fn(jellyfin::routing_middleware))
        // correlation sourcing runs inside the TraceLayer span (added
        // first, so the layer wraps it) so the span recording lands.
        .layer(axum::middleware::from_fn(melodee_server::correlation::middleware))
        .layer(axum::middleware::from_fn(
            melodee_server::security_headers::middleware,
        ))
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
    if let Some(service) = &direct_play_shutdown {
        service.close_admission();
    }
    if let Some(service) = &transcoding_shutdown {
        service.close_admission();
    }
    let _ = shutdown_tx.send(());

    // Signal every worker immediately, then await server drain and worker
    // cancellation together while PostgreSQL is still available for safe
    // lease/error bookkeeping.
    let drain = async move {
        let (server_result, (), (), (), (), (), (), (), (), (), (), ()) = tokio::join!(
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
            stop_enrichment_worker(enrichment_handle),
        );
        server_result??;
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    };
    match tokio::time::timeout(Duration::from_secs(shutdown_grace_seconds), drain).await {
        Ok(result) => result?,
        Err(_) => tracing::warn!(
            shutdown_grace_seconds,
            "graceful shutdown deadline expired; terminating remaining work"
        ),
    }
    if tokio::time::timeout(Duration::from_secs(5), pool.close())
        .await
        .is_err()
    {
        tracing::warn!("postgresql pool close deadline expired");
    }

    Ok(())
}

#[cfg(test)]
mod native_configuration_tests {
    use super::{
        bool_setting_value, parse_native_stream_concurrency, parse_shutdown_grace_seconds,
        site_name_value, validate_public_origin,
    };

    #[test]
    fn compatibility_surface_settings_have_documented_defaults() {
        assert!(bool_setting_value(None, true));
        assert!(!bool_setting_value(None, false));
        assert!(!bool_setting_value(Some(&serde_json::json!(false)), true));
        assert!(bool_setting_value(Some(&serde_json::json!(true)), false));
        assert_eq!(site_name_value(None), "Melodee");
        assert_eq!(site_name_value(Some(&serde_json::json!("  "))), "Melodee");
        assert_eq!(
            site_name_value(Some(&serde_json::json!("  Home Melodee  "))),
            "Home Melodee"
        );
    }

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
    fn shutdown_grace_is_strictly_bounded() {
        assert_eq!(parse_shutdown_grace_seconds("1"), Ok(1));
        assert_eq!(parse_shutdown_grace_seconds("600"), Ok(600));
        assert!(parse_shutdown_grace_seconds("0").is_err());
        assert!(parse_shutdown_grace_seconds("601").is_err());
        assert!(parse_shutdown_grace_seconds(" 30").is_err());
        assert!(parse_shutdown_grace_seconds("forever").is_err());
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

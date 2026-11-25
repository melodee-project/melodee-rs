//! Durable, fail-degraded external-provider enrichment worker (ADR-0162).

use melodee_persistence::runtime_settings::var as config_var;
use std::{fmt, time::Duration};

use futures_util::{StreamExt, stream::FuturesUnordered};
use melodee_persistence::{
    ClaimedEnrichmentTask, DEFAULT_ENRICHMENT_LEASE_SECONDS, EnrichmentEntityType,
    EnrichmentOutcome, ProviderArtworkStore, ProviderCacheStore, ProviderEnrichmentStore,
    ProviderEnrichmentStoreError, ProviderIdentityCandidateObservation,
    ProviderIdentityCandidateStore,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::{sync::watch, task::JoinHandle, time::MissedTickBehavior};
use tracing::{Instrument, error, info, info_span, warn};

use crate::providers::{ProviderError, ProviderRuntime};

pub const ENV_ENRICHMENT_WORKER_ENABLED: &str = "MELODEE__ENRICHMENT__WORKER__ENABLED";
pub const ENV_ENRICHMENT_WORKER_POLL_INTERVAL_SECONDS: &str =
    "MELODEE__ENRICHMENT__WORKER__POLL_INTERVAL_SECONDS";
pub const ENV_ENRICHMENT_WORKER_BATCH_SIZE: &str = "MELODEE__ENRICHMENT__WORKER__BATCH_SIZE";
pub const ENV_ENRICHMENT_WORKER_LEASE_SECONDS: &str = "MELODEE__ENRICHMENT__WORKER__LEASE_SECONDS";

const DEFAULT_POLL_SECONDS: u64 = 15;
const DEFAULT_BATCH_SIZE: i64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrichmentWorkerConfig {
    pub enabled: bool,
    pub poll_interval: Duration,
    pub batch_size: i64,
    pub lease_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichmentWorkerConfigError {
    key: &'static str,
    value: String,
}

impl fmt::Display for EnrichmentWorkerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} has an invalid value '{}'",
            self.key, self.value
        )
    }
}

impl std::error::Error for EnrichmentWorkerConfigError {}

impl EnrichmentWorkerConfig {
    pub fn from_env() -> Result<Self, EnrichmentWorkerConfigError> {
        Ok(Self {
            enabled: env_bool(ENV_ENRICHMENT_WORKER_ENABLED, true)?,
            poll_interval: Duration::from_secs(env_u64(
                ENV_ENRICHMENT_WORKER_POLL_INTERVAL_SECONDS,
                DEFAULT_POLL_SECONDS,
                1,
                86_400,
            )?),
            batch_size: env_i64(ENV_ENRICHMENT_WORKER_BATCH_SIZE, DEFAULT_BATCH_SIZE, 1, 32)?,
            lease_seconds: env_i64(
                ENV_ENRICHMENT_WORKER_LEASE_SECONDS,
                DEFAULT_ENRICHMENT_LEASE_SECONDS,
                5,
                900,
            )?,
        })
    }
}

impl Default for EnrichmentWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::from_secs(DEFAULT_POLL_SECONDS),
            batch_size: DEFAULT_BATCH_SIZE,
            lease_seconds: DEFAULT_ENRICHMENT_LEASE_SECONDS,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EnrichmentWorkerStats {
    pub claimed: u64,
    pub succeeded: u64,
    pub cache_hits: u64,
    pub disabled: u64,
    pub retried: u64,
    pub failed: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum EnrichmentWorkerError {
    #[error("provider enrichment persistence failed")]
    Persistence(#[from] ProviderEnrichmentStoreError),
    #[error("provider enrichment database lookup failed")]
    Database(#[from] sqlx::Error),
    #[error("provider runtime construction failed")]
    Runtime(#[from] ProviderError),
}

/// Runs one bounded claim/process pass. A provider error is recorded against
/// its task and does not fail the catalog or abort other claimed tasks.
pub async fn run_enrichment_once(
    pool: &PgPool,
    config: &EnrichmentWorkerConfig,
    worker: &str,
) -> Result<EnrichmentWorkerStats, EnrichmentWorkerError> {
    let now = OffsetDateTime::now_utc();
    if let Err(error) = ProviderCacheStore::new().purge_expired(pool, now).await {
        warn!(%error, "expired external-provider cache cleanup failed; enrichment will continue");
    }
    let store = ProviderEnrichmentStore;
    let tasks = store
        .claim(pool, worker, config.batch_size, config.lease_seconds, now)
        .await?;
    let runtime = ProviderRuntime::new(pool.clone())?;
    let artwork = ProviderArtworkStore;
    let mut stats = EnrichmentWorkerStats {
        claimed: tasks.len() as u64,
        ..EnrichmentWorkerStats::default()
    };
    let mut pending = FuturesUnordered::new();
    for task in tasks {
        let task_pool = pool.clone();
        let task_runtime = runtime.clone();
        let task_store = store;
        let task_artwork = artwork;
        let task_worker = worker.to_owned();
        pending.push(async move {
            let result = process_task(
                task_pool,
                task_runtime,
                task_store,
                task_artwork,
                task_worker,
                task.clone(),
            )
            .await;
            (task, result)
        });
        if pending.len() >= 8
            && let Some((task, result)) = pending.next().await
        {
            record_task_result(pool, &store, worker, task, result, &mut stats).await;
        }
    }
    while let Some((task, result)) = pending.next().await {
        record_task_result(pool, &store, worker, task, result, &mut stats).await;
    }
    Ok(stats)
}

async fn record_task_result(
    pool: &PgPool,
    store: &ProviderEnrichmentStore,
    worker: &str,
    task: ClaimedEnrichmentTask,
    result: Result<EnrichmentOutcome, ProviderError>,
    stats: &mut EnrichmentWorkerStats,
) {
    match result {
        Ok(outcome) => match outcome {
            EnrichmentOutcome::CacheHit => stats.cache_hits += 1,
            EnrichmentOutcome::Disabled => stats.disabled += 1,
            _ => stats.succeeded += 1,
        },
        Err(error) => {
            let category = error.category;
            let message = error.message.clone();
            let retry_at = Some(
                OffsetDateTime::now_utc()
                    + time::Duration::seconds(backoff_seconds(task.attempt_count, category)),
            );
            warn!(
                task_id = %task.id,
                entity_type = %task.entity_type,
                entity_id = %task.entity_id,
                provider = %task.provider,
                purpose = %task.purpose,
                attempt = task.attempt_count,
                error_category = category,
                next_retry_at = ?retry_at,
                "provider enrichment attempt failed"
            );
            match store
                .fail_or_retry(
                    pool,
                    &task,
                    worker,
                    category,
                    &message,
                    retry_at,
                    Some(&task.request_key),
                    OffsetDateTime::now_utc(),
                )
                .await
            {
                Ok(()) if task.attempt_count < melodee_persistence::MAX_ENRICHMENT_ATTEMPTS => {
                    stats.retried += 1;
                }
                Ok(()) => stats.failed += 1,
                Err(lease_error) => {
                    warn!(%lease_error, task_id=%task.id, "enrichment task lease was lost while recording failure")
                }
            }
        }
    }
}

async fn process_task(
    pool: PgPool,
    runtime: ProviderRuntime,
    store: ProviderEnrichmentStore,
    artwork: ProviderArtworkStore,
    worker: String,
    task: ClaimedEnrichmentTask,
) -> Result<EnrichmentOutcome, ProviderError> {
    let capability = runtime
        .capabilities()
        .await?
        .into_iter()
        .find(|capability| capability.provider == task.provider);
    if capability.is_none_or(|capability| !capability.enabled) {
        store
            .complete(
                &pool,
                &task,
                &worker,
                EnrichmentOutcome::Disabled,
                Some(&task.request_key),
                OffsetDateTime::now_utc(),
            )
            .await
            .map_err(store_error)?;
        return Ok(EnrichmentOutcome::Disabled);
    }
    if task.purpose.ends_with("artwork")
        && has_administrator_override(&pool, &task)
            .await
            .map_err(database_error)?
    {
        store
            .complete(
                &pool,
                &task,
                &worker,
                EnrichmentOutcome::Success,
                Some(&task.request_key),
                OffsetDateTime::now_utc(),
            )
            .await
            .map_err(store_error)?;
        return Ok(EnrichmentOutcome::Success);
    }
    let query = entity_name(&pool, &task).await.map_err(database_error)?;
    let Some(query) = query else {
        store
            .complete(
                &pool,
                &task,
                &worker,
                EnrichmentOutcome::Success,
                Some(&task.request_key),
                OffsetDateTime::now_utc(),
            )
            .await
            .map_err(store_error)?;
        return Ok(EnrichmentOutcome::Success);
    };
    let (image_url, cache_hit, cache_key) = match task.entity_type.as_str() {
        "artist" => {
            let selected = std::iter::once(task.provider.clone()).collect();
            let result = runtime
                .lookup_artists(&query, 10, None, Some(&selected))
                .await?;
            if let Some(failure) = result
                .failed_providers
                .into_iter()
                .find(|failure| failure.provider == task.provider)
            {
                return Err(ProviderError {
                    category: "provider_lookup",
                    message: format!("{}: {}", failure.category, failure.message),
                });
            }
            let candidate_store = ProviderIdentityCandidateStore;
            let candidate_observations = result
                .candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let rank = i32::try_from(index + 1).map_err(|_| ProviderError {
                        category: "provider_response",
                        message: "provider candidate rank exceeded bounds".to_owned(),
                    })?;
                    let confidence = 7_500_i32.saturating_sub(rank.saturating_sub(1) * 500);
                    Ok(ProviderIdentityCandidateObservation {
                        external_id: candidate.external_id.clone(),
                        candidate_rank: rank,
                        confidence,
                        payload: serde_json::to_value(candidate).map_err(|error| {
                            ProviderError {
                                category: "provider_response",
                                message: error.to_string(),
                            }
                        })?,
                    })
                })
                .collect::<Result<Vec<_>, ProviderError>>()?;
            candidate_store
                .upsert_artist_candidates(
                    &pool,
                    task.entity_id,
                    &task.provider,
                    &query,
                    &candidate_observations,
                    OffsetDateTime::now_utc(),
                )
                .await
                .map_err(store_error)?;
            let image_url = result
                .candidates
                .into_iter()
                .find_map(|candidate| candidate.image_url.or(candidate.thumbnail_url));
            (image_url, result.cache_hit, task.request_key.clone())
        }
        "release" => {
            let result = runtime.enrich_release(&query, &task.provider, 10).await?;
            (
                extract_image_url(&result.payload),
                result.cache_hit,
                result.cache_key,
            )
        }
        _ => {
            return Err(ProviderError {
                category: "invalid_task",
                message: "provider enrichment task has an unknown entity type".to_owned(),
            });
        }
    };
    if task.purpose.ends_with("artwork")
        && let Some(url) = image_url
    {
        let (mime, bytes) = runtime.download_image(&task.provider, &url).await?;
        artwork
            .upsert(
                &pool,
                if task.entity_type == "artist" {
                    EnrichmentEntityType::Artist
                } else {
                    EnrichmentEntityType::Release
                },
                task.entity_id,
                &task.provider,
                &url,
                &mime,
                &bytes,
                7_500,
                OffsetDateTime::now_utc(),
            )
            .await
            .map_err(store_error)?;
    }
    let outcome = if cache_hit {
        EnrichmentOutcome::CacheHit
    } else {
        EnrichmentOutcome::Success
    };
    store
        .complete(
            &pool,
            &task,
            &worker,
            outcome,
            Some(&cache_key),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(store_error)?;
    Ok(outcome)
}

async fn entity_name(
    pool: &PgPool,
    task: &ClaimedEnrichmentTask,
) -> Result<Option<String>, sqlx::Error> {
    match task.entity_type.as_str() {
        "artist" => {
            sqlx::query_scalar(
                "SELECT canonical_name FROM artists
             WHERE id=$1 AND status NOT IN ('merged','deprecated')",
            )
            .bind(task.entity_id)
            .fetch_optional(pool)
            .await
        }
        "release" => {
            sqlx::query_scalar("SELECT title FROM releases WHERE id=$1 AND status='active'")
                .bind(task.entity_id)
                .fetch_optional(pool)
                .await
        }
        _ => Ok(None),
    }
}

async fn has_administrator_override(
    pool: &PgPool,
    task: &ClaimedEnrichmentTask,
) -> Result<bool, sqlx::Error> {
    match task.entity_type.as_str() {
        "artist" => {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM catalog_entity_images
                 WHERE entity_type='artist' AND artist_id=$1)",
            )
            .bind(task.entity_id)
            .fetch_one(pool)
            .await
        }
        "release" => {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM catalog_entity_images
                 WHERE entity_type='release' AND release_id=$1)",
            )
            .bind(task.entity_id)
            .fetch_one(pool)
            .await
        }
        _ => Ok(false),
    }
}

fn extract_image_url(value: &serde_json::Value) -> Option<String> {
    fn walk(value: &serde_json::Value, depth: usize) -> Option<String> {
        if depth > 6 {
            return None;
        }
        match value {
            serde_json::Value::Object(object) => {
                for key in [
                    "image_url",
                    "imageUrl",
                    "artworkUrl100",
                    "artworkUrl600",
                    "cover",
                    "coverArtArchiveUrl",
                    "cover_xl",
                    "cover_big",
                    "thumbnail",
                    "url",
                ] {
                    if let Some(url) = object
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
                    {
                        return Some(url.to_owned());
                    }
                }
                object.values().find_map(|value| walk(value, depth + 1))
            }
            serde_json::Value::Array(values) => {
                values.iter().find_map(|value| walk(value, depth + 1))
            }
            _ => None,
        }
    }
    walk(value, 0)
}

fn backoff_seconds(attempt: i32, category: &str) -> i64 {
    if category == "rate_limited" {
        return 60;
    }
    10_i64
        .saturating_mul(
            2_i64.saturating_pow(u32::try_from(attempt.saturating_sub(1)).unwrap_or(0).min(8)),
        )
        .min(3_600)
}

fn store_error(error: ProviderEnrichmentStoreError) -> ProviderError {
    ProviderError {
        category: "persistence",
        message: error.to_string(),
    }
}

fn database_error(error: sqlx::Error) -> ProviderError {
    ProviderError {
        category: "database",
        message: error.to_string(),
    }
}

#[derive(Debug)]
pub struct EnrichmentSchedulerHandle {
    stop: watch::Sender<bool>,
    join: JoinHandle<Option<EnrichmentWorkerStats>>,
}

impl EnrichmentSchedulerHandle {
    pub async fn stop(self) -> Option<EnrichmentWorkerStats> {
        let _ = self.stop.send(true);
        match self.join.await {
            Ok(stats) => stats,
            Err(error) => {
                error!(%error, "enrichment worker task failed during shutdown");
                None
            }
        }
    }
}

pub fn start_enrichment_loop(
    config: EnrichmentWorkerConfig,
    pool: PgPool,
) -> Option<EnrichmentSchedulerHandle> {
    if !config.enabled {
        info!("provider enrichment worker disabled by configuration");
        return None;
    }
    let (stop, mut task_stop) = watch::channel(false);
    let join = tokio::spawn(
        async move {
            let worker = format!("provider-enrichment-{}", std::process::id());
            let mut ticker = tokio::time::interval(config.poll_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            let mut latest = None;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let started = OffsetDateTime::now_utc();
                        let tick = melodee_persistence::scheduled_tick(started, config.poll_interval);
                        let history = melodee_persistence::JobRunHistoryStore;
                        let run = match history.start_tick(&pool, "enrichment", tick).await {
                            Ok(run) => run,
                            Err(error) => {
                                error!(%error, "enrichment tick history could not be recorded");
                                None
                            }
                        };
                        let Some(run) = run else {
                            continue;
                        };
                        match run_enrichment_once(&pool, &config, &worker).await {
                            Ok(stats) => {
                                let _ = history.finish(&pool, run.id, OffsetDateTime::now_utc(), true, None).await;
                                if stats.claimed > 0 {
                                    info!(?stats, "provider enrichment pass completed");
                                }
                                latest = Some(stats);
                            }
                            Err(error) => {
                                let message = error.to_string();
                                let _ = history.finish(&pool, run.id, OffsetDateTime::now_utc(), false, Some(&message)).await;
                                error!(%error, "provider enrichment pass failed");
                            }
                        }
                    }
                    _ = task_stop.changed() => break,
                }
            }
            latest
        }
        .instrument(info_span!("provider_enrichment_loop")),
    );
    Some(EnrichmentSchedulerHandle { stop, join })
}

fn env_bool(key: &'static str, default: bool) -> Result<bool, EnrichmentWorkerConfigError> {
    match config_var(key) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(EnrichmentWorkerConfigError { key, value }),
        },
        Err(_) => Ok(default),
    }
}

fn env_i64(
    key: &'static str,
    default: i64,
    min: i64,
    max: i64,
) -> Result<i64, EnrichmentWorkerConfigError> {
    let value = config_var(key)
        .ok()
        .map(|value| value.parse::<i64>())
        .transpose()
        .map_err(|_| EnrichmentWorkerConfigError {
            key,
            value: config_var(key).unwrap_or_default(),
        })?
        .unwrap_or(default);
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(EnrichmentWorkerConfigError {
            key,
            value: value.to_string(),
        })
    }
}

fn env_u64(
    key: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, EnrichmentWorkerConfigError> {
    let value = config_var(key)
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|_| EnrichmentWorkerConfigError {
            key,
            value: config_var(key).unwrap_or_default(),
        })?
        .unwrap_or(default);
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(EnrichmentWorkerConfigError {
            key,
            value: value.to_string(),
        })
    }
}

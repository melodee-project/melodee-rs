//! Bounded outbox consumer and server scheduler for the active generation.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use melodee_persistence::{
    SEARCH_PROJECTION_NAME, SearchCatalogRecord, SearchEntityType, SearchOperation,
    SearchOutboxItem, SearchProjectionStore,
};
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::SearchConfig;
use crate::documents::{ArtistDoc, ReleaseDoc, TrackDoc};
use crate::error::SearchError;
use crate::generation::SearchRoot;
use crate::rebuild::run_rebuild_once_with_cancel;
use crate::store::SearchStore;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchPassStats {
    pub claimed: usize,
    pub applied: usize,
    pub acknowledged: usize,
    pub stale_or_idempotent: usize,
    pub errors: usize,
    pub no_active_generation: bool,
}

pub struct SearchSchedulerHandle {
    cancel: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl SearchSchedulerHandle {
    pub async fn stop(self) {
        self.cancel.store(true, Ordering::Release);
        if let Err(error) = self.task.await {
            error!(%error, "search scheduler task failed while stopping");
        }
    }
}

pub fn start_search_loop(config: SearchConfig, pool: PgPool) -> SearchSchedulerHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let task_cancel = Arc::clone(&cancel);
    let task = tokio::spawn(async move {
        while !task_cancel.load(Ordering::Acquire) {
            if let Err(error) = ensure_initial_rebuild(&config, &pool).await {
                error!(%error, "unable to provision initial search rebuild");
            }
            match run_rebuild_once_with_cancel(&config, &pool, &task_cancel).await {
                Ok(Some(job)) => info!(job_id = %job, "search rebuild pass completed"),
                Ok(None) => {}
                Err(error) => error!(%error, "search rebuild pass failed"),
            }
            if task_cancel.load(Ordering::Acquire) {
                break;
            }
            match apply_outbox_once(&config, &pool).await {
                Ok(stats) if stats.claimed > 0 => info!(
                    claimed = stats.claimed,
                    applied = stats.applied,
                    acknowledged = stats.acknowledged,
                    errors = stats.errors,
                    "search projection pass completed"
                ),
                Ok(_) => {}
                Err(error) => error!(%error, "search projection pass failed"),
            }
            tokio::time::sleep(Duration::from_millis(config.poll_interval_ms)).await;
        }
    });
    SearchSchedulerHandle { cancel, task }
}

async fn ensure_initial_rebuild(config: &SearchConfig, pool: &PgPool) -> Result<(), SearchError> {
    let root = SearchRoot::open(&config.search_root)?;
    let root_text = root.path().to_string_lossy();
    if SearchProjectionStore::active_generation(pool, root_text.as_ref())
        .await?
        .is_none()
    {
        let status = SearchProjectionStore::status(
            pool,
            SEARCH_PROJECTION_NAME,
            root_text.as_ref(),
            OffsetDateTime::now_utc(),
        )
        .await?;
        if status.latest_rebuild_status.as_deref() == Some("failed") {
            return Err(SearchError::InvalidQuery(
                "latest search rebuild failed; inspect status and request a replacement rebuild after correcting the cause"
                    .to_string(),
            ));
        }
        match SearchProjectionStore::request_rebuild(
            pool,
            root_text.as_ref(),
            "server-startup",
            OffsetDateTime::now_utc(),
        )
        .await
        {
            Ok(job) => info!(job_id = %job.job_id, "requested initial search generation"),
            Err(melodee_persistence::SearchStoreError::RebuildConflict(_)) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub async fn apply_outbox_once(
    config: &SearchConfig,
    pool: &PgPool,
) -> Result<SearchPassStats, SearchError> {
    config.validate().map_err(SearchError::InvalidQuery)?;
    let root = SearchRoot::open(&config.search_root)?;
    let root_text = root.path().to_string_lossy();
    let Some(active) = SearchProjectionStore::active_generation(pool, root_text.as_ref()).await?
    else {
        return Ok(SearchPassStats {
            no_active_generation: true,
            ..SearchPassStats::default()
        });
    };
    let generation_path = root.generation_path(active.generation_id);
    let _ = root.read_manifest(active.generation_id)?;
    let worker = format!("search-{}", Uuid::now_v7());
    let now = OffsetDateTime::now_utc();
    let items = SearchProjectionStore::claim_outbox(
        pool,
        SEARCH_PROJECTION_NAME,
        &worker,
        config.claim_batch_size,
        Duration::from_secs(config.lease_seconds),
        now,
    )
    .await?;
    if items.is_empty() {
        return Ok(SearchPassStats::default());
    }
    let ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
    let mut loaded = Vec::with_capacity(items.len());
    for item in items {
        let record = if item.operation == SearchOperation::Delete {
            None
        } else {
            match SearchProjectionStore::load_catalog_record(pool, item.entity_type, item.entity_id)
                .await
            {
                Ok(record) => record,
                Err(error) => {
                    let now = OffsetDateTime::now_utc();
                    let (failure_kind, retry_at) = match &error {
                        melodee_persistence::SearchStoreError::Database(_) => {
                            ("catalog_read", Some(now + time::Duration::seconds(1)))
                        }
                        _ => ("catalog_contract", None),
                    };
                    if let Err(evidence_error) = SearchProjectionStore::record_outbox_failure(
                        pool,
                        SEARCH_PROJECTION_NAME,
                        &worker,
                        &[item.id],
                        failure_kind,
                        &error.to_string(),
                        retry_at,
                        now,
                    )
                    .await
                    {
                        warn!(%evidence_error, %error, outbox_id = item.id, "search catalog-read failure evidence could not be recorded");
                    }
                    if let Err(release_error) = SearchProjectionStore::release_outbox_lease(
                        pool,
                        SEARCH_PROJECTION_NAME,
                        &worker,
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    {
                        warn!(%release_error, %error, "remaining search outbox leases could not be released");
                    }
                    return Err(error.into());
                }
            }
        };
        loaded.push((item, record));
    }
    SearchProjectionStore::renew_outbox_lease(
        pool,
        SEARCH_PROJECTION_NAME,
        &worker,
        &ids,
        Duration::from_secs(config.lease_seconds),
        OffsetDateTime::now_utc(),
    )
    .await?;

    let applied = tokio::task::spawn_blocking(move || apply_batch(&generation_path, loaded))
        .await
        .map_err(|error| SearchError::Retryable(format!("index task failed: {error}")))?;
    match applied {
        Ok(applied) => {
            SearchProjectionStore::acknowledge_outbox(
                pool,
                SEARCH_PROJECTION_NAME,
                &worker,
                &ids,
                OffsetDateTime::now_utc(),
            )
            .await?;
            Ok(SearchPassStats {
                claimed: ids.len(),
                applied,
                acknowledged: ids.len(),
                stale_or_idempotent: ids.len().saturating_sub(applied),
                errors: 0,
                no_active_generation: false,
            })
        }
        Err(error) => {
            let retry_at = OffsetDateTime::now_utc() + time::Duration::seconds(1);
            if let Err(release_error) = SearchProjectionStore::record_outbox_failure(
                pool,
                SEARCH_PROJECTION_NAME,
                &worker,
                &ids,
                "index_io",
                &error.to_string(),
                Some(retry_at),
                OffsetDateTime::now_utc(),
            )
            .await
            {
                warn!(%release_error, %error, "search failure evidence could not be recorded");
            }
            Err(error)
        }
    }
}

fn apply_batch(
    generation_path: &Path,
    rows: Vec<(SearchOutboxItem, Option<SearchCatalogRecord>)>,
) -> Result<usize, SearchError> {
    let mut store = SearchStore::open(generation_path)?;
    let mut applied = 0;
    for (item, record) in rows {
        match (item.operation, record) {
            (_, Some(SearchCatalogRecord::NoDocument)) => {}
            (SearchOperation::Upsert, Some(SearchCatalogRecord::Artist(record))) => {
                applied += usize::from(store.upsert_artist(&ArtistDoc::from(record))?);
            }
            (SearchOperation::Upsert, Some(SearchCatalogRecord::Release(record))) => {
                applied += usize::from(store.upsert_release(&ReleaseDoc::from(record))?);
            }
            (SearchOperation::Upsert, Some(SearchCatalogRecord::Track(record))) => {
                applied += usize::from(store.upsert_track(&TrackDoc::from(record))?);
            }
            (SearchOperation::Delete, _) | (SearchOperation::Upsert, None) => {
                if item.entity_type != SearchEntityType::AudioIdentity {
                    applied += usize::from(store.delete(
                        item.entity_type.as_str(),
                        &item.entity_id.to_string(),
                        item.entity_revision,
                    )?);
                }
            }
        }
    }
    // The caller acknowledges PostgreSQL only after this durable commit.
    store.commit()?;
    Ok(applied)
}

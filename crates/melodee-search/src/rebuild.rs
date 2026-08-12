//! Generational rebuild orchestration from a bounded PostgreSQL snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use melodee_persistence::{
    SearchCatalogRecord, SearchDocumentCounts, SearchOperation, SearchProjectionStore,
    SearchSnapshotReader, StoredSearchRebuildJob,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

use crate::config::SearchConfig;
use crate::documents::{ArtistDoc, ReleaseDoc, TrackDoc};
use crate::error::SearchError;
use crate::generation::{GenerationManifest, SearchRoot};
use crate::schema::SCHEMA_VERSION;
use crate::store::SearchStore;

pub async fn request_rebuild(
    config: &SearchConfig,
    pool: &PgPool,
    requested_by: &str,
) -> Result<StoredSearchRebuildJob, SearchError> {
    let root = SearchRoot::open(&config.search_root)?;
    SearchProjectionStore::request_rebuild(
        pool,
        root.path().to_string_lossy().as_ref(),
        requested_by,
        OffsetDateTime::now_utc(),
    )
    .await
    .map_err(SearchError::from)
}

pub async fn run_rebuild_once(
    config: &SearchConfig,
    pool: &PgPool,
) -> Result<Option<Uuid>, SearchError> {
    run_rebuild_once_with_cancel(config, pool, &AtomicBool::new(false)).await
}

#[doc(hidden)]
pub async fn run_rebuild_once_with_cancel(
    config: &SearchConfig,
    pool: &PgPool,
    cancel: &AtomicBool,
) -> Result<Option<Uuid>, SearchError> {
    config.validate().map_err(SearchError::InvalidQuery)?;
    let root = SearchRoot::open(&config.search_root)?;
    let worker = format!("search-rebuild-{}", Uuid::now_v7());
    let Some(job) = SearchProjectionStore::claim_rebuild(
        pool,
        root.path().to_string_lossy().as_ref(),
        &worker,
        i32::try_from(SCHEMA_VERSION)
            .map_err(|_| SearchError::InvalidQuery("schema version is too large".to_string()))?,
        Duration::from_secs(config.lease_seconds),
        OffsetDateTime::now_utc(),
    )
    .await?
    else {
        return Ok(None);
    };
    let result = execute_claimed_rebuild(config, pool, &root, &worker, &job, cancel).await;
    match result {
        Ok(()) => Ok(Some(job.job_id)),
        Err(SearchError::Retryable(message)) if message == "search rebuild cancelled" => {
            let _ = SearchProjectionStore::release_rebuild_lease(
                pool,
                job.job_id,
                &worker,
                OffsetDateTime::now_utc(),
            )
            .await;
            Ok(Some(job.job_id))
        }
        Err(error) => {
            let now = OffsetDateTime::now_utc();
            if is_permanent_rebuild_error(&error) {
                if let Err(evidence_error) = SearchProjectionStore::record_rebuild_failure(
                    pool,
                    job.job_id,
                    &worker,
                    "rebuild_contract",
                    &error.to_string(),
                    now,
                )
                .await
                {
                    warn!(%evidence_error, error = ?error, "permanent rebuild failure evidence could not be recorded");
                }
            } else {
                let retry_at = now + time::Duration::seconds(5);
                if let Err(evidence_error) = SearchProjectionStore::record_rebuild_retry(
                    pool,
                    job.job_id,
                    &worker,
                    &error.to_string(),
                    retry_at,
                    now,
                )
                .await
                {
                    warn!(%evidence_error, error = ?error, "retryable rebuild failure evidence could not be recorded");
                }
            }
            Err(error)
        }
    }
}

fn is_permanent_rebuild_error(error: &SearchError) -> bool {
    matches!(
        error,
        SearchError::InvalidQuery(_) | SearchError::UnownedRoot(_)
    ) || matches!(
        error,
        SearchError::Persistence(
            melodee_persistence::SearchStoreError::InvalidRequest(_)
                | melodee_persistence::SearchStoreError::UnsupportedEntity(_)
                | melodee_persistence::SearchStoreError::CorruptCatalog(_)
        )
    )
}

async fn execute_claimed_rebuild(
    config: &SearchConfig,
    pool: &PgPool,
    root: &SearchRoot,
    worker: &str,
    initial_job: &StoredSearchRebuildJob,
    cancel: &AtomicBool,
) -> Result<(), SearchError> {
    let generation_id = initial_job.generation_id.ok_or_else(|| {
        SearchError::InvalidQuery("claimed rebuild has no generation".to_string())
    })?;
    let mut generation = SearchProjectionStore::get_generation(pool, generation_id).await?;
    if initial_job.status == "running" {
        check_cancel(cancel)?;
        let generation_path = root.reset_generation(
            generation_id,
            OffsetDateTime::now_utc(),
            SCHEMA_VERSION,
            generation.source_high_water_mark,
        )?;
        let mut store = SearchStore::create(&generation_path)?;
        let mut snapshot = SearchSnapshotReader::begin(pool).await?;
        let mut counts = SearchDocumentCounts {
            artists: 0,
            releases: 0,
            tracks: 0,
        };
        let mut after = None;
        loop {
            check_cancel(cancel)?;
            renew(config, pool, initial_job.job_id, worker).await?;
            let page = snapshot
                .artists_page(after, config.snapshot_page_size)
                .await?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|record| record.internal_id);
            for record in page {
                store.upsert_artist(&ArtistDoc::from(record))?;
                counts.artists = counts.artists.checked_add(1).ok_or_else(|| {
                    SearchError::InvalidQuery("Artist count overflow".to_string())
                })?;
            }
            store.commit()?;
        }
        let mut after = None;
        loop {
            check_cancel(cancel)?;
            renew(config, pool, initial_job.job_id, worker).await?;
            let page = snapshot
                .releases_page(after, config.snapshot_page_size)
                .await?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|record| record.internal_id);
            for record in page {
                store.upsert_release(&ReleaseDoc::from(record))?;
                counts.releases = counts.releases.checked_add(1).ok_or_else(|| {
                    SearchError::InvalidQuery("Release count overflow".to_string())
                })?;
            }
            store.commit()?;
        }
        let mut after = None;
        loop {
            check_cancel(cancel)?;
            renew(config, pool, initial_job.job_id, worker).await?;
            let page = snapshot
                .tracks_page(after, config.snapshot_page_size)
                .await?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|record| record.internal_id);
            for record in page {
                store.upsert_track(&TrackDoc::from(record))?;
                counts.tracks = counts
                    .tracks
                    .checked_add(1)
                    .ok_or_else(|| SearchError::InvalidQuery("Track count overflow".to_string()))?;
            }
            store.commit()?;
        }
        snapshot.finish().await?;
        let stats = store.stats()?;
        if stats.artists as u64 != counts.artists
            || stats.releases as u64 != counts.releases
            || stats.tracks as u64 != counts.tracks
            || stats.schema_version != SCHEMA_VERSION
        {
            return Err(SearchError::Retryable(
                "rebuilt generation failed document-count validation".to_string(),
            ));
        }
        let manifest = root.read_manifest(generation_id)?;
        let manifest_hash = root.seal_generation(manifest, counts.clone())?;
        SearchProjectionStore::mark_generation_validated(
            pool,
            initial_job.job_id,
            worker,
            &counts,
            &manifest_hash,
            generation.source_high_water_mark,
            OffsetDateTime::now_utc(),
        )
        .await?;
        generation = SearchProjectionStore::get_generation(pool, generation_id).await?;
    }

    if generation.status != "validated" {
        return Err(SearchError::InvalidQuery(format!(
            "generation is {}, not validated",
            generation.status
        )));
    }
    catch_up_and_activate(
        config,
        pool,
        root,
        worker,
        initial_job.job_id,
        generation,
        cancel,
    )
    .await
}

async fn catch_up_and_activate(
    config: &SearchConfig,
    pool: &PgPool,
    root: &SearchRoot,
    worker: &str,
    job_id: Uuid,
    mut generation: melodee_persistence::StoredSearchGeneration,
    cancel: &AtomicBool,
) -> Result<(), SearchError> {
    let path = root.generation_path(generation.generation_id);
    loop {
        check_cancel(cancel)?;
        renew(config, pool, job_id, worker).await?;
        let latest = SearchProjectionStore::max_outbox_id(pool).await?;
        let mut cursor = generation.caught_up_outbox_id;
        while cursor < latest {
            let rows = SearchProjectionStore::load_outbox_after(
                pool,
                cursor,
                latest,
                config.claim_batch_size,
            )
            .await?;
            if rows.is_empty() {
                return Err(SearchError::Retryable(
                    "outbox catch-up stalled before its high-water mark".to_string(),
                ));
            }
            let mut store = SearchStore::open(&path)?;
            for item in &rows {
                let record = if item.operation == SearchOperation::Delete {
                    None
                } else {
                    SearchProjectionStore::load_catalog_record(
                        pool,
                        item.entity_type,
                        item.entity_id,
                    )
                    .await?
                };
                apply_record(&mut store, item, record)?;
            }
            store.commit()?;
            let stats = store.stats()?;
            let current_counts = melodee_persistence::SearchDocumentCounts {
                artists: u64::try_from(stats.artists)
                    .map_err(|_| SearchError::Retryable("Artist count overflow".to_string()))?,
                releases: u64::try_from(stats.releases)
                    .map_err(|_| SearchError::Retryable("Release count overflow".to_string()))?,
                tracks: u64::try_from(stats.tracks)
                    .map_err(|_| SearchError::Retryable("Track count overflow".to_string()))?,
            };
            cursor = rows.last().map(|item| item.id).ok_or_else(|| {
                SearchError::Retryable("outbox catch-up returned no rows".to_string())
            })?;
            SearchProjectionStore::update_generation_catchup(
                pool,
                job_id,
                worker,
                cursor,
                &current_counts,
                OffsetDateTime::now_utc(),
            )
            .await?;
        }
        // Persistence holds the outbox SHARE lock only inside this bounded
        // metadata transaction. Keep all index/catalog/filesystem work above
        // this call so activation cannot extend catalog-writer blocking.
        match SearchProjectionStore::activate_generation(
            pool,
            job_id,
            worker,
            OffsetDateTime::now_utc(),
        )
        .await
        {
            Ok(_) => {
                retire_old_generations(pool, root).await;
                return Ok(());
            }
            Err(melodee_persistence::SearchStoreError::NeedsCatchup(_)) => {
                // `activate_generation` rechecks max(outbox.id) while holding
                // the activation SHARE lock. A row committed after our
                // unlocked read reaches this branch; reloading the fenced
                // cursor and looping is the mechanism that closes that race.
                generation =
                    SearchProjectionStore::get_generation(pool, generation.generation_id).await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

async fn retire_old_generations(pool: &PgPool, root: &SearchRoot) {
    let root_text = root.path().to_string_lossy();
    let ids =
        match SearchProjectionStore::retired_generation_ids(pool, root_text.as_ref(), 16).await {
            Ok(ids) => ids,
            Err(error) => {
                warn!(error = ?error, "unable to list retired search generations");
                return;
            }
        };
    for generation_id in ids {
        let path = root.generation_path(generation_id);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                if let Err(error) = root.remove_generation(generation_id) {
                    warn!(%generation_id, error = ?error, "retired search generation was not removed");
                    continue;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Crash window: filesystem retirement completed but the
                // PostgreSQL evidence write did not. Adopt the absent owned
                // generation rather than retrying a removal forever.
            }
            Err(error) => {
                warn!(%generation_id, error = ?error, "retired generation path could not be inspected");
                continue;
            }
        }
        if let Err(error) = SearchProjectionStore::record_generation_removed(
            pool,
            generation_id,
            root_text.as_ref(),
            OffsetDateTime::now_utc(),
        )
        .await
        {
            warn!(%generation_id, error = ?error, "retired generation removal was not recorded");
        }
    }
}

fn apply_record(
    store: &mut SearchStore,
    item: &melodee_persistence::SearchOutboxItem,
    record: Option<SearchCatalogRecord>,
) -> Result<(), SearchError> {
    match (item.operation, record) {
        (_, Some(SearchCatalogRecord::NoDocument)) => Ok(()),
        (SearchOperation::Upsert, Some(SearchCatalogRecord::Artist(record))) => {
            store.upsert_artist(&ArtistDoc::from(record)).map(|_| ())
        }
        (SearchOperation::Upsert, Some(SearchCatalogRecord::Release(record))) => {
            store.upsert_release(&ReleaseDoc::from(record)).map(|_| ())
        }
        (SearchOperation::Upsert, Some(SearchCatalogRecord::Track(record))) => {
            store.upsert_track(&TrackDoc::from(record)).map(|_| ())
        }
        (SearchOperation::Delete, _) | (SearchOperation::Upsert, None) => {
            if item.entity_type == melodee_persistence::SearchEntityType::AudioIdentity {
                Ok(())
            } else {
                store
                    .delete(
                        item.entity_type.as_str(),
                        &item.entity_id.to_string(),
                        item.entity_revision,
                    )
                    .map(|_| ())
            }
        }
    }
}

async fn renew(
    config: &SearchConfig,
    pool: &PgPool,
    job_id: Uuid,
    worker: &str,
) -> Result<(), SearchError> {
    SearchProjectionStore::renew_rebuild_lease(
        pool,
        job_id,
        worker,
        Duration::from_secs(config.lease_seconds),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), SearchError> {
    if cancel.load(Ordering::Acquire) {
        Err(SearchError::Retryable(
            "search rebuild cancelled".to_string(),
        ))
    } else {
        Ok(())
    }
}

#[must_use]
pub fn generation_manifest(
    generation_id: Uuid,
    created_at: OffsetDateTime,
    source_high_water_mark: i64,
) -> GenerationManifest {
    GenerationManifest {
        generation_id,
        kind: "melodee-search-generation".to_string(),
        created_at,
        document_counts: melodee_persistence::SearchDocumentCounts {
            artists: 0,
            releases: 0,
            tracks: 0,
        },
        schema_version: SCHEMA_VERSION,
        source_high_water_mark,
    }
}

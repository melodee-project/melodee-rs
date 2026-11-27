//! Generational rebuild orchestration from a bounded PostgreSQL snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use melodee_persistence::{
    SearchCatalogRecord, SearchDocumentCounts, SearchOperation, SearchProjectionStore,
    SearchSnapshotReader, StoredSearchRebuildJob,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::{info, warn};
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

// A process-wide bound on queued and executing rebuild index work. The permit
// moves into the blocking closure: cancellation of an async waiter cannot
// incorrectly advertise capacity while the closure is still running.
const REBUILD_BLOCKING_LIMIT: usize = 1;
static REBUILD_BLOCKING: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

#[derive(Clone, Copy)]
struct RebuildTask<'a> {
    config: &'a SearchConfig,
    pool: &'a PgPool,
    job_id: Uuid,
    worker: &'a str,
    cancel: &'a AtomicBool,
}

/// Admit before spawning, renew while queued or running, and reap the task on
/// cancellation or lease loss. A finished index task is not acknowledged or
/// activated until the caller continues under a valid rebuild lease.
async fn rebuild_blocking<T: Send + 'static>(
    task: RebuildTask<'_>,
    work: impl FnOnce() -> Result<T, SearchError> + Send + 'static,
) -> Result<T, SearchError> {
    let semaphore = REBUILD_BLOCKING
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(REBUILD_BLOCKING_LIMIT)))
        .clone();
    let admission = semaphore.acquire_owned();
    tokio::pin!(admission);
    let heartbeat_period =
        (Duration::from_secs(task.config.lease_seconds) / 3).max(Duration::from_millis(100));
    let mut heartbeat = tokio::time::interval(heartbeat_period);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut cancellation = tokio::time::interval(Duration::from_millis(100));
    cancellation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let permit = loop {
        check_cancel(task.cancel)?;
        tokio::select! {
            permit = &mut admission => break permit.map_err(|error| SearchError::Retryable(format!("rebuild admission failed: {error}")))?,
            _ = heartbeat.tick() => renew(task.config, task.pool, task.job_id, task.worker).await?,
            _ = cancellation.tick() => {}
        }
    };
    check_cancel(task.cancel)?;
    let mut handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    });
    loop {
        tokio::select! {
            result = &mut handle => {
                check_cancel(task.cancel)?;
                // Task completion and a scheduled heartbeat can become ready
                // together. Revalidate ownership even if the completion arm
                // won so a stale worker cannot continue towards activation.
                renew(task.config, task.pool, task.job_id, task.worker).await?;
                return result.map_err(|error| SearchError::Retryable(format!("index task failed: {error}")))?;
            }
            _ = heartbeat.tick() => {
                if let Err(error) = renew(task.config, task.pool, task.job_id, task.worker).await {
                    let _ = handle.await;
                    return Err(error);
                }
            }
            _ = cancellation.tick() => {
                if let Err(error) = check_cancel(task.cancel) {
                    let _ = handle.await;
                    return Err(error);
                }
            }
        }
    }
}

/// Run one bounded page's document construction and optional commit off the
/// async runtime, returning its writer for the next page.
async fn run_index_blocking(
    mut store: SearchStore,
    work: impl FnOnce(&mut SearchStore) -> Result<(), SearchError> + Send + 'static,
    commit: bool,
    task: RebuildTask<'_>,
) -> Result<SearchStore, SearchError> {
    let started = std::time::Instant::now();
    let result = rebuild_blocking(task, move || {
        work(&mut store)?;
        if commit {
            store.commit()?;
        }
        Ok::<SearchStore, SearchError>(store)
    })
    .await;
    melodee_persistence::metrics::observe(
        melodee_persistence::metrics::Metric::SearchRebuildPage,
        started.elapsed(),
    );
    result
}

/// Commits buffered documents on the blocking pool and returns the store.
async fn commit_store_blocking(
    mut store: SearchStore,
    task: RebuildTask<'_>,
) -> Result<SearchStore, SearchError> {
    rebuild_blocking(task, move || {
        store.commit()?;
        Ok::<SearchStore, SearchError>(store)
    })
    .await
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
    let task = RebuildTask {
        config,
        pool,
        job_id: initial_job.job_id,
        worker,
        cancel,
    };
    let mut generation = SearchProjectionStore::get_generation(pool, generation_id).await?;
    if initial_job.status == "running" {
        check_cancel(cancel)?;
        // Reset mutates generation storage. Keep its existing synchronous
        // lifecycle until cross-process takeover can fence an in-flight reset.
        let generation_path = root.reset_generation(
            generation_id,
            OffsetDateTime::now_utc(),
            SCHEMA_VERSION,
            generation.source_high_water_mark,
        )?;
        let mut store =
            rebuild_blocking(task, move || SearchStore::create(&generation_path)).await?;
        // Record this snapshot's visible outbox rows inside the snapshot so
        // late-committing low ids stay a delivery obligation (PERF-V001).
        let mut snapshot = SearchSnapshotReader::begin(pool, generation_id).await?;
        let snapshot_started = std::time::Instant::now();
        #[cfg(feature = "test-support")]
        if let Some(barrier) = &config.rebuild_test_barrier {
            barrier.pause_after_snapshot_started().await;
        }
        let mut commit_count = 0_u64;
        let mut counts = SearchDocumentCounts {
            artists: 0,
            releases: 0,
            tracks: 0,
        };
        // PERF-006 / RM-14: explicit document and time bounds for rebuild
        // commits. The previous per-page commit produced ~N/page commits; the
        // uncommitted buffer is bounded by documents and wall time.
        let commit_documents = u64::try_from(config.snapshot_commit_documents)
            .map_err(|_| SearchError::InvalidQuery("commit bound is too large".to_string()))?;
        let commit_interval = std::time::Duration::from_millis(config.snapshot_commit_interval_ms);
        let mut pending_documents = 0_u64;
        let mut last_commit = std::time::Instant::now();
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
            let page_count = u64::try_from(page.len())
                .map_err(|_| SearchError::InvalidQuery("Artist count overflow".to_string()))?;
            pending_documents = pending_documents.saturating_add(page_count);
            let should_commit =
                pending_documents >= commit_documents || last_commit.elapsed() >= commit_interval;
            store = run_index_blocking(
                store,
                move |store| {
                    for record in page {
                        store.insert_artist_fresh(&ArtistDoc::from(record))?;
                    }
                    Ok(())
                },
                should_commit,
                task,
            )
            .await?;
            if should_commit {
                commit_count = commit_count.saturating_add(1);
                pending_documents = 0;
                last_commit = std::time::Instant::now();
            }
            counts.artists = counts
                .artists
                .checked_add(page_count)
                .ok_or_else(|| SearchError::InvalidQuery("Artist count overflow".to_string()))?;
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
            let page_count = u64::try_from(page.len())
                .map_err(|_| SearchError::InvalidQuery("Release count overflow".to_string()))?;
            pending_documents = pending_documents.saturating_add(page_count);
            let should_commit =
                pending_documents >= commit_documents || last_commit.elapsed() >= commit_interval;
            store = run_index_blocking(
                store,
                move |store| {
                    for record in page {
                        store.insert_release_fresh(&ReleaseDoc::from(record))?;
                    }
                    Ok(())
                },
                should_commit,
                task,
            )
            .await?;
            if should_commit {
                commit_count = commit_count.saturating_add(1);
                pending_documents = 0;
                last_commit = std::time::Instant::now();
            }
            counts.releases = counts
                .releases
                .checked_add(page_count)
                .ok_or_else(|| SearchError::InvalidQuery("Release count overflow".to_string()))?;
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
            let page_count = u64::try_from(page.len())
                .map_err(|_| SearchError::InvalidQuery("Track count overflow".to_string()))?;
            pending_documents = pending_documents.saturating_add(page_count);
            let should_commit =
                pending_documents >= commit_documents || last_commit.elapsed() >= commit_interval;
            store = run_index_blocking(
                store,
                move |store| {
                    for record in page {
                        store.insert_track_fresh(&TrackDoc::from(record))?;
                    }
                    Ok(())
                },
                should_commit,
                task,
            )
            .await?;
            if should_commit {
                commit_count = commit_count.saturating_add(1);
                pending_documents = 0;
                last_commit = std::time::Instant::now();
            }
            counts.tracks = counts
                .tracks
                .checked_add(page_count)
                .ok_or_else(|| SearchError::InvalidQuery("Track count overflow".to_string()))?;
        }
        if pending_documents > 0 {
            store = commit_store_blocking(store, task).await?;
            commit_count = commit_count.saturating_add(1);
        }
        snapshot.finish().await?;
        info!(job_id = %initial_job.job_id, %generation_id, snapshot_seconds = snapshot_started.elapsed().as_secs_f64(), commit_count, artists = counts.artists, releases = counts.releases, tracks = counts.tracks, "search rebuild snapshot indexed");
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
    let task = RebuildTask {
        config,
        pool,
        job_id,
        worker,
        cancel,
    };
    let catchup_started = std::time::Instant::now();
    let mut catchup_batches = 0_u64;
    let mut catchup_rows = 0_u64;
    // Deterministic test interleaving: pause after the snapshot is validated
    // and before catch-up so a test can drive the ordinary outbox consumer
    // against the previous generation. Compiled only for `test-support`.
    #[cfg(feature = "test-support")]
    if let Some(barrier) = &config.rebuild_test_barrier {
        barrier.pause_before_catchup().await;
    }
    loop {
        check_cancel(cancel)?;
        renew(config, pool, job_id, worker).await?;
        // PERF-V001 / RM-05: replay every outstanding delivery obligation of
        // this generation in ascending id order. The obligation set is defined
        // by the rebuild snapshot (`search_rebuild_snapshot_outbox`), not by an
        // id-ordered cursor, so a low id that committed after the snapshot is
        // replayed even when the ordinary worker already acknowledged it
        // against the previous generation.
        let mut after = 0_i64;
        loop {
            check_cancel(cancel)?;
            renew(config, pool, job_id, worker).await?;
            let pending = SearchProjectionStore::pending_outbox_replay_ids(
                pool,
                generation.generation_id,
                after,
                config.claim_batch_size,
            )
            .await?;
            let Some(&last) = pending.last() else {
                break;
            };
            let rows = SearchProjectionStore::load_outbox_by_ids(pool, &pending).await?;
            if rows.is_empty() {
                return Err(SearchError::Retryable(
                    "outbox replay rows disappeared before they were applied".to_string(),
                ));
            }
            let records = SearchProjectionStore::load_catalog_records(pool, &rows).await?;
            let catchup_path = path.clone();
            let stats = rebuild_blocking(task, move || {
                let mut store = SearchStore::open(&catchup_path)?;
                for (item, record) in rows.iter().zip(records) {
                    apply_record(&mut store, item, record)?;
                }
                store.commit()?;
                store.stats()
            })
            .await?;
            // Apply, then commit, then record the durable replay progress.
            // A crash between commit and record replays an idempotent batch;
            // a crash before commit replays from the last recorded point.
            let current_counts = melodee_persistence::SearchDocumentCounts {
                artists: u64::try_from(stats.artists)
                    .map_err(|_| SearchError::Retryable("Artist count overflow".to_string()))?,
                releases: u64::try_from(stats.releases)
                    .map_err(|_| SearchError::Retryable("Release count overflow".to_string()))?,
                tracks: u64::try_from(stats.tracks)
                    .map_err(|_| SearchError::Retryable("Track count overflow".to_string()))?,
            };
            SearchProjectionStore::record_outbox_replay(
                pool,
                generation.generation_id,
                &pending,
                OffsetDateTime::now_utc(),
            )
            .await?;
            SearchProjectionStore::update_generation_catchup(
                pool,
                job_id,
                worker,
                last,
                &current_counts,
                OffsetDateTime::now_utc(),
            )
            .await?;
            catchup_batches = catchup_batches.saturating_add(1);
            catchup_rows =
                catchup_rows.saturating_add(u64::try_from(pending.len()).unwrap_or(u64::MAX));
            after = last;
        }
        // Persistence holds the outbox ACCESS EXCLUSIVE fence only inside this
        // bounded metadata transaction. Keep all index/catalog/filesystem work
        // above this call so activation cannot extend catalog-writer blocking.
        let activation_started = std::time::Instant::now();
        match SearchProjectionStore::activate_generation(
            pool,
            job_id,
            worker,
            OffsetDateTime::now_utc(),
        )
        .await
        {
            Ok(_) => {
                #[cfg(feature = "test-support")]
                if let Some(barrier) = &config.rebuild_test_barrier {
                    barrier.mark_activated();
                }
                info!(%job_id, generation_id = %generation.generation_id, catchup_seconds = catchup_started.elapsed().as_secs_f64(), catchup_batches, catchup_rows, activation_seconds = activation_started.elapsed().as_secs_f64(), "search rebuild activated");
                retire_old_generations(pool, root).await;
                return Ok(());
            }
            Err(
                melodee_persistence::SearchStoreError::NeedsCatchup(_)
                | melodee_persistence::SearchStoreError::ActivationContended,
            ) => {
                // A transaction that inserted an outbox row committed between
                // the last replay query and the fence, or the fence timed out.
                // Reload the generation and replay the remaining obligations;
                // never activate a stale generation.
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

#[cfg(test)]
mod admission_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn queued_and_running_work_remains_bounded_during_cancellation_and_lease_failure() {
        let semaphore = REBUILD_BLOCKING
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(REBUILD_BLOCKING_LIMIT)))
            .clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(150))
            .connect_lazy("postgres://test:test@127.0.0.1:1/test")
            .expect("lazy pool");
        let config = SearchConfig {
            lease_seconds: 1,
            ..SearchConfig::default()
        };
        let cancelled = AtomicBool::new(false);
        let task = RebuildTask {
            config: &config,
            pool: &pool,
            job_id: Uuid::now_v7(),
            worker: "test",
            cancel: &cancelled,
        };

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let closure_started = Arc::clone(&started);
        let (result, ()) = tokio::join!(
            rebuild_blocking(task, move || {
                closure_started.store(true, Ordering::SeqCst);
                Ok(())
            }),
            async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                cancelled.store(true, Ordering::SeqCst);
            }
        );
        assert!(matches!(result, Err(SearchError::Retryable(_))));
        assert!(!started.load(Ordering::SeqCst));
        drop(permit);

        cancelled.store(false, Ordering::SeqCst);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let (result, ()) = tokio::join!(
            rebuild_blocking(task, move || {
                started_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
                Ok(())
            }),
            async {
                started_rx.await.unwrap();
                cancelled.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert!(semaphore.clone().try_acquire_owned().is_err());
                finish_tx.send(()).unwrap();
            }
        );
        assert!(matches!(result, Err(SearchError::Retryable(_))));

        // Simulate a failed heartbeat while a blocking task is running. The
        // connection is deliberately unavailable; the task must still be
        // reaped and retain the permit until its closure really exits.
        cancelled.store(false, Ordering::SeqCst);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let (result, ()) = tokio::join!(
            rebuild_blocking(task, move || {
                started_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
                Ok(())
            }),
            async {
                started_rx.await.unwrap();
                tokio::time::sleep(Duration::from_millis(750)).await;
                assert!(semaphore.clone().try_acquire_owned().is_err());
                finish_tx.send(()).unwrap();
            }
        );
        assert!(result.is_err());
        assert!(semaphore.try_acquire_owned().is_ok());
    }
}

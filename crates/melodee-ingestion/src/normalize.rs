//! Leased Release normalization worker.
//!
//! Claims work items in the `normalizing` state, runs the media normalization
//! stage (canonical MP3 encoding, acoustic fingerprinting, artwork
//! normalization, sidecar parsing) in a blocking thread with continuous lease
//! renewal, and publishes results atomically with the `normalizing ->
//! validating` transition.
//!
//! Lease safety: `publish_normalization` re-checks lease ownership inside its
//! transaction, so a worker that outlives its lease can never publish
//! authoritative stale results. Long-running FFmpeg/fpcalc work runs in
//! `spawn_blocking` with a cancellation flag that is set if the lease is lost,
//! bounded by a heartbeat that renews the lease while work is in progress.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration as StdDuration, Instant},
};

use ::time::OffsetDateTime;
use melodee_domain::{FailureClass, WorkErrorCode};
use melodee_media::{NormalizationError, NormalizationOptions, normalize_release};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use tokio::{
    task::JoinSet,
    time::{self, Duration as TokioDuration, MissedTickBehavior},
};
use tracing::{Instrument as _, error, info, warn};

use crate::{
    IngestionConfig, ReconcileResult, ReconcileStats,
    error::{IngestionError, Result},
    owned_source_root,
    scheduler::StopToken,
    snapshot::snapshot_release_directory,
    verify_owned_source,
};
use melodee_persistence::WorkStoreError;

#[derive(Debug, Clone, Default)]
pub struct NormalizationStats {
    pub claimed: u64,
    pub completed: u64,
    pub failed: u64,
    pub errors: u64,
    pub duration_ms: i64,
}

pub async fn normalize_once(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<NormalizationStats> {
    normalize_once_with_shutdown(
        ingestion,
        store,
        pool,
        work_config,
        now,
        Arc::new(AtomicBool::new(false)),
    )
    .await
}

async fn normalize_once_with_shutdown(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
    shutdown: Arc<AtomicBool>,
) -> Result<NormalizationStats> {
    work_config
        .validate()
        .map_err(|error| IngestionError::InspectionTask(error.to_string()))?;
    if ingestion.normalization_concurrency == 0 {
        return Err(IngestionError::InspectionTask(
            "normalization concurrency must be greater than zero".to_owned(),
        ));
    }
    let started = Instant::now();
    let mut claim_config = *work_config;
    claim_config.claim_batch_size = ingestion.normalization_concurrency;
    let worker = WorkerId::new();
    let items = store
        .claim_normalizing(pool, &worker, &claim_config, now)
        .await?;
    let mut stats = NormalizationStats {
        claimed: items.len() as u64,
        ..NormalizationStats::default()
    };

    let mut tasks = JoinSet::new();
    for item in items {
        let task_ingestion = ingestion.clone();
        let task_store = *store;
        let task_pool = pool.clone();
        let task_worker = worker.clone();
        let task_work_config = *work_config;
        let task_shutdown = shutdown.clone();
        // the spawned task crosses the join-set boundary; the span
        // carries the durable ids explicitly so every nested event
        // (including media-tool invocations) inherits them.
        let span = tracing::info_span!(
            "normalize_work_item",
            work_item_id = %item.id,
            correlation_id = %item.correlation_id,
        );
        tasks.spawn(
            async move {
                normalize_one(
                    &task_ingestion,
                    &task_store,
                    &task_pool,
                    &task_worker,
                    item,
                    &task_work_config,
                    task_shutdown,
                )
                .await
            }
            .instrument(span),
        );
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(NormalizationOutcome::Completed)) => stats.completed += 1,
            Ok(Ok(NormalizationOutcome::Failed)) => stats.failed += 1,
            Ok(Err(error)) => {
                stats.errors += 1;
                error!(%error, "normalization item failed outside normal failure recording");
            }
            Err(error) => {
                stats.errors += 1;
                error!(%error, "normalization task failed to join");
            }
        }
    }
    stats.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);
    info!(
        claimed = stats.claimed,
        completed = stats.completed,
        failed = stats.failed,
        errors = stats.errors,
        "normalization batch completed"
    );
    Ok(stats)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NormalizationOutcome {
    Completed,
    Failed,
}

async fn normalization_filesystem<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    cancellation: &AtomicBool,
    shutdown: &AtomicBool,
) -> Result<T> {
    let task = crate::admission::run(work);
    tokio::pin!(task);
    let mut poll = time::interval(TokioDuration::from_millis(100));
    loop {
        if cancellation.load(Ordering::SeqCst) || shutdown.load(Ordering::SeqCst) {
            return Err(IngestionError::InspectionTask(
                "normalization canceled during filesystem work".to_owned(),
            ));
        }
        tokio::select! {
            result = &mut task => return result.map_err(IngestionError::InspectionTask),
            _ = poll.tick() => {}
        }
    }
}

async fn normalize_one(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: melodee_domain::ReleaseWorkItem,
    work_config: &WorkEngineConfig,
    shutdown: Arc<AtomicBool>,
) -> Result<NormalizationOutcome> {
    let cancellation = Arc::new(AtomicBool::new(false));
    let heartbeat_store = *store;
    let heartbeat_pool = pool.clone();
    let heartbeat_worker = worker.clone();
    let heartbeat_lease = work_config.lease_duration;
    let heartbeat_item_id = item.id;
    let heartbeat_cancellation = cancellation.clone();
    let heartbeat_interval = heartbeat_interval(work_config.lease_duration)?;

    let mut heartbeat_handle = tokio::task::JoinSet::new();
    heartbeat_handle.spawn(async move {
        let mut interval = time::interval(heartbeat_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        interval.tick().await; // skip initial tick
        loop {
            interval.tick().await;
            if heartbeat_cancellation.load(Ordering::SeqCst) {
                return;
            }
            match heartbeat_store
                .renew_lease(
                    &heartbeat_pool,
                    heartbeat_item_id,
                    &heartbeat_worker,
                    heartbeat_lease,
                    OffsetDateTime::now_utc(),
                )
                .await
            {
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        work_item_id = %heartbeat_item_id,
                        %error,
                        "normalization lease renewal failed; canceling in-flight work"
                    );
                    heartbeat_cancellation.store(true, Ordering::SeqCst);
                    return;
                }
            }
        }
    });

    let owned = match owned_source_root(ingestion, store, pool, &item).await {
        Ok(owned) => owned,
        Err(error) => {
            return record_normalization_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "normalization.staged_source_missing",
                &error.to_string(),
                FailureClass::Permanent,
                OffsetDateTime::now_utc(),
            )
            .await
            .map(|_| NormalizationOutcome::Failed);
        }
    };
    let task_owned = owned.clone();
    let verification = normalization_filesystem(
        move || verify_owned_source(&task_owned),
        &cancellation,
        &shutdown,
    )
    .await?;
    if let Err(error) = verification {
        return record_normalization_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "normalization.staged_source_integrity",
            &error.to_string(),
            FailureClass::Permanent,
            OffsetDateTime::now_utc(),
        )
        .await
        .map(|_| NormalizationOutcome::Failed);
    }
    let source_root = owned.root.clone();

    let stored = store.get_manifest(pool, item.id).await?;
    let Some(manifest_row) = stored else {
        return record_normalization_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "normalization.manifest_missing",
            "normalizing work item has no persisted inspection manifest",
            FailureClass::Permanent,
            OffsetDateTime::now_utc(),
        )
        .await
        .map(|_| NormalizationOutcome::Failed);
    };
    let mut manifest: melodee_domain::ReleaseManifest =
        serde_json::from_value(manifest_row.payload).map_err(|e| {
            IngestionError::Inspection(melodee_media::InspectionError::Serialization(e))
        })?;
    let task_root = source_root.clone();
    let ready_marker = ingestion.ready_marker_name.clone();
    let before_snapshot = match normalization_filesystem(
        move || snapshot_release_directory(&task_root, &ready_marker),
        &cancellation,
        &shutdown,
    )
    .await?
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return record_normalization_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "normalization.source_changed",
                &error.to_string(),
                FailureClass::Retryable,
                OffsetDateTime::now_utc(),
            )
            .await
            .map(|_| NormalizationOutcome::Failed);
        }
    };
    let cancellation_for_task = cancellation.clone();
    let options = NormalizationOptions {
        ffmpeg_path: ingestion.ffmpeg_path.clone(),
        ffmpeg_timeout: std::time::Duration::try_from(ingestion.ffmpeg_timeout)
            .map_err(|error| IngestionError::InspectionTask(error.to_string()))?,
        fpcalc_path: ingestion.fpcalc_path.clone(),
        fpcalc_timeout: std::time::Duration::try_from(ingestion.fpcalc_timeout)
            .map_err(|error| IngestionError::InspectionTask(error.to_string()))?,
        ffprobe_path: ingestion.ffprobe_path.clone(),
        ffprobe_timeout: std::time::Duration::try_from(ingestion.ffprobe_timeout)
            .map_err(|error| IngestionError::InspectionTask(error.to_string()))?,
        work_area: ingestion.work_area_base.clone(),
        attempt_id: uuid::Uuid::now_v7(),
        cancellation: cancellation_for_task,
        shutdown: Some(shutdown.clone()),
        ..NormalizationOptions::default()
    };

    let normalization_root = source_root.clone();
    let admission = crate::admission::heavy_ingestion();
    tokio::pin!(admission);
    let mut cancel_poll = time::interval(TokioDuration::from_millis(100));
    let permit = loop {
        if cancellation.load(Ordering::SeqCst) || shutdown.load(Ordering::SeqCst) {
            break None;
        }
        tokio::select! {
            result = &mut admission => break Some(result),
            _ = cancel_poll.tick() => {}
        }
    };
    let permit = match permit {
        Some(Ok(permit)) => permit,
        result => {
            return Err(IngestionError::InspectionTask(match result {
                Some(Err(error)) => error.to_string(),
                _ => "normalization canceled while awaiting admission".to_owned(),
            }));
        }
    };
    let normalization_result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        normalize_release(&mut manifest, &normalization_root, &options)
    })
    .await;

    // If the heartbeat set the cancellation flag, the normalization may have
    // already returned Canceled; either way we must not publish.
    if cancellation.load(Ordering::SeqCst) {
        return Err(IngestionError::InspectionTask(
            "normalization canceled due to lease loss".to_owned(),
        ));
    }

    let mut report = match normalization_result {
        Ok(Ok(report)) => report,
        Ok(Err(NormalizationError::Canceled)) => {
            return Err(IngestionError::InspectionTask(
                "normalization canceled mid-flight".to_owned(),
            ));
        }
        Ok(Err(error)) => {
            let classification = classify_normalization_error(&error);
            return record_normalization_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                &classification.code,
                &error.to_string(),
                classification.class,
                OffsetDateTime::now_utc(),
            )
            .await
            .map(|_| NormalizationOutcome::Failed);
        }
        Err(join_error) => {
            return Err(IngestionError::InspectionTask(join_error.to_string()));
        }
    };
    if let Some(normalization) = report.manifest.normalization.as_mut() {
        normalization.provenance.worker_id = Some(worker.as_str().to_owned());
    }
    let task_root = source_root.clone();
    let ready_marker = ingestion.ready_marker_name.clone();
    let after_snapshot = match normalization_filesystem(
        move || snapshot_release_directory(&task_root, &ready_marker),
        &cancellation,
        &shutdown,
    )
    .await?
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return record_normalization_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "normalization.source_changed",
                &error.to_string(),
                FailureClass::Retryable,
                OffsetDateTime::now_utc(),
            )
            .await
            .map(|_| NormalizationOutcome::Failed);
        }
    };
    let source_changed = if after_snapshot.signature != before_snapshot.signature {
        true
    } else {
        let task_owned = owned.clone();
        normalization_filesystem(
            move || verify_owned_source(&task_owned),
            &cancellation,
            &shutdown,
        )
        .await?
        .is_err()
    };
    if source_changed {
        return record_normalization_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "normalization.source_changed",
            "source changed during normalization",
            FailureClass::Retryable,
            OffsetDateTime::now_utc(),
        )
        .await
        .map(|_| NormalizationOutcome::Failed);
    }

    if cancellation.load(Ordering::SeqCst) || shutdown.load(Ordering::SeqCst) {
        return Err(IngestionError::InspectionTask(
            "normalization canceled before publication".to_owned(),
        ));
    }
    let publish_result = store
        .publish_normalization(
            pool,
            item.id,
            worker,
            &report.manifest,
            &report.manifest_hash,
            &item.source_path,
            report.duration_ms,
            OffsetDateTime::now_utc(),
        )
        .await;

    if let Err(error) = publish_result {
        match &error {
            WorkStoreError::Database(sqlx::Error::Database(db_error))
                if matches!(db_error.code().as_deref(), Some("22P05" | "22021")) =>
            {
                // The manifest write rolled back. The same input will fail after
                // every lease expiry, so record the failure while we own it.
                return record_normalization_failure(
                    store, pool, worker, item.id, work_config,
                    "normalization.invalid_manifest_text",
                    "PostgreSQL rejected text in the normalized metadata. Check the Release tags and sidecar files before retrying.",
                    FailureClass::Permanent, OffsetDateTime::now_utc(),
                )
                .await
                .map(|_| NormalizationOutcome::Failed);
            }
            WorkStoreError::LeaseExpired(_) | WorkStoreError::NotLeaseOwner(_) => {
                warn!(
                    work_item_id = %item.id,
                    %error,
                    "normalization publish rejected: lease lost"
                );
            }
            _ => {
                error!(%error, work_item_id = %item.id, "normalization publish failed");
            }
        }
        return Err(IngestionError::Persistence(error));
    }

    Ok(NormalizationOutcome::Completed)
}

pub(crate) fn heartbeat_interval(lease_duration: ::time::Duration) -> Result<TokioDuration> {
    let lease = StdDuration::try_from(lease_duration)
        .map_err(|error| IngestionError::InspectionTask(error.to_string()))?;
    let interval = lease / 3;
    if interval.is_zero() {
        return Err(IngestionError::InspectionTask(
            "lease duration is too short for safe renewal".to_owned(),
        ));
    }
    Ok(interval.min(StdDuration::from_secs(30)))
}

struct NormalizedErrorClassification {
    code: String,
    class: FailureClass,
}

fn classify_normalization_error(error: &NormalizationError) -> NormalizedErrorClassification {
    match error {
        NormalizationError::Canceled => NormalizedErrorClassification {
            code: "normalization.canceled".to_owned(),
            class: FailureClass::Retryable,
        },
        NormalizationError::Ffmpeg(_) | NormalizationError::Fpcalc(_) => {
            NormalizedErrorClassification {
                code: "normalization.tool_failed".to_owned(),
                class: FailureClass::Retryable,
            }
        }
        NormalizationError::SourceMissing(_) | NormalizationError::SourceUnreadable { .. } => {
            NormalizedErrorClassification {
                code: "normalization.source_error".to_owned(),
                class: FailureClass::Retryable,
            }
        }
        NormalizationError::SourceChanged(_) => NormalizedErrorClassification {
            code: "normalization.source_changed".to_owned(),
            class: FailureClass::Retryable,
        },
        _ => NormalizedErrorClassification {
            code: "normalization.execution_failed".to_owned(),
            class: FailureClass::Retryable,
        },
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_normalization_failure(
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    id: uuid::Uuid,
    work_config: &WorkEngineConfig,
    code: &str,
    message: &str,
    class: FailureClass,
    now: OffsetDateTime,
) -> Result<()> {
    store
        .record_failure(
            pool,
            id,
            worker,
            WorkErrorCode::new(code),
            message,
            class,
            &work_config.retry_policy,
            now,
        )
        .await?;
    Ok(())
}

/// Starts a graceful periodic normalization worker.
pub fn start_normalization_loop(
    ingestion: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    work_config: WorkEngineConfig,
) -> crate::SchedulerHandle {
    let stop = StopToken::new();
    let stop_requested = stop.signal.clone().notified_owned();
    let handle = tokio::spawn(async move {
        if !ingestion.enabled() {
            return Vec::new();
        }
        let interval = TokioDuration::try_from(ingestion.reconciliation_interval)
            .unwrap_or_else(|_| TokioDuration::from_secs(30));
        let claim_batch_size = u64::from(ingestion.normalization_concurrency);
        let mut ticker = time::interval(interval);
        let mut results = Vec::new();
        tokio::pin!(stop_requested);
        loop {
            let should_run = tokio::select! {
                _ = ticker.tick() => true,
                _ = &mut stop_requested => false,
            };
            if !should_run {
                break;
            }

            // Drain a full batch rather than sleeping out the tick.
            let mut stopping = false;
            loop {
                let shutdown = Arc::new(AtomicBool::new(false));
                let batch = normalize_once_with_shutdown(
                    &ingestion,
                    &store,
                    &pool,
                    &work_config,
                    OffsetDateTime::now_utc(),
                    shutdown.clone(),
                );
                tokio::pin!(batch);
                let (interrupted, result) = tokio::select! {
                    result = &mut batch => (false, result),
                    _ = &mut stop_requested => {
                        shutdown.store(true, Ordering::SeqCst);
                        (true, batch.await)
                    }
                };
                let full_batch = match result {
                    Ok(stats) => {
                        let full_batch = stats.errors == 0 && stats.claimed >= claim_batch_size;
                        crate::scheduler::retain_result(
                            &mut results,
                            ReconcileResult {
                                source: "normalization".to_owned(),
                                stats: ReconcileStats {
                                    duration_ms: stats.duration_ms,
                                    errors: stats.errors,
                                    ..ReconcileStats::default()
                                },
                            },
                        );
                        full_batch
                    }
                    Err(error) => {
                        error!(%error, "normalization loop failed");
                        false
                    }
                };
                if interrupted {
                    stopping = true;
                    break;
                }
                if !full_batch {
                    break;
                }
                tokio::task::yield_now().await;
            }
            if stopping {
                break;
            }
        }
        results
    });
    crate::SchedulerHandle::from_parts(handle, stop)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn filesystem_work_stops_waiting_on_lease_cancellation() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            normalization_filesystem(
                move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    let _ = finished_tx.send(());
                },
                &task_cancellation,
                &shutdown,
            )
            .await
        });
        started_rx.await.unwrap();
        cancellation.store(true, Ordering::SeqCst);
        let result = time::timeout(TokioDuration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        release_tx.send(()).unwrap();
        finished_rx.await.unwrap();
    }

    #[tokio::test]
    async fn filesystem_work_does_not_start_after_shutdown() {
        let cancellation = AtomicBool::new(false);
        let shutdown = AtomicBool::new(true);
        let invoked = Arc::new(AtomicBool::new(false));
        let task_invoked = invoked.clone();
        let result = normalization_filesystem(
            move || task_invoked.store(true, Ordering::SeqCst),
            &cancellation,
            &shutdown,
        )
        .await;
        assert!(result.is_err());
        assert!(!invoked.load(Ordering::SeqCst));
    }

    #[test]
    fn classify_canceled_is_retryable() {
        let classification = classify_normalization_error(&NormalizationError::Canceled);
        assert_eq!(classification.code, "normalization.canceled");
        assert_eq!(classification.class, FailureClass::Retryable);
    }

    #[test]
    fn classify_ffmpeg_failure_is_retryable() {
        let classification =
            classify_normalization_error(&NormalizationError::Ffmpeg("segfault".to_owned()));
        assert_eq!(classification.code, "normalization.tool_failed");
        assert_eq!(classification.class, FailureClass::Retryable);
    }

    #[test]
    fn classify_fpcalc_failure_is_retryable() {
        let classification =
            classify_normalization_error(&NormalizationError::Fpcalc("timeout".to_owned()));
        assert_eq!(classification.code, "normalization.tool_failed");
        assert_eq!(classification.class, FailureClass::Retryable);
    }

    #[test]
    fn classify_source_missing_is_retryable() {
        let classification = classify_normalization_error(&NormalizationError::SourceMissing(
            PathBuf::from("/missing"),
        ));
        assert_eq!(classification.code, "normalization.source_error");
        assert_eq!(classification.class, FailureClass::Retryable);
    }

    #[test]
    fn classify_source_changed_is_retryable_and_distinct() {
        let classification = classify_normalization_error(&NormalizationError::SourceChanged(
            "track.flac".to_owned(),
        ));
        assert_eq!(classification.code, "normalization.source_changed");
        assert_eq!(classification.class, FailureClass::Retryable);
    }

    #[test]
    fn heartbeat_cadence_is_derived_from_short_lease() {
        assert_eq!(
            heartbeat_interval(::time::Duration::seconds(3)).unwrap(),
            StdDuration::from_secs(1)
        );
        assert_eq!(
            heartbeat_interval(::time::Duration::seconds(30)).unwrap(),
            StdDuration::from_secs(10)
        );
    }

    #[test]
    fn heartbeat_cadence_is_capped_well_below_long_lease() {
        assert_eq!(
            heartbeat_interval(::time::Duration::minutes(5)).unwrap(),
            StdDuration::from_secs(30)
        );
    }

    #[test]
    fn heartbeat_rejects_non_positive_or_sub_tick_lease() {
        assert!(heartbeat_interval(::time::Duration::ZERO).is_err());
        assert!(heartbeat_interval(::time::Duration::nanoseconds(2)).is_err());
    }

    #[test]
    fn classify_unexpected_error_is_retryable() {
        let classification = classify_normalization_error(&NormalizationError::WorkAreaCreation {
            path: PathBuf::from("/tmp"),
            message: "denied".to_owned(),
        });
        assert_eq!(classification.code, "normalization.execution_failed");
        assert_eq!(classification.class, FailureClass::Retryable);
    }
}

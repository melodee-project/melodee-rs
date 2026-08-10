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
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use ::time::OffsetDateTime;
use melodee_domain::{FailureClass, WorkErrorCode};
use melodee_media::{NormalizationError, NormalizationOptions, normalize_release};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use tokio::time::{self, Duration as TokioDuration};
use tracing::{error, info, warn};

use crate::{
    InboundSource, IngestionConfig, ReconcileResult, ReconcileStats,
    error::{IngestionError, Result},
    scheduler::StopToken,
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

    for item in items {
        match normalize_one(ingestion, store, pool, &worker, item, work_config, now).await {
            Ok(()) => stats.completed += 1,
            Err(IngestionError::Inspection(_)) => stats.failed += 1,
            Err(error) => {
                stats.errors += 1;
                error!(%error, "normalization item failed outside normal failure recording");
            }
        }
    }
    stats.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);
    info!(
        claimed = stats.claimed,
        completed = stats.completed,
        failed = stats.failed,
        "normalization batch completed"
    );
    Ok(stats)
}

async fn normalize_one(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: melodee_domain::ReleaseWorkItem,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<()> {
    let source = find_source(ingestion, &item);
    let Some(source) = source else {
        return record_normalization_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "normalization.source_not_found",
            "source library not configured for work item",
            FailureClass::Permanent,
            now,
        )
        .await
        .map(|_| ());
    };
    let source_root = match resolve_candidate(source, &item.source_path) {
        Ok(root) => root,
        Err(error) => {
            return record_normalization_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "normalization.source_unresolvable",
                &error,
                FailureClass::Permanent,
                now,
            )
            .await
            .map(|_| ());
        }
    };

    let stored = store.get_manifest(pool, item.id).await?;
    let manifest_row = stored.ok_or_else(|| {
        IngestionError::Inspection(melodee_media::InspectionError::SourceMissing(
            item.source_path.clone().into(),
        ))
    })?;
    let mut manifest: melodee_domain::ReleaseManifest =
        serde_json::from_value(manifest_row.payload).map_err(|e| {
            IngestionError::Inspection(melodee_media::InspectionError::Serialization(e))
        })?;

    let cancellation = std::sync::Arc::new(AtomicBool::new(false));
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
        cancellation: cancellation_for_task,
        ..NormalizationOptions::default()
    };

    let heartbeat_store = *store;
    let heartbeat_pool = pool.clone();
    let heartbeat_worker = worker.clone();
    let heartbeat_lease = work_config.lease_duration;
    let heartbeat_item_id = item.id;
    let heartbeat_cancellation = cancellation.clone();

    let heartbeat_handle = tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(60));
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

    let normalization_result = tokio::task::spawn_blocking(move || {
        normalize_release(&mut manifest, &source_root, &options)
    })
    .await;

    heartbeat_handle.abort();

    // If the heartbeat set the cancellation flag, the normalization may have
    // already returned Canceled; either way we must not publish.
    if cancellation.load(Ordering::SeqCst) {
        return Err(IngestionError::InspectionTask(
            "normalization canceled due to lease loss".to_owned(),
        ));
    }

    let report = match normalization_result {
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
                now,
            )
            .await
            .map(|_| ());
        }
        Err(join_error) => {
            return Err(IngestionError::InspectionTask(join_error.to_string()));
        }
    };

    let publish_result = store
        .publish_normalization(
            pool,
            item.id,
            worker,
            &report.manifest,
            &report.manifest_hash,
            &item.source_path,
            report.duration_ms,
            now,
        )
        .await;

    if let Err(error) = publish_result {
        match &error {
            WorkStoreError::LeaseExpired(_) | WorkStoreError::NotLeaseOwner(_) => {
                warn!(
                    work_item_id = %item.id,
                    %error,
                    "normalization publish rejected: lease lost"
                );
            }
            _ => {
                error!(%error, work_item_id = %item.id, "normalization publish failed");
                return Err(IngestionError::Persistence(error));
            }
        }
    }

    Ok(())
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

fn find_source<'a>(
    config: &'a IngestionConfig,
    item: &melodee_domain::ReleaseWorkItem,
) -> Option<&'a InboundSource> {
    config.sources.iter().find(|source| {
        source.id == item.source_library_id
            || (source.id.is_none() && item.source_library_id.is_none())
    })
}

fn resolve_candidate(
    source: &InboundSource,
    source_path: &str,
) -> std::result::Result<PathBuf, String> {
    let relative = Path::new(source_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err("source path is not a safe relative path".to_owned());
    }
    let root = source
        .root
        .canonicalize()
        .map_err(|error| format!("source root is unreadable: {error}"))?;
    let candidate = root.join(relative);
    let canonical = candidate
        .canonicalize()
        .map_err(|error| format!("source candidate is unreadable: {error}"))?;
    if canonical.strip_prefix(&root).is_err() {
        return Err("source candidate escapes configured inbound root".to_owned());
    }
    if !canonical.is_dir() {
        return Err("source candidate is not a directory".to_owned());
    }
    Ok(canonical)
}

/// Starts a graceful periodic normalization worker.
pub fn start_normalization_loop(
    ingestion: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    work_config: WorkEngineConfig,
) -> crate::SchedulerHandle {
    let stop = StopToken::new();
    let notifier = stop.signal.clone();
    let stop_for_task = stop.clone();
    let handle = tokio::spawn(async move {
        if !ingestion.enabled() {
            return Vec::new();
        }
        let interval = TokioDuration::try_from(ingestion.reconciliation_interval)
            .unwrap_or_else(|_| TokioDuration::from_secs(30));
        let mut ticker = time::interval(interval);
        let mut results = Vec::new();
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    match normalize_once(&ingestion, &store, &pool, &work_config, OffsetDateTime::now_utc()).await {
                        Ok(stats) => results.push(ReconcileResult {
                            source: "normalization".to_owned(),
                            stats: ReconcileStats {
                                duration_ms: stats.duration_ms,
                                errors: stats.errors,
                                ..ReconcileStats::default()
                            },
                        }),
                        Err(error) => error!(%error, "normalization loop failed"),
                    }
                }
                _ = notifier.notified() => break,
            }
        }
        results
    });
    crate::SchedulerHandle::from_parts(handle, stop_for_task)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn classify_unexpected_error_is_retryable() {
        let classification = classify_normalization_error(&NormalizationError::WorkAreaCreation {
            path: PathBuf::from("/tmp"),
            message: "denied".to_owned(),
        });
        assert_eq!(classification.code, "normalization.execution_failed");
        assert_eq!(classification.class, FailureClass::Retryable);
    }
}

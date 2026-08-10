//! Leased Release inspection and durable manifest handoff.

use std::{
    path::{Component, Path, PathBuf},
    time::Instant,
};

use ::time::OffsetDateTime;
use melodee_domain::{FailureClass, IngestionState, WorkErrorCode};
use melodee_media::{InspectionError, InspectionOptions, inspect_release};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use tokio::time::{self, Duration as TokioDuration};
use tracing::{error, info, warn};

use crate::{
    InboundSource, IngestionConfig,
    error::{IngestionError, Result},
    snapshot::snapshot_release_directory,
};

#[derive(Debug, Clone, Default)]
pub struct InspectionStats {
    pub claimed: u64,
    pub completed: u64,
    pub structurally_invalid: u64,
    pub source_changed: u64,
    pub failed: u64,
    pub errors: u64,
    pub duration_ms: i64,
}

/// Claims and inspects one bounded batch of Ready work. PostgreSQL owns
/// exclusivity; the configured batch is also the worker's backpressure bound.
pub async fn inspect_ready_once(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<InspectionStats> {
    let started = Instant::now();
    let mut claim_config = *work_config;
    claim_config.claim_batch_size = ingestion.inspection_concurrency;
    let worker = WorkerId::new();
    let items = store.claim(pool, &worker, &claim_config, now).await?;
    let mut stats = InspectionStats {
        claimed: items.len() as u64,
        ..InspectionStats::default()
    };

    for item in items {
        match inspect_one(ingestion, store, pool, &worker, item, work_config, now).await {
            Ok(InspectionOutcome::Published) => stats.completed += 1,
            Ok(InspectionOutcome::SourceChanged) => stats.source_changed += 1,
            Ok(InspectionOutcome::Invalid) => stats.structurally_invalid += 1,
            Ok(InspectionOutcome::Failed) => stats.failed += 1,
            Err(error) => {
                stats.errors += 1;
                error!(%error, "inspection item failed outside normal failure recording");
            }
        }
    }
    stats.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);
    info!(
        claimed = stats.claimed,
        completed = stats.completed,
        source_changed = stats.source_changed,
        structurally_invalid = stats.structurally_invalid,
        failed = stats.failed,
        "inspection batch completed"
    );
    Ok(stats)
}

#[derive(Debug, Clone, Copy)]
enum InspectionOutcome {
    Published,
    SourceChanged,
    Invalid,
    Failed,
}

async fn inspect_one(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: melodee_domain::ReleaseWorkItem,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<InspectionOutcome> {
    store
        .transition(
            pool,
            item.id,
            Some(worker),
            melodee_domain::TransitionKind::Worker,
            IngestionState::Inspecting,
            Some("inspection-claimed"),
            now,
        )
        .await?;

    let source = match find_source(ingestion, &item) {
        Some(source) => source,
        None => {
            return record_inspection_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "inspection.source_not_configured",
                "the work item source is no longer configured",
                FailureClass::Permanent,
                now,
            )
            .await
            .map(|_| InspectionOutcome::Failed);
        }
    };
    let root = match resolve_candidate(source, &item.source_path) {
        Ok(path) => path,
        Err(message) => {
            return record_inspection_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "inspection.source_path_invalid",
                &message,
                FailureClass::Permanent,
                now,
            )
            .await
            .map(|_| InspectionOutcome::Failed);
        }
    };

    let expected = store.latest_snapshot(pool, item.id).await?;
    let before = match snapshot_release_directory(&root, &ingestion.ready_marker_name) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return return_to_settling(
                store,
                pool,
                worker,
                item.id,
                &format!("source-unreadable: {error}"),
                now,
            )
            .await
            .map(|_| InspectionOutcome::SourceChanged);
        }
    };
    if expected
        .as_ref()
        .is_some_and(|snapshot| snapshot.signature != before.signature)
    {
        return return_to_settling(
            store,
            pool,
            worker,
            item.id,
            "source-changed-before-inspection",
            now,
        )
        .await
        .map(|_| InspectionOutcome::SourceChanged);
    }

    // Extend the lease immediately before entering the blocking media/tool
    // boundary. Publication also re-checks ownership, so an inspection that
    // outlives its lease can never publish stale authoritative state.
    store
        .renew_lease(pool, item.id, worker, work_config.lease_duration, now)
        .await?;

    let options = InspectionOptions {
        ffprobe_path: ingestion.ffprobe_path.clone(),
        ffprobe_timeout: std::time::Duration::try_from(ingestion.ffprobe_timeout)
            .map_err(|error| IngestionError::InspectionTask(error.to_string()))?,
        ..InspectionOptions::default()
    };
    let work_item_id = item.id;
    let source_library_id = item.source_library_id;
    let source_path = item.source_path.clone();
    let source_snapshot_id = expected.as_ref().map(|snapshot| snapshot.id);
    let source_signature = expected.as_ref().map(|snapshot| snapshot.signature.clone());
    let inspection_root = root.clone();
    let inspection_signature = source_signature.clone();
    let inspection_started = Instant::now();
    let report_result = tokio::task::spawn_blocking(move || {
        inspect_release(
            work_item_id,
            source_library_id,
            &source_path,
            &inspection_root,
            source_snapshot_id,
            inspection_signature.as_deref(),
            &options,
        )
    })
    .await
    .map_err(|error| IngestionError::InspectionTask(error.to_string()))?;
    let report = match report_result {
        Ok(report) => report,
        Err(
            error @ (InspectionError::SourceMissing(_) | InspectionError::SourceUnreadable { .. }),
        ) => {
            return return_to_settling(
                store,
                pool,
                worker,
                item.id,
                &format!("source-disappeared-during-inspection: {error}"),
                now,
            )
            .await
            .map(|_| InspectionOutcome::SourceChanged);
        }
        Err(error) => {
            return record_inspection_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "inspection.execution_failed",
                &error.to_string(),
                FailureClass::Retryable,
                now,
            )
            .await
            .map(|_| InspectionOutcome::Failed);
        }
    };

    let after = match snapshot_release_directory(&root, &ingestion.ready_marker_name) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return return_to_settling(
                store,
                pool,
                worker,
                item.id,
                &format!("source-changed-after-inspection: {error}"),
                now,
            )
            .await
            .map(|_| InspectionOutcome::SourceChanged);
        }
    };
    if after.signature != before.signature
        || expected
            .as_ref()
            .is_some_and(|snapshot| snapshot.signature != after.signature)
    {
        warn!(work_item_id = %item.id, "source changed during inspection; manifest discarded");
        return return_to_settling(
            store,
            pool,
            worker,
            item.id,
            "source-changed-during-inspection",
            now,
        )
        .await
        .map(|_| InspectionOutcome::SourceChanged);
    }
    if !report.structurally_valid {
        let reason = report
            .manifest
            .findings
            .iter()
            .filter(|finding| finding.severity == melodee_domain::FindingSeverity::Error)
            .map(|finding| finding.code.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let failure_class = if reason
            .split(',')
            .any(|code| code == "media.ffprobe_timeout")
        {
            FailureClass::Retryable
        } else {
            FailureClass::Permanent
        };
        return record_inspection_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "inspection.structural_invalid",
            &reason,
            failure_class,
            now,
        )
        .await
        .map(|_| InspectionOutcome::Invalid);
    }

    let generation_duration_ms = inspection_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX);
    store
        .publish_manifest(
            pool,
            item.id,
            worker,
            &report.manifest,
            &report.manifest_hash,
            source_snapshot_id,
            source_signature.as_deref(),
            generation_duration_ms,
            now,
        )
        .await?;
    Ok(InspectionOutcome::Published)
}

#[allow(clippy::too_many_arguments)]
async fn record_inspection_failure(
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

async fn return_to_settling(
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    id: uuid::Uuid,
    reason: &str,
    now: OffsetDateTime,
) -> Result<()> {
    store
        .transition(
            pool,
            id,
            Some(worker),
            melodee_domain::TransitionKind::Worker,
            IngestionState::Settling,
            Some(reason),
            now,
        )
        .await?;
    store.release_lease(pool, id, worker, now).await?;
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

/// Starts a graceful periodic inspection worker. Reconciliation and this loop
/// share PostgreSQL leases, so a process restart recovers an interrupted claim.
pub fn start_inspection_loop(
    ingestion: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    work_config: WorkEngineConfig,
) -> crate::SchedulerHandle {
    let stop = crate::StopToken::new();
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
                    match inspect_ready_once(&ingestion, &store, &pool, &work_config, OffsetDateTime::now_utc()).await {
                        Ok(stats) => results.push(crate::ReconcileResult {
                            source: "inspection".to_owned(),
                            stats: crate::ReconcileStats {
                                duration_ms: stats.duration_ms,
                                errors: stats.errors,
                                ..crate::ReconcileStats::default()
                            },
                        }),
                        Err(error) => error!(%error, "inspection loop failed"),
                    }
                }
                _ = notifier.notified() => break,
            }
        }
        results
    });
    crate::SchedulerHandle::from_parts(handle, stop_for_task)
}

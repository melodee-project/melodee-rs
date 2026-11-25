//! Leased Release inspection and durable manifest handoff.

use std::time::Instant;

use ::time::OffsetDateTime;
use melodee_domain::{FailureClass, IngestionState, WorkErrorCode};
use melodee_media::{InspectionError, InspectionOptions, inspect_release};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use tokio::time::{self, Duration as TokioDuration};
use tracing::{Instrument as _, error, info, warn};

use crate::{
    AcquisitionError, IngestionConfig, acquire_owned_source,
    error::{IngestionError, Result},
    gate::{DirectoryGateOutcome, DirectoryProcessingGate},
    snapshot::snapshot_release_directory,
    verify_owned_source,
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
    /// Candidates denied by the `directoryProcessingStart` script hook.
    /// Acquisition has already made staging authoritative, so a
    /// denied candidate remains staged as a retryable failed work item.
    pub script_skipped: u64,
}

/// Claims and inspects one bounded batch of Ready work. PostgreSQL owns
/// exclusivity; the configured batch is also the worker's backpressure bound.
/// `gate` is the scripting hook injection point (None = no hooks).
pub async fn inspect_ready_once(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
    gate: Option<&dyn DirectoryProcessingGate>,
) -> Result<InspectionStats> {
    let started = Instant::now();
    let mut claim_config = *work_config;
    claim_config.claim_batch_size = ingestion.inspection_concurrency;
    store
        .recover_expired_inspections(pool, &claim_config, now)
        .await?;
    let heartbeat_interval = crate::normalize::heartbeat_interval(work_config.lease_duration)?;
    let worker = WorkerId::new();
    let items = store.claim(pool, &worker, &claim_config, now).await?;
    let mut stats = InspectionStats {
        claimed: items.len() as u64,
        ..InspectionStats::default()
    };

    for item in items {
        // every event inside the item boundary inherits both the
        // work-item id and the durable correlation id from this span.
        let span = tracing::info_span!(
            "inspect_work_item",
            work_item_id = %item.id,
            correlation_id = %item.correlation_id,
        );
        // Hashing and media inspection can outlive one lease. Run renewal
        // independently of synchronous filesystem reads. Dropping the pass
        // (including shutdown) aborts its renewal task.
        let id = item.id;
        let mut heartbeat = tokio::task::JoinSet::new();
        let renewal_pool = pool.clone();
        let renewal_worker = worker.clone();
        let lease_duration = work_config.lease_duration;
        heartbeat.spawn(async move {
            loop {
                time::sleep(heartbeat_interval).await;
                if let Err(error) = WorkItemStore
                    .renew_lease(
                        &renewal_pool,
                        id,
                        &renewal_worker,
                        lease_duration,
                        now + started.elapsed(),
                    )
                    .await
                {
                    return error;
                }
            }
        });
        let inspection = inspect_one(
            ingestion,
            store,
            pool,
            &worker,
            item,
            work_config,
            now + started.elapsed(),
            gate,
        )
        .instrument(span);
        let outcome = tokio::select! {
            biased;
            result = inspection => result,
            lost = heartbeat.join_next() => match lost {
                Some(Ok(error)) => Err(error.into()),
                _ => Err(IngestionError::InspectionTask("inspection lease renewal stopped".into())),
            },
        };
        match outcome {
            Ok(InspectionOutcome::Published) => stats.completed += 1,
            Ok(InspectionOutcome::SourceChanged) => stats.source_changed += 1,
            Ok(InspectionOutcome::Invalid) => stats.structurally_invalid += 1,
            Ok(InspectionOutcome::Failed) => stats.failed += 1,
            Ok(InspectionOutcome::ScriptSkipped) => stats.script_skipped += 1,
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
        script_skipped = stats.script_skipped,
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
    ScriptSkipped,
}

#[allow(clippy::too_many_arguments)]
async fn inspect_one(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: melodee_domain::ReleaseWorkItem,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
    gate: Option<&dyn DirectoryProcessingGate>,
) -> Result<InspectionOutcome> {
    let started = Instant::now();
    store
        .transition(
            pool,
            item.id,
            Some(worker),
            melodee_domain::TransitionKind::Worker,
            IngestionState::Inspecting,
            Some("inspection-claimed"),
            now + started.elapsed(),
        )
        .await?;

    let expected = store.latest_snapshot(pool, item.id).await?;
    let owned = match acquire_owned_source(
        ingestion,
        store,
        pool,
        worker,
        &item,
        expected
            .as_ref()
            .map(|snapshot| snapshot.signature.as_str()),
        now + started.elapsed(),
    )
    .await
    {
        Ok(owned) => owned,
        Err(AcquisitionError::SourceChanged) => {
            return return_to_settling(
                store,
                pool,
                worker,
                item.id,
                "source-changed-during-staging-acquisition",
                now + started.elapsed(),
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
                "inspection.staging_handoff_failed",
                &error.to_string(),
                FailureClass::Retryable,
                now + started.elapsed(),
            )
            .await
            .map(|_| InspectionOutcome::Failed);
        }
    };
    if let Err(error) = verify_owned_source(&owned) {
        return record_inspection_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "inspection.staged_source_integrity",
            &error.to_string(),
            FailureClass::Permanent,
            now + started.elapsed(),
        )
        .await
        .map(|_| InspectionOutcome::Failed);
    }
    let root = owned.root.clone();
    let before = match snapshot_release_directory(&root, &ingestion.ready_marker_name) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return record_inspection_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "inspection.staged_source_unreadable",
                &error.to_string(),
                FailureClass::Permanent,
                now + started.elapsed(),
            )
            .await
            .map(|_| InspectionOutcome::SourceChanged);
        }
    };
    // `directoryProcessingStart` hook: the candidate is stable (the
    // snapshot signature was just re-verified) and about to enter media
    // inspection — the Rust pipeline point equivalent to the legacy
    // pre-plugin-processing gate. The boundary is infallible by contract
    // (fail-degraded allow; ADR-0135 §4). Deny is a retryable failure after
    // durable acquisition: the source remains in staging and is never moved
    // back into the inbound directory. Physical quarantine moves are deferred
    // (durable decision contract); legacy `delete` stays rewritten
    // to deny.
    if let Some(gate) = gate {
        match gate.evaluate(&root, &before).await {
            DirectoryGateOutcome::Allow => {}
            DirectoryGateOutcome::Skip { reason } => {
                info!(
                    work_item_id = %item.id,
                    source_path = %item.source_path,
                    %reason,
                    "directory processing skipped by directoryProcessingStart hook"
                );
                return record_inspection_failure(
                    store,
                    pool,
                    worker,
                    item.id,
                    work_config,
                    "inspection.script_denied",
                    &reason,
                    FailureClass::Retryable,
                    now + started.elapsed(),
                )
                .await
                .map(|_| InspectionOutcome::ScriptSkipped);
            }
        }
    }

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
            return record_inspection_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "inspection.staged_source_changed",
                &error.to_string(),
                FailureClass::Permanent,
                now + started.elapsed(),
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
                now + started.elapsed(),
            )
            .await
            .map(|_| InspectionOutcome::Failed);
        }
    };

    let after = match snapshot_release_directory(&root, &ingestion.ready_marker_name) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return record_inspection_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "inspection.staged_source_unreadable",
                &error.to_string(),
                FailureClass::Permanent,
                now + started.elapsed(),
            )
            .await
            .map(|_| InspectionOutcome::SourceChanged);
        }
    };
    if after.signature != before.signature || verify_owned_source(&owned).is_err() {
        warn!(work_item_id = %item.id, "source changed during inspection; manifest discarded");
        return record_inspection_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "inspection.staged_source_changed",
            "staged source changed during inspection",
            FailureClass::Permanent,
            now + started.elapsed(),
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
            now + started.elapsed(),
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
            now + started.elapsed(),
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

/// Starts a graceful periodic inspection worker. Reconciliation and this loop
/// share PostgreSQL leases, so a process restart recovers an interrupted claim.
/// `gate` is the scripting hook injection point (None = no hooks).
pub fn start_inspection_loop(
    ingestion: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    work_config: WorkEngineConfig,
    gate: Option<std::sync::Arc<dyn DirectoryProcessingGate>>,
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
        // Drain a full batch instead of sleeping out the tick: the interval is the
        // idle cadence, not a throughput ceiling.
        let claim_batch_size = u64::from(ingestion.inspection_concurrency);
        let mut ticker = time::interval(interval);
        let mut results = Vec::new();
        let stop_requested = notifier.notified();
        tokio::pin!(stop_requested);
        'scheduler: loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = &mut stop_requested => break,
            }
            loop {
                tokio::select! {
                    result = inspect_ready_once(&ingestion, &store, &pool, &work_config, OffsetDateTime::now_utc(), gate.as_deref()) => {
                        match result {
                            Ok(stats) => {
                                let full_batch = stats.errors == 0 && stats.claimed >= claim_batch_size;
                                results.push(crate::ReconcileResult {
                                    source: "inspection".to_owned(),
                                    stats: crate::ReconcileStats {
                                        duration_ms: stats.duration_ms,
                                        errors: stats.errors,
                                        ..crate::ReconcileStats::default()
                                    },
                                });
                                if !full_batch {
                                    break;
                                }
                            }
                            Err(error) => {
                                error!(%error, "inspection loop failed");
                                break;
                            }
                        }
                    }
                    _ = &mut stop_requested => break 'scheduler,
                };
                tokio::task::yield_now().await;
            }
        }
        results
    });
    crate::SchedulerHandle::from_parts(handle, stop_for_task)
}

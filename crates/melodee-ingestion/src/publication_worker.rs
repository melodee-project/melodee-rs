//! Leased validation and canonical-publication workers.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use ::time::OffsetDateTime;
use melodee_domain::{
    FailureClass, PublicationAuthorization, PublicationJournalStep, ReleaseId, ReleaseManifest,
    ValidationOutcome, WorkErrorCode,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use tokio::{
    sync::mpsc,
    task::JoinSet,
    time::{self, Duration as TokioDuration, MissedTickBehavior},
};
use tracing::{Instrument as _, error, info, warn};

use crate::{
    IngestionConfig, PublicationBundle, PublicationError, PublicationMilestone, PublicationOutcome,
    ReconcileResult, ReconcileStats, SchedulerHandle, StopToken, StructuralValidationError,
    error::{IngestionError, Result},
    normalize::heartbeat_interval,
    publication::build_publication_bundle,
    validate::{derive_work_root, validate_normalized_manifest_with_cancellation},
};

#[cfg(not(feature = "test-support"))]
use crate::publication::publish_canonical_bundle_with_observer;
#[cfg(feature = "test-support")]
use crate::publication::publish_canonical_bundle_with_observer_and_test_barrier;

#[derive(Debug, Clone, Default)]
pub struct ValidationStats {
    pub claimed: u64,
    pub completed: u64,
    pub needs_review: u64,
    pub failed: u64,
    pub errors: u64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct PublicationStats {
    pub claimed: u64,
    pub completed: u64,
    pub adopted: u64,
    pub failed: u64,
    pub errors: u64,
    pub duration_ms: i64,
}

pub async fn validate_once(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<ValidationStats> {
    validate_once_with_shutdown(
        ingestion,
        store,
        pool,
        work_config,
        now,
        Arc::new(AtomicBool::new(false)),
    )
    .await
}

async fn validate_once_with_shutdown(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
    shutdown: Arc<AtomicBool>,
) -> Result<ValidationStats> {
    if validated_publication_roots(ingestion, work_config)
        .await?
        .is_none()
    {
        return Ok(ValidationStats::default());
    }
    let started = Instant::now();
    let mut claim_config = *work_config;
    claim_config.claim_batch_size = ingestion.validation_concurrency;
    let worker = WorkerId::new();
    let items = store
        .claim_validating(pool, &worker, &claim_config, now)
        .await?;
    let mut stats = ValidationStats {
        claimed: items.len() as u64,
        ..ValidationStats::default()
    };
    let mut tasks = JoinSet::new();
    for item in items {
        let item_id = item.id;
        let task_ingestion = ingestion.clone();
        let task_store = *store;
        let task_pool = pool.clone();
        let task_worker = worker.clone();
        let task_config = *work_config;
        let task_shutdown = shutdown.clone();
        let span = tracing::info_span!(
            "validate_work_item",
            work_item_id = %item.id,
            correlation_id = %item.correlation_id,
        );
        tasks.spawn(
            async move {
                let result = validate_one(
                    &task_ingestion,
                    &task_store,
                    &task_pool,
                    &task_worker,
                    item,
                    &task_config,
                    task_shutdown,
                )
                .await;
                (item_id, result)
            }
            .instrument(span),
        );
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((_, Ok(ValidationWorkerOutcome::Completed))) => stats.completed += 1,
            Ok((_, Ok(ValidationWorkerOutcome::NeedsReview))) => stats.needs_review += 1,
            Ok((_, Ok(ValidationWorkerOutcome::Failed))) => stats.failed += 1,
            Ok((id, Err(error))) => {
                stats.errors += 1;
                error!(work_item_id = %id, %error, "validation item failed outside durable failure recording");
                release_lease_after_unexpected_error(store, pool, id, &worker, "validation").await;
            }
            Err(error) => {
                stats.errors += 1;
                error!(%error, "validation task failed to join");
            }
        }
    }
    stats.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);
    info!(
        claimed = stats.claimed,
        completed = stats.completed,
        needs_review = stats.needs_review,
        failed = stats.failed,
        "validation batch completed"
    );
    Ok(stats)
}

#[derive(Debug, Clone, Copy)]
enum ValidationWorkerOutcome {
    Completed,
    NeedsReview,
    Failed,
}

async fn validate_one(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: melodee_domain::ReleaseWorkItem,
    work_config: &WorkEngineConfig,
    shutdown: Arc<AtomicBool>,
) -> Result<ValidationWorkerOutcome> {
    let stored = store.get_manifest(pool, item.id).await?;
    let Some(stored) = stored else {
        record_failure(
            store,
            pool,
            worker,
            item.id,
            work_config,
            "validation.manifest_missing",
            "validating work item has no normalized manifest",
            FailureClass::Permanent,
        )
        .await?;
        return Ok(ValidationWorkerOutcome::Failed);
    };
    let manifest: ReleaseManifest = match serde_json::from_value(stored.payload) {
        Ok(manifest) => manifest,
        Err(error) => {
            record_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "validation.manifest_invalid",
                &format!("stored normalized manifest is invalid: {error}"),
                FailureClass::Permanent,
            )
            .await?;
            return Ok(ValidationWorkerOutcome::Failed);
        }
    };
    let cancellation = Arc::new(AtomicBool::new(false));
    let task_cancellation = cancellation.clone();
    let task_shutdown = shutdown.clone();
    let work_area = ingestion.work_area_base.clone();
    let task_manifest = manifest.clone();
    let permit = admit_with_heartbeat(store, pool, item.id, worker, work_config, &shutdown).await?;
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if task_shutdown.load(Ordering::SeqCst) {
            task_cancellation.store(true, Ordering::SeqCst);
        }
        validate_normalized_manifest_with_cancellation(
            &task_manifest,
            &work_area,
            &task_cancellation,
        )
    });
    let validation = await_with_heartbeat(
        handle,
        store,
        pool,
        item.id,
        worker,
        work_config,
        cancellation,
        shutdown,
    )
    .await?;
    let validated = match validation {
        Ok(validated) => validated,
        Err(StructuralValidationError::Canceled) => {
            store
                .release_lease(pool, item.id, worker, OffsetDateTime::now_utc())
                .await?;
            return Ok(ValidationWorkerOutcome::Failed);
        }
        Err(error) => {
            let (code, class) = classify_validation_error(&error);
            record_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                code,
                &error.to_string(),
                class,
            )
            .await?;
            return Ok(ValidationWorkerOutcome::Failed);
        }
    };
    if validated.validation.outcome == ValidationOutcome::NeedsReview {
        store
            .record_validation_review(
                pool,
                item.id,
                worker,
                &validated.validation,
                OffsetDateTime::now_utc(),
            )
            .await?;
        return Ok(ValidationWorkerOutcome::NeedsReview);
    }

    let bundle = match build_publication_bundle(
        &manifest,
        &stored.manifest_hash,
        ReleaseId::new(),
        PublicationAuthorization::phase3_structural(),
        validated,
    ) {
        Ok(bundle) => bundle,
        Err(error) => {
            record_failure(
                store,
                pool,
                worker,
                item.id,
                work_config,
                "validation.publication_plan_invalid",
                &error.to_string(),
                FailureClass::Permanent,
            )
            .await?;
            return Ok(ValidationWorkerOutcome::Failed);
        }
    };
    store
        .stage_authorized_publication(
            pool,
            item.id,
            worker,
            &bundle.manifest,
            &bundle.plan,
            OffsetDateTime::now_utc(),
        )
        .await?;
    Ok(ValidationWorkerOutcome::Completed)
}

pub async fn publish_once(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<PublicationStats> {
    publish_once_with_shutdown(
        ingestion,
        store,
        pool,
        work_config,
        now,
        Arc::new(AtomicBool::new(false)),
    )
    .await
}

async fn publish_once_with_shutdown(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
    shutdown: Arc<AtomicBool>,
) -> Result<PublicationStats> {
    let Some(roots) = validated_publication_roots(ingestion, work_config).await? else {
        return Ok(PublicationStats::default());
    };
    let started = Instant::now();
    let mut claim_config = *work_config;
    claim_config.claim_batch_size = ingestion.publication_concurrency;
    let worker = WorkerId::new();
    let items = store
        .claim_publication(pool, &worker, &claim_config, now)
        .await?;
    let mut stats = PublicationStats {
        claimed: items.len() as u64,
        ..PublicationStats::default()
    };
    let mut tasks = JoinSet::new();
    for item in items {
        let item_id = item.id;
        let task_ingestion = ingestion.clone();
        let task_store = *store;
        let task_pool = pool.clone();
        let task_worker = worker.clone();
        let task_config = *work_config;
        let task_shutdown = shutdown.clone();
        let storage_root = roots.storage_root.clone();
        let span = tracing::info_span!(
            "publish_work_item",
            work_item_id = %item.id,
            correlation_id = %item.correlation_id,
        );
        tasks.spawn(
            async move {
                let result = publish_one(
                    &task_ingestion,
                    &task_store,
                    &task_pool,
                    &task_worker,
                    item,
                    &task_config,
                    &storage_root,
                    task_shutdown,
                )
                .await;
                (item_id, result)
            }
            .instrument(span),
        );
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((_, Ok(PublicationWorkerOutcome::Completed))) => stats.completed += 1,
            Ok((_, Ok(PublicationWorkerOutcome::Adopted))) => {
                stats.completed += 1;
                stats.adopted += 1;
            }
            Ok((_, Ok(PublicationWorkerOutcome::Failed))) => stats.failed += 1,
            Ok((id, Err(error))) => {
                stats.errors += 1;
                error!(work_item_id = %id, %error, "publication item failed outside durable failure recording");
                release_lease_after_unexpected_error(store, pool, id, &worker, "publication").await;
            }
            Err(error) => {
                stats.errors += 1;
                error!(%error, "publication task failed to join");
            }
        }
    }
    stats.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);
    info!(
        claimed = stats.claimed,
        completed = stats.completed,
        adopted = stats.adopted,
        failed = stats.failed,
        "publication batch completed"
    );
    Ok(stats)
}

async fn release_lease_after_unexpected_error(
    store: &WorkItemStore,
    pool: &PgPool,
    id: uuid::Uuid,
    worker: &WorkerId,
    stage: &'static str,
) {
    if let Err(release_error) = store
        .release_lease(pool, id, worker, OffsetDateTime::now_utc())
        .await
    {
        error!(
            work_item_id = %id,
            %release_error,
            stage,
            "failed to release lease after unexpected per-item error"
        );
    }
}

#[derive(Debug, Clone, Copy)]
enum PublicationWorkerOutcome {
    Completed,
    Adopted,
    Failed,
}

#[allow(clippy::too_many_arguments)]
async fn publish_one(
    ingestion: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: melodee_domain::ReleaseWorkItem,
    work_config: &WorkEngineConfig,
    storage_root: &std::path::Path,
    shutdown: Arc<AtomicBool>,
) -> Result<PublicationWorkerOutcome> {
    let Some(stored_publication) = store.get_publication(pool, item.id).await? else {
        record_publication_problem(
            store,
            pool,
            item.id,
            worker,
            PublicationJournalStep::Failed,
            "publication.failed",
            "publishing work item has no durable publication plan",
        )
        .await?;
        return Ok(PublicationWorkerOutcome::Failed);
    };
    let Some(stored_manifest) = store.get_manifest(pool, item.id).await? else {
        record_publication_problem(
            store,
            pool,
            item.id,
            worker,
            PublicationJournalStep::Failed,
            "publication.failed",
            "publishing work item has no durable v3 manifest",
        )
        .await?;
        return Ok(PublicationWorkerOutcome::Failed);
    };
    let manifest: ReleaseManifest = match serde_json::from_value(stored_manifest.payload) {
        Ok(manifest) => manifest,
        Err(error) => {
            record_publication_problem(
                store,
                pool,
                item.id,
                worker,
                PublicationJournalStep::Failed,
                "publication.failed",
                &format!("stored v3 manifest is invalid: {error}"),
            )
            .await?;
            return Ok(PublicationWorkerOutcome::Failed);
        }
    };
    let work_root = match derive_work_root(&manifest, &ingestion.work_area_base) {
        Ok(work_root) => work_root,
        Err(error) => {
            record_publication_problem(
                store,
                pool,
                item.id,
                worker,
                PublicationJournalStep::Failed,
                "publication.failed",
                &error.to_string(),
            )
            .await?;
            return Ok(PublicationWorkerOutcome::Failed);
        }
    };
    let bundle = PublicationBundle {
        manifest,
        plan: stored_publication.plan,
        plan_hash: stored_publication.plan_hash,
        work_root,
    };
    let cancellation = Arc::new(AtomicBool::new(false));
    let (milestone_tx, mut milestone_rx) = mpsc::channel(64);
    let task_bundle = bundle.clone();
    let task_root = storage_root.to_path_buf();
    let task_cancellation = cancellation.clone();
    let observer_cancellation = cancellation.clone();
    let nfs_io_budget = ingestion.nfs_io_budget.clone();
    #[cfg(feature = "test-support")]
    let copy_test_barrier = ingestion.publication_copy_test_barrier.clone();
    let permit = admit_with_heartbeat(store, pool, item.id, worker, work_config, &shutdown).await?;
    let mut handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        #[cfg(feature = "test-support")]
        {
            publish_canonical_bundle_with_observer_and_test_barrier(
                &task_root,
                &task_bundle,
                &task_cancellation,
                &nfs_io_budget,
                |milestone| {
                    if milestone_tx.blocking_send(milestone).is_err() {
                        observer_cancellation.store(true, Ordering::SeqCst);
                    }
                },
                copy_test_barrier.as_ref(),
            )
        }
        #[cfg(not(feature = "test-support"))]
        publish_canonical_bundle_with_observer(
            &task_root,
            &task_bundle,
            &task_cancellation,
            &nfs_io_budget,
            |milestone| {
                if milestone_tx.blocking_send(milestone).is_err() {
                    observer_cancellation.store(true, Ordering::SeqCst);
                }
            },
        )
    });
    let mut heartbeat = time::interval(heartbeat_interval(work_config.lease_duration)?);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut shutdown_poll = time::interval(TokioDuration::from_millis(100));
    shutdown_poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    shutdown_poll.tick().await;
    let publication_result = loop {
        tokio::select! {
            result = &mut handle => break result.map_err(|error| IngestionError::InspectionTask(error.to_string()))?,
            milestone = milestone_rx.recv() => {
                if let Some(milestone) = milestone
                    && let Err(error) = record_milestone(store, pool, item.id, worker, &bundle.plan_hash, milestone).await
                {
                    cancellation.store(true, Ordering::SeqCst);
                    reap_publication_task(&mut handle, &mut milestone_rx).await;
                    return Err(error);
                }
            }
            _ = heartbeat.tick() => {
                if let Err(error) = store.renew_lease(
                    pool,
                    item.id,
                    worker,
                    work_config.lease_duration,
                    OffsetDateTime::now_utc(),
                ).await {
                    cancellation.store(true, Ordering::SeqCst);
                    reap_publication_task(&mut handle, &mut milestone_rx).await;
                    return Err(IngestionError::Persistence(error));
                }
            }
            _ = shutdown_poll.tick() => {
                if shutdown.load(Ordering::SeqCst) {
                    cancellation.store(true, Ordering::SeqCst);
                }
            }
        }
    };
    while let Ok(milestone) = milestone_rx.try_recv() {
        record_milestone(store, pool, item.id, worker, &bundle.plan_hash, milestone).await?;
    }

    match publication_result {
        Ok(receipt) => {
            store
                .complete_publication(
                    pool,
                    item.id,
                    worker,
                    &bundle.plan_hash,
                    &receipt.sealed_manifest_hash,
                    OffsetDateTime::now_utc(),
                )
                .await?;
            Ok(match receipt.outcome {
                PublicationOutcome::Published => PublicationWorkerOutcome::Completed,
                PublicationOutcome::Adopted => PublicationWorkerOutcome::Adopted,
            })
        }
        Err(error) => {
            let conflict = matches!(error, PublicationError::TargetConflict { .. });
            let step = if conflict {
                PublicationJournalStep::TargetConflict
            } else {
                PublicationJournalStep::Failed
            };
            let code = if conflict {
                "publication.target_conflict"
            } else {
                "publication.failed"
            };
            record_publication_problem(
                store,
                pool,
                item.id,
                worker,
                step,
                code,
                &error.to_string(),
            )
            .await?;
            Ok(PublicationWorkerOutcome::Failed)
        }
    }
}

async fn record_milestone(
    store: &WorkItemStore,
    pool: &PgPool,
    id: uuid::Uuid,
    worker: &WorkerId,
    plan_hash: &str,
    milestone: PublicationMilestone,
) -> Result<()> {
    let key = match (&milestone.step, milestone.asset_path.as_deref()) {
        (PublicationJournalStep::AssetCopied, Some(asset)) => {
            format!("asset:{asset}")
        }
        (step, _) => format!("{}:{plan_hash}", step.as_str()),
    };
    store
        .record_publication_progress(
            pool,
            id,
            worker,
            &key,
            milestone.step,
            milestone.asset_path.as_deref(),
            milestone.size_bytes,
            milestone.byte_hash.as_deref(),
            None,
            OffsetDateTime::now_utc(),
        )
        .await?;
    Ok(())
}

async fn record_publication_problem(
    store: &WorkItemStore,
    pool: &PgPool,
    id: uuid::Uuid,
    worker: &WorkerId,
    step: PublicationJournalStep,
    code: &str,
    message: &str,
) -> Result<()> {
    store
        .record_publication_problem(
            pool,
            id,
            worker,
            step,
            WorkErrorCode::new(code),
            message,
            None,
            OffsetDateTime::now_utc(),
        )
        .await?;
    Ok(())
}

async fn reap_publication_task<T>(
    handle: &mut tokio::task::JoinHandle<T>,
    milestones: &mut mpsc::Receiver<PublicationMilestone>,
) {
    loop {
        tokio::select! {
            _result = &mut *handle => return,
            milestone = milestones.recv() => {
                if milestone.is_none() {
                    let _result = handle.await;
                    return;
                }
            }
        }
    }
}

// Workers own leases while waiting for scarce blocking capacity. Continue
// renewing them without scheduling blocking work or holding another permit.
async fn admit_with_heartbeat(
    store: &WorkItemStore,
    pool: &PgPool,
    id: uuid::Uuid,
    worker: &WorkerId,
    work_config: &WorkEngineConfig,
    shutdown: &AtomicBool,
) -> Result<tokio::sync::OwnedSemaphorePermit> {
    let admission = crate::admission::heavy_ingestion();
    tokio::pin!(admission);
    let mut heartbeat = time::interval(heartbeat_interval(work_config.lease_duration)?);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut shutdown_poll = time::interval(TokioDuration::from_millis(100));
    shutdown_poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        if shutdown.load(Ordering::SeqCst) {
            store
                .release_lease(pool, id, worker, OffsetDateTime::now_utc())
                .await?;
            return Err(IngestionError::InspectionTask(
                "shutdown while awaiting ingestion admission".to_owned(),
            ));
        }
        tokio::select! {
            result = &mut admission => return result.map_err(|error| IngestionError::InspectionTask(error.to_string())),
            _ = heartbeat.tick() => {
                store.renew_lease(pool, id, worker, work_config.lease_duration, OffsetDateTime::now_utc()).await?;
            }
            _ = shutdown_poll.tick() => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn await_with_heartbeat<T>(
    mut handle: tokio::task::JoinHandle<T>,
    store: &WorkItemStore,
    pool: &PgPool,
    id: uuid::Uuid,
    worker: &WorkerId,
    work_config: &WorkEngineConfig,
    cancellation: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
) -> Result<T> {
    let mut heartbeat = time::interval(heartbeat_interval(work_config.lease_duration)?);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut shutdown_poll = time::interval(TokioDuration::from_millis(100));
    shutdown_poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    shutdown_poll.tick().await;
    loop {
        tokio::select! {
            result = &mut handle => return result.map_err(|error| IngestionError::InspectionTask(error.to_string())),
            _ = heartbeat.tick() => {
                if let Err(error) = store.renew_lease(
                    pool,
                    id,
                    worker,
                    work_config.lease_duration,
                    OffsetDateTime::now_utc(),
                ).await {
                    cancellation.store(true, Ordering::SeqCst);
                    let _ = handle.await;
                    return Err(IngestionError::Persistence(error));
                }
            }
            _ = shutdown_poll.tick() => {
                if shutdown.load(Ordering::SeqCst) {
                    cancellation.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

async fn validated_publication_roots(
    ingestion: &IngestionConfig,
    work_config: &WorkEngineConfig,
) -> Result<Option<crate::PublicationRoots>> {
    if !ingestion.publication_enabled() {
        return Ok(None);
    }
    work_config
        .validate()
        .map_err(|error| IngestionError::InspectionTask(error.to_string()))?;
    if ingestion.validation_concurrency == 0 || ingestion.publication_concurrency == 0 {
        return Err(IngestionError::InspectionTask(
            "validation and publication concurrency must be greater than zero".to_owned(),
        ));
    }
    // Proving path containment involves filesystem calls. An unavailable mount
    // must not pin a Tokio executor thread while it waits on the kernel.
    let config = ingestion.clone();
    Ok(
        tokio::task::spawn_blocking(move || config.publication_roots())
            .await
            .map_err(|error| IngestionError::InspectionTask(error.to_string()))??,
    )
}

fn classify_validation_error(error: &StructuralValidationError) -> (&'static str, FailureClass) {
    match error {
        StructuralValidationError::Io { .. }
        | StructuralValidationError::SizeMismatch { .. }
        | StructuralValidationError::HashMismatch { .. } => {
            ("validation.asset_mismatch", FailureClass::Retryable)
        }
        StructuralValidationError::Canceled => ("validation.canceled", FailureClass::Retryable),
        _ => ("validation.manifest_invalid", FailureClass::Permanent),
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_failure(
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    id: uuid::Uuid,
    work_config: &WorkEngineConfig,
    code: &str,
    message: &str,
    class: FailureClass,
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
            OffsetDateTime::now_utc(),
        )
        .await?;
    Ok(())
}

pub fn start_validation_loop(
    ingestion: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    work_config: WorkEngineConfig,
) -> SchedulerHandle {
    start_stage_loop("validation", ingestion, pool, store, work_config, true)
}

pub fn start_publication_loop(
    ingestion: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    work_config: WorkEngineConfig,
) -> SchedulerHandle {
    start_stage_loop("publication", ingestion, pool, store, work_config, false)
}

enum AfterBatch {
    Drain,
    Poll,
    BackOff { made_progress: bool },
}

impl AfterBatch {
    fn from_claims(claimed: u64, limit: u64) -> Self {
        if claimed >= limit {
            Self::Drain
        } else {
            Self::Poll
        }
    }
}

fn start_stage_loop(
    stage: &'static str,
    ingestion: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    work_config: WorkEngineConfig,
    validation: bool,
) -> SchedulerHandle {
    let stop = StopToken::new();
    let stop_requested = stop.signal.clone().notified_owned();
    let handle = tokio::spawn(async move {
        if !ingestion.publication_enabled() {
            return Vec::new();
        }
        let claim_batch_size = u64::from(if validation {
            ingestion.validation_concurrency
        } else {
            ingestion.publication_concurrency
        });
        let interval = TokioDuration::try_from(ingestion.reconciliation_interval)
            .unwrap_or_else(|_| TokioDuration::from_secs(30));
        let mut ticker = time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let retry_base =
            TokioDuration::try_from(work_config.retry_policy.base_delay).unwrap_or(interval);
        let retry_max =
            TokioDuration::try_from(work_config.retry_policy.max_delay).unwrap_or(retry_base);
        let mut retry_delay = retry_base;
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
            // Drain a full batch rather than sleeping out the tick: the interval is
            // the idle cadence, not a throughput ceiling.
            let mut stopping = false;
            loop {
                let shutdown = Arc::new(AtomicBool::new(false));
                let batch = async {
                    if validation {
                        validate_once_with_shutdown(
                            &ingestion,
                            &store,
                            &pool,
                            &work_config,
                            OffsetDateTime::now_utc(),
                            shutdown.clone(),
                        )
                        .await
                        .map(|stats| {
                            (
                                stats.duration_ms,
                                stats.errors,
                                if stats.errors > 0 {
                                    AfterBatch::Poll
                                } else {
                                    AfterBatch::from_claims(stats.claimed, claim_batch_size)
                                },
                            )
                        })
                    } else {
                        publish_once_with_shutdown(
                            &ingestion,
                            &store,
                            &pool,
                            &work_config,
                            OffsetDateTime::now_utc(),
                            shutdown.clone(),
                        )
                        .await
                        .map(|stats| {
                            let next = if stats.failed > 0 || stats.errors > 0 {
                                AfterBatch::BackOff {
                                    made_progress: stats.completed > 0,
                                }
                            } else {
                                AfterBatch::from_claims(stats.claimed, claim_batch_size)
                            };
                            (stats.duration_ms, stats.errors, next)
                        })
                    }
                };
                tokio::pin!(batch);
                let (interrupted, result) = tokio::select! {
                    result = &mut batch => (false, result),
                    _ = &mut stop_requested => {
                        shutdown.store(true, Ordering::SeqCst);
                        (true, batch.await)
                    }
                };
                let next = match result {
                    Ok((duration_ms, errors, next)) => {
                        crate::scheduler::retain_result(
                            &mut results,
                            ReconcileResult {
                                source: stage.to_owned(),
                                stats: ReconcileStats {
                                    duration_ms,
                                    errors,
                                    ..ReconcileStats::default()
                                },
                            },
                        );
                        next
                    }
                    Err(error) => {
                        error!(%error, stage, "ingestion stage loop failed");
                        if validation {
                            AfterBatch::Poll
                        } else {
                            AfterBatch::BackOff {
                                made_progress: false,
                            }
                        }
                    }
                };
                if interrupted {
                    stopping = true;
                    break;
                }
                if let AfterBatch::BackOff { made_progress } = next {
                    if made_progress {
                        retry_delay = retry_base;
                    }
                    let delay = retry_delay.max(interval);
                    warn!(
                        stage,
                        ?delay,
                        "pausing publication claims after a failed batch"
                    );
                    ticker.reset_after(delay);
                    retry_delay = retry_delay.saturating_mul(2).min(retry_max);
                    break;
                }
                retry_delay = retry_base;
                if matches!(next, AfterBatch::Poll) {
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
    SchedulerHandle::from_parts(handle, stop)
}

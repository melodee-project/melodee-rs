//! Periodic reconciliation scheduler.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use melodee_persistence::WorkItemStore;
use sqlx::PgPool;
use tokio::time::{self, Duration as TokioDuration};
use tracing::{error, info, warn};

use crate::{ReconcileResult, config::IngestionConfig};

#[derive(Debug, Clone)]
pub struct StopToken {
    pub(crate) signal: Arc<tokio::sync::Notify>,
}

impl StopToken {
    pub fn new() -> Self {
        Self {
            signal: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn stop(&self) {
        // Each scheduler owns exactly one stop consumer. `notify_one` retains
        // a permit when that consumer is busy, unlike `notify_waiters`; a stop
        // during a reconciliation pass must not disappear before its next poll.
        self.signal.notify_one();
    }
}

impl Default for StopToken {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct SchedulerHandle {
    join: tokio::task::JoinHandle<Vec<ReconcileResult>>,
    stop: StopToken,
}

/// Upper bound on process-lifetime retained scheduler results.
///
/// Production reads these only at shutdown and the durable lease/job history
/// is the diagnostic record, so retaining one result per pass for the whole
/// process lifetime is pure growth (measured ~1.2-1.4 MB/day per enabled
/// stage). A small ring keeps a shutdown snapshot useful without unbounded
/// growth. It does not replace persistent lease-event history.
pub const MAX_RETAINED_SCHEDULER_RESULTS: usize = 64;

/// Appends a result to the bounded retention ring.
pub(crate) fn retain_result(results: &mut Vec<ReconcileResult>, result: ReconcileResult) {
    if results.len() >= MAX_RETAINED_SCHEDULER_RESULTS {
        results.remove(0);
    }
    results.push(result);
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
#[derive(Clone)]
struct RetainedScheduleOptions {
    next_check: ::time::Duration,
    retry_after: ::time::Duration,
    idle_delay: TokioDuration,
    hook: Option<Arc<crate::custody::RetainedObservationHook>>,
    #[cfg(feature = "test-support")]
    force_unsupported_directory: bool,
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
impl Default for RetainedScheduleOptions {
    fn default() -> Self {
        Self {
            next_check: ::time::Duration::minutes(5),
            retry_after: ::time::Duration::seconds(30),
            idle_delay: TokioDuration::from_secs(5),
            hook: None,
            #[cfg(feature = "test-support")]
            force_unsupported_directory: false,
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
#[derive(Default)]
struct RetainedScheduleOptions;

/// One independently budgeted retained-source turn.
#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
pub(crate) async fn retained_recheck_turn(
    config: &IngestionConfig,
    pool: &PgPool,
    store: &WorkItemStore,
    worker: &melodee_persistence::WorkerId,
) -> Result<bool, melodee_persistence::WorkStoreError> {
    retained_recheck_turn_inner(
        config,
        pool,
        store,
        worker,
        &RetainedScheduleOptions::default(),
    )
    .await
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
async fn retained_recheck_turn_inner(
    config: &IngestionConfig,
    pool: &PgPool,
    store: &WorkItemStore,
    worker: &melodee_persistence::WorkerId,
    options: &RetainedScheduleOptions,
) -> Result<bool, melodee_persistence::WorkStoreError> {
    let duration = ::time::Duration::minutes(2);
    let mut leases = store
        .claim_due_retained_sources(pool, worker, 1, duration)
        .await?;
    let Some(lease) = leases.pop() else {
        return Ok(false);
    };
    let observation = if lease.baseline_id().is_some() {
        if lease.baseline_directory_turn() {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                #[cfg(feature = "test-support")]
                let unsupported = options.force_unsupported_directory;
                #[cfg(not(feature = "test-support"))]
                let unsupported = false;
                if unsupported {
                    defer_unsupported_directory(store, pool, &lease, options.next_check).await
                } else {
                    crate::custody::reobserve_retained_directory_page(
                        config,
                        pool,
                        store,
                        &lease,
                        duration,
                        options.next_check,
                    )
                    .await
                }
            }
            #[cfg(target_vendor = "apple")]
            {
                defer_unsupported_directory(store, pool, &lease, options.next_check).await
            }
        } else {
            crate::custody::reobserve_retained_baseline_page(
                config,
                pool,
                store,
                &lease,
                duration,
                options.next_check,
            )
            .await
        }
    } else {
        crate::custody::reobserve_retained_source(
            config,
            pool,
            store,
            worker,
            &lease,
            duration,
            options.next_check,
            options.hook.as_deref(),
        )
        .await
        .map(|_| ())
    };
    if let Err(error) = observation {
        let reason = match &error {
            crate::custody::RetainedReobserveError::Missing
            | crate::custody::RetainedReobserveError::Acquisition(
                crate::acquisition::AcquisitionError::SourceNotConfigured,
            ) => melodee_persistence::RetainedRetryReason::Missing,
            crate::custody::RetainedReobserveError::Ambiguous => {
                melodee_persistence::RetainedRetryReason::Ambiguous
            }
            crate::custody::RetainedReobserveError::Acquisition(
                crate::acquisition::AcquisitionError::SourceChanged,
            ) => melodee_persistence::RetainedRetryReason::Unstable,
            crate::custody::RetainedReobserveError::Acquisition(
                crate::acquisition::AcquisitionError::Persistence(
                    melodee_persistence::WorkStoreError::CustodyHashBudgetExhausted
                    | melodee_persistence::WorkStoreError::RetainedSourceBudgetExhausted,
                ),
            ) => melodee_persistence::RetainedRetryReason::Capacity,
            crate::custody::RetainedReobserveError::Acquisition(
                crate::acquisition::AcquisitionError::Persistence(
                    melodee_persistence::WorkStoreError::RetainedFreshIntakeRequired,
                ),
            ) => melodee_persistence::RetainedRetryReason::AwaitingIntake,
            _ => melodee_persistence::RetainedRetryReason::Other,
        };
        warn!(error = %error, "retained source reobservation deferred");
        store
            .defer_retained_source(pool, &lease, options.retry_after, reason)
            .await?;
    }
    Ok(true)
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
async fn defer_unsupported_directory(
    store: &WorkItemStore,
    pool: &PgPool,
    lease: &melodee_persistence::RetainedSourceLease,
    retry_after: ::time::Duration,
) -> Result<(), crate::custody::RetainedReobserveError> {
    store
        .defer_unsupported_retained_directories(
            pool,
            lease,
            retry_after,
            ::time::Duration::seconds(1),
        )
        .await
        .map_err(|error| crate::custody::RetainedReobserveError::Acquisition(error.into()))
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[doc(hidden)]
pub async fn test_retained_recheck_turn(
    config: &IngestionConfig,
    pool: &PgPool,
    store: &WorkItemStore,
    worker: &melodee_persistence::WorkerId,
) -> Result<bool, melodee_persistence::WorkStoreError> {
    retained_recheck_turn(config, pool, store, worker).await
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[doc(hidden)]
pub async fn test_retained_recheck_turn_without_directory_support(
    config: &IngestionConfig,
    pool: &PgPool,
    store: &WorkItemStore,
    worker: &melodee_persistence::WorkerId,
) -> Result<bool, melodee_persistence::WorkStoreError> {
    let options = RetainedScheduleOptions {
        force_unsupported_directory: true,
        ..RetainedScheduleOptions::default()
    };
    retained_recheck_turn_inner(config, pool, store, worker, &options).await
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[doc(hidden)]
pub async fn test_retained_recheck_turn_with_mutation(
    config: &IngestionConfig,
    pool: &PgPool,
    store: &WorkItemStore,
    worker: &melodee_persistence::WorkerId,
    path: &std::path::Path,
    replacement: &[u8],
) -> Result<bool, melodee_persistence::WorkStoreError> {
    let path = path.to_owned();
    let replacement = replacement.to_owned();
    let hook = move || {
        std::fs::write(&path, &replacement)
            .map_err(|error| crate::acquisition::AcquisitionError::Task(error.to_string()))
    };
    let options = RetainedScheduleOptions {
        hook: Some(Arc::new(hook)),
        ..RetainedScheduleOptions::default()
    };
    retained_recheck_turn_inner(config, pool, store, worker, &options).await
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[doc(hidden)]
pub async fn test_retained_recheck_turn_with_root_replacement(
    config: &IngestionConfig,
    pool: &PgPool,
    store: &WorkItemStore,
    worker: &melodee_persistence::WorkerId,
    root: &std::path::Path,
    parked: &std::path::Path,
    replacement: &std::path::Path,
) -> Result<bool, melodee_persistence::WorkStoreError> {
    let root = root.to_owned();
    let parked = parked.to_owned();
    let replacement = replacement.to_owned();
    let hook = move || {
        std::fs::rename(&root, &parked)
            .and_then(|()| std::fs::rename(&replacement, &root))
            .map_err(|error| crate::acquisition::AcquisitionError::Task(error.to_string()))
    };
    let options = RetainedScheduleOptions {
        hook: Some(Arc::new(hook)),
        ..RetainedScheduleOptions::default()
    };
    retained_recheck_turn_inner(config, pool, store, worker, &options).await
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[doc(hidden)]
pub fn test_start_retained_scheduler(
    config: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    mutation: Option<(std::path::PathBuf, Vec<u8>)>,
) -> SchedulerHandle {
    let hook = mutation.map(|(path, bytes)| {
        let fired = AtomicBool::new(false);
        Arc::new(move || {
            if !fired.swap(true, Ordering::AcqRel) {
                std::fs::write(&path, &bytes).map_err(|error| {
                    crate::acquisition::AcquisitionError::Task(error.to_string())
                })?;
            }
            Ok(())
        }) as Arc<crate::custody::RetainedObservationHook>
    });
    let options = RetainedScheduleOptions {
        next_check: ::time::Duration::seconds(1),
        retry_after: ::time::Duration::seconds(1),
        idle_delay: TokioDuration::from_millis(100),
        hook,
        ..RetainedScheduleOptions::default()
    };
    start_reconciliation_loop_inner(config, pool, store, options)
}

impl SchedulerHandle {
    pub(crate) const fn from_parts(
        join: tokio::task::JoinHandle<Vec<ReconcileResult>>,
        stop: StopToken,
    ) -> Self {
        Self { join, stop }
    }
}

impl SchedulerHandle {
    pub async fn stop(self) -> Vec<ReconcileResult> {
        self.stop.stop();
        self.join.await.unwrap_or_default()
    }
}

pub fn start_reconciliation_loop(
    config: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
) -> SchedulerHandle {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    let options = RetainedScheduleOptions {
        next_check: config.retained_recheck_interval,
        ..RetainedScheduleOptions::default()
    };
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    let options = RetainedScheduleOptions;
    start_reconciliation_loop_inner(config, pool, store, options)
}

fn start_reconciliation_loop_inner(
    config: IngestionConfig,
    pool: PgPool,
    store: WorkItemStore,
    retained_options: RetainedScheduleOptions,
) -> SchedulerHandle {
    let reconciler_running = Arc::new(AtomicBool::new(false));
    let stop = StopToken::new();
    let notifier = stop.signal.clone();

    let handle = tokio::spawn(async move {
        let mut results = Vec::new();
        let cleanup_pool = pool.clone();
        let cleanup_store = store;
        let mut cleanup = tokio::task::JoinSet::new();
        cleanup.spawn(async move {
            let mut prefer_expired = false;
            loop {
                let mut found = false;
                for _ in 0..4 {
                    let Ok(_permit) = crate::admission::heavy_ingestion().await else {
                        break;
                    };
                    prefer_expired = !prefer_expired;
                    let mut page_found = false;
                    match cleanup_store
                        .cleanup_expired_staged_snapshot_page(&cleanup_pool, prefer_expired)
                        .await
                    {
                        Ok(true) => page_found = true,
                        Ok(false) => {}
                        Err(error) => {
                            error!(%error, "staged settling snapshot cleanup failed");
                        }
                    }
                    match cleanup_store
                        .cleanup_abandoned_custody_hash_spool_page(
                            &cleanup_pool,
                            ::time::OffsetDateTime::now_utc(),
                        )
                        .await
                    {
                        Ok(true) => page_found = true,
                        Ok(false) => {}
                        Err(error) => error!(%error, "custody hash spool cleanup failed"),
                    }
                    match cleanup_store
                        .cleanup_retained_chunk_checkpoint(&cleanup_pool)
                        .await
                    {
                        Ok(true) => page_found = true,
                        Ok(false) => {}
                        Err(error) => error!(%error, "retained chunk checkpoint cleanup failed"),
                    }
                    match cleanup_store
                        .cleanup_abandoned_retained_baseline_page(&cleanup_pool)
                        .await
                    {
                        Ok(true) => page_found = true,
                        Ok(false) => {}
                        Err(error) => error!(%error, "retained baseline cleanup failed"),
                    }
                    if !page_found {
                        break;
                    }
                    found = true;
                }
                let delay = if found {
                    TokioDuration::from_millis(250)
                } else {
                    TokioDuration::from_secs(5)
                };
                time::sleep(delay).await;
            }
        });
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        if config.enabled() {
            let retained_config = config.clone();
            let retained_pool = pool.clone();
            let retained_store = store;
            cleanup.spawn(async move {
                let worker = melodee_persistence::WorkerId::new();
                loop {
                    let delay = match retained_recheck_turn_inner(
                        &retained_config,
                        &retained_pool,
                        &retained_store,
                        &worker,
                        &retained_options,
                    )
                    .await
                    {
                        Ok(true) => TokioDuration::from_millis(250),
                        Ok(false) => retained_options.idle_delay,
                        Err(error) => {
                            error!(%error, "retained source reobservation failed");
                            retained_options.idle_delay
                        }
                    };
                    time::sleep(delay).await;
                }
            });
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
        let _ = retained_options;
        let reconciler = crate::reconcile::Reconciler::new(config.clone(), store);
        let interval = TokioDuration::try_from(config.reconciliation_interval)
            .unwrap_or_else(|_| TokioDuration::from_secs(30));
        let mut ticker = time::interval(interval);

        'scheduler: loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let retained_sources = if config.enabled() { config.sources.as_slice() } else { &[] };
                    if let Err(error) = crate::discovery_scan::cleanup(&pool, retained_sources).await {
                        error!(%error, "discovery spool cleanup failed");
                    }
                    if !config.enabled() {
                        continue;
                    }
                    if reconciler_running
                        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                        .is_err()
                    {
                        continue;
                    }

                    let now = ::time::OffsetDateTime::now_utc();
                    tokio::select! {
                        outcome = reconcile_scheduler_pass(&reconciler, &pool, now) => {
                            match outcome {
                                Ok(recent) => {
                                    results = recent;
                                    info!("reconciliation loop completed");
                                }
                                Err(error) => error!(%error, "reconciliation loop failed"),
                            }
                        }
                        _ = notifier.notified() => {
                            reconciler_running.store(false, Ordering::Release);
                            break 'scheduler;
                        }
                    }
                    reconciler_running.store(false, Ordering::Release);
                }
                _ = notifier.notified() => {
                    reconciler_running.store(false, Ordering::Release);
                    break;
                }
            }
        }

        cleanup.shutdown().await;
        results
    });

    SchedulerHandle { join: handle, stop }
}

// The public one-shot operation returns every source result. The long-lived
// scheduler only needs a bounded shutdown snapshot; collect as each source
// completes rather than materializing the one-shot Vec first. Preserve the
// previous all-or-nothing result replacement if any source fails.
async fn reconcile_scheduler_pass(
    reconciler: &crate::reconcile::Reconciler,
    pool: &PgPool,
    now: ::time::OffsetDateTime,
) -> crate::error::Result<Vec<ReconcileResult>> {
    let mut recent = Vec::with_capacity(MAX_RETAINED_SCHEDULER_RESULTS);
    for source in &reconciler.config.sources {
        match crate::reconcile::reconcile_inbound_source_with_wait(
            &reconciler.config,
            &reconciler.store,
            pool,
            source,
            now,
            std::time::Duration::from_secs(1),
            true,
        )
        .await
        {
            Ok(result) => retain_result(&mut recent, result),
            Err(crate::error::IngestionError::Persistence(
                melodee_persistence::WorkStoreError::DiscoveryScanBusy(_)
                | melodee_persistence::WorkStoreError::DiscoveryScanIncomplete(_),
            )) => {
                warn!(source = %source.name, "source reconciliation requires retry; continuing with later sources")
            }
            Err(crate::error::IngestionError::Source(error)) => {
                warn!(source = %source.name, %error, "source discovery failed; continuing with later sources");
            }
            Err(error) => return Err(error),
        }
    }
    Ok(recent)
}

#[cfg(test)]
mod tests {
    use super::{MAX_RETAINED_SCHEDULER_RESULTS, StopToken, retain_result};
    use crate::{ReconcileResult, ReconcileStats};

    fn result(source: &str) -> ReconcileResult {
        ReconcileResult {
            source: source.to_owned(),
            stats: ReconcileStats::default(),
        }
    }

    #[test]
    fn retained_result_capacity_is_bounded_across_many_ticks() {
        let mut results = Vec::new();
        for tick in 0..10_000 {
            retain_result(&mut results, result(&format!("tick-{tick}")));
            assert!(results.len() <= MAX_RETAINED_SCHEDULER_RESULTS);
        }
        assert_eq!(results.len(), MAX_RETAINED_SCHEDULER_RESULTS);
        assert_eq!(
            results.last().map(|entry| entry.source.as_str()),
            Some("tick-9999"),
            "the newest result is retained"
        );
    }

    #[tokio::test]
    async fn stop_requested_before_consumer_waits_is_not_lost() {
        let stop = StopToken::new();
        stop.stop();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), stop.signal.notified())
                .await
                .is_ok()
        );
    }
}

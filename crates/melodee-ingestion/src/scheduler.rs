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
    let reconciler_running = Arc::new(AtomicBool::new(false));
    let stop = StopToken::new();
    let notifier = stop.signal.clone();

    let handle = tokio::spawn(async move {
        let mut results = Vec::new();
        let cleanup_pool = pool.clone();
        let cleanup_store = store.clone();
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
                    match cleanup_store
                        .cleanup_expired_staged_snapshot_page(&cleanup_pool, prefer_expired)
                        .await
                    {
                        Ok(true) => found = true,
                        Ok(false) => break,
                        Err(error) => {
                            error!(%error, "staged settling snapshot cleanup failed");
                            break;
                        }
                    }
                }
                let delay = if found {
                    TokioDuration::from_millis(250)
                } else {
                    TokioDuration::from_secs(5)
                };
                time::sleep(delay).await;
            }
        });
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

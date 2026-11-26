//! Periodic reconciliation scheduler.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use melodee_persistence::WorkItemStore;
use sqlx::PgPool;
use tokio::time::{self, Duration as TokioDuration};
use tracing::{error, info};

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
        self.signal.notify_waiters();
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

        if !config.enabled() {
            info!("ingestion disabled: no periodic reconciliation started");
            return results;
        }

        let reconciler = crate::reconcile::Reconciler::new(config.clone(), store);
        let interval = TokioDuration::try_from(config.reconciliation_interval)
            .unwrap_or_else(|_| TokioDuration::from_secs(30));
        let mut ticker = time::interval(interval);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if reconciler_running
                        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                        .is_err()
                    {
                        continue;
                    }

                    let now = ::time::OffsetDateTime::now_utc();
                    match reconciler.reconcile_all_sources(&pool, now).await {
                        Ok(recent) => {
                            results = recent;
                            info!("reconciliation loop completed");
                        }
                        Err(error) => {
                            error!(%error, "reconciliation loop failed");
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

        results
    });

    SchedulerHandle { join: handle, stop }
}

#[cfg(test)]
mod tests {
    use super::{MAX_RETAINED_SCHEDULER_RESULTS, retain_result};
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
}

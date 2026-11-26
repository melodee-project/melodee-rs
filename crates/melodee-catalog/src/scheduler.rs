//! Independently stoppable catalog-import scheduler.

use melodee_persistence::{CatalogStore, WorkEngineConfig};
use sqlx::PgPool;
use tokio::time::{self, Duration as TokioDuration};
use tracing::{error, info};

use crate::{
    CatalogConfig, CatalogImportStats,
    cancellation::CatalogCancellation,
    importer::{
        import_once_with_cancellation, reconcile_affected_identities_once,
        reconcile_artist_identities_once, reconcile_strong_release_identities,
    },
};

/// Lifecycle handle for the catalog scheduler.
#[derive(Debug)]
pub struct CatalogSchedulerHandle {
    join: tokio::task::JoinHandle<Option<CatalogImportStats>>,
    cancellation: CatalogCancellation,
}

impl CatalogSchedulerHandle {
    /// Signal the scheduler and wait for an in-flight atomic transaction.
    ///
    /// Claimed work that has not started its transaction releases its lease;
    /// an already-started fenced transaction is allowed to finish atomically.
    pub async fn stop(self) -> Option<CatalogImportStats> {
        self.cancellation.cancel();
        match self.join.await {
            Ok(stats) => stats,
            Err(error) => {
                error!(%error, "catalog scheduler task failed during shutdown");
                None
            }
        }
    }
}

/// Start periodic PostgreSQL-only catalog import.
pub fn start_catalog_loop(
    config: CatalogConfig,
    pool: PgPool,
    store: CatalogStore,
    work_config: WorkEngineConfig,
) -> CatalogSchedulerHandle {
    let cancellation = CatalogCancellation::default();
    let task_cancellation = cancellation.clone();
    let join = tokio::spawn(async move {
        if !config.enabled {
            info!("catalog import disabled: no periodic worker started");
            return None;
        }

        // Converge safe candidates left by an older binary even when no new
        // Published work remains to trigger the post-import reconciliation.
        reconcile_artist_identities_once(&store, &pool).await;
        // PERF-004: the startup backstop previously covered Artist identity
        // only. Strong Release identity needs the same convergence path so a
        // catalog with no new Published work still repairs a missed merge.
        reconcile_strong_release_identities(&store, &pool).await;

        // The poll interval is the idle cadence, not a throughput ceiling: a pass
        // that claims a full batch means more `published` work is queued, so run
        // again immediately instead of sleeping out the rest of the tick.
        let claim_batch_size = config.claim_limit() as usize;
        let interval = TokioDuration::try_from(config.poll_interval)
            .unwrap_or_else(|_| TokioDuration::from_secs(5));
        let mut ticker = time::interval(interval);
        let mut latest = None;
        // Periodic catalog-wide backstop independent of the seed queue. Covers
        // an installation upgraded from a binary that never persisted seeds,
        // a restored backup, and any seeds lost to an unrecovered failure.
        let backstop_interval = TokioDuration::from_secs(30 * 60);
        let mut last_backstop = std::time::Instant::now();
        // Bound one import burst before a coalesced closure drain, so the
        // durable seed queue cannot grow without limit during a long backfill.
        let max_full_passes_before_drain = 32u32;
        loop {
            if task_cancellation.is_cancelled() {
                break;
            }
            tokio::select! {
                _ = ticker.tick() => {}
                () = task_cancellation.cancelled() => break,
            }
            let mut full_passes = 0u32;
            loop {
                let import_started = std::time::Instant::now();
                let import_result = import_once_with_cancellation(
                    &config,
                    &store,
                    &pool,
                    &work_config,
                    ::time::OffsetDateTime::now_utc(),
                    &task_cancellation,
                )
                .await;
                melodee_persistence::metrics::observe(
                    melodee_persistence::metrics::Metric::CatalogImportPass,
                    import_started.elapsed(),
                );
                match import_result {
                    Ok(stats) => {
                        let full_batch = stats.claimed >= claim_batch_size;
                        latest = Some(stats);
                        full_passes = full_passes.saturating_add(1);
                        if !full_batch
                            || task_cancellation.is_cancelled()
                            || full_passes >= max_full_passes_before_drain
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    Err(error) => {
                        error!(%error, "catalog import pass failed before completion");
                        break;
                    }
                }
            }
            // One coalesced affected-identity closure pass for every batch of
            // imports instead of five catalog-wide queries per album.
            reconcile_affected_identities_once(&store, &pool).await;
            if last_backstop.elapsed() >= backstop_interval {
                reconcile_artist_identities_once(&store, &pool).await;
                reconcile_strong_release_identities(&store, &pool).await;
                last_backstop = std::time::Instant::now();
            }
        }
        latest
    });

    CatalogSchedulerHandle { join, cancellation }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    async fn disabled_scheduler_stops_without_opening_postgres() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://melodee:melodee@127.0.0.1:1/melodee")
            .expect("syntactically valid lazy PostgreSQL URL");
        let config = CatalogConfig {
            enabled: false,
            ..CatalogConfig::default()
        };

        let handle = start_catalog_loop(config, pool, CatalogStore, WorkEngineConfig::default());

        assert_eq!(handle.stop().await, None);
    }
}

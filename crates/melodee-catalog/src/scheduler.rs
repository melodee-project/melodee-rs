//! Independently stoppable catalog-import scheduler.

use melodee_persistence::{CatalogStore, WorkEngineConfig};
use sqlx::PgPool;
use tokio::time::{self, Duration as TokioDuration};
use tracing::{error, info};

use crate::{
    CatalogConfig, CatalogImportStats, cancellation::CatalogCancellation,
    importer::import_once_with_cancellation,
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

        let interval = TokioDuration::try_from(config.poll_interval)
            .unwrap_or_else(|_| TokioDuration::from_secs(5));
        let mut ticker = time::interval(interval);
        let mut latest = None;
        loop {
            if task_cancellation.is_cancelled() {
                break;
            }
            tokio::select! {
                _ = ticker.tick() => {
                    match import_once_with_cancellation(
                        &config,
                        &store,
                        &pool,
                        &work_config,
                        ::time::OffsetDateTime::now_utc(),
                        &task_cancellation,
                    ).await {
                        Ok(stats) => latest = Some(stats),
                        Err(error) => error!(%error, "catalog import pass failed before completion"),
                    }
                }
                () = task_cancellation.cancelled() => break,
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

//! One bounded manifest-v3 catalog-import pass.

use std::time::Instant;

use melodee_domain::{FailureClass, WorkErrorCode};
use melodee_persistence::{
    CatalogImportRequest, CatalogImportResult, CatalogStore, CatalogStoreError, WorkEngineConfig,
    WorkStoreError, WorkerId,
};
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::task::JoinSet;
use tracing::{Instrument, error, info, info_span};

use crate::{
    CatalogConfig, CatalogImportStats, LOCAL_RESOLVER_VERSION, cancellation::CatalogCancellation,
};

#[derive(Debug, Error)]
pub enum CatalogImportError {
    #[error("catalog persistence error: {0}")]
    Store(#[from] CatalogStoreError),
}

enum CatalogTaskOutput {
    Completed {
        work_item_id: uuid::Uuid,
        result: CatalogImportResult,
    },
    Failed {
        work_item_id: uuid::Uuid,
        error: CatalogStoreError,
        failure_record: FailureRecord,
    },
    Released {
        work_item_id: uuid::Uuid,
        result: Result<(), CatalogStoreError>,
    },
}

enum FailureRecord {
    Recorded,
    Failed(CatalogStoreError),
}

type CatalogJoinResult = Result<CatalogTaskOutput, tokio::task::JoinError>;

/// Claim and import at most one configured catalog batch.
///
/// All semantic writes and the final `Cataloged` transition remain inside
/// `CatalogStore::import_manifest_v3`; this layer only owns bounded scheduling.
pub async fn import_once(
    config: &CatalogConfig,
    store: &CatalogStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<CatalogImportStats, CatalogImportError> {
    import_once_with_cancellation(
        config,
        store,
        pool,
        work_config,
        now,
        &CatalogCancellation::default(),
    )
    .await
}

pub(crate) async fn import_once_with_cancellation(
    config: &CatalogConfig,
    store: &CatalogStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
    cancellation: &CatalogCancellation,
) -> Result<CatalogImportStats, CatalogImportError> {
    let started = Instant::now();
    let worker = WorkerId::new();
    let claim_config = bounded_claim_config(config, work_config);
    let claimed = store
        .claim_cataloging(pool, &worker, &claim_config, now)
        .await?;
    let mut stats = CatalogImportStats {
        claimed: claimed.len(),
        ..CatalogImportStats::default()
    };
    let concurrency = usize::try_from(config.concurrency).unwrap_or(usize::MAX);
    let mut tasks = JoinSet::new();

    for item in claimed {
        if cancellation.is_cancelled() {
            let result = store
                .release_lease(pool, item.id, &worker, OffsetDateTime::now_utc())
                .await;
            record_result(
                Some(Ok(CatalogTaskOutput::Released {
                    work_item_id: item.id,
                    result,
                })),
                &mut stats,
            );
            continue;
        }
        while tasks.len() >= concurrency {
            record_result(tasks.join_next().await, &mut stats);
        }
        let pool = pool.clone();
        let store = *store;
        let worker = worker.clone();
        let cancellation = cancellation.clone();
        let lease_duration = work_config.lease_duration;
        let retry_policy = work_config.retry_policy;
        tasks.spawn(async move {
            let span = info_span!(
                "catalog_import",
                work_item_id = %item.id,
                work_item_api_key = %item.api_key,
                worker_id = %worker,
            );
            async move {
                if cancellation.is_cancelled() {
                    return CatalogTaskOutput::Released {
                        work_item_id: item.id,
                        result: store
                            .release_lease(&pool, item.id, &worker, OffsetDateTime::now_utc())
                            .await,
                    };
                }

                // Claim time may precede task startup. Renew immediately before
                // the atomic transaction so the full configured lease protects
                // useful work rather than time spent scheduling the task.
                if let Err(error) = store
                    .renew_lease(
                        &pool,
                        item.id,
                        &worker,
                        lease_duration,
                        OffsetDateTime::now_utc(),
                    )
                    .await
                {
                    return failed_output(&store, &pool, item.id, &worker, error, &retry_policy)
                        .await;
                }

                // Cancellation wins until the catalog transaction begins. Once
                // it begins, graceful shutdown lets that fenced transaction
                // commit or roll back atomically.
                if cancellation.is_cancelled() {
                    return CatalogTaskOutput::Released {
                        work_item_id: item.id,
                        result: store
                            .release_lease(&pool, item.id, &worker, OffsetDateTime::now_utc())
                            .await,
                    };
                }

                let request = CatalogImportRequest::new(
                    LOCAL_RESOLVER_VERSION,
                    format!("catalog-worker:{worker}"),
                );
                match store
                    .import_manifest_v3(
                        &pool,
                        item.id,
                        &worker,
                        &request,
                        OffsetDateTime::now_utc(),
                    )
                    .await
                {
                    Ok(result) => CatalogTaskOutput::Completed {
                        work_item_id: item.id,
                        result,
                    },
                    Err(error) => {
                        failed_output(&store, &pool, item.id, &worker, error, &retry_policy).await
                    }
                }
            }
            .instrument(span)
            .await
        });
    }

    while let Some(result) = tasks.join_next().await {
        record_result(Some(result), &mut stats);
    }

    stats.duration_ms = started.elapsed().as_millis();
    info!(
        claimed = stats.claimed,
        completed = stats.completed,
        already_completed = stats.already_completed,
        released = stats.released,
        failed = stats.failed,
        errors = stats.errors,
        duration_ms = stats.duration_ms,
        "bounded catalog import pass completed"
    );
    Ok(stats)
}

async fn failed_output(
    store: &CatalogStore,
    pool: &PgPool,
    work_item_id: uuid::Uuid,
    worker: &WorkerId,
    error: CatalogStoreError,
    retry_policy: &melodee_domain::RetryPolicy,
) -> CatalogTaskOutput {
    let (code, class) = failure_disposition(&error);
    let failure_record = match store
        .record_failure(
            pool,
            work_item_id,
            worker,
            WorkErrorCode::new(code),
            &error.to_string(),
            class,
            retry_policy,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(_) => FailureRecord::Recorded,
        Err(error) => FailureRecord::Failed(error),
    };
    CatalogTaskOutput::Failed {
        work_item_id,
        error,
        failure_record,
    }
}

fn bounded_claim_config(
    config: &CatalogConfig,
    work_config: &WorkEngineConfig,
) -> WorkEngineConfig {
    WorkEngineConfig {
        // Claim only work that can begin immediately. This prevents a short
        // lease from aging while an item waits behind this process's own
        // concurrency gate.
        claim_batch_size: config.claim_limit(),
        ..*work_config
    }
}

fn record_result(result: Option<CatalogJoinResult>, stats: &mut CatalogImportStats) {
    match result {
        Some(Ok(CatalogTaskOutput::Completed {
            work_item_id,
            result,
        })) => {
            if result.inserted {
                stats.completed += 1;
            } else {
                stats.already_completed += 1;
            }
            info!(
                %work_item_id,
                release_id = %result.release_id,
                track_count = result.track_count,
                artist_count = result.artist_count,
                media_asset_count = result.media_asset_count,
                set_based_mutation_statement_count = result.set_based_mutation_statement_count,
                inserted = result.inserted,
                "catalog import completed"
            );
        }
        Some(Ok(CatalogTaskOutput::Failed {
            work_item_id,
            error,
            failure_record,
        })) => {
            stats.failed += 1;
            match failure_record {
                FailureRecord::Recorded => {
                    error!(%work_item_id, %error, "catalog import failed and was durably recorded");
                }
                FailureRecord::Failed(record_error) => {
                    stats.errors += 1;
                    error!(
                        %work_item_id,
                        %error,
                        %record_error,
                        "catalog import failed outside durable failure recording"
                    );
                }
            }
        }
        Some(Ok(CatalogTaskOutput::Released {
            work_item_id,
            result,
        })) => match result {
            Ok(()) => {
                stats.released += 1;
                info!(%work_item_id, "catalog lease released before import on cancellation");
            }
            Err(error) => {
                stats.failed += 1;
                stats.errors += 1;
                error!(%work_item_id, %error, "catalog cancellation could not release lease");
            }
        },
        Some(Err(error)) => {
            stats.failed += 1;
            stats.errors += 1;
            error!(%error, "catalog import task failed");
        }
        None => {}
    }
}

fn failure_disposition(error: &CatalogStoreError) -> (&'static str, FailureClass) {
    match error {
        CatalogStoreError::Database(_) | CatalogStoreError::Work(WorkStoreError::Database(_)) => {
            ("catalog.database", FailureClass::Retryable)
        }
        CatalogStoreError::Serialization(_) => {
            ("catalog.manifest_invalid", FailureClass::Permanent)
        }
        CatalogStoreError::Domain(_) => ("catalog.domain_invalid", FailureClass::Permanent),
        CatalogStoreError::InvalidRequest => ("catalog.request_invalid", FailureClass::Permanent),
        CatalogStoreError::CorrectionScopeExceeded { .. } => {
            ("catalog.correction_scope_exceeded", FailureClass::Permanent)
        }
        CatalogStoreError::InvalidTransition(_) => {
            ("catalog.transition_invalid", FailureClass::Permanent)
        }
        CatalogStoreError::ImportConflict(_) => {
            ("catalog.import_conflict", FailureClass::Permanent)
        }
        CatalogStoreError::IncompleteTrackMapping(_) => {
            ("catalog.track_mapping_incomplete", FailureClass::Permanent)
        }
        CatalogStoreError::NumericOverflow => ("catalog.numeric_overflow", FailureClass::Permanent),
        CatalogStoreError::CorruptRole(_) => ("catalog.role_invalid", FailureClass::Permanent),
        CatalogStoreError::LockedDecision => ("catalog.locked_decision", FailureClass::Permanent),
        CatalogStoreError::NegativeAssertion => {
            ("catalog.negative_assertion", FailureClass::Permanent)
        }
        CatalogStoreError::MutationConflict => {
            ("catalog.mutation_conflict", FailureClass::Permanent)
        }
        CatalogStoreError::Work(
            WorkStoreError::NotLeaseOwner(_)
            | WorkStoreError::LeaseExpired(_)
            | WorkStoreError::LeaseConflict(_),
        ) => ("catalog.lease_lost", FailureClass::Retryable),
        CatalogStoreError::Work(
            WorkStoreError::NotFound(_) | WorkStoreError::InvalidState(_, _),
        ) => ("catalog.work_item_invalid", FailureClass::Permanent),
        CatalogStoreError::Work(_) => ("catalog.work_item_invalid", FailureClass::Permanent),
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;

    #[test]
    fn short_lease_claims_only_work_that_can_run_immediately() {
        let catalog_config = CatalogConfig {
            concurrency: 2,
            batch_size: 16,
            ..CatalogConfig::default()
        };
        let work_config = WorkEngineConfig {
            lease_duration: Duration::seconds(1),
            ..WorkEngineConfig::default()
        };

        let claim_config = bounded_claim_config(&catalog_config, &work_config);

        assert_eq!(claim_config.claim_batch_size, 2);
        assert_eq!(claim_config.lease_duration, Duration::seconds(1));
    }

    #[test]
    fn correction_scope_failures_are_permanent_and_actionable() {
        let error = CatalogStoreError::CorrectionScopeExceeded {
            dimension: melodee_persistence::CatalogCorrectionDimension::Releases,
            limit: melodee_persistence::MAX_CATALOG_CORRECTION_RELEASES,
        };

        assert_eq!(
            failure_disposition(&error),
            ("catalog.correction_scope_exceeded", FailureClass::Permanent)
        );
    }
}

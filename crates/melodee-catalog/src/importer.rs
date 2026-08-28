//! One bounded manifest-v3 catalog-import pass.

use std::time::Instant;

use melodee_domain::{
    ArtistId, ArtistMatchDecision, ArtistMatchDecisionId, ArtistMatchEvidence,
    ArtistMatchEvidenceId, ArtistMatchOutcome, ArtistMerge, ArtistMergeId, Confidence,
    DecisionAuthority, EvidenceKind, EvidencePolarity, FailureClass, NewArtistMatchDecision,
    ObservedArtistId, WorkErrorCode,
};
use melodee_persistence::{
    ArtistReconciliationEvidence, CatalogImportRequest, CatalogImportResult, CatalogStore,
    CatalogStoreError, WorkEngineConfig, WorkStoreError, WorkerId,
};
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::{
    CatalogConfig, CatalogImportStats, LOCAL_RESOLVER_VERSION, LocalArtistCandidate,
    LocalArtistResolver, LocalResolutionConstraints, ResolutionEvidence, ResolutionEvidenceSource,
    cancellation::CatalogCancellation,
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
                    Ok(result) => {
                        reconcile_artist_identities_once(&store, &pool).await;
                        reconcile_strong_release_identities(&store, &pool).await;
                        CatalogTaskOutput::Completed {
                            work_item_id: item.id,
                            result,
                        }
                    }
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

/// Runs one bounded Artist reconciliation sweep. The scheduler invokes this
/// once at startup to converge durable duplicates left by older binaries, and
/// successful imports invoke it again for newly committed observations.
pub(crate) async fn reconcile_artist_identities_once(store: &CatalogStore, pool: &PgPool) {
    reconcile_strong_artist_identities(store, pool).await;
    reconcile_same_release_track_artist_identities(store, pool).await;
    reconcile_repeated_primary_release_artist_identities(store, pool).await;
    propose_name_only_artist_matches(store, pool).await;
}

/// Runs the bounded production caller for the pure local resolver after each
/// successful import. Strong embedded evidence may converge identities; names
/// alone never enter this path. Merge mutations remain append-only and are
/// safe to retry when another worker wins the row locks first.
async fn reconcile_strong_artist_identities(store: &CatalogStore, pool: &PgPool) {
    let candidates = match store.strong_artist_merge_candidates(pool, 64).await {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, "strong Artist reconciliation scan failed; import remains durable");
            return;
        }
    };
    let Some(confidence) = Confidence::new(9_500).ok() else {
        return;
    };
    let resolver = LocalArtistResolver::default();
    for candidate in candidates {
        let evaluation = match resolver.evaluate(
            &[LocalArtistCandidate {
                artist_id: ArtistId::from_uuid(candidate.target_artist_id),
                evidence: vec![ResolutionEvidence {
                    kind: EvidenceKind::EmbeddedExternalId,
                    polarity: EvidencePolarity::Supports,
                    confidence,
                    source: ResolutionEvidenceSource::PersistedCatalog,
                }],
            }],
            &LocalResolutionConstraints::default(),
        ) {
            Ok(evaluation) => evaluation,
            Err(error) => {
                warn!(%error, provider=%candidate.provider, external_id=%candidate.external_id, "Artist resolver rejected bounded evidence");
                continue;
            }
        };
        if !matches!(
            evaluation.recommendation,
            crate::ArtistResolutionRecommendation::Confirmable { .. }
        ) {
            continue;
        }
        let merge = ArtistMerge {
            id: ArtistMergeId::new(),
            source_artist_id: ArtistId::from_uuid(candidate.source_artist_id),
            source_public_api_id: melodee_domain::ArtistPublicId::from_uuid(
                candidate.source_public_api_id,
            ),
            target_artist_id: ArtistId::from_uuid(candidate.target_artist_id),
            actor: "catalog-reconciler".to_owned(),
            reason: format!(
                "strong embedded {} evidence converged external identifier {}",
                candidate.provider, candidate.external_id
            ),
            occurred_at: time::OffsetDateTime::now_utc(),
        };
        let evidence = ArtistReconciliationEvidence::EmbeddedExternalId {
            provider: candidate.provider.clone(),
            external_id: candidate.external_id.clone(),
        };
        match store
            .reconcile_artist(pool, &merge, LOCAL_RESOLVER_VERSION, &evidence)
            .await
        {
            Ok(()) => info!(
                source_artist_id=%candidate.source_artist_id,
                target_artist_id=%candidate.target_artist_id,
                provider=%candidate.provider,
                external_id=%candidate.external_id,
                "strong Artist identity converged"
            ),
            Err(melodee_persistence::CatalogStoreError::MutationConflict) => {
                debug!(source_artist_id=%candidate.source_artist_id, "Artist merge was already reconciled or fenced")
            }
            Err(error) => {
                warn!(%error, source_artist_id=%candidate.source_artist_id, "Artist merge candidate was preserved without mutation")
            }
        }
    }
}

/// Runs ADR-0173's Release-bounded Track-credit predicate before name-only
/// proposals. Collapsing repeated track-only observations to one
/// provisional identity per Release first prevents candidate ordering from
/// leaving same-Release siblings behind when a name also occurs elsewhere.
async fn reconcile_same_release_track_artist_identities(store: &CatalogStore, pool: &PgPool) {
    let candidates = match store
        .same_release_track_artist_merge_candidates(pool, 64)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, "same-Release Track Artist reconciliation scan failed; import remains durable");
            return;
        }
    };
    for candidate in candidates {
        let merge = ArtistMerge {
            id: ArtistMergeId::new(),
            source_artist_id: ArtistId::from_uuid(candidate.source_artist_id),
            source_public_api_id: melodee_domain::ArtistPublicId::from_uuid(
                candidate.source_public_api_id,
            ),
            target_artist_id: ArtistId::from_uuid(candidate.target_artist_id),
            actor: "catalog-reconciler".to_owned(),
            reason: format!(
                "exact structured Track credit {} recurred within Release {}",
                candidate.normalized_name, candidate.release_id
            ),
            occurred_at: OffsetDateTime::now_utc(),
        };
        let evidence = ArtistReconciliationEvidence::SameReleaseTrackCredit {
            release_id: candidate.release_id,
            normalized_name: candidate.normalized_name.clone(),
        };
        match store
            .reconcile_artist(pool, &merge, LOCAL_RESOLVER_VERSION, &evidence)
            .await
        {
            Ok(()) => info!(
                source_artist_id=%candidate.source_artist_id,
                target_artist_id=%candidate.target_artist_id,
                release_id=%candidate.release_id,
                normalized_name=%candidate.normalized_name,
                "same-Release Track Artist identity converged"
            ),
            Err(CatalogStoreError::MutationConflict) => {
                debug!(source_artist_id=%candidate.source_artist_id, "same-Release Track Artist merge was already reconciled or fenced")
            }
            Err(error) => {
                warn!(%error, source_artist_id=%candidate.source_artist_id, "same-Release Track Artist candidate was preserved without mutation")
            }
        }
    }
}

/// Consolidates repeated, exact primary Release Artist credits only when the
/// complete active name group is still provisional and carries no
/// differentiating identity evidence. The persisted Release role is the
/// contextual evidence that keeps this separate from a name-only match.
async fn reconcile_repeated_primary_release_artist_identities(store: &CatalogStore, pool: &PgPool) {
    let candidates = match store
        .repeated_primary_release_artist_merge_candidates(pool, 64)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, "repeated primary Release Artist reconciliation scan failed; import remains durable");
            return;
        }
    };
    for candidate in candidates {
        let merge = ArtistMerge {
            id: ArtistMergeId::new(),
            source_artist_id: ArtistId::from_uuid(candidate.source_artist_id),
            source_public_api_id: melodee_domain::ArtistPublicId::from_uuid(
                candidate.source_public_api_id,
            ),
            target_artist_id: ArtistId::from_uuid(candidate.target_artist_id),
            actor: "catalog-reconciler".to_owned(),
            reason: format!(
                "exact primary Release Artist credit {} recurred without differentiating evidence",
                candidate.normalized_name
            ),
            occurred_at: OffsetDateTime::now_utc(),
        };
        let evidence = ArtistReconciliationEvidence::RepeatedPrimaryReleaseCredit {
            normalized_name: candidate.normalized_name.clone(),
        };
        match store
            .reconcile_artist(pool, &merge, LOCAL_RESOLVER_VERSION, &evidence)
            .await
        {
            Ok(()) => info!(
                source_artist_id=%candidate.source_artist_id,
                target_artist_id=%candidate.target_artist_id,
                normalized_name=%candidate.normalized_name,
                "repeated primary Release Artist identity converged"
            ),
            Err(CatalogStoreError::MutationConflict) => {
                debug!(source_artist_id=%candidate.source_artist_id, "repeated primary Release Artist merge was already reconciled or fenced")
            }
            Err(error) => {
                warn!(%error, source_artist_id=%candidate.source_artist_id, "repeated primary Release Artist candidate was preserved without mutation")
            }
        }
    }
}

/// Turns exact normalized-name equality into reviewable evidence without
/// changing either Melodee-owned Artist identity. Proposals do not advance the
/// authoritative current pointer, so legitimate same-name Artists remain
/// distinct unless an administrator confirms or merges them.
async fn propose_name_only_artist_matches(store: &CatalogStore, pool: &PgPool) {
    let candidates = match store
        .name_only_artist_match_proposal_candidates(pool, 64)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, "name-only Artist proposal scan failed; import remains durable");
            return;
        }
    };
    let Some(confidence) = Confidence::new(7_000).ok() else {
        return;
    };
    for candidate in candidates {
        let now = OffsetDateTime::now_utc();
        let evidence = ArtistMatchEvidence {
            id: ArtistMatchEvidenceId::new(),
            observed_artist_id: ObservedArtistId::from_uuid(candidate.observed_artist_id),
            candidate_artist_id: ArtistId::from_uuid(candidate.candidate_artist_id),
            kind: EvidenceKind::NameSimilarity,
            polarity: EvidencePolarity::Supports,
            confidence,
            source: "catalog-name-proposal".to_owned(),
        };
        if let Err(error) = store
            .record_artist_match_evidence(pool, &evidence, Default::default(), now)
            .await
        {
            warn!(%error, observed_artist_id=%candidate.observed_artist_id, "name-only Artist evidence could not be recorded");
            continue;
        }
        let reason = format!(
            "exact normalized name '{}' requires identity review",
            candidate.normalized_name
        );
        let decision = match ArtistMatchDecision::new(
            NewArtistMatchDecision {
                id: ArtistMatchDecisionId::new(),
                observed_artist_id: ObservedArtistId::from_uuid(candidate.observed_artist_id),
                artist_id: ArtistId::from_uuid(candidate.candidate_artist_id),
                outcome: ArtistMatchOutcome::Proposed,
                authority: DecisionAuthority::Automated {
                    resolver_version: LOCAL_RESOLVER_VERSION.to_owned(),
                },
                locked: false,
                reason,
                decided_at: now,
            },
            std::slice::from_ref(&evidence),
        ) {
            Ok(decision) => decision,
            Err(error) => {
                warn!(%error, observed_artist_id=%candidate.observed_artist_id, "name-only Artist proposal was invalid");
                continue;
            }
        };
        match store
            .record_artist_match_decision(pool, &decision, confidence, Default::default())
            .await
        {
            Ok(()) => info!(
                observed_artist_id=%candidate.observed_artist_id,
                current_artist_id=%candidate.current_artist_id,
                candidate_artist_id=%candidate.candidate_artist_id,
                normalized_name=%candidate.normalized_name,
                "name-only Artist match proposed for review"
            ),
            Err(CatalogStoreError::MutationConflict | CatalogStoreError::NegativeAssertion) => {
                debug!(observed_artist_id=%candidate.observed_artist_id, "name-only Artist proposal was already recorded or suppressed")
            }
            Err(error) => {
                warn!(%error, observed_artist_id=%candidate.observed_artist_id, "name-only Artist proposal was preserved without an identity mutation")
            }
        }
    }
}

/// Reconciles duplicate active Releases only when the durable catalog contains
/// the same trusted provider identifier. Release names and normalized titles
/// are intentionally not identity predicates. The audited persistence
/// mutation keeps this pass restart-safe: a concurrent winner is fenced and
/// the next import pass will observe the resulting state.
async fn reconcile_strong_release_identities(store: &CatalogStore, pool: &PgPool) {
    let candidates = match store.strong_release_merge_candidates(pool, 64).await {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(%error, "strong Release reconciliation scan failed; import remains durable");
            return;
        }
    };
    for candidate in candidates {
        let reason = format!(
            "trusted {} evidence converged external identifier {}",
            candidate.provider, candidate.external_id
        );
        match store
            .merge_release(
                pool,
                candidate.source_release_id,
                candidate.target_release_id,
                "catalog-reconciler",
                &reason,
                OffsetDateTime::now_utc(),
            )
            .await
        {
            Ok(event_id) => info!(
                source_release_id=%candidate.source_release_id,
                target_release_id=%candidate.target_release_id,
                provider=%candidate.provider,
                external_id=%candidate.external_id,
                merge_event_id=%event_id,
                "strong Release identity converged"
            ),
            Err(CatalogStoreError::MutationConflict) => {
                debug!(source_release_id=%candidate.source_release_id, "Release merge was already reconciled or fenced")
            }
            Err(error) => {
                warn!(%error, source_release_id=%candidate.source_release_id, "Release merge candidate was preserved without mutation")
            }
        }
    }
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
        CatalogStoreError::ProviderEnrichment(
            melodee_persistence::ProviderEnrichmentStoreError::Database(_),
        ) => (
            "catalog.provider_enrichment_database",
            FailureClass::Retryable,
        ),
        CatalogStoreError::ProviderEnrichment(_) => (
            "catalog.provider_enrichment_invalid",
            FailureClass::Permanent,
        ),
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

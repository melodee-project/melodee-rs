//! Phase 5 policy worker: periodically claims Cataloged work items, classifies
//! Releases, assesses Track novelty against the catalog, evaluates policy, and
//! records durable decisions before transitioning the work item.
//!
//! This worker integrates the melodee-policy engine with the ingestion
//! state machine (SPEC section 15, PRD-POLICY-001 through PRD-POLICY-008).
//! It does NOT perform filesystem deletion; Delete dispositions are recorded
//! as Pending decisions and route the work item to NeedsReview for operator
//! confirmation (SPEC section 11.2: `NeedsReview --> Deleted: authorized decision recorded`).

use std::sync::Arc;

use melodee_domain::{
    Confidence, DecisionCompletionState, Disposition, IngestionState, PolicyActor,
    ReleasePolicyDecision, ReleasePolicyDecisionId, TrackNovelty, TrackNoveltyCategory,
    TrackNoveltyId, TransitionKind,
};
use melodee_persistence::{
    CatalogStore, PolicyEligibleWorkItem, PolicyStore, WorkEngineConfig, WorkItemStore, WorkerId,
};
use melodee_policy::{
    NoveltyAssessment, NoveltyEngine, NoveltyInput, PolicyConfig, PolicyEngine, PolicyInput,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::{debug, error, info, warn};

/// Environment key for enabling the policy worker.
pub const ENV_POLICY_WORKER_ENABLED: &str = "MELODEE__POLICY__WORKER__ENABLED";

/// Polling interval for the policy worker loop.
const POLICY_POLL_INTERVAL_SECONDS: u64 = 60;

/// Scheduler handle for graceful shutdown of the policy worker loop.
pub struct PolicySchedulerHandle {
    handle: tokio::task::JoinHandle<()>,
    stop: tokio::sync::watch::Sender<()>,
}

impl PolicySchedulerHandle {
    pub async fn stop(self) {
        let _ = self.stop.send(());
        self.handle.await.unwrap_or_else(|e| {
            tracing::warn!("policy worker task did not shut down cleanly: {e}");
        });
    }
}

/// Starts the periodic policy evaluation loop.
pub fn start_policy_loop(
    policy_config: PolicyConfig,
    catalog_config: melodee_catalog::CatalogConfig,
    pool: PgPool,
) -> Option<PolicySchedulerHandle> {
    let enabled = std::env::var(ENV_POLICY_WORKER_ENABLED)
        .map(|v| v == "true" || v == "1")
        .unwrap_or(true);

    if !enabled {
        info!("policy worker disabled by configuration");
        return None;
    }

    if !catalog_config.enabled {
        info!(
            "policy worker disabled: catalog import is not enabled (no catalog data to evaluate)"
        );
        return None;
    }

    let (stop_tx, stop_rx) = tokio::sync::watch::channel::<()>(());
    let policy_config = Arc::new(policy_config);
    let engine = PolicyEngine::new(PolicyConfig {
        batch_size: policy_config.batch_size,
        concurrency: policy_config.concurrency,
        destructive_confidence_bps: policy_config.destructive_confidence_bps,
    });
    let novelty_engine = NoveltyEngine;
    let work_config = WorkEngineConfig::from_env().unwrap_or_default();
    let worker_id = WorkerId::new();

    let handle = tokio::spawn(async move {
        info!("starting policy worker loop");
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(POLICY_POLL_INTERVAL_SECONDS));
        let mut stop_rx = stop_rx.clone();

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = process_policy_batch(
                        &engine,
                        &novelty_engine,
                        &pool,
                        &policy_config,
                        &work_config,
                        &worker_id,
                    )
                    .await
                    {
                        error!("policy worker batch error: {e}");
                    }
                }
                _ = stop_rx.changed() => {
                    info!("policy worker shutting down");
                    break;
                }
            }
        }
    });

    Some(PolicySchedulerHandle {
        handle,
        stop: stop_tx,
    })
}

/// Processes one batch of Cataloged work items: classifies, assesses novelty,
/// evaluates policy, and records durable decisions before state transitions.
async fn process_policy_batch(
    engine: &PolicyEngine,
    novelty_engine: &NoveltyEngine,
    pool: &PgPool,
    _config: &PolicyConfig,
    work_config: &WorkEngineConfig,
    worker_id: &WorkerId,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let catalog_store = CatalogStore;
    let policy_store = PolicyStore;
    let work_store = WorkItemStore;
    let now = OffsetDateTime::now_utc();

    let claimed = catalog_store
        .claim_policy_eligible(pool, worker_id, work_config, now)
        .await?;

    if claimed.is_empty() {
        debug!("policy worker: no eligible Cataloged work items");
        return Ok(());
    }

    info!(
        "policy worker: claimed {} Cataloged releases for policy evaluation",
        claimed.len()
    );

    for work_item in &claimed {
        if let Err(e) = process_single_release(
            engine,
            novelty_engine,
            pool,
            &work_store,
            &catalog_store,
            &policy_store,
            worker_id,
            work_item,
            now,
        )
        .await
        {
            warn!(
                "policy worker: error processing release work item {}: {}",
                work_item.item.id, e
            );
            let _ = work_store
                .transition_for_policy(
                    pool,
                    work_item.item.id,
                    worker_id,
                    TransitionKind::Worker,
                    IngestionState::Failed,
                    Some("policy_evaluation_error"),
                    now,
                )
                .await;
        }
    }

    Ok(())
}

/// Processes a single Cataloged release through the policy pipeline.
#[allow(clippy::too_many_arguments)]
async fn process_single_release(
    engine: &PolicyEngine,
    novelty_engine: &NoveltyEngine,
    pool: &PgPool,
    work_store: &WorkItemStore,
    _catalog_store: &CatalogStore,
    policy_store: &PolicyStore,
    worker_id: &WorkerId,
    work_item: &PolicyEligibleWorkItem,
    now: OffsetDateTime,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let release_id = melodee_domain::ReleaseId::from_uuid(work_item.release_id);

    // Check if a policy decision already exists for this release.
    if let Some(existing) = policy_store.get_decision(pool, release_id).await? {
        debug!(
            "policy worker: release {} already has decision {:?}, skipping",
            release_id, existing.disposition
        );
        match existing.disposition {
            Disposition::Accept => {
                work_item
                    .item
                    .state
                    .ensure_transition(IngestionState::Indexing, TransitionKind::Worker)?;
                work_store
                    .transition_for_policy(
                        pool,
                        work_item.item.id,
                        worker_id,
                        TransitionKind::Worker,
                        IngestionState::Indexing,
                        Some("policy_decision_applied"),
                        now,
                    )
                    .await?;
            }
            Disposition::Ignore => {
                work_store
                    .transition_for_policy(
                        pool,
                        work_item.item.id,
                        worker_id,
                        TransitionKind::Worker,
                        IngestionState::Ignored,
                        Some("policy_decision_applied"),
                        now,
                    )
                    .await?;
            }
            Disposition::Quarantine => {
                work_store
                    .transition_for_policy(
                        pool,
                        work_item.item.id,
                        worker_id,
                        TransitionKind::Worker,
                        IngestionState::Quarantined,
                        Some("policy_decision_applied"),
                        now,
                    )
                    .await?;
            }
            Disposition::Delete | Disposition::Review => {
                // Leave in Cataloged; operator must review or confirm.
                debug!(
                    "policy worker: release {} in state {:?} with disposition {:?}",
                    release_id, work_item.item.state, existing.disposition
                );
            }
        }
        return Ok(());
    }

    // Load catalog track facts for novelty analysis.
    let track_facts = policy_store.load_release_tracks(pool, release_id).await?;

    // Load catalog novelty context (duplicate/identity sets) in a single query.
    let (catalog_exact_duplicates, catalog_known_identities) = policy_store
        .load_catalog_novelty_context(pool, release_id)
        .await?;

    // Build novelty input from catalog facts.
    let novelty_input = build_novelty_input(
        release_id,
        &track_facts,
        &catalog_exact_duplicates,
        &catalog_known_identities,
        now,
    );
    let assessment: NoveltyAssessment = novelty_engine
        .assess(&novelty_input)
        .map_err(|e| format!("novelty assessment failed: {e}"))?;

    // Determine corrupt media from catalog findings.
    let has_corrupt_media = track_facts
        .iter()
        .any(|f| f.byte_hash.is_none() && !f.has_confirmed_audio_identity);

    // Build PolicyInput — the engine performs classification internally.
    let policy_input = PolicyInput {
        release_id,
        title: None,
        directory_name: Some(work_item.item.current_path.clone()),
        parent_directory_name: Some(work_item.item.source_path.clone()),
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: Default::default(),
        actors: vec![PolicyActor::System {
            policy_rule_set_version: engine.rule_set_version(),
        }],
        novelty_category: assessment.category,
        has_corrupt_media,
        now,
    };

    // Evaluate policy (engine classifies internally from PolicyInput).
    let evaluation = engine.evaluate(&policy_input);

    // Persist classification, novelty, and decision atomically.
    let mut tx = pool.begin().await?;
    policy_store
        .store_classification_tx(&mut tx, &evaluation.classification)
        .await?;

    policy_store
        .store_novelty_tx(&mut tx, &assessment.evaluation, &assessment.track_novelty)
        .await?;

    // Record the decision as Pending (not completed). Completion happens
    // separately after destructive action is executed.
    let decision = ReleasePolicyDecision {
        id: ReleasePolicyDecisionId::new(),
        release_id,
        rule_set_id: engine.rule_set_id(),
        rule_set_version: engine.rule_set_version(),
        matched_rule_ids: evaluation.decision.matched_rule_ids.clone(),
        disposition: evaluation.decision.disposition,
        evidence_summary: evaluation.decision.evidence_summary.clone(),
        source_fingerprint: Some(format!(
            "policy_worker_v{}_{}",
            engine.rule_set_version(),
            now.unix_timestamp()
        )),
        actor: PolicyActor::System {
            policy_rule_set_version: engine.rule_set_version(),
        },
        decided_at: now,
        completion_state: DecisionCompletionState::Pending,
        completed_at: None,
    };

    policy_store.record_decision(&mut tx, &decision).await?;

    // Commit the transaction (atomic: classification + novelty + decision).
    tx.commit().await?;

    // Transition the work item based on disposition.
    let target_state = match evaluation.decision.disposition {
        Disposition::Accept => IngestionState::Indexing,
        Disposition::Review => IngestionState::NeedsReview,
        Disposition::Ignore => IngestionState::Ignored,
        Disposition::Quarantine => IngestionState::Quarantined,
        Disposition::Delete => {
            // Delete requires operator authorization: route to NeedsReview
            // with a Pending Delete decision. Operator confirms via the
            // NeedsReview -> Deleted transition.
            IngestionState::NeedsReview
        }
    };

    work_item
        .item
        .state
        .ensure_transition(target_state, TransitionKind::Worker)?;

    let reason_str = format!(
        "policy_disposition_{}",
        evaluation.decision.disposition.as_str()
    );

    work_store
        .transition_for_policy(
            pool,
            work_item.item.id,
            worker_id,
            TransitionKind::Worker,
            target_state,
            Some(&reason_str),
            now,
        )
        .await?;

    info!(
        "policy worker: release {} -> {:?} (disposition: {:?})",
        release_id, target_state, evaluation.decision.disposition
    );

    Ok(())
}

/// Builds a NoveltyInput from catalog track facts and catalog novelty context.
fn build_novelty_input(
    release_id: melodee_domain::ReleaseId,
    track_facts: &[melodee_persistence::CatalogTrackFact],
    catalog_exact_duplicates: &std::collections::BTreeSet<melodee_domain::AudioIdentityId>,
    catalog_known_identities: &std::collections::BTreeSet<melodee_domain::AudioIdentityId>,
    now: OffsetDateTime,
) -> NoveltyInput {
    let evaluator_version = "melodee-policy-novelty-v1".to_string();

    let track_novelty: Vec<TrackNovelty> = track_facts
        .iter()
        .map(|f| {
            let track_id = melodee_domain::TrackId::from_uuid(f.track_id);
            let audio_identity_id = melodee_domain::AudioIdentityId::from_uuid(f.audio_identity_id);
            let media_asset_id = f
                .media_asset_id
                .map(melodee_domain::MediaAssetId::from_uuid);

            let (category, evidence_kind, confidence_bps, detail) =
                if catalog_exact_duplicates.contains(&audio_identity_id) {
                    (
                        TrackNoveltyCategory::ExactDuplicate,
                        melodee_domain::NoveltyEvidenceKind::ByteHashMatch,
                        9_000u16,
                        "byte-hash match found in catalog outside this Release",
                    )
                } else if catalog_known_identities.contains(&audio_identity_id) {
                    (
                        TrackNoveltyCategory::KnownAudioIdentity,
                        melodee_domain::NoveltyEvidenceKind::AudioIdentityMatch,
                        8_000u16,
                        "audio identity confirmed in catalog outside this Release",
                    )
                } else if f.has_confirmed_audio_identity {
                    (
                        TrackNoveltyCategory::KnownAudioIdentity,
                        melodee_domain::NoveltyEvidenceKind::AudioIdentityMatch,
                        7_000u16,
                        "audio identity has local match decision, not yet in catalog",
                    )
                } else {
                    (
                        TrackNoveltyCategory::NewTrack,
                        melodee_domain::NoveltyEvidenceKind::NoCatalogMatch,
                        5_000u16,
                        "no catalog match for this Track's audio identity",
                    )
                };

            let confidence = Confidence::new(confidence_bps).unwrap_or(Confidence::zero());

            TrackNovelty {
                id: TrackNoveltyId::new(),
                release_id,
                track_id,
                audio_identity_id,
                media_asset_id,
                category,
                confidence,
                evidence_kind,
                detail: detail.to_string(),
                evaluated_at: now,
            }
        })
        .collect();

    NoveltyInput {
        release_id,
        track_audio: track_facts
            .iter()
            .map(|f| {
                (
                    melodee_domain::TrackId::from_uuid(f.track_id),
                    melodee_domain::AudioIdentityId::from_uuid(f.audio_identity_id),
                    f.byte_hash.clone(),
                )
            })
            .collect(),
        catalog_exact_duplicates: catalog_exact_duplicates.clone(),
        catalog_known_identities: catalog_known_identities.clone(),
        track_novelty,
        evaluator_version,
        now,
    }
}

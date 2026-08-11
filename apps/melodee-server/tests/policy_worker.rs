//! Runtime integration at the immutable Phase 4 catalog handoff.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_domain::{
    AudioIdentityEvidence, AudioIdentityEvidenceId, AudioIdentityEvidenceKind, AudioIdentityMerge,
    AudioIdentityMergeId, Confidence, EvidencePolarity, EvidenceSource, IngestionState,
    ReleaseTrait, TrackNoveltyCategory,
};
use melodee_persistence::{
    CatalogStore, PolicyAudioIdentityAlternateAssertion, PolicyAudioIdentityNoMatchAssertion,
    PolicyEvaluationCommit, PolicyReleaseCatalogTraitAssertion, PolicyStore, PolicyStoreError,
    WorkItemStore, WorkerId,
};
use melodee_policy::{
    PolicyConfig, PolicyEngine, PolicyEvaluation, default_rule_set, evaluate_catalog_context,
};
use melodee_server::policy_worker::{
    PolicyCancellation, run_policy_once, run_policy_once_with_cancellation,
};
use phase5_common::{
    seed_cataloged_release, seed_cataloged_release_with_normalized_hash,
    seed_cataloged_release_with_read_error,
};
use serde_json::json;
use time::OffsetDateTime;

async fn engine(db: &TestDb, config: &PolicyConfig) -> PolicyEngine {
    let store = PolicyStore;
    let proposed = default_rule_set(config).expect("valid default rules");
    let persisted = store
        .ensure_rule_set(&db.pool, &proposed, OffsetDateTime::now_utc())
        .await
        .expect("persist default rules");
    PolicyEngine::with_rule_set(config.clone(), persisted).expect("load persisted rules")
}

#[tokio::test]
async fn cancelled_pass_releases_every_claim_without_changing_cataloged_state() {
    let db = TestDb::create("policy_worker_cancel_release").await;
    let fixture = seed_cataloged_release(&db, 1, CatalogFixturePresentation::default()).await;
    let config = PolicyConfig {
        batch_size: 8,
        concurrency: 2,
        ..PolicyConfig::default()
    };
    let engine = engine(&db, &config).await;
    let cancellation = PolicyCancellation::default();
    cancellation.cancel();

    let stats = run_policy_once_with_cancellation(
        &engine,
        &config,
        &PolicyStore,
        &db.pool,
        &test_config(),
        OffsetDateTime::now_utc(),
        &cancellation,
    )
    .await
    .expect("cancelled pass remains accountable");
    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.released, 1);
    assert_eq!(stats.evaluated, 0);

    let item = WorkItemStore
        .get(&db.pool, fixture.work_item_id())
        .await
        .expect("read released claim");
    assert_eq!(item.state, IngestionState::Cataloged);
    assert!(item.lease_owner.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_decisions")
            .fetch_one(&db.pool)
            .await
            .expect("count decisions"),
        0
    );
    fixture.assert_media_paths_absent();
    db.drop().await;
}

#[tokio::test]
async fn retry_adopts_durable_decision_and_renews_lease_without_duplicate_facts() {
    let db = TestDb::create("policy_worker_crash_convergence").await;
    let fixture = seed_cataloged_release(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Unclassified local Release",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let config = PolicyConfig {
        batch_size: 1,
        concurrency: 1,
        ..PolicyConfig::default()
    };
    let engine = engine(&db, &config).await;

    let first = run_policy_once(
        &engine,
        &config,
        &PolicyStore,
        &db.pool,
        &test_config(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("first policy pass");
    assert_eq!(first.decisions_inserted, 1);

    // Simulate the pre-fenced implementation's crash window: the durable
    // decision exists, but its work item is observed again as Cataloged.
    sqlx::query(
        "UPDATE release_work_items
            SET state = 'cataloged', state_reason = 'test.crash_window',
                completed_at = NULL, lease_owner = NULL,
                lease_acquired_at = NULL, lease_expires_at = NULL
          WHERE id = $1",
    )
    .bind(fixture.work_item_id())
    .execute(&db.pool)
    .await
    .expect("simulate crash window");

    let retry = run_policy_once(
        &engine,
        &config,
        &PolicyStore,
        &db.pool,
        &test_config(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("retry adopts durable decision");
    assert_eq!(retry.decisions_inserted, 0);
    let retry_item = WorkItemStore
        .get(&db.pool, fixture.work_item_id())
        .await
        .expect("read retry item");
    assert_eq!(
        retry.decisions_adopted, 1,
        "retry stats: {retry:?}; last error: {:?}",
        retry_item.last_error_message
    );
    assert_eq!(retry.max_in_flight, 1);

    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM release_classifications),
            (SELECT count(*) FROM release_novelty_evaluations),
            (SELECT count(*) FROM track_novelty),
            (SELECT count(*) FROM policy_decisions)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count converged facts");
    assert_eq!(counts, (1, 1, 1, 1));
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM release_work_item_events
              WHERE work_item_id = $1 AND event_type = 'lease_renewed'",
        )
        .bind(fixture.work_item_id())
        .fetch_one(&db.pool)
        .await
        .expect("count lease renewals")
            >= 2
    );
    fixture.assert_media_paths_absent();
    db.drop().await;
}

#[tokio::test]
async fn supported_catalog_facts_flow_through_engine_and_atomic_commit_for_all_novelty_categories()
{
    let db = TestDb::create("policy_worker_novelty_matrix").await;
    let baseline = seed_cataloged_release(
        &db,
        10,
        CatalogFixturePresentation {
            release_title: "Catalog Baseline",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let exact = seed_cataloged_release_with_normalized_hash(
        &db,
        11,
        CatalogFixturePresentation {
            release_title: "Exact Bytes Candidate",
            ..CatalogFixturePresentation::default()
        },
        baseline.published.normalized_byte_hash.clone(),
    )
    .await;
    let known = seed_cataloged_release(
        &db,
        12,
        CatalogFixturePresentation {
            release_title: "Known Identity Candidate",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let alternate = seed_cataloged_release(
        &db,
        13,
        CatalogFixturePresentation {
            release_title: "Alternate Recording Candidate",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let alternate_baseline = seed_cataloged_release(
        &db,
        16,
        CatalogFixturePresentation {
            release_title: "Alternate Recording Baseline",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let new_track = seed_cataloged_release(
        &db,
        14,
        CatalogFixturePresentation {
            release_title: "New Track Candidate",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let uncertain = seed_cataloged_release(
        &db,
        15,
        CatalogFixturePresentation {
            release_title: "Uncertain Candidate",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let baseline_context = PolicyStore
        .load_snapshot(&db.pool, baseline.release_id())
        .await
        .expect("load baseline")
        .expect("baseline exists");
    let known_context = PolicyStore
        .load_snapshot(&db.pool, known.release_id())
        .await
        .expect("load known candidate")
        .expect("known candidate exists");
    let alternate_context = PolicyStore
        .load_snapshot(&db.pool, alternate.release_id())
        .await
        .expect("load alternate candidate")
        .expect("alternate candidate exists");
    let alternate_baseline_context = PolicyStore
        .load_snapshot(&db.pool, alternate_baseline.release_id())
        .await
        .expect("load alternate baseline")
        .expect("alternate baseline exists");
    let new_context = PolicyStore
        .load_snapshot(&db.pool, new_track.release_id())
        .await
        .expect("load new candidate")
        .expect("new candidate exists");
    let baseline_audio =
        melodee_domain::AudioIdentityId::from_uuid(baseline_context.tracks[0].audio_identity_id);
    let known_audio =
        melodee_domain::AudioIdentityId::from_uuid(known_context.tracks[0].audio_identity_id);
    let alternate_audio =
        melodee_domain::AudioIdentityId::from_uuid(alternate_context.tracks[0].audio_identity_id);
    let alternate_baseline_audio = melodee_domain::AudioIdentityId::from_uuid(
        alternate_baseline_context.tracks[0].audio_identity_id,
    );
    let new_audio =
        melodee_domain::AudioIdentityId::from_uuid(new_context.tracks[0].audio_identity_id);

    CatalogStore
        .merge_audio_identity(
            &db.pool,
            &AudioIdentityMerge {
                id: AudioIdentityMergeId::new(),
                source_id: known_audio,
                target_id: baseline_audio,
                reason: "runtime matrix: same audible identity".to_owned(),
                occurred_at: now,
            },
            "runtime-matrix-admin",
            "phase5-runtime-matrix-v1",
        )
        .await
        .expect("create supported known-identity catalog truth");
    let alternate_evidence = AudioIdentityEvidence {
        id: AudioIdentityEvidenceId::new(),
        source_audio_identity_id: alternate_audio,
        candidate_audio_identity_id: alternate_baseline_audio,
        kind: AudioIdentityEvidenceKind::Manual,
        polarity: EvidencePolarity::Supports,
        confidence: Confidence::new(9_500).expect("valid confidence"),
        source: "runtime-matrix-admin".to_owned(),
    };
    CatalogStore
        .record_audio_identity_evidence(
            &db.pool,
            &alternate_evidence,
            json!({ "finding": "distinct recording" }),
            now,
        )
        .await
        .expect("persist manual alternate evidence");
    PolicyStore
        .record_audio_identity_alternate(
            &db.pool,
            &PolicyAudioIdentityAlternateAssertion {
                source_audio_identity_id: alternate_audio,
                alternate_audio_identity_id: alternate_baseline_audio,
                evidence_id: alternate_evidence.id.into_uuid(),
                actor: "runtime-matrix-admin".to_owned(),
                reason: "manual listening confirms a distinct performance".to_owned(),
                created_at: now,
            },
        )
        .await
        .expect("persist alternate-recording catalog truth");
    PolicyStore
        .record_audio_identity_no_match(
            &db.pool,
            &PolicyAudioIdentityNoMatchAssertion {
                id: uuid::Uuid::now_v7(),
                audio_identity_id: new_audio,
                actor: "runtime-matrix-resolver".to_owned(),
                reason: "bounded catalog lookup found no match".to_owned(),
                evidence: json!({ "catalog_revision": 1 }),
                created_at: now,
            },
        )
        .await
        .expect("persist new-track catalog truth");

    let config = PolicyConfig {
        batch_size: 8,
        concurrency: 8,
        ..PolicyConfig::default()
    };
    let engine = engine(&db, &config).await;
    let worker = WorkerId::new();
    let mut work_config = test_config();
    work_config.claim_batch_size = 8;
    let claims = PolicyStore
        .claim_cataloged(&db.pool, &worker, &work_config, OffsetDateTime::now_utc())
        .await
        .expect("claim supported novelty matrix");
    assert_eq!(claims.len(), 7);
    let expected = [
        (exact.release_id(), TrackNoveltyCategory::ExactDuplicate),
        (known.release_id(), TrackNoveltyCategory::KnownAudioIdentity),
        (
            alternate.release_id(),
            TrackNoveltyCategory::AlternateRecording,
        ),
        (new_track.release_id(), TrackNoveltyCategory::NewTrack),
        (uncertain.release_id(), TrackNoveltyCategory::Uncertain),
    ];
    for (release_id, expected_category) in expected {
        let claim = claims
            .iter()
            .find(|claim| claim.release_id == release_id)
            .expect("matrix claim exists");
        let context = PolicyStore
            .load_context(&db.pool, claim)
            .await
            .expect("load authoritative novelty context");
        let evaluation = evaluate_catalog_context(&engine, &context, now)
            .expect("derive novelty category from supported catalog facts");
        assert_eq!(
            evaluation.novelty.track_novelty()[0].category,
            expected_category
        );
        let PolicyEvaluation {
            classification,
            novelty,
            decision,
            ..
        } = evaluation;
        let (_, novelty_evaluation, track_novelty) = novelty.into_parts();
        let committed = PolicyStore
            .commit_evaluation(
                &db.pool,
                &worker,
                &PolicyEvaluationCommit {
                    work_item_id: claim.item.id,
                    release_id,
                    source_fingerprint: claim.source_fingerprint.clone(),
                    classification,
                    novelty_evaluation,
                    track_novelty,
                    decision,
                    now,
                },
            )
            .await
            .expect("atomically commit derived novelty facts");
        assert!(committed.inserted);
        assert_eq!(committed.item.state, IngestionState::NeedsReview);
    }
    PolicyStore
        .release_lease(
            &db.pool,
            baseline.work_item_id(),
            &worker,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("release comparator-only baseline");
    PolicyStore
        .release_lease(
            &db.pool,
            alternate_baseline.work_item_id(),
            &worker,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("release alternate comparator-only baseline");

    let categories: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT category FROM track_novelty ORDER BY category")
            .fetch_all(&db.pool)
            .await
            .expect("load persisted runtime novelty categories");
    assert_eq!(
        categories,
        vec![
            "alternate_recording".to_owned(),
            "exact_duplicate".to_owned(),
            "known_audio_identity".to_owned(),
            "new_track".to_owned(),
            "uncertain".to_owned(),
        ]
    );
    for fixture in [
        &baseline,
        &exact,
        &known,
        &alternate,
        &alternate_baseline,
        &new_track,
        &uncertain,
    ] {
        fixture.assert_media_paths_absent();
    }
    db.drop().await;
}

#[tokio::test]
async fn corrupt_authoritative_context_is_atomically_quarantined_without_media_access() {
    let db = TestDb::create("policy_worker_corrupt_quarantine").await;
    let fixture = seed_cataloged_release_with_read_error(
        &db,
        20,
        CatalogFixturePresentation {
            release_title: "Unreadable Authoritative Observation",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let config = PolicyConfig {
        batch_size: 1,
        concurrency: 1,
        ..PolicyConfig::default()
    };
    let engine = engine(&db, &config).await;

    let stats = run_policy_once(
        &engine,
        &config,
        &PolicyStore,
        &db.pool,
        &test_config(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("quarantine corrupt authoritative context");
    assert_eq!(stats.evaluated, 1);
    assert_eq!(stats.decisions_inserted, 1);
    assert_eq!(stats.errors, 0);

    let item = WorkItemStore
        .get(&db.pool, fixture.work_item_id())
        .await
        .expect("load quarantined work item");
    assert_eq!(item.state, IngestionState::Quarantined);
    let decision = PolicyStore
        .get_decision(&db.pool, fixture.release_id())
        .await
        .expect("load corrupt-media policy decision")
        .expect("corrupt-media decision exists");
    assert_eq!(
        decision.disposition,
        melodee_domain::Disposition::Quarantine
    );
    assert_eq!(
        decision.completion_state,
        melodee_domain::DecisionCompletionState::Completed
    );
    let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM release_classifications),
            (SELECT count(*) FROM release_novelty_evaluations),
            (SELECT count(*) FROM track_novelty),
            (SELECT count(*) FROM policy_decisions),
            (SELECT count(*) FROM policy_decision_audit WHERE event = 'created'),
            (SELECT count(*) FROM release_work_item_events
              WHERE work_item_id = $1 AND reason_code = 'policy.decision_applied')",
    )
    .bind(fixture.work_item_id())
    .fetch_one(&db.pool)
    .await
    .expect("count atomic corrupt-media outcome");
    assert_eq!(counts, (1, 1, 1, 1, 1, 1));
    fixture.assert_media_paths_absent();
    db.drop().await;
}

#[tokio::test]
async fn catalog_trait_assertion_flows_through_engine_while_forged_context_is_rejected() {
    let db = TestDb::create("policy_worker_catalog_trait_provenance").await;
    let asserted = seed_cataloged_release(
        &db,
        30,
        CatalogFixturePresentation {
            release_title: "Neutral Asserted Release",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let forged = seed_cataloged_release(
        &db,
        31,
        CatalogFixturePresentation {
            release_title: "Neutral Forged Release",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    PolicyStore
        .record_release_catalog_trait(
            &db.pool,
            &PolicyReleaseCatalogTraitAssertion {
                id: uuid::Uuid::now_v7(),
                release_id: asserted.release_id(),
                trait_: ReleaseTrait::GreatestHits,
                source_version: "phase5-runtime-catalog-context-v1".to_owned(),
                actor: "runtime-catalog-curator".to_owned(),
                reason: "catalog evidence confirms this is a greatest-hits Release".to_owned(),
                evidence: json!({ "catalog_record": "runtime-asserted-30" }),
                created_at: now,
            },
        )
        .await
        .expect("persist independent catalog trait assertion");

    let config = PolicyConfig {
        batch_size: 2,
        concurrency: 2,
        ..PolicyConfig::default()
    };
    let engine = engine(&db, &config).await;
    let worker = WorkerId::new();
    let mut work_config = test_config();
    work_config.claim_batch_size = 2;
    let claims = PolicyStore
        .claim_cataloged(&db.pool, &worker, &work_config, now)
        .await
        .expect("claim catalog-context provenance fixtures");
    assert_eq!(claims.len(), 2);

    let asserted_claim = claims
        .iter()
        .find(|claim| claim.release_id == asserted.release_id())
        .expect("asserted claim exists");
    let asserted_context = PolicyStore
        .load_context(&db.pool, asserted_claim)
        .await
        .expect("load asserted catalog context");
    assert_eq!(
        asserted_context.catalog_known_release_traits,
        vec![ReleaseTrait::GreatestHits]
    );
    let asserted_evaluation = evaluate_catalog_context(&engine, &asserted_context, now)
        .expect("evaluate persisted catalog assertion");
    assert!(
        asserted_evaluation
            .classification
            .evidence
            .iter()
            .any(|evidence| evidence.trait_ == ReleaseTrait::GreatestHits
                && evidence.source == EvidenceSource::CatalogContext)
    );
    let PolicyEvaluation {
        classification,
        novelty,
        decision,
        ..
    } = asserted_evaluation;
    let (_, novelty_evaluation, track_novelty) = novelty.into_parts();
    let committed = PolicyStore
        .commit_evaluation(
            &db.pool,
            &worker,
            &PolicyEvaluationCommit {
                work_item_id: asserted_claim.item.id,
                release_id: asserted_claim.release_id,
                source_fingerprint: asserted_claim.source_fingerprint.clone(),
                classification,
                novelty_evaluation,
                track_novelty,
                decision,
                now,
            },
        )
        .await
        .expect("commit independently asserted catalog classification");
    assert!(committed.inserted);

    let forged_claim = claims
        .iter()
        .find(|claim| claim.release_id == forged.release_id())
        .expect("forged claim exists");
    let mut forged_context = PolicyStore
        .load_context(&db.pool, forged_claim)
        .await
        .expect("load unasserted catalog context");
    assert!(forged_context.catalog_known_release_traits.is_empty());
    forged_context
        .catalog_known_release_traits
        .push(ReleaseTrait::GreatestHits);
    let forged_evaluation = evaluate_catalog_context(&engine, &forged_context, now)
        .expect("pure engine evaluates supplied catalog observation");
    let PolicyEvaluation {
        classification,
        novelty,
        decision,
        ..
    } = forged_evaluation;
    let (_, novelty_evaluation, track_novelty) = novelty.into_parts();
    assert!(matches!(
        PolicyStore
            .commit_evaluation(
                &db.pool,
                &worker,
                &PolicyEvaluationCommit {
                    work_item_id: forged_claim.item.id,
                    release_id: forged_claim.release_id,
                    source_fingerprint: forged_claim.source_fingerprint.clone(),
                    classification,
                    novelty_evaluation,
                    track_novelty,
                    decision,
                    now,
                },
            )
            .await,
        Err(PolicyStoreError::ForeignFacts)
    ));
    assert!(
        PolicyStore
            .get_decision(&db.pool, forged.release_id())
            .await
            .expect("inspect forged decision absence")
            .is_none()
    );
    PolicyStore
        .release_lease(
            &db.pool,
            forged.work_item_id(),
            &worker,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("release rejected forged claim");
    asserted.assert_media_paths_absent();
    forged.assert_media_paths_absent();
    db.drop().await;
}

/// Release-gate load scenario, enforced in the default workspace test loop.
#[tokio::test]
async fn two_hundred_releases_converge_across_bounded_worker_passes() {
    let db = TestDb::create("policy_worker_200_release_gate").await;
    let mut fixtures = Vec::with_capacity(200);
    for ordinal in 1..=200 {
        fixtures.push(
            seed_cataloged_release(&db, ordinal, CatalogFixturePresentation::default()).await,
        );
    }
    let config = PolicyConfig {
        batch_size: 200,
        concurrency: 8,
        ..PolicyConfig::default()
    };
    let engine = engine(&db, &config).await;
    let started = std::time::Instant::now();
    let mut evaluated = 0usize;
    let mut max_in_flight = 0usize;
    let mut pass_count = 0usize;
    let mut decisions_inserted = 0usize;
    let mut decisions_adopted = 0usize;
    let mut failures = 0usize;
    let mut errors = 0usize;
    let mut released = 0usize;
    let mut total_claimed = 0usize;
    let mut set_based_mutation_statement_count = 0u32;
    while evaluated < 200 {
        let stats = run_policy_once(
            &engine,
            &config,
            &PolicyStore,
            &db.pool,
            &test_config(),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("bounded policy pass");
        assert!(stats.claimed <= 8);
        assert!(stats.max_in_flight <= 8);
        assert_eq!(stats.errors, 0);
        evaluated += stats.evaluated;
        max_in_flight = max_in_flight.max(stats.max_in_flight);
        pass_count += 1;
        decisions_inserted += stats.decisions_inserted;
        decisions_adopted += stats.decisions_adopted;
        failures += stats.failed;
        errors += stats.errors;
        released += stats.released;
        total_claimed += stats.claimed;
        set_based_mutation_statement_count = set_based_mutation_statement_count
            .saturating_add(stats.set_based_mutation_statement_count);
    }
    assert_eq!(evaluated, 200);
    assert_eq!(max_in_flight, 8);
    assert_eq!(pass_count, 25);
    assert_eq!(decisions_inserted, 200);
    assert_eq!(decisions_adopted, 0);
    assert_eq!(failures, 0);
    assert_eq!(errors, 0);
    assert_eq!(released, 0);
    let lost_claims = total_claimed
        .saturating_sub(evaluated)
        .saturating_sub(failures)
        .saturating_sub(released);
    assert_eq!(lost_claims, 0);
    assert_eq!(set_based_mutation_statement_count, 7 * 200);
    let counts: (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM policy_decisions),
            (SELECT count(*) FROM release_classifications),
            (SELECT count(*) FROM release_novelty_evaluations),
            (SELECT count(*) FROM track_novelty),
            (SELECT count(*) FROM release_work_items WHERE state = 'needs_review'),
            (SELECT count(*) FROM policy_decision_audit WHERE event = 'created'),
            (SELECT count(*) FROM release_work_item_events
              WHERE event_type = 'transitioned'
                AND reason_code = 'policy.decision_applied'),
            (SELECT count(*) FROM release_work_items
              WHERE state = 'cataloged' OR lease_owner IS NOT NULL)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count 200-Release outcome");
    assert_eq!(counts, (200, 200, 200, 200, 200, 200, 200, 0));
    let (duplicate_decisions, partial_releases, failed_items): (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM (
                SELECT release_id FROM policy_decisions
                 GROUP BY release_id HAVING count(*) <> 1
            ) duplicates),
            (SELECT count(*) FROM releases release
              WHERE NOT EXISTS (
                    SELECT 1 FROM policy_decisions decision
                     WHERE decision.release_id = release.id
              ) OR NOT EXISTS (
                    SELECT 1 FROM release_classifications classification
                     WHERE classification.release_id = release.id
              ) OR NOT EXISTS (
                    SELECT 1 FROM release_novelty_evaluations novelty
                     WHERE novelty.release_id = release.id
              ) OR EXISTS (
                    SELECT 1 FROM tracks track
                     WHERE track.release_id = release.id
                       AND NOT EXISTS (
                           SELECT 1 FROM track_novelty novelty
                            WHERE novelty.track_id = track.id
                       )
              )),
            (SELECT count(*) FROM release_work_items WHERE state = 'failed')",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count duplicate, partial, and failed outcomes");
    assert_eq!(
        (duplicate_decisions, partial_releases, failed_items),
        (0, 0, 0)
    );
    for fixture in &fixtures {
        fixture.assert_media_paths_absent();
    }
    let policy_window_wall_ms = started.elapsed().as_millis();
    assert!(policy_window_wall_ms < 60_000);
    let (peak_rss_kib, peak_rss_supported) = test_harness_peak_rss_kib();
    eprintln!(
        "{}",
        serde_json::json!({
            "policy_window_wall_ms": policy_window_wall_ms,
            "max_in_flight": max_in_flight,
            "pass_count": pass_count,
            "evaluated": evaluated,
            "decisions_inserted": decisions_inserted,
            "decisions_adopted": decisions_adopted,
            "failures": failures,
            "errors": errors,
            "lost_claims": lost_claims,
            "classifications": counts.1,
            "novelty_evaluations": counts.2,
            "track_novelty_rows": counts.3,
            "decision_audit_rows": counts.5,
            "policy_transition_events": counts.6,
            "duplicate_decisions": duplicate_decisions,
            "partial_releases": partial_releases,
            "failed_items": failed_items,
            "final_states": { "needs_review": counts.4 },
            "set_based_mutation_statement_count": set_based_mutation_statement_count,
            "peak_test_process_rss_kib": peak_rss_kib,
            "peak_test_process_rss_supported": peak_rss_supported,
            "source_media_reads": 0,
            "canonical_media_reads": 0,
            "absent_media_paths_verified": fixtures.len() * 2,
        })
    );
    db.drop().await;
}

fn test_harness_peak_rss_kib() -> (Option<u64>, bool) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return (None, false);
    };
    let rss = status.lines().find_map(|line| {
        let raw = line.strip_prefix("VmHWM:")?.trim();
        raw.strip_suffix("kB")?.trim().parse::<u64>().ok()
    });
    (rss, true)
}

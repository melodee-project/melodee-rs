//! Atomic policy fact/decision/state acceptance and adversarial rejection.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use std::collections::BTreeSet;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_domain::{
    ClassificationEvidenceKind, Confidence, DecisionCompletionState, Disposition,
    DomainInvariantError, EvidenceSource, IngestionState, NoveltyEvidenceKind, PolicyActor,
    PolicyRule, PolicyRuleSetId, PolicyRuleType, ReleaseTrait, TrackNoveltyCategory,
};
use melodee_persistence::{
    MAX_POLICY_CLASSIFICATION_EVIDENCE, PolicyCatalogContext, PolicyReleaseCatalogTraitAssertion,
    PolicyStore, PolicyStoreError, WorkerId,
};
use phase5_common::{
    policy_rule_set, seed_cataloged_release, seed_cataloged_release_with_read_error,
    unknown_review_commit,
};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

async fn claimed_context(
    db: &TestDb,
    worker: &WorkerId,
    now: OffsetDateTime,
) -> (phase5_common::CatalogedPolicyFixture, PolicyCatalogContext) {
    let fixture = seed_cataloged_release(
        db,
        1,
        CatalogFixturePresentation {
            release_title: "Ordinary Release",
            release_artist: "Policy Commit Fixture",
            track_title: "New Track",
            track_artist: "Policy Commit Fixture",
            release_date: "2005-06-07",
        },
    )
    .await;
    let mut claims = PolicyStore
        .claim_cataloged(&db.pool, worker, &test_config(), now)
        .await
        .expect("claim policy commit fixture");
    assert_eq!(claims.len(), 1);
    let claim = claims.pop().expect("one claim");
    let context = PolicyStore
        .load_context(&db.pool, &claim)
        .await
        .expect("load policy commit context");
    (fixture, context)
}

#[tokio::test]
async fn valid_evaluation_commits_facts_decision_audit_and_state_atomically() {
    let db = TestDb::create("phase5_atomic_commit").await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let (fixture, context) = claimed_context(&db, &worker, now).await;
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist actual rule set");
    let request = unknown_review_commit(&context, rule_set.id, now);

    let committed = PolicyStore
        .commit_evaluation(&db.pool, &worker, &request)
        .await
        .expect("commit atomic policy evaluation");
    assert!(committed.inserted);
    assert_eq!(committed.item.state, IngestionState::NeedsReview);
    assert!(committed.item.lease_owner.is_none());
    assert_eq!(committed.decision.disposition, Disposition::Review);
    assert_eq!(
        committed.decision.completion_state,
        DecisionCompletionState::Pending
    );
    assert_eq!(committed.set_based_mutation_statement_count, 8);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM releases WHERE id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("reviewed Release status"),
        "deprecated"
    );

    let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM release_classifications),
            (SELECT count(*) FROM release_classification_evidence),
            (SELECT count(*) FROM track_novelty),
            (SELECT count(*) FROM release_novelty_evaluations),
            (SELECT count(*) FROM policy_decisions),
            (SELECT count(*) FROM policy_decision_audit)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count atomic policy rows");
    assert_eq!(counts, (1, 1, 1, 1, 1, 1));
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn forged_novelty_actor_and_rule_outcomes_fail_without_partial_writes() {
    let db = TestDb::create("phase5_commit_spoofing").await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let (fixture, context) = claimed_context(&db, &worker, now).await;
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist spoof-test rule set");

    let mut forged_novelty = unknown_review_commit(&context, rule_set.id, now);
    forged_novelty.track_novelty[0].category = TrackNoveltyCategory::ExactDuplicate;
    forged_novelty.track_novelty[0].evidence_kind = NoveltyEvidenceKind::ByteHashMatch;
    forged_novelty.novelty_evaluation.uncertain_count = 0;
    forged_novelty.novelty_evaluation.exact_duplicate_count = 1;
    forged_novelty.decision.matched_rule_ids = vec!["redundant-greatest-hits".to_string()];
    forged_novelty.decision.disposition = Disposition::Ignore;
    assert!(
        PolicyStore
            .commit_evaluation(&db.pool, &worker, &forged_novelty)
            .await
            .is_err(),
        "caller labels cannot forge a redundant collection"
    );

    let mut forged_classification = unknown_review_commit(&context, rule_set.id, now);
    forged_classification.classification.traits =
        [ReleaseTrait::GreatestHits].into_iter().collect();
    forged_classification.classification.confidence =
        Confidence::new(4_000).expect("valid confidence");
    forged_classification.classification.evidence[0].trait_ = ReleaseTrait::GreatestHits;
    forged_classification.classification.evidence[0].source = EvidenceSource::ReleaseTitle;
    forged_classification.classification.evidence[0].kind =
        ClassificationEvidenceKind::TitlePattern;
    forged_classification.classification.evidence[0].confidence =
        Confidence::new(4_000).expect("valid confidence");
    forged_classification.classification.evidence[0].detail =
        "caller falsely labeled an ordinary title Greatest Hits".to_string();
    assert!(
        PolicyStore
            .commit_evaluation(&db.pool, &worker, &forged_classification)
            .await
            .is_err(),
        "caller labels cannot forge a title classification absent in immutable observations"
    );

    let mut forged_actor = unknown_review_commit(&context, rule_set.id, now);
    forged_actor.decision.actor = PolicyActor::Administrator {
        actor: "spoofed-admin".to_string(),
    };
    assert!(
        PolicyStore
            .commit_evaluation(&db.pool, &worker, &forged_actor)
            .await
            .is_err(),
        "an ambient administrator actor is not an explicit override"
    );

    let mut forged_rule = unknown_review_commit(&context, rule_set.id, now);
    forged_rule.decision.disposition = Disposition::Accept;
    assert!(
        PolicyStore
            .commit_evaluation(&db.pool, &worker, &forged_rule)
            .await
            .is_err(),
        "the persisted rule outcome cannot be replaced by caller input"
    );

    let mut forged_summary = unknown_review_commit(&context, rule_set.id, now);
    forged_summary.decision.evidence_summary =
        "Caller supplied a plausible but non-canonical explanation".to_string();
    assert!(
        PolicyStore
            .commit_evaluation(&db.pool, &worker, &forged_summary)
            .await
            .is_err(),
        "the durable explanation must be derived by the same rule evaluation"
    );

    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM release_classifications),
            (SELECT count(*) FROM track_novelty),
            (SELECT count(*) FROM release_novelty_evaluations),
            (SELECT count(*) FROM policy_decisions)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count rejected policy rows");
    assert_eq!(counts, (0, 0, 0, 0));
    let item = melodee_persistence::WorkItemStore
        .get(&db.pool, fixture.work_item_id())
        .await
        .expect("read item after rejected commits");
    assert_eq!(item.state, IngestionState::Cataloged);
    assert_eq!(item.lease_owner.as_deref(), Some(worker.as_str()));
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn classification_evidence_cap_plus_one_rejects_before_any_materialization() {
    let db = TestDb::create("phase5_classification_evidence_cap").await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let (fixture, context) = claimed_context(&db, &worker, now).await;
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist evidence-cap rule set");
    let mut request = unknown_review_commit(&context, rule_set.id, now);
    let prototype = request.classification.evidence[0].clone();
    request.classification.evidence = (0..=MAX_POLICY_CLASSIFICATION_EVIDENCE)
        .map(|ordinal| {
            let mut evidence = prototype.clone();
            evidence.detail = format!("bounded classification evidence {ordinal:04}");
            evidence
        })
        .collect();

    let error = PolicyStore
        .commit_evaluation(&db.pool, &worker, &request)
        .await
        .expect_err("cap+1 classification evidence must fail closed");
    assert!(matches!(
        error,
        PolicyStoreError::Domain(DomainInvariantError::ClassificationScopeExceeded {
            dimension: "evidence rows",
            limit: MAX_POLICY_CLASSIFICATION_EVIDENCE,
        })
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT
                (SELECT count(*) FROM release_classifications)
              + (SELECT count(*) FROM release_classification_evidence)
              + (SELECT count(*) FROM track_novelty)
              + (SELECT count(*) FROM release_novelty_evaluations)
              + (SELECT count(*) FROM policy_decisions)",
        )
        .fetch_one(&db.pool)
        .await
        .expect("cap rejection materializes no policy rows"),
        0
    );
    let item = melodee_persistence::WorkItemStore
        .get(&db.pool, fixture.work_item_id())
        .await
        .expect("read item after cap rejection");
    assert_eq!(item.state, IngestionState::Cataloged);
    assert_eq!(item.lease_owner.as_deref(), Some(worker.as_str()));
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn immutable_media_read_error_routes_quarantine_atomically_without_reopening_paths() {
    let db = TestDb::create("phase5_corrupt_media_quarantine").await;
    let fixture = seed_cataloged_release_with_read_error(
        &db,
        3,
        CatalogFixturePresentation {
            release_title: "Unreadable Local Release",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let claim = PolicyStore
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim corrupt-media fixture")
        .pop()
        .expect("one corrupt-media claim");
    let context = PolicyStore
        .load_context(&db.pool, &claim)
        .await
        .expect("load corrupt-media context");
    assert!(context.has_corrupt_media);
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist quarantine rule set");
    let mut request = unknown_review_commit(&context, rule_set.id, now);
    request.decision.matched_rule_ids = vec!["corrupt-media".to_string()];
    request.decision.disposition = Disposition::Quarantine;
    request.decision.evidence_summary = "Corrupt or unreadable media".to_string();

    let committed = PolicyStore
        .commit_evaluation(&db.pool, &worker, &request)
        .await
        .expect("commit authoritative quarantine outcome");
    assert_eq!(committed.item.state, IngestionState::Quarantined);
    assert_eq!(committed.decision.disposition, Disposition::Quarantine);
    assert_eq!(
        committed.decision.completion_state,
        DecisionCompletionState::Completed
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM releases WHERE id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("quarantined Release status"),
        "deprecated"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_decision_audit")
            .fetch_one(&db.pool)
            .await
            .expect("quarantine decision is audited"),
        1
    );
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn initial_none_skips_existing_disposition_rule_and_exact_restart_adopts_later_match() {
    let db = TestDb::create("phase5_existing_disposition_none").await;
    let fixture = seed_cataloged_release(&db, 4, CatalogFixturePresentation::default()).await;
    let first_worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let claim = PolicyStore
        .claim_cataloged(&db.pool, &first_worker, &test_config(), now)
        .await
        .expect("claim initial policy item")
        .pop()
        .expect("one initial claim");
    let context = PolicyStore
        .load_context(&db.pool, &claim)
        .await
        .expect("load initial context");
    assert_eq!(context.existing_disposition, None);

    let mut rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    let later_unknown = rule_set
        .rules
        .len()
        .checked_sub(1)
        .expect("rule set contains Unknown fallback");
    rule_set.rules.insert(
        later_unknown,
        PolicyRule {
            id: "existing-ignore-only".to_string(),
            rule_type: PolicyRuleType::UnknownClassification,
            predicate_json: json!({ "existing_dispositions": ["ignore"] }),
            disposition: Disposition::Review,
            confidence_threshold: None,
        },
    );
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist existing-disposition rule set");

    let mut forged_earlier_match = unknown_review_commit(&context, rule_set.id, now);
    forged_earlier_match.decision.matched_rule_ids = vec!["existing-ignore-only".to_string()];
    assert!(matches!(
        PolicyStore
            .commit_evaluation(&db.pool, &first_worker, &forged_earlier_match)
            .await,
        Err(PolicyStoreError::DecisionConflict)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_decisions")
            .fetch_one(&db.pool)
            .await
            .expect("forged earlier match writes no decision"),
        0
    );

    let request = unknown_review_commit(&context, rule_set.id, now);
    assert_eq!(request.decision.matched_rule_ids, ["unknown"]);
    let inserted = PolicyStore
        .commit_evaluation(&db.pool, &first_worker, &request)
        .await
        .expect("later no-existing-disposition rule commits");
    assert!(inserted.inserted);

    let restart_at = OffsetDateTime::now_utc();
    sqlx::query(
        "UPDATE release_work_items
            SET state = 'cataloged', state_reason = NULL, completed_at = NULL,
                lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL,
                updated_at = $2
          WHERE id = $1",
    )
    .bind(fixture.work_item_id())
    .bind(restart_at)
    .execute(&db.pool)
    .await
    .expect("simulate restart after decision before authoritative state route");
    let restarted_worker = WorkerId::new();
    let restarted_claim = PolicyStore
        .claim_cataloged(&db.pool, &restarted_worker, &test_config(), restart_at)
        .await
        .expect("restart reclaims Cataloged item")
        .pop()
        .expect("one restart claim");
    assert_eq!(restarted_claim.item.id, fixture.work_item_id());
    let mut retry = request;
    retry.now = restart_at;
    let adopted = PolicyStore
        .commit_evaluation(&db.pool, &restarted_worker, &retry)
        .await
        .expect("exact later-rule decision is adopted");
    assert!(!adopted.inserted);
    assert_eq!(adopted.item.state, IngestionState::NeedsReview);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM policy_decisions),
            (SELECT count(*) FROM policy_decision_audit)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("adoption preserves one decision and audit root");
    assert_eq!(counts, (1, 1));
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn catalog_trait_assertion_is_immutable_and_required_by_context_and_commit() {
    let db = TestDb::create("phase5_catalog_trait_assertion").await;
    let asserted = seed_cataloged_release(&db, 5, CatalogFixturePresentation::default()).await;
    let now = OffsetDateTime::now_utc();
    let assertion = PolicyReleaseCatalogTraitAssertion {
        id: Uuid::now_v7(),
        release_id: asserted.release_id(),
        trait_: ReleaseTrait::Remaster,
        source_version: "catalog-analysis-v1".to_string(),
        actor: "catalog-analyzer".to_string(),
        reason: "independent catalog chronology confirms a remaster".to_string(),
        evidence: json!({ "catalog_release_count": 2 }),
        created_at: now,
    };
    PolicyStore
        .record_release_catalog_trait(&db.pool, &assertion)
        .await
        .expect("record independent catalog trait");
    PolicyStore
        .record_release_catalog_trait(&db.pool, &assertion)
        .await
        .expect("adopt exact catalog trait retry");
    for (invalid_id, invalid_release, label) in [
        (
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").expect("valid UUIDv4 fixture"),
            asserted.release_id().into_uuid(),
            "non-UUIDv7 assertion ID",
        ),
        (Uuid::now_v7(), Uuid::now_v7(), "foreign Release assertion"),
    ] {
        assert!(
            sqlx::query(
                "INSERT INTO release_catalog_trait_assertions
                    (id, release_id, trait, source_version, actor, reason, evidence, created_at)
                 VALUES ($1, $2, 'live', 'catalog-analysis-v1', 'schema-test',
                         'invalid assertion fixture', '{}'::jsonb, $3)",
            )
            .bind(invalid_id)
            .bind(invalid_release)
            .bind(now)
            .execute(&db.pool)
            .await
            .is_err(),
            "schema rejects {label}"
        );
    }
    let mut conflicting = assertion.clone();
    conflicting.id = Uuid::now_v7();
    assert!(matches!(
        PolicyStore
            .record_release_catalog_trait(&db.pool, &conflicting)
            .await,
        Err(PolicyStoreError::DecisionConflict)
    ));
    assert!(
        sqlx::query(
            "UPDATE release_catalog_trait_assertions SET reason = 'tampered' WHERE id = $1"
        )
        .bind(assertion.id)
        .execute(&db.pool)
        .await
        .is_err(),
        "catalog trait evidence is immutable"
    );
    assert!(
        sqlx::query("DELETE FROM release_catalog_trait_assertions WHERE id = $1")
            .bind(assertion.id)
            .execute(&db.pool)
            .await
            .is_err(),
        "catalog trait evidence is append-only"
    );

    let worker = WorkerId::new();
    let claim = PolicyStore
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim asserted Release")
        .pop()
        .expect("one asserted Release claim");
    let context = PolicyStore
        .load_context(&db.pool, &claim)
        .await
        .expect("load asserted catalog context");
    assert_eq!(
        context.catalog_known_release_traits,
        [ReleaseTrait::Remaster]
    );
    let mut rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    let later_unknown = rule_set.rules.len() - 1;
    rule_set.rules.insert(
        later_unknown,
        PolicyRule {
            id: "catalog-remaster".to_string(),
            rule_type: PolicyRuleType::TraitDisposition,
            predicate_json: json!({ "required_traits": ["remaster"] }),
            disposition: Disposition::Review,
            confidence_threshold: None,
        },
    );
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist catalog trait rule");
    let mut request = unknown_review_commit(&context, rule_set.id, now);
    request.classification.traits = BTreeSet::from([ReleaseTrait::Remaster]);
    request.classification.evidence[0].trait_ = ReleaseTrait::Remaster;
    request.classification.evidence[0].source = EvidenceSource::CatalogContext;
    request.classification.evidence[0].kind = ClassificationEvidenceKind::CatalogKnownRelease;
    request.classification.evidence[0].confidence =
        Confidence::new(6_000).expect("valid catalog confidence");
    request.classification.evidence[0].detail = "Catalog context identified Remaster".to_string();
    request.classification.confidence = Confidence::new(6_000).expect("valid catalog confidence");
    request.decision.matched_rule_ids = vec!["catalog-remaster".to_string()];
    request.decision.evidence_summary =
        "Configured trait rule catalog-remaster matched".to_string();
    let committed = PolicyStore
        .commit_evaluation(&db.pool, &worker, &request)
        .await
        .expect("commit independently asserted catalog trait");
    assert_eq!(committed.item.state, IngestionState::NeedsReview);

    let forged = seed_cataloged_release(&db, 6, CatalogFixturePresentation::default()).await;
    let forged_worker = WorkerId::new();
    let forged_claim = PolicyStore
        .claim_cataloged(
            &db.pool,
            &forged_worker,
            &test_config(),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("claim unasserted Release")
        .pop()
        .expect("one unasserted Release claim");
    let forged_context = PolicyStore
        .load_context(&db.pool, &forged_claim)
        .await
        .expect("load unasserted context");
    assert!(forged_context.catalog_known_release_traits.is_empty());
    let mut forged_request = unknown_review_commit(&forged_context, rule_set.id, now);
    forged_request.classification = request.classification.clone();
    forged_request.classification.id = melodee_domain::ReleaseClassificationId::new();
    forged_request.classification.release_id = forged.release_id();
    for evidence in &mut forged_request.classification.evidence {
        evidence.id = forged_request.classification.id;
        evidence.release_id = forged.release_id();
    }
    forged_request.decision.matched_rule_ids = vec!["catalog-remaster".to_string()];
    forged_request.decision.evidence_summary =
        "Configured trait rule catalog-remaster matched".to_string();
    assert!(matches!(
        PolicyStore
            .commit_evaluation(&db.pool, &forged_worker, &forged_request)
            .await,
        Err(PolicyStoreError::ForeignFacts)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy_decisions WHERE release_id = $1",
        )
        .bind(forged.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("forged catalog trait writes no decision"),
        0
    );
    asserted.assert_media_paths_absent();
    forged.assert_media_paths_absent();

    db.drop().await;
}

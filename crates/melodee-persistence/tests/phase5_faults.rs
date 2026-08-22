//! Lease fencing and restart convergence at the transaction boundary.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_domain::{IngestionState, PolicyRuleSetId};
use melodee_persistence::{PolicyEvaluationCommit, PolicyStore, PolicyStoreError, WorkerId};
use phase5_common::{policy_rule_set, seed_cataloged_release, unknown_review_commit};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn insert_crash_visible_policy_rows(db: &TestDb, request: &PolicyEvaluationCommit) {
    let mut tx = db.pool.begin().await.expect("begin crash simulation");
    let classification = &request.classification;
    sqlx::query(
        "INSERT INTO release_classifications
            (id, release_id, confidence, classifier_version, classified_at,
             created_at, rule_set_id, rule_set_version)
         VALUES ($1, $2, $3, $4, $5, $5, $6, $7)",
    )
    .bind(classification.id.into_uuid())
    .bind(classification.release_id.into_uuid())
    .bind(i32::from(classification.confidence.basis_points()))
    .bind(&classification.classifier_version)
    .bind(classification.classified_at)
    .bind(request.decision.rule_set_id.into_uuid())
    .bind(i32::try_from(request.decision.rule_set_version).expect("rule version fits"))
    .execute(&mut *tx)
    .await
    .expect("insert crash-visible classification");
    for trait_ in &classification.traits {
        sqlx::query(
            "INSERT INTO release_traits (classification_id, trait, created_at)
             VALUES ($1, $2, $3)",
        )
        .bind(classification.id.into_uuid())
        .bind(trait_.as_str())
        .bind(classification.classified_at)
        .execute(&mut *tx)
        .await
        .expect("insert crash-visible trait");
    }
    for evidence in &classification.evidence {
        sqlx::query(
            "INSERT INTO release_classification_evidence
                (id, trait, source, kind, confidence, detail, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(classification.id.into_uuid())
        .bind(evidence.trait_.as_str())
        .bind(evidence.source.as_str())
        .bind(evidence.kind.as_str())
        .bind(i32::from(evidence.confidence.basis_points()))
        .bind(&evidence.detail)
        .bind(classification.classified_at)
        .execute(&mut *tx)
        .await
        .expect("insert crash-visible classification evidence");
    }

    let evaluation = &request.novelty_evaluation;
    let evaluation_id = Uuid::now_v7();
    for novelty in &request.track_novelty {
        sqlx::query(
            "INSERT INTO track_novelty
                (id, evaluation_id, release_id, track_id, audio_identity_id,
                 media_asset_id, category, confidence, evidence_kind, detail,
                 evaluated_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)",
        )
        .bind(novelty.id.into_uuid())
        .bind(evaluation_id)
        .bind(novelty.release_id.into_uuid())
        .bind(novelty.track_id.into_uuid())
        .bind(novelty.audio_identity_id.into_uuid())
        .bind(novelty.media_asset_id.map(|id| id.into_uuid()))
        .bind(novelty.category.as_str())
        .bind(i32::from(novelty.confidence.basis_points()))
        .bind(novelty.evidence_kind.as_str())
        .bind(&novelty.detail)
        .bind(novelty.evaluated_at)
        .execute(&mut *tx)
        .await
        .expect("insert crash-visible Track novelty");
    }
    sqlx::query(
        "INSERT INTO release_novelty_evaluations
            (id, release_id, policy_rule_set_id, exact_duplicate_count,
             known_audio_identity_count, alternate_recording_count,
             new_track_count, uncertain_count, total_track_count,
             evaluated_at, evaluator_version, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $10)",
    )
    .bind(evaluation_id)
    .bind(evaluation.release_id.into_uuid())
    .bind(evaluation.policy_rule_set_id.into_uuid())
    .bind(i32::try_from(evaluation.exact_duplicate_count).expect("count fits"))
    .bind(i32::try_from(evaluation.known_audio_identity_count).expect("count fits"))
    .bind(i32::try_from(evaluation.alternate_recording_count).expect("count fits"))
    .bind(i32::try_from(evaluation.new_track_count).expect("count fits"))
    .bind(i32::try_from(evaluation.uncertain_count).expect("count fits"))
    .bind(i32::try_from(evaluation.total_track_count).expect("count fits"))
    .bind(evaluation.evaluated_at)
    .bind(&evaluation.evaluator_version)
    .execute(&mut *tx)
    .await
    .expect("insert crash-visible novelty aggregate");

    let decision = &request.decision;
    sqlx::query(
        "INSERT INTO policy_decisions
            (id, classification_id, novelty_evaluation_id, work_item_id,
             release_id, rule_set_id, rule_set_version,
             matched_rule_ids, disposition, evidence_summary, source_fingerprint,
             actor, decided_at, completion_state, completed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(decision.id.into_uuid())
    .bind(classification.id.into_uuid())
    .bind(evaluation_id)
    .bind(request.work_item_id)
    .bind(decision.release_id.into_uuid())
    .bind(decision.rule_set_id.into_uuid())
    .bind(i32::try_from(decision.rule_set_version).expect("rule version fits"))
    .bind(sqlx::types::Json(&decision.matched_rule_ids))
    .bind(decision.disposition.as_str())
    .bind(&decision.evidence_summary)
    .bind(&decision.source_fingerprint)
    .bind(serde_json::to_value(&decision.actor).expect("serialize actor"))
    .bind(decision.decided_at)
    .bind(decision.completion_state.as_str())
    .bind(decision.completed_at)
    .execute(&mut *tx)
    .await
    .expect("insert durable decision immediately before simulated crash");
    tx.commit()
        .await
        .expect("commit durable facts and decision before simulated crash");
}

#[tokio::test]
async fn stolen_policy_lease_fences_all_writes_then_direct_retry_converges() {
    let db = TestDb::create("phase5_stale_commit_fencing").await;
    let fixture = seed_cataloged_release(&db, 1, CatalogFixturePresentation::default()).await;
    let store = PolicyStore;
    let stale_worker = WorkerId::new();
    let winner = WorkerId::new();
    let claimed_at = OffsetDateTime::now_utc();
    let mut config = test_config();
    config.claim_batch_size = 1;
    config.lease_duration = Duration::seconds(2);
    let stale_claim = store
        .claim_cataloged(&db.pool, &stale_worker, &config, claimed_at)
        .await
        .expect("first worker claims policy item")
        .pop()
        .expect("one first-worker claim");
    let stale_context = store
        .load_context(&db.pool, &stale_claim)
        .await
        .expect("first worker loads immutable context");
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), claimed_at);
    store
        .ensure_rule_set(&db.pool, &rule_set, claimed_at)
        .await
        .expect("persist fencing rules");

    let reclaimed_at = claimed_at + config.lease_duration + Duration::seconds(1);
    let winner_claim = store
        .claim_cataloged(&db.pool, &winner, &config, reclaimed_at)
        .await
        .expect("winner directly reclaims expired lease")
        .pop()
        .expect("one reclaimed claim");
    let mut stale_request = unknown_review_commit(&stale_context, rule_set.id, claimed_at);
    stale_request.now = reclaimed_at;
    assert!(matches!(
        store
            .commit_evaluation(&db.pool, &stale_worker, &stale_request)
            .await,
        Err(PolicyStoreError::Work(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT
                (SELECT count(*) FROM release_classifications)
              + (SELECT count(*) FROM track_novelty)
              + (SELECT count(*) FROM release_novelty_evaluations)
              + (SELECT count(*) FROM policy_decisions)",
        )
        .fetch_one(&db.pool)
        .await
        .expect("stale attempt leaves no policy rows"),
        0
    );

    let winner_context = store
        .load_context(&db.pool, &winner_claim)
        .await
        .expect("winner loads current context");
    let committed = store
        .commit_evaluation(
            &db.pool,
            &winner,
            &unknown_review_commit(&winner_context, rule_set.id, reclaimed_at),
        )
        .await
        .expect("winner commits after direct reclaim");
    assert!(committed.inserted);
    assert_eq!(committed.item.state, IngestionState::NeedsReview);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_decisions")
            .fetch_one(&db.pool)
            .await
            .expect("exactly one authoritative decision"),
        1
    );
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn crash_after_durable_decision_before_state_is_adopted_exactly_once() {
    let db = TestDb::create("phase5_decision_crash_adoption").await;
    let fixture = seed_cataloged_release(&db, 2, CatalogFixturePresentation::default()).await;
    let store = PolicyStore;
    let crashed_worker = WorkerId::new();
    let restarted_worker = WorkerId::new();
    let claimed_at = OffsetDateTime::now_utc();
    let mut config = test_config();
    config.claim_batch_size = 1;
    config.lease_duration = Duration::seconds(10);
    let claim = store
        .claim_cataloged(&db.pool, &crashed_worker, &config, claimed_at)
        .await
        .expect("crashed worker claims policy item")
        .pop()
        .expect("one crash claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load crash context");
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), claimed_at);
    store
        .ensure_rule_set(&db.pool, &rule_set, claimed_at)
        .await
        .expect("persist crash rules");
    let request = unknown_review_commit(&context, rule_set.id, claimed_at);
    insert_crash_visible_policy_rows(&db, &request).await;

    let restart_at = claimed_at + Duration::seconds(1);
    sqlx::query(
        "UPDATE release_work_items
            SET lease_owner = NULL, lease_acquired_at = NULL,
                lease_expires_at = NULL, updated_at = $2
          WHERE id = $1 AND state = 'cataloged'",
    )
    .bind(fixture.work_item_id())
    .bind(restart_at)
    .execute(&db.pool)
    .await
    .expect("simulate lease recovery after process termination");
    let restarted_claim = store
        .claim_cataloged(&db.pool, &restarted_worker, &config, restart_at)
        .await
        .expect("restart directly reclaims partial policy item")
        .pop()
        .expect("one restarted claim");
    assert_eq!(restarted_claim.item.id, fixture.work_item_id());

    let mut retry = request.clone();
    retry.now = restart_at;
    let adopted = store
        .commit_evaluation(&db.pool, &restarted_worker, &retry)
        .await
        .expect("adopt identical durable facts and decision");
    assert!(!adopted.inserted);
    assert_eq!(adopted.set_based_mutation_statement_count, 2);
    assert_eq!(adopted.item.state, IngestionState::NeedsReview);
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM release_classifications),
            (SELECT count(*) FROM track_novelty),
            (SELECT count(*) FROM release_novelty_evaluations),
            (SELECT count(*) FROM policy_decisions),
            (SELECT count(*) FROM policy_decision_audit)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("restart leaves one authoritative row per policy fact");
    assert_eq!(counts, (1, 1, 1, 1, 1));
    fixture.assert_media_paths_absent();

    db.drop().await;
}

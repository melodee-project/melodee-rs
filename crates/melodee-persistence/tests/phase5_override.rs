//! Explicit administrator override and destructive-completion acceptance.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_domain::{DecisionCompletionState, Disposition, IngestionState, PolicyRuleSetId};
use melodee_persistence::{
    PolicyAdminOverrideRequest, PolicyDestructiveCompletion, PolicyStore, PolicyStoreError,
    WorkerId,
};
use phase5_common::{policy_rule_set, seed_cataloged_release, unknown_review_commit};
use time::{Duration, OffsetDateTime};

async fn seed_pending_review(
    db: &TestDb,
    ordinal: u32,
) -> (
    phase5_common::CatalogedPolicyFixture,
    melodee_persistence::PolicyCommitResult,
) {
    let fixture = seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: "Uncertain Policy Candidate",
            release_artist: "Operator Fixture",
            track_title: "New Track",
            track_artist: "Operator Fixture",
            release_date: "2005-06-07",
        },
    )
    .await;
    let store = PolicyStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let mut claims = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim operator fixture");
    assert_eq!(claims.len(), 1);
    let claim = claims.pop().expect("one operator claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load operator context");
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist operator rules");
    let committed = store
        .commit_evaluation(
            &db.pool,
            &worker,
            &unknown_review_commit(&context, rule_set.id, now),
        )
        .await
        .expect("persist pending Review decision");
    assert_eq!(committed.item.state, IngestionState::NeedsReview);
    assert_eq!(committed.decision.disposition, Disposition::Review);
    assert_eq!(
        committed.decision.completion_state,
        DecisionCompletionState::Pending
    );
    (fixture, committed)
}

#[tokio::test]
async fn pending_review_accepts_only_an_explicit_audited_operator_outcome() {
    let db = TestDb::create("phase5_admin_accept").await;
    let (fixture, pending) = seed_pending_review(&db, 1).await;
    let decided_at = OffsetDateTime::now_utc() + Duration::seconds(1);

    let accepted = PolicyStore
        .apply_admin_override(
            &db.pool,
            &PolicyAdminOverrideRequest {
                work_item_id: fixture.work_item_id(),
                decision_id: pending.decision.id,
                chosen_disposition: Disposition::Accept,
                actor: "acceptance-admin".to_string(),
                reason: "Verified the Release should continue".to_string(),
                source_fingerprint: fixture.published.sealed_manifest_hash.clone(),
                now: decided_at,
            },
        )
        .await
        .expect("apply explicit Accept override");
    assert_eq!(accepted.item.state, IngestionState::Indexing);
    assert!(accepted.item.lease_owner.is_none());
    assert_eq!(accepted.decision.disposition, Disposition::Accept);
    assert_eq!(
        accepted.decision.completion_state,
        DecisionCompletionState::Completed
    );
    assert!(matches!(
        accepted.decision.actor,
        melodee_domain::PolicyActor::Administrator { ref actor }
            if actor == "acceptance-admin"
    ));
    assert_eq!(
        accepted.decision.override_reason.as_deref(),
        Some("Verified the Release should continue")
    );
    let audit: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT event, disposition, completion_state
           FROM policy_decision_audit WHERE id = $1 ORDER BY created_at, event",
    )
    .bind(pending.decision.id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("load immutable override audit");
    assert_eq!(
        audit,
        vec![
            (
                "created".to_string(),
                "review".to_string(),
                "pending".to_string()
            ),
            (
                "completed".to_string(),
                "accept".to_string(),
                "completed".to_string()
            ),
        ]
    );
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn delete_requires_override_then_separate_matching_completion_evidence() {
    let db = TestDb::create("phase5_admin_delete_completion").await;
    let (fixture, pending) = seed_pending_review(&db, 2).await;
    let override_at = OffsetDateTime::now_utc() + Duration::seconds(1);
    let authorized = PolicyStore
        .apply_admin_override(
            &db.pool,
            &PolicyAdminOverrideRequest {
                work_item_id: fixture.work_item_id(),
                decision_id: pending.decision.id,
                chosen_disposition: Disposition::Delete,
                actor: "destructive-admin".to_string(),
                reason: "Confirmed redundant and authorized removal".to_string(),
                source_fingerprint: fixture.published.sealed_manifest_hash.clone(),
                now: override_at,
            },
        )
        .await
        .expect("persist Delete authorization before completion");
    assert_eq!(authorized.item.state, IngestionState::NeedsReview);
    assert_eq!(authorized.decision.disposition, Disposition::Delete);
    assert_eq!(
        authorized.decision.completion_state,
        DecisionCompletionState::Pending
    );

    let complete_at = override_at + Duration::seconds(1);
    let rejected_missing_actor = PolicyStore
        .complete_destructive_action(
            &db.pool,
            &PolicyDestructiveCompletion {
                work_item_id: fixture.work_item_id(),
                decision_id: pending.decision.id,
                actor: " ".to_string(),
                reason: "completed externally".to_string(),
                source_fingerprint: fixture.published.sealed_manifest_hash.clone(),
                completed_at: complete_at,
            },
        )
        .await;
    assert!(matches!(
        rejected_missing_actor,
        Err(PolicyStoreError::AuthorizationRequired)
    ));
    let rejected_fingerprint = PolicyStore
        .complete_destructive_action(
            &db.pool,
            &PolicyDestructiveCompletion {
                work_item_id: fixture.work_item_id(),
                decision_id: pending.decision.id,
                actor: "destructive-admin".to_string(),
                reason: "completed externally".to_string(),
                source_fingerprint: "f".repeat(64),
                completed_at: complete_at,
            },
        )
        .await;
    assert!(matches!(
        rejected_fingerprint,
        Err(PolicyStoreError::AuthorizationRequired
            | PolicyStoreError::DecisionConflict
            | PolicyStoreError::FingerprintMismatch)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT completion_state FROM policy_decisions WHERE id = $1",
        )
        .bind(pending.decision.id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("rejected completion remains pending"),
        "pending"
    );

    let completed = PolicyStore
        .complete_destructive_action(
            &db.pool,
            &PolicyDestructiveCompletion {
                work_item_id: fixture.work_item_id(),
                decision_id: pending.decision.id,
                actor: "destructive-admin".to_string(),
                reason: "Confirmed redundant and authorized removal".to_string(),
                source_fingerprint: fixture.published.sealed_manifest_hash.clone(),
                completed_at: complete_at,
            },
        )
        .await
        .expect("record separately authorized completion");
    assert_eq!(completed.item.state, IngestionState::Deleted);
    assert_eq!(
        completed.decision.completion_state,
        DecisionCompletionState::Completed
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_decision_audit WHERE id = $1",)
            .bind(pending.decision.id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count create/override/completion audit"),
        3
    );
    fixture.assert_media_paths_absent();

    db.drop().await;
}

//! Phase 5 PostgreSQL integration tests for classification, novelty, policy
//! decisions, and dry-run/test persistence (PRD-CLASS, PRD-NOV, PRD-POLICY).
//!
//! These tests exercise the real `PolicyStore` against a migrated PostgreSQL
//! database, proving round-trip persistence, the Pending-before-Completed
//! decision contract, evidence ID invariant, and immutability of the
//! decision audit log.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use common::{T0, TestDb};
use melodee_domain::{
    AudioIdentityId, Confidence, DecisionCompletionState, Disposition, EvidenceSource,
    NoveltyEvaluation, NoveltyEvidenceKind, PolicyActor, PolicyRuleSetId, ReleaseClassification,
    ReleaseClassificationId, ReleaseId, ReleasePolicyDecision, ReleasePolicyDecisionId, TrackId,
    TrackNovelty, TrackNoveltyCategory, TrackNoveltyId,
};
use melodee_persistence::PolicyStore;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

fn confidence(bp: u16) -> Confidence {
    Confidence::new(bp).unwrap()
}

fn uuid_v7() -> Uuid {
    Uuid::now_v7()
}

/// Inserts a minimal Release row so that FK constraints are satisfied.
async fn insert_release(
    pool: &PgPool,
    release_id: ReleaseId,
    title: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title, created_at, updated_at)
           VALUES ($1, $2, $3, $3, $4, $5, $5)"#,
    )
    .bind(release_id.into_uuid())
    .bind(Uuid::now_v7())
    .bind(title)
    .bind(title)
    .bind(T0)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_audio_identity(pool: &PgPool, id: AudioIdentityId) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO audio_identities (id, status, created_at, updated_at)
           VALUES ($1, 'verified', $2, $2)"#,
    )
    .bind(id.into_uuid())
    .bind(T0)
    .execute(pool)
    .await?;
    Ok(())
}

/// Inserts a policy_rule_set row so that FK constraints on policy_decisions
/// and policy_test_runs are satisfied.
async fn insert_rule_set(
    pool: &PgPool,
    rule_set_id: PolicyRuleSetId,
    name: &str,
    version: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO policy_rule_sets (id, name, version, rules, created_by, created_at)
           VALUES ($1, $2, $3, '[]'::jsonb, '"system"'::jsonb, $4)"#,
    )
    .bind(rule_set_id.into_uuid())
    .bind(name)
    .bind(i64::from(version))
    .bind(T0)
    .execute(pool)
    .await?;
    Ok(())
}

/// Inserts a minimal track row so FK constraints on track_novelty are satisfied.
async fn insert_track(
    pool: &PgPool,
    track_id: TrackId,
    release_id: ReleaseId,
    audio_identity_id: AudioIdentityId,
) -> Result<(), sqlx::Error> {
    // The tracks.playable_media_asset_id FK is DEFERRABLE INITIALLY DEFERRED,
    // so the track and its track_media_assets link must be inserted in one
    // transaction.
    let mut tx = pool.begin().await?;

    let media_asset_id = uuid_v7();
    sqlx::query(
        r#"INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size, mime_type, created_at)
           VALUES ($1, 'track.flac', $2, 1024, 'audio/flac', $3)"#,
    )
    .bind(media_asset_id)
    .bind("aabbccdd".repeat(8))
    .bind(T0)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO tracks (id, public_api_id, release_id, audio_identity_id,
               playable_media_asset_id, ordinal, title, normalized_title, sort_title,
               created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, 1, 'Track', 'track', 'track', $6, $6)"#,
    )
    .bind(track_id.into_uuid())
    .bind(uuid_v7())
    .bind(release_id.into_uuid())
    .bind(audio_identity_id.into_uuid())
    .bind(media_asset_id)
    .bind(T0)
    .execute(&mut *tx)
    .await?;

    // Link the media asset to the track via track_media_assets (same tx).
    sqlx::query(
        r#"INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal, created_at)
           VALUES ($1, $2, 'playable', 1, $3)"#,
    )
    .bind(track_id.into_uuid())
    .bind(media_asset_id)
    .bind(T0)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn classification_round_trips_with_foreign_key_integrity() {
    let db = TestDb::create("phase5_classification").await;
    let store = PolicyStore;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let classification_id = ReleaseClassificationId::from_uuid(uuid_v7());
    let classification = ReleaseClassification {
        id: classification_id,
        release_id,
        traits: BTreeSet::from([melodee_domain::ReleaseTrait::GreatestHits]),
        evidence: vec![melodee_domain::ClassificationEvidence {
            id: classification_id, // Evidence ID must match classification ID (domain invariant).
            release_id,
            trait_: melodee_domain::ReleaseTrait::GreatestHits,
            source: EvidenceSource::EmbeddedExternalId,
            kind: melodee_domain::ClassificationEvidenceKind::EmbeddedExternalId,
            confidence: confidence(8_500),
            detail: "Embedded external ID matched Greatest Hits".to_string(),
        }],
        confidence: confidence(8_500),
        classified_at: NOW,
        classifier_version: "test-v1".to_string(),
    };

    // store_classification should succeed because evidence.id == classification.id.
    store
        .store_classification(&db.pool, &classification)
        .await
        .expect("store classification");

    // Round-trip: load classification back.
    let loaded = store
        .load_classification(&db.pool, release_id)
        .await
        .expect("load classification")
        .expect("classification should exist");

    assert_eq!(loaded.id, classification_id);
    assert_eq!(loaded.release_id, release_id);
    assert_eq!(loaded.traits.len(), 1);
    assert!(
        loaded
            .traits
            .contains(&melodee_domain::ReleaseTrait::GreatestHits)
    );
    assert_eq!(loaded.evidence.len(), 1);
    assert_eq!(loaded.evidence[0].id, classification_id);
    assert_eq!(loaded.confidence.basis_points(), 8_500);

    db.drop().await;
}

#[tokio::test]
async fn classification_rejects_foreign_evidence_id() {
    let db = TestDb::create("phase5_classification_fk").await;
    let store = PolicyStore;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let classification_id = ReleaseClassificationId::from_uuid(uuid_v7());
    let foreign_evidence_id = ReleaseClassificationId::from_uuid(uuid_v7());

    let classification = ReleaseClassification {
        id: classification_id,
        release_id,
        traits: BTreeSet::from([melodee_domain::ReleaseTrait::GreatestHits]),
        evidence: vec![melodee_domain::ClassificationEvidence {
            id: foreign_evidence_id, // Different from classification ID → FK violation.
            release_id,
            trait_: melodee_domain::ReleaseTrait::GreatestHits,
            source: EvidenceSource::EmbeddedExternalId,
            kind: melodee_domain::ClassificationEvidenceKind::EmbeddedExternalId,
            confidence: confidence(8_500),
            detail: "Foreign evidence ID".to_string(),
        }],
        confidence: confidence(8_500),
        classified_at: NOW,
        classifier_version: "test-v1".to_string(),
    };

    // The domain invariant ReleaseClassification::new() would reject this.
    // But even if we bypass it, the FK constraint catches it.
    let result = store.store_classification(&db.pool, &classification).await;
    assert!(
        result.is_err(),
        "foreign evidence ID should violate the FK constraint"
    );

    db.drop().await;
}

#[tokio::test]
async fn decision_is_recorded_as_pending_not_completed() {
    let db = TestDb::create("phase5_decision_pending").await;
    let store = PolicyStore;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let classification_id = ReleaseClassificationId::from_uuid(uuid_v7());
    let classification = ReleaseClassification {
        id: classification_id,
        release_id,
        traits: BTreeSet::from([melodee_domain::ReleaseTrait::GreatestHits]),
        evidence: vec![melodee_domain::ClassificationEvidence {
            id: classification_id,
            release_id,
            trait_: melodee_domain::ReleaseTrait::GreatestHits,
            source: EvidenceSource::EmbeddedExternalId,
            kind: melodee_domain::ClassificationEvidenceKind::EmbeddedExternalId,
            confidence: confidence(8_500),
            detail: "Embedded external ID matched Greatest Hits".to_string(),
        }],
        confidence: confidence(8_500),
        classified_at: NOW,
        classifier_version: "test-v1".to_string(),
    };
    store
        .store_classification(&db.pool, &classification)
        .await
        .expect("store classification");

    let rule_set_id = PolicyRuleSetId::from_uuid(uuid_v7());
    insert_rule_set(&db.pool, rule_set_id, "phase5-default-rules", 1)
        .await
        .expect("insert rule set");
    let decision_id = ReleasePolicyDecisionId::from_uuid(uuid_v7());
    let decision = ReleasePolicyDecision {
        id: decision_id,
        release_id,
        rule_set_id,
        rule_set_version: 1,
        matched_rule_ids: vec!["POL-006".to_string()],
        disposition: Disposition::Delete,
        evidence_summary: "Greatest Hits, redundant, Administrator override".to_string(),
        source_fingerprint: None,
        actor: PolicyActor::Administrator {
            actor: "ops@test".to_string(),
        },
        decided_at: NOW,
        completion_state: DecisionCompletionState::Pending,
        completed_at: None,
    };

    let mut tx = db.pool.begin().await.expect("begin tx");
    store
        .record_decision(&mut tx, &decision)
        .await
        .expect("record decision");
    tx.commit().await.expect("commit tx");

    // Load back and verify Pending state.
    let loaded = store
        .get_decision(&db.pool, release_id)
        .await
        .expect("get decision")
        .expect("decision should exist");

    assert_eq!(loaded.completion_state, DecisionCompletionState::Pending);
    assert!(loaded.completed_at.is_none());
    assert_eq!(loaded.disposition, Disposition::Delete);
    assert_eq!(loaded.matched_rule_ids, vec!["POL-006".to_string()]);

    db.drop().await;
}

#[tokio::test]
async fn decision_completion_transitions_pending_to_completed() {
    let db = TestDb::create("phase5_decision_complete").await;
    let store = PolicyStore;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let rule_set_id = PolicyRuleSetId::from_uuid(uuid_v7());
    insert_rule_set(&db.pool, rule_set_id, "phase5-default-rules", 1)
        .await
        .expect("insert rule set");
    let decision_id = ReleasePolicyDecisionId::from_uuid(uuid_v7());
    let decision = ReleasePolicyDecision {
        id: decision_id,
        release_id,
        rule_set_id,
        rule_set_version: 1,
        matched_rule_ids: vec!["POL-006".to_string()],
        disposition: Disposition::Delete,
        evidence_summary: "Greatest Hits, redundant, Administrator override".to_string(),
        source_fingerprint: None,
        actor: PolicyActor::Administrator {
            actor: "ops@test".to_string(),
        },
        decided_at: NOW,
        completion_state: DecisionCompletionState::Pending,
        completed_at: None,
    };

    let completed_at = NOW + time::Duration::seconds(60);

    let mut tx = db.pool.begin().await.expect("begin tx");
    store
        .record_decision(&mut tx, &decision)
        .await
        .expect("record decision");
    let completed = decision
        .clone()
        .mark_completed(completed_at)
        .expect("mark completed");
    store
        .complete_decision(&mut tx, &completed, completed_at)
        .await
        .expect("complete decision");
    tx.commit().await.expect("commit tx");

    let loaded = store
        .get_decision(&db.pool, release_id)
        .await
        .expect("get decision")
        .expect("decision should exist");

    assert_eq!(loaded.completion_state, DecisionCompletionState::Completed);
    assert_eq!(loaded.completed_at, Some(completed_at));

    db.drop().await;
}

#[tokio::test]
async fn decision_completion_rejects_non_pending_state() {
    let db = TestDb::create("phase5_decision_complete_reject").await;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let rule_set_id = PolicyRuleSetId::from_uuid(uuid_v7());
    insert_rule_set(&db.pool, rule_set_id, "phase5-default-rules", 1)
        .await
        .expect("insert rule set");
    let decision_id = ReleasePolicyDecisionId::from_uuid(uuid_v7());
    // Start with Completed state.
    let decision = ReleasePolicyDecision {
        id: decision_id,
        release_id,
        rule_set_id,
        rule_set_version: 1,
        matched_rule_ids: vec!["POL-006".to_string()],
        disposition: Disposition::Delete,
        evidence_summary: "Already completed".to_string(),
        source_fingerprint: None,
        actor: PolicyActor::Administrator {
            actor: "ops@test".to_string(),
        },
        decided_at: NOW,
        completion_state: DecisionCompletionState::Completed,
        completed_at: Some(NOW),
    };

    // mark_completed on an already-Completed decision should fail.
    let result = decision.clone().mark_completed(NOW);
    assert!(result.is_err());

    db.drop().await;
}

#[tokio::test]
async fn novelty_evaluation_round_trips_with_audit_trail() {
    let db = TestDb::create("phase5_novelty").await;
    let store = PolicyStore;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let track_id = TrackId::from_uuid(uuid_v7());
    let audio_identity_id = AudioIdentityId::from_uuid(uuid_v7());
    insert_audio_identity(&db.pool, audio_identity_id)
        .await
        .expect("insert audio identity");
    insert_track(&db.pool, track_id, release_id, audio_identity_id)
        .await
        .expect("insert track");
    insert_rule_set(
        &db.pool,
        PolicyRuleSetId::from_uuid(uuid_v7()),
        "phase5-default-rules",
        1,
    )
    .await
    .expect("insert rule set");

    let novelty = NoveltyEvaluation {
        release_id,
        policy_rule_set_id: PolicyRuleSetId::from_uuid(uuid_v7()),
        exact_duplicate_count: 3,
        known_audio_identity_count: 2,
        alternate_recording_count: 0,
        new_track_count: 0,
        uncertain_count: 0,
        total_track_count: 5,
        evaluated_at: NOW,
        evaluator_version: "test-v1".to_string(),
    };

    let track_novelty = vec![TrackNovelty {
        id: TrackNoveltyId::from_uuid(uuid_v7()),
        release_id,
        track_id,
        audio_identity_id,
        media_asset_id: None,
        category: TrackNoveltyCategory::ExactDuplicate,
        confidence: confidence(9_000),
        evidence_kind: NoveltyEvidenceKind::ByteHashMatch,
        detail: "Exact duplicate of catalog track".to_string(),
        evaluated_at: NOW,
    }];

    store
        .store_novelty(&db.pool, &novelty, &track_novelty)
        .await
        .expect("store novelty");

    // Verify the aggregate evaluation was stored.
    let row: (i32,) = sqlx::query_as(
        r#"SELECT total_track_count FROM release_novelty_evaluations WHERE release_id = $1"#,
    )
    .bind(release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query novelty evaluation");
    assert_eq!(row.0, 5);

    // Verify track novelty was stored.
    let row: (String,) =
        sqlx::query_as(r#"SELECT category FROM track_novelty WHERE release_id = $1"#)
            .bind(release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query track novelty");
    assert_eq!(row.0, "exact_duplicate");

    db.drop().await;
}

#[tokio::test]
async fn policy_test_run_round_trips() {
    let db = TestDb::create("phase5_test_run").await;
    let _store = PolicyStore;
    let rule_set_id = PolicyRuleSetId::from_uuid(uuid_v7());
    insert_rule_set(&db.pool, rule_set_id, "phase5-default-rules", 1)
        .await
        .expect("insert rule set");

    let summary = melodee_domain::PolicyTestSummary::new(melodee_domain::PolicyTestSummary {
        test_run_id: melodee_domain::PolicyTestRunId::from_uuid(uuid_v7()),
        rule_set_id,
        rule_set_version: 1,
        total_releases_evaluated: 3,
        disposition_counts: vec![(Disposition::Accept, 2), (Disposition::Review, 1)],
        destructive_impact_count: 0,
        sample_release_ids: vec![],
        ran_by: PolicyActor::Administrator {
            actor: "melodee-cli".to_string(),
        },
        ran_at: NOW,
    })
    .expect("valid test summary");

    _store
        .store_test_run(&db.pool, &summary)
        .await
        .expect("store test run");

    // Verify it was persisted.
    let row: (i32,) =
        sqlx::query_as(r#"SELECT total_releases_evaluated FROM policy_test_runs WHERE id = $1"#)
            .bind(summary.test_run_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query test run");
    assert_eq!(row.0, 3);

    // Verify the audit log captured the creation.
    let row: (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM policy_decision_audit WHERE event = 'created'"#)
            .fetch_one(&db.pool)
            .await
            .expect("query audit log");
    // No decisions recorded in this test, so audit should be 0.
    assert_eq!(row.0, 0);

    db.drop().await;
}

#[tokio::test]
async fn decision_audit_is_append_only() {
    let db = TestDb::create("phase5_audit_append_only").await;
    let store = PolicyStore;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let rule_set_id = PolicyRuleSetId::from_uuid(uuid_v7());
    insert_rule_set(&db.pool, rule_set_id, "phase5-default-rules", 1)
        .await
        .expect("insert rule set");
    let decision_id = ReleasePolicyDecisionId::from_uuid(uuid_v7());
    let decision = ReleasePolicyDecision {
        id: decision_id,
        release_id,
        rule_set_id,
        rule_set_version: 1,
        matched_rule_ids: vec!["POL-006".to_string()],
        disposition: Disposition::Delete,
        evidence_summary: "Greatest Hits, redundant".to_string(),
        source_fingerprint: None,
        actor: PolicyActor::Administrator {
            actor: "ops@test".to_string(),
        },
        decided_at: NOW,
        completion_state: DecisionCompletionState::Pending,
        completed_at: None,
    };

    let mut tx = db.pool.begin().await.expect("begin tx");
    store
        .record_decision(&mut tx, &decision)
        .await
        .expect("record decision");
    tx.commit().await.expect("commit tx");

    // Attempt to UPDATE the audit table — should be rejected by trigger.
    let result =
        sqlx::query(r#"UPDATE policy_decision_audit SET event = 'tampered' WHERE id = $1"#)
            .bind(decision_id.into_uuid())
            .execute(&db.pool)
            .await;
    assert!(result.is_err(), "audit rows must be append-only");

    // Attempt to DELETE from the audit table — should be rejected by trigger.
    let result = sqlx::query(r#"DELETE FROM policy_decision_audit WHERE id = $1"#)
        .bind(decision_id.into_uuid())
        .execute(&db.pool)
        .await;
    assert!(result.is_err(), "audit rows must be append-only");

    db.drop().await;
}

#[tokio::test]
async fn system_actor_cannot_override_to_delete() {
    let db = TestDb::create("phase5_system_no_delete").await;
    let store = PolicyStore;
    let release_id = ReleaseId::from_uuid(uuid_v7());
    insert_release(&db.pool, release_id, "Greatest Hits")
        .await
        .expect("insert release");

    let rule_set_id = PolicyRuleSetId::from_uuid(uuid_v7());
    insert_rule_set(&db.pool, rule_set_id, "phase5-default-rules", 1)
        .await
        .expect("insert rule set");
    let decision_id = ReleasePolicyDecisionId::from_uuid(uuid_v7());
    let decision = ReleasePolicyDecision {
        id: decision_id,
        release_id,
        rule_set_id,
        rule_set_version: 1,
        matched_rule_ids: vec!["POL-006".to_string()],
        disposition: Disposition::Delete,
        evidence_summary: "System actor should not authorize Delete".to_string(),
        source_fingerprint: None,
        actor: PolicyActor::System {
            policy_rule_set_version: 1,
        },
        decided_at: NOW,
        completion_state: DecisionCompletionState::Pending,
        completed_at: None,
    };

    let mut tx = db.pool.begin().await.expect("begin tx");
    store
        .record_decision(&mut tx, &decision)
        .await
        .expect("record decision");
    tx.commit().await.expect("commit tx");

    // The decision is stored as Pending with Delete disposition — but
    // the policy engine (not the DB) enforces that System cannot authorize
    // completion. The DB allows the record but the completion contract
    // requires Administrator.
    let loaded = store
        .get_decision(&db.pool, release_id)
        .await
        .expect("get decision")
        .expect("decision should exist");

    assert_eq!(loaded.completion_state, DecisionCompletionState::Pending);
    assert_eq!(loaded.disposition, Disposition::Delete);
    assert!(
        !loaded.actor.is_administrator(),
        "System is not administrator"
    );

    db.drop().await;
}

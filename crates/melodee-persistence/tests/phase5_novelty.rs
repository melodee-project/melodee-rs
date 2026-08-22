//! Authoritative catalog novelty facts consumed and revalidated by.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_domain::{
    AudioIdentityEvidence, AudioIdentityEvidenceId, AudioIdentityEvidenceKind, AudioIdentityMerge,
    AudioIdentityMergeId, Confidence, EvidencePolarity, IngestionState, NoveltyEvidenceKind,
    PolicyRuleSetId, TrackNoveltyCategory,
};
use melodee_persistence::{
    CatalogStore, PolicyAudioIdentityAlternateAssertion, PolicyAudioIdentityNoMatchAssertion,
    PolicyCatalogContext, PolicyEvaluationCommit, PolicyStore, PolicyStoreError, WorkerId,
};
use phase5_common::{
    policy_rule_set, seed_cataloged_release, seed_cataloged_release_with_normalized_hash,
    unknown_review_commit,
};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

fn request_for_category(
    context: &PolicyCatalogContext,
    rule_set_id: PolicyRuleSetId,
    category: TrackNoveltyCategory,
    now: OffsetDateTime,
) -> PolicyEvaluationCommit {
    let mut request = unknown_review_commit(context, rule_set_id, now);
    request.novelty_evaluation.exact_duplicate_count = 0;
    request.novelty_evaluation.known_audio_identity_count = 0;
    request.novelty_evaluation.alternate_recording_count = 0;
    request.novelty_evaluation.new_track_count = 0;
    request.novelty_evaluation.uncertain_count = 0;
    let (evidence_kind, confidence, detail) = match category {
        TrackNoveltyCategory::ExactDuplicate => {
            request.novelty_evaluation.exact_duplicate_count = 1;
            (
                NoveltyEvidenceKind::ByteHashMatch,
                Confidence::new(10_000).expect("valid confidence"),
                "Canonical byte hash exists in another Release",
            )
        }
        TrackNoveltyCategory::KnownAudioIdentity => {
            request.novelty_evaluation.known_audio_identity_count = 1;
            (
                NoveltyEvidenceKind::AudioIdentityMatch,
                Confidence::new(9_000).expect("valid confidence"),
                "AudioIdentity is durably referenced by another Release",
            )
        }
        TrackNoveltyCategory::AlternateRecording => {
            request.novelty_evaluation.alternate_recording_count = 1;
            (
                NoveltyEvidenceKind::AudioIdentityDistinct,
                Confidence::new(8_500).expect("valid confidence"),
                "Catalog evidence confirms a distinct alternate recording",
            )
        }
        TrackNoveltyCategory::NewTrack => {
            request.novelty_evaluation.new_track_count = 1;
            (
                NoveltyEvidenceKind::NoCatalogMatch,
                Confidence::new(8_500).expect("valid confidence"),
                "Confirmed AudioIdentity has no catalog match",
            )
        }
        TrackNoveltyCategory::Uncertain => {
            request.novelty_evaluation.uncertain_count = 1;
            (
                NoveltyEvidenceKind::InsufficientEvidence,
                Confidence::zero(),
                "AudioIdentity lacks durable fingerprint or match confirmation",
            )
        }
    };
    request.track_novelty[0].category = category;
    request.track_novelty[0].evidence_kind = evidence_kind;
    request.track_novelty[0].confidence = confidence;
    request.track_novelty[0].detail = detail.to_string();
    request
}

#[tokio::test]
async fn audited_no_catalog_match_changes_uncertain_to_new_and_is_revalidated_at_commit() {
    let db = TestDb::create("phase5_explicit_new_track").await;
    let fixture = seed_cataloged_release(&db, 1, CatalogFixturePresentation::default()).await;
    let store = PolicyStore;
    let before = store
        .load_snapshot(&db.pool, fixture.release_id())
        .await
        .expect("load uncertain snapshot")
        .expect("cataloged snapshot exists");
    assert_eq!(before.tracks.len(), 1);
    assert!(!before.tracks[0].has_confirmed_audio_identity);

    let now = OffsetDateTime::now_utc();
    let audio_identity_id =
        melodee_domain::AudioIdentityId::from_uuid(before.tracks[0].audio_identity_id);
    let assertion = PolicyAudioIdentityNoMatchAssertion {
        id: Uuid::now_v7(),
        audio_identity_id,
        actor: "acceptance-resolver".to_string(),
        reason: "bounded catalog search found no matching AudioIdentity".to_string(),
        evidence: json!({ "catalog_revision": 1, "strategy": "phase5-acceptance" }),
        created_at: now,
    };
    store
        .record_audio_identity_no_match(&db.pool, &assertion)
        .await
        .expect("persist explicit no-match assertion");
    store
        .record_audio_identity_no_match(&db.pool, &assertion)
        .await
        .expect("adopt exact no-match retry");
    let mut conflicting = assertion.clone();
    conflicting.id = Uuid::now_v7();
    assert!(matches!(
        store
            .record_audio_identity_no_match(&db.pool, &conflicting)
            .await,
        Err(PolicyStoreError::DecisionConflict)
    ));

    let worker = WorkerId::new();
    let claim = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim explicit-new fixture")
        .pop()
        .expect("one explicit-new claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load explicit-new context");
    assert!(context.tracks[0].has_confirmed_audio_identity);
    assert!(context.catalog_exact_byte_hashes.is_empty());
    assert!(context.catalog_known_audio_identities.is_empty());
    assert!(context.catalog_alternate_audio_identities.is_empty());

    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist explicit-new rules");
    let request = request_for_category(&context, rule_set.id, TrackNoveltyCategory::NewTrack, now);
    let committed = store
        .commit_evaluation(&db.pool, &worker, &request)
        .await
        .expect("commit revalidated NewTrack fact");
    assert_eq!(committed.item.state, IngestionState::NeedsReview);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT category FROM track_novelty WHERE release_id = $1",
        )
        .bind(fixture.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("load persisted NewTrack category"),
        "new_track"
    );
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn exact_known_alternate_new_and_uncertain_catalog_truth_commit_without_media_reads() {
    let db = TestDb::create("phase5_novelty_catalog_matrix").await;
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
    let snapshot = |release_id| PolicyStore.load_snapshot(&db.pool, release_id);
    let baseline_snapshot = snapshot(baseline.release_id())
        .await
        .expect("load baseline")
        .expect("baseline exists");
    let known_snapshot = snapshot(known.release_id())
        .await
        .expect("load known fixture")
        .expect("known fixture exists");
    let alternate_snapshot = snapshot(alternate.release_id())
        .await
        .expect("load alternate fixture")
        .expect("alternate fixture exists");
    let new_snapshot = snapshot(new_track.release_id())
        .await
        .expect("load new fixture")
        .expect("new fixture exists");
    let baseline_audio =
        melodee_domain::AudioIdentityId::from_uuid(baseline_snapshot.tracks[0].audio_identity_id);
    let known_audio =
        melodee_domain::AudioIdentityId::from_uuid(known_snapshot.tracks[0].audio_identity_id);
    let alternate_audio =
        melodee_domain::AudioIdentityId::from_uuid(alternate_snapshot.tracks[0].audio_identity_id);
    let new_audio =
        melodee_domain::AudioIdentityId::from_uuid(new_snapshot.tracks[0].audio_identity_id);

    CatalogStore
        .merge_audio_identity(
            &db.pool,
            &AudioIdentityMerge {
                id: AudioIdentityMergeId::new(),
                source_id: known_audio,
                target_id: baseline_audio,
                reason: "acceptance fixture: same audible identity".to_string(),
                occurred_at: now,
            },
            "acceptance-admin",
            "phase5-novelty-matrix-v1",
        )
        .await
        .expect("create supported known-identity catalog truth");

    let alternate_evidence = AudioIdentityEvidence {
        id: AudioIdentityEvidenceId::new(),
        source_audio_identity_id: alternate_audio,
        candidate_audio_identity_id: baseline_audio,
        kind: AudioIdentityEvidenceKind::Manual,
        polarity: EvidencePolarity::Supports,
        confidence: Confidence::new(9_500).expect("valid confidence"),
        source: "phase5-alternate-admin".to_string(),
    };
    CatalogStore
        .record_audio_identity_evidence(
            &db.pool,
            &alternate_evidence,
            json!({ "finding": "distinct recording" }),
            now,
        )
        .await
        .expect("persist exact manual alternate evidence");
    let alternate_assertion = PolicyAudioIdentityAlternateAssertion {
        source_audio_identity_id: alternate_audio,
        alternate_audio_identity_id: baseline_audio,
        evidence_id: alternate_evidence.id.into_uuid(),
        actor: "acceptance-admin".to_string(),
        reason: "manual listening confirms distinct performance".to_string(),
        created_at: now,
    };
    PolicyStore
        .record_audio_identity_alternate(&db.pool, &alternate_assertion)
        .await
        .expect("persist explicit alternate relation");
    PolicyStore
        .record_audio_identity_alternate(&db.pool, &alternate_assertion)
        .await
        .expect("adopt exact alternate retry");
    PolicyStore
        .record_audio_identity_no_match(
            &db.pool,
            &PolicyAudioIdentityNoMatchAssertion {
                id: Uuid::now_v7(),
                audio_identity_id: new_audio,
                actor: "acceptance-resolver".to_string(),
                reason: "bounded catalog lookup found no match".to_string(),
                evidence: json!({ "catalog_revision": 1 }),
                created_at: now,
            },
        )
        .await
        .expect("persist NewTrack catalog assertion");

    let worker = WorkerId::new();
    let mut config = test_config();
    config.claim_batch_size = 10;
    let claims = PolicyStore
        .claim_cataloged(&db.pool, &worker, &config, now)
        .await
        .expect("claim complete novelty matrix");
    assert_eq!(claims.len(), 6);
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist matrix rule set");
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
    for (release_id, category) in expected {
        let claim = claims
            .iter()
            .find(|claim| claim.release_id == release_id)
            .expect("matrix claim exists");
        let context = PolicyStore
            .load_context(&db.pool, claim)
            .await
            .expect("load bounded novelty context");
        match category {
            TrackNoveltyCategory::ExactDuplicate => assert!(
                context
                    .catalog_exact_byte_hashes
                    .contains(&exact.published.normalized_byte_hash)
            ),
            TrackNoveltyCategory::KnownAudioIdentity => assert!(
                context
                    .catalog_known_audio_identities
                    .contains(&baseline_audio)
            ),
            TrackNoveltyCategory::AlternateRecording => {
                let release_audio =
                    melodee_domain::AudioIdentityId::from_uuid(context.tracks[0].audio_identity_id);
                assert!(
                    context
                        .catalog_alternate_audio_identities
                        .contains(&release_audio),
                    "alternate relation must be visible from either stored orientation"
                );
            }
            TrackNoveltyCategory::NewTrack => {
                assert!(context.tracks[0].has_confirmed_audio_identity)
            }
            TrackNoveltyCategory::Uncertain => {
                assert!(!context.tracks[0].has_confirmed_audio_identity)
            }
        }
        let committed = PolicyStore
            .commit_evaluation(
                &db.pool,
                &worker,
                &request_for_category(&context, rule_set.id, category, now),
            )
            .await
            .expect("commit revalidated novelty category");
        assert_eq!(committed.item.state, IngestionState::NeedsReview);
    }
    let baseline_claim = claims
        .iter()
        .find(|claim| claim.release_id == baseline.release_id())
        .expect("alternate target claim exists");
    let baseline_context = PolicyStore
        .load_context(&db.pool, baseline_claim)
        .await
        .expect("load reverse-orientation alternate target");
    assert!(
        baseline_context
            .catalog_alternate_audio_identities
            .contains(&baseline_audio),
        "alternate relation must be visible from its stored target orientation"
    );
    PolicyStore
        .release_lease(&db.pool, baseline.work_item_id(), &worker, now)
        .await
        .expect("release comparator-only baseline");
    let categories: Vec<String> =
        sqlx::query_scalar("SELECT category FROM track_novelty ORDER BY category")
            .fetch_all(&db.pool)
            .await
            .expect("load exact persisted novelty category set");
    assert_eq!(
        categories,
        vec![
            "alternate_recording".to_string(),
            "exact_duplicate".to_string(),
            "known_audio_identity".to_string(),
            "new_track".to_string(),
            "uncertain".to_string(),
        ]
    );
    for fixture in [
        &baseline, &exact, &known, &alternate, &new_track, &uncertain,
    ] {
        fixture.assert_media_paths_absent();
    }

    db.drop().await;
}

#[tokio::test]
async fn alternate_recording_relation_is_symmetric_for_context_and_commit() {
    let db = TestDb::create("phase5_alternate_symmetric").await;
    let left = seed_cataloged_release(
        &db,
        20,
        CatalogFixturePresentation {
            release_title: "Alternate Left",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let right = seed_cataloged_release(
        &db,
        21,
        CatalogFixturePresentation {
            release_title: "Alternate Right",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let left_snapshot = PolicyStore
        .load_snapshot(&db.pool, left.release_id())
        .await
        .expect("load left alternate snapshot")
        .expect("left snapshot exists");
    let right_snapshot = PolicyStore
        .load_snapshot(&db.pool, right.release_id())
        .await
        .expect("load right alternate snapshot")
        .expect("right snapshot exists");
    let left_audio =
        melodee_domain::AudioIdentityId::from_uuid(left_snapshot.tracks[0].audio_identity_id);
    let right_audio =
        melodee_domain::AudioIdentityId::from_uuid(right_snapshot.tracks[0].audio_identity_id);
    let now = OffsetDateTime::now_utc();
    let evidence = AudioIdentityEvidence {
        id: AudioIdentityEvidenceId::new(),
        source_audio_identity_id: left_audio,
        candidate_audio_identity_id: right_audio,
        kind: AudioIdentityEvidenceKind::Manual,
        polarity: EvidencePolarity::Supports,
        confidence: Confidence::new(9_500).expect("valid confidence"),
        source: "symmetric-alternate-admin".to_string(),
    };
    CatalogStore
        .record_audio_identity_evidence(
            &db.pool,
            &evidence,
            json!({ "finding": "distinct symmetric recordings" }),
            now,
        )
        .await
        .expect("persist supporting manual evidence");
    PolicyStore
        .record_audio_identity_alternate(
            &db.pool,
            &PolicyAudioIdentityAlternateAssertion {
                source_audio_identity_id: left_audio,
                alternate_audio_identity_id: right_audio,
                evidence_id: evidence.id.into_uuid(),
                actor: "symmetric-alternate-admin".to_string(),
                reason: "manual comparison confirms distinct performances".to_string(),
                created_at: now,
            },
        )
        .await
        .expect("persist one oriented alternate relation");

    let worker = WorkerId::new();
    let mut config = test_config();
    config.claim_batch_size = 2;
    let claims = PolicyStore
        .claim_cataloged(&db.pool, &worker, &config, now)
        .await
        .expect("claim both alternate orientations");
    assert_eq!(claims.len(), 2);
    let rule_set = policy_rule_set(PolicyRuleSetId::new(), now);
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist symmetric alternate rules");
    for (release_id, own_audio) in [
        (left.release_id(), left_audio),
        (right.release_id(), right_audio),
    ] {
        let claim = claims
            .iter()
            .find(|claim| claim.release_id == release_id)
            .expect("orientation claim exists");
        let context = PolicyStore
            .load_context(&db.pool, claim)
            .await
            .expect("load oriented alternate context");
        assert!(
            context
                .catalog_alternate_audio_identities
                .contains(&own_audio),
            "both relation endpoints must be confirmed as alternate"
        );
        let committed = PolicyStore
            .commit_evaluation(
                &db.pool,
                &worker,
                &request_for_category(
                    &context,
                    rule_set.id,
                    TrackNoveltyCategory::AlternateRecording,
                    now,
                ),
            )
            .await
            .expect("commit symmetric alternate category");
        assert_eq!(committed.item.state, IngestionState::NeedsReview);
    }
    let categories: Vec<String> =
        sqlx::query_scalar("SELECT category FROM track_novelty ORDER BY release_id")
            .fetch_all(&db.pool)
            .await
            .expect("load symmetric persisted categories");
    assert_eq!(categories, ["alternate_recording", "alternate_recording"]);
    left.assert_media_paths_absent();
    right.assert_media_paths_absent();

    db.drop().await;
}

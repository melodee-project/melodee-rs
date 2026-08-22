//! read-side store coverage: multi-state review queue paging,
//! persisted novelty reads, and the Artist identity review reads
//! (unresolved queue, observation detail, decision feed, public-id lookup).

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, seed_ready_item, test_config};
use melodee_domain::{
    ArtistMatchOutcome, Confidence, DecisionAuthority, FailureClass, IngestionState,
    NewArtistMatchDecision, TrackNoveltyCategory, TransitionKind, WorkErrorCode,
};
use melodee_persistence::{CatalogStore, ObservedArtistSort, PolicyStore, WorkItemStore, WorkerId};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn list_page_states_filters_and_paginates() {
    let db = TestDb::create("review_list_page_states").await;
    let store = WorkItemStore;
    let config = test_config();

    let _ready_id = seed_ready_item(&store, &db.pool, "review/ready").await;
    let target = seed_ready_item(&store, &db.pool, "review/failed").await;
    let worker = WorkerId::new();
    let claimed = store
        .claim(&db.pool, &worker, &config, OffsetDateTime::now_utc())
        .await
        .expect("claim fixture");
    assert!(
        claimed.iter().any(|item| item.id == target),
        "claim must lease the failure fixture"
    );

    // Drive one item to failed through the legal worker path.
    store
        .transition(
            &db.pool,
            target,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("start inspection");
    store
        .record_failure(
            &db.pool,
            target,
            &worker,
            WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
            "fixture failure",
            FailureClass::Permanent,
            &config.retry_policy,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("record permanent failure");

    let review_states = [IngestionState::Failed, IngestionState::Quarantined];
    let (page, total) = store
        .list_page_states(
            &db.pool,
            &review_states,
            50,
            0,
            melodee_persistence::WorkItemSort::default_order(),
        )
        .await
        .expect("review page");
    assert_eq!(total, 1);
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, target);
    assert_eq!(page[0].state, IngestionState::Failed);

    let (empty, zero) = store
        .list_page_states(
            &db.pool,
            &[],
            50,
            0,
            melodee_persistence::WorkItemSort::default_order(),
        )
        .await
        .expect("empty state set short-circuits");
    assert!(empty.is_empty());
    assert_eq!(zero, 0);

    db.drop().await;
}

#[tokio::test]
async fn load_novelty_replays_persisted_counts_and_tracks() {
    let db = TestDb::create("review_load_novelty").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Novelty Read Fixture",
            release_artist: "Novelty Artist",
            track_title: "Novelty Track",
            track_artist: "Novelty Artist",
            release_date: "2002-03-04",
        },
    )
    .await;
    let store = PolicyStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let mut claims = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim cataloged fixture");
    let claim = claims.pop().expect("one claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load context");
    let rule_set = phase5_common::policy_rule_set(melodee_domain::PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist rule set");
    store
        .commit_evaluation(
            &db.pool,
            &worker,
            &phase5_common::unknown_review_commit(&context, rule_set.id, now),
        )
        .await
        .expect("commit evaluation");

    let novelty = store
        .load_novelty(&db.pool, fixture.release_id())
        .await
        .expect("load novelty")
        .expect("persisted evaluation");
    assert_eq!(novelty.uncertain_count, 1);
    assert_eq!(novelty.total_track_count, 1);
    assert_eq!(novelty.tracks.len(), 1);
    assert_eq!(novelty.tracks[0].category, TrackNoveltyCategory::Uncertain);
    assert_eq!(novelty.evaluator_version, "melodee.policy.novelty-v1");

    // Unknown releases have no evaluation.
    let unknown = PolicyStore
        .load_novelty(&db.pool, melodee_domain::ReleaseId::new())
        .await
        .expect("load missing novelty");
    assert!(unknown.is_none());

    db.drop().await;
}

#[tokio::test]
async fn identity_review_reads_surface_observations_evidence_and_feed() {
    let db = TestDb::create("review_identity_reads").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Identity Read Fixture",
            release_artist: "Resolved Artist",
            track_title: "Identity Track",
            track_artist: "Resolved Artist",
            release_date: "2003-04-05",
        },
    )
    .await;
    let release_uuid = fixture.release_id().into_uuid();

    // The import auto-confirm leaves no unresolved observations.
    let (none, zero) = CatalogStore
        .list_unresolved_observations(&db.pool, 50, 0, ObservedArtistSort::CreatedAtAsc)
        .await
        .expect("list unresolved");
    assert!(none.is_empty());
    assert_eq!(zero, 0);

    // Seed a raw, unresolved observation against the imported Release.
    let observation_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO observed_artists (
            id, release_id, track_id, scope, source_ordinal, raw_value,
            normalized_value, display_value, source_pointer, source_payload,
            created_at
         ) VALUES ($1,$2,NULL,'release',9,'The  unresolved ','the unresolved','The Unresolved','/fixture/artist',$3,$4)",
    )
    .bind(observation_id)
    .bind(release_uuid)
    .bind(serde_json::json!({ "value": "The Unresolved" }))
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed unresolved observation");

    // A pre-existing Artist to propose against.
    let candidate = melodee_domain::Artist::new(melodee_domain::NewArtist {
        id: melodee_domain::ArtistId::new(),
        public_api_id: melodee_domain::ArtistPublicId::from_uuid(Uuid::now_v7()),
        canonical_name: "The Unresolved".to_string(),
        normalized_name: "the unresolved".to_string(),
        sort_name: "Unresolved, The".to_string(),
        artist_type: melodee_domain::ArtistType::Group,
        disambiguation: None,
        active_dates: None,
        status: melodee_domain::ArtistStatus::Verified,
        created_at: now,
    })
    .expect("valid candidate artist");
    CatalogStore
        .create_artist(&db.pool, &candidate)
        .await
        .expect("create candidate");

    // Immutable evidence plus a proposed (non-authoritative) decision.
    let evidence = melodee_domain::ArtistMatchEvidence {
        id: melodee_domain::ArtistMatchEvidenceId::new(),
        observed_artist_id: melodee_domain::ObservedArtistId::from_uuid(observation_id),
        candidate_artist_id: candidate.id(),
        kind: melodee_domain::EvidenceKind::Alias,
        polarity: melodee_domain::EvidencePolarity::Supports,
        confidence: Confidence::new(7_500).expect("valid confidence"),
        source: "fixture-resolver".to_string(),
    };
    CatalogStore
        .record_artist_match_evidence(
            &db.pool,
            &evidence,
            serde_json::json!({ "alias": "The Unresolved" }),
            now,
        )
        .await
        .expect("record evidence");
    let decision = melodee_domain::ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: melodee_domain::ArtistMatchDecisionId::new(),
            observed_artist_id: melodee_domain::ObservedArtistId::from_uuid(observation_id),
            artist_id: candidate.id(),
            outcome: ArtistMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "fixture-resolver-v1".to_string(),
            },
            locked: false,
            reason: "alias match proposed".to_string(),
            decided_at: now,
        },
        std::slice::from_ref(&evidence),
    )
    .expect("valid proposal");
    CatalogStore
        .record_artist_match_decision(
            &db.pool,
            &decision,
            Confidence::new(7_500).expect("valid confidence"),
            serde_json::json!({}),
        )
        .await
        .expect("record proposal");

    let (items, total) = CatalogStore
        .list_unresolved_observations(&db.pool, 50, 0, ObservedArtistSort::CreatedAtAsc)
        .await
        .expect("list unresolved after proposal");
    assert_eq!(total, 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, observation_id);
    assert_eq!(items[0].display_value, "The Unresolved");
    assert_eq!(items[0].release_title, "Identity Read Fixture");
    assert_eq!(items[0].pending_match_count, 1);
    assert_eq!(items[0].evidence_count, 1);

    let detail = CatalogStore
        .observed_artist_detail(&db.pool, observation_id)
        .await
        .expect("detail read")
        .expect("observation exists");
    assert!(detail.current.is_none());
    assert_eq!(detail.candidates.len(), 1);
    let candidate_row = &detail.candidates[0];
    assert_eq!(candidate_row.artist_id, candidate.id().into_uuid());
    assert_eq!(candidate_row.latest_outcome, "proposed");
    assert_eq!(candidate_row.latest_confidence, 7_500);
    assert_eq!(candidate_row.decision_count, 1);
    assert_eq!(candidate_row.evidence.len(), 1);
    assert_eq!(candidate_row.evidence[0].kind, "alias");
    assert_eq!(candidate_row.evidence[0].polarity, "supports");

    // Unknown observation -> None.
    let missing = CatalogStore
        .observed_artist_detail(&db.pool, Uuid::now_v7())
        .await
        .expect("missing detail read");
    assert!(missing.is_none());

    // The audit feed carries the evidence/proposal events with actor/reason.
    let (feed, feed_total) = CatalogStore
        .list_identity_decisions(&db.pool, 50, 0)
        .await
        .expect("identity feed");
    assert!(feed_total >= 2);
    assert!(
        feed.iter()
            .any(|entry| entry.event_type == "artist_match_decision_proposed"
                && entry.entity_id == observation_id)
    );
    assert!(
        feed.iter()
            .any(|entry| entry.event_type == "artist_match_evidence_recorded")
    );

    // Public-id resolution for merge/split entry points.
    let resolved = CatalogStore
        .artist_by_public_id(&db.pool, candidate.public_api_id().into_uuid())
        .await
        .expect("artist lookup")
        .expect("artist found");
    assert_eq!(resolved.id, candidate.id());
    assert_eq!(resolved.status, "verified");
    assert!(
        CatalogStore
            .artist_by_public_id(&db.pool, Uuid::now_v7())
            .await
            .expect("missing artist lookup")
            .is_none()
    );

    db.drop().await;
}

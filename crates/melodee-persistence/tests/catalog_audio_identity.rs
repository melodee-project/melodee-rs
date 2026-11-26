//! AudioIdentity evidence, correction, and immutable-byte proofs.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use std::collections::BTreeSet;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::{
    AudioIdentityDecisionId, AudioIdentityEvidence, AudioIdentityEvidenceId,
    AudioIdentityEvidenceKind, AudioIdentityId, AudioIdentityMatchDecision,
    AudioIdentityMatchOutcome, AudioIdentityMerge, AudioIdentityMergeId, AudioIdentitySplit,
    AudioIdentitySplitId, Confidence, DecisionAuthority, EvidencePolarity,
    NewAudioIdentityMatchDecision, TrackId,
};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, CatalogStoreError, WorkItemStore, WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[tokio::test]
async fn audio_and_release_corrections_lock_shared_tracks_in_order() {
    let db = TestDb::create("audio_release_track_order").await;
    let mut fixtures = Vec::new();
    for ordinal in 0..3 {
        fixtures.push(
            seed_published_release(
                &db,
                140 + ordinal,
                CatalogFixturePresentation {
                    release_title: match ordinal {
                        0 => "Audio Race A",
                        1 => "Audio Race B",
                        _ => "Audio Race C",
                    },
                    ..CatalogFixturePresentation::default()
                },
            )
            .await,
        );
    }
    let now = OffsetDateTime::now_utc();
    let worker = WorkerId::new();
    CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim fixtures");
    for fixture in &fixtures {
        CatalogStore
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("audio-release-race", "test"),
                now,
            )
            .await
            .expect("import fixture");
    }
    let releases = fixtures
        .iter()
        .map(|f| f.release_id.into_uuid())
        .collect::<Vec<_>>();
    let tracks: Vec<(Uuid,Uuid)> = sqlx::query_as("SELECT id,audio_identity_id FROM tracks WHERE release_id=ANY($1) ORDER BY array_position($1,release_id)")
        .bind(&releases).fetch_all(&db.pool).await.expect("read Tracks");
    assert_eq!(tracks.len(), 3);
    let source_audio = tracks[0].1;
    let target_audio = tracks[2].1;
    sqlx::query("UPDATE tracks SET audio_identity_id=$2 WHERE id=$1")
        .bind(tracks[1].0)
        .bind(source_audio)
        .execute(&db.pool)
        .await
        .expect("share source AudioIdentity");
    let mut holder = db.pool.begin().await.expect("begin Track holder");
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR UPDATE")
        .bind(tracks[0].0.min(tracks[1].0))
        .execute(&mut *holder)
        .await
        .expect("hold first Track");
    let audio = AudioIdentityMerge {
        id: AudioIdentityMergeId::new(),
        source_id: AudioIdentityId::from_uuid(source_audio),
        target_id: AudioIdentityId::from_uuid(target_audio),
        reason: "concurrent correction".into(),
        occurred_at: now,
    };
    let merges = releases[..2]
        .iter()
        .copied()
        .map(|source_release_id| melodee_persistence::ReleaseMerge {
            source_release_id,
            target_release_id: releases[2],
            actor: "test".into(),
            reason: "concurrent correction".into(),
            occurred_at: now,
        })
        .collect::<Vec<_>>();
    let audio_pool = db.pool.clone();
    let release_pool = db.pool.clone();
    let audio_task = tokio::spawn(async move {
        CatalogStore
            .merge_audio_identity(&audio_pool, &audio, "test", "race")
            .await
    });
    let release_task =
        tokio::spawn(async move { CatalogStore.merge_releases(&release_pool, &merges).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'")
                .fetch_one(&db.pool).await.expect("count Track waiters");
            if waiting >= 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("both corrections wait at the first shared Track");
    holder.commit().await.expect("release Track holder");
    let (audio_result, release_result) =
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            (
                audio_task.await.expect("audio join"),
                release_task.await.expect("Release join"),
            )
        })
        .await
        .expect("corrections finish without deadlock");
    release_result.expect("Release merge succeeds");
    assert!(
        audio_result.is_ok() || matches!(audio_result, Err(CatalogStoreError::MutationConflict)),
        "audio correction must succeed or reject changed binding: {audio_result:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
        .bind(releases[2])
        .fetch_one(&db.pool)
        .await
        .expect("count moved Tracks");
    assert_eq!(count, 3);
    db.drop().await;
}

#[tokio::test]
async fn audio_evidence_locked_merge_and_selective_split_never_move_or_rewrite_media() {
    let db = TestDb::create("catalog_audio_identity_corrections").await;
    let first = seed_published_release(
        &db,
        101,
        CatalogFixturePresentation {
            release_title: "Audio Source",
            release_artist: "Audio Fixture",
            track_title: "Same Performance A",
            track_artist: "Audio Fixture",
            release_date: "2010-01-01",
        },
    )
    .await;
    let second = seed_published_release(
        &db,
        102,
        CatalogFixturePresentation {
            release_title: "Audio Target",
            release_artist: "Audio Fixture",
            track_title: "Same Performance B",
            track_artist: "Audio Fixture",
            release_date: "2011-01-01",
        },
    )
    .await;
    let store = CatalogStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        store
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim AudioIdentity fixtures")
            .len(),
        2
    );
    for fixture in [&first, &second] {
        store
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("audio-correction-import-v1", "audio-test"),
                now,
            )
            .await
            .expect("import AudioIdentity fixture");
    }

    let first_manifest = WorkItemStore
        .get_manifest(&db.pool, first.work_item_id)
        .await
        .expect("read first v3")
        .expect("first v3 exists");
    let second_manifest = WorkItemStore
        .get_manifest(&db.pool, second.work_item_id)
        .await
        .expect("read second v3")
        .expect("second v3 exists");
    let first_publication = WorkItemStore
        .get_publication(&db.pool, first.work_item_id)
        .await
        .expect("read first publication")
        .expect("first publication exists");
    let second_publication = WorkItemStore
        .get_publication(&db.pool, second.work_item_id)
        .await
        .expect("read second publication")
        .expect("second publication exists");
    let media_before: Vec<(Uuid, String, String, i64)> = sqlx::query_as(
        "SELECT id,canonical_relative_path,byte_hash,byte_size FROM media_assets ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("snapshot MediaAssets");
    let tracks_before: Vec<(Uuid, Uuid, Option<String>, i64)> = sqlx::query_as(
        "SELECT id,audio_identity_id,source_path,revision FROM tracks ORDER BY release_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("snapshot Tracks");
    assert_eq!(tracks_before.len(), 2);
    let source_track = TrackId::from_uuid(tracks_before[0].0);
    let target_track = TrackId::from_uuid(tracks_before[1].0);
    let source_audio = AudioIdentityId::from_uuid(tracks_before[0].1);
    let target_audio = AudioIdentityId::from_uuid(tracks_before[1].1);
    assert_ne!(source_audio, target_audio);

    let fingerprint = AudioIdentityEvidence {
        id: AudioIdentityEvidenceId::new(),
        source_audio_identity_id: source_audio,
        candidate_audio_identity_id: target_audio,
        kind: AudioIdentityEvidenceKind::AcousticFingerprint,
        polarity: EvidencePolarity::Supports,
        confidence: Confidence::new(9_000).expect("valid confidence"),
        source: "fingerprint-fixture".to_owned(),
    };
    store
        .record_audio_identity_evidence(
            &db.pool,
            &fingerprint,
            serde_json::json!({"fingerprint": "fixture-match"}),
            now,
        )
        .await
        .expect("persist fingerprint evidence");

    let unpersisted_evidence = AudioIdentityEvidence {
        id: AudioIdentityEvidenceId::new(),
        source_audio_identity_id: source_audio,
        candidate_audio_identity_id: target_audio,
        kind: AudioIdentityEvidenceKind::Duration,
        polarity: EvidencePolarity::Supports,
        confidence: Confidence::new(8_000).expect("valid confidence"),
        source: "unpersisted-fixture".to_owned(),
    };
    let unbacked_decision = AudioIdentityMatchDecision::new(
        NewAudioIdentityMatchDecision {
            id: AudioIdentityDecisionId::new(),
            source_audio_identity_id: source_audio,
            candidate_audio_identity_id: target_audio,
            outcome: AudioIdentityMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "audio-resolver-v1".to_owned(),
            },
            locked: false,
            reason: "must reject an unpersisted evidence ID".to_owned(),
            decided_at: now,
        },
        std::slice::from_ref(&unpersisted_evidence),
    )
    .expect("domain-valid decision with an unpersisted evidence reference");
    assert!(matches!(
        store
            .record_audio_identity_match_decision(
                &db.pool,
                &unbacked_decision,
                serde_json::json!({}),
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));

    let proposal = AudioIdentityMatchDecision::new(
        NewAudioIdentityMatchDecision {
            id: AudioIdentityDecisionId::new(),
            source_audio_identity_id: source_audio,
            candidate_audio_identity_id: target_audio,
            outcome: AudioIdentityMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "audio-resolver-v1".to_owned(),
            },
            locked: false,
            reason: "fingerprint alone remains a proposal".to_owned(),
            decided_at: now + Duration::seconds(1),
        },
        std::slice::from_ref(&fingerprint),
    )
    .expect("valid fingerprint-only proposal");
    store
        .record_audio_identity_match_decision(
            &db.pool,
            &proposal,
            serde_json::json!({"fixture": "fingerprint-only"}),
        )
        .await
        .expect("persist fingerprint-only proposal");
    let proposed_state: (String, Uuid, String, Uuid) = sqlx::query_as(
        "SELECT decision.outcome,track.audio_identity_id,audio.status,
                current.decision_id
         FROM tracks track
         JOIN audio_identities audio ON audio.id=track.audio_identity_id
         JOIN audio_identity_match_current current
           ON current.source_audio_identity_id=$1
          AND current.candidate_audio_identity_id=$2
         JOIN audio_identity_match_decisions decision ON decision.id=current.decision_id
         WHERE track.id=$3",
    )
    .bind(source_audio.into_uuid())
    .bind(target_audio.into_uuid())
    .bind(source_track.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query proposal without merge");
    assert_eq!(
        proposed_state,
        (
            "proposed".to_owned(),
            source_audio.into_uuid(),
            "provisional".to_owned(),
            proposal.id().into_uuid(),
        )
    );
    let merge_count_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audio_identity_merge_events")
            .fetch_one(&db.pool)
            .await
            .expect("query no automatic merge");
    assert_eq!(merge_count_before, 0);

    let confirmed = AudioIdentityMatchDecision::new(
        NewAudioIdentityMatchDecision {
            id: AudioIdentityDecisionId::new(),
            source_audio_identity_id: source_audio,
            candidate_audio_identity_id: target_audio,
            outcome: AudioIdentityMatchOutcome::Confirmed,
            authority: DecisionAuthority::Administrator {
                actor: "catalog-admin".to_owned(),
            },
            locked: true,
            reason: "administrator confirms equivalent audible performance".to_owned(),
            decided_at: now + Duration::seconds(2),
        },
        std::slice::from_ref(&fingerprint),
    )
    .expect("valid locked administrator confirmation");
    store
        .record_audio_identity_match_decision(
            &db.pool,
            &confirmed,
            serde_json::json!({"fixture": "manual-confirmation"}),
        )
        .await
        .expect("persist locked AudioIdentity confirmation");
    let automated_refresh = AudioIdentityMatchDecision::new(
        NewAudioIdentityMatchDecision {
            id: AudioIdentityDecisionId::new(),
            source_audio_identity_id: source_audio,
            candidate_audio_identity_id: target_audio,
            outcome: AudioIdentityMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "audio-resolver-v2".to_owned(),
            },
            locked: false,
            reason: "provider refresh cannot supersede locked admin".to_owned(),
            decided_at: now + Duration::seconds(3),
        },
        std::slice::from_ref(&fingerprint),
    )
    .expect("valid automated refresh");
    assert!(matches!(
        store
            .record_audio_identity_match_decision(
                &db.pool,
                &automated_refresh,
                serde_json::json!({}),
            )
            .await,
        Err(CatalogStoreError::LockedDecision)
    ));

    let merge = AudioIdentityMerge {
        id: AudioIdentityMergeId::new(),
        source_id: source_audio,
        target_id: target_audio,
        reason: "apply locked administrator confirmation".to_owned(),
        occurred_at: now + Duration::seconds(4),
    };
    store
        .merge_audio_identity(
            &db.pool,
            &merge,
            "catalog-admin",
            "manual-audio-correction-v1",
        )
        .await
        .expect("merge AudioIdentity across Releases");
    let after_merge: (Uuid, i64, String, Option<Uuid>) = sqlx::query_as(
        "SELECT track.audio_identity_id,track.revision,audio.status,
                audio.merged_into_audio_identity_id
         FROM tracks track
         JOIN audio_identities audio ON audio.id=$2
         WHERE track.id=$1",
    )
    .bind(source_track.into_uuid())
    .bind(source_audio.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query merged AudioIdentity and Track revision");
    assert_eq!(
        after_merge,
        (
            target_audio.into_uuid(),
            2,
            "merged".to_owned(),
            Some(target_audio.into_uuid()),
        )
    );

    let split = AudioIdentitySplit {
        id: AudioIdentitySplitId::new(),
        source_id: target_audio,
        new_identity_id: AudioIdentityId::new(),
        track_ids: [source_track].into_iter().collect(),
        reason: "split only the selected Track occurrence".to_owned(),
        occurred_at: now + Duration::seconds(5),
    };
    store
        .split_audio_identity(
            &db.pool,
            &split,
            "catalog-admin",
            "manual-audio-correction-v2",
        )
        .await
        .expect("selectively split AudioIdentity");
    let audio_owners: Vec<(Uuid, Uuid, i64)> =
        sqlx::query_as("SELECT id,audio_identity_id,revision FROM tracks ORDER BY release_id")
            .fetch_all(&db.pool)
            .await
            .expect("query selective AudioIdentity split");
    assert_eq!(
        audio_owners,
        vec![
            (
                source_track.into_uuid(),
                split.new_identity_id.into_uuid(),
                3
            ),
            (target_track.into_uuid(), target_audio.into_uuid(), 1),
        ]
    );
    let track_outbox_revisions: BTreeSet<i64> = sqlx::query_scalar(
        "SELECT entity_revision FROM catalog_projection_outbox
         WHERE entity_type='track' AND entity_id=$1",
    )
    .bind(source_track.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("query Track revision outbox")
    .into_iter()
    .collect();
    assert_eq!(track_outbox_revisions, [1, 2, 3].into_iter().collect());

    let first_overlay_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=$1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query source Release overlay revisions");
    let second_overlay_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=$1")
            .bind(second.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query target Release overlay revisions");
    assert_eq!(first_overlay_count, 3);
    assert_eq!(second_overlay_count, 1);
    let correction_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM audio_identity_merge_events WHERE id=$1),
            (SELECT count(*) FROM audio_identity_split_events WHERE id=$2),
            (SELECT count(*) FROM catalog_audit_events
             WHERE event_type IN ('audio_identity_merged','audio_identity_split'))",
    )
    .bind(merge.id.into_uuid())
    .bind(split.id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query AudioIdentity correction audit");
    assert_eq!(
        correction_counts,
        (1, 1, 6),
        "two AudioIdentity and one Track audit row are emitted by each correction"
    );

    let media_after: Vec<(Uuid, String, String, i64)> = sqlx::query_as(
        "SELECT id,canonical_relative_path,byte_hash,byte_size FROM media_assets ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("query MediaAssets after AudioIdentity corrections");
    assert_eq!(media_after, media_before);
    for (fixture, manifest_before, publication_before) in [
        (&first, first_manifest, first_publication),
        (&second, second_manifest, second_publication),
    ] {
        let manifest_after = WorkItemStore
            .get_manifest(&db.pool, fixture.work_item_id)
            .await
            .expect("read immutable post-correction v3")
            .expect("v3 remains");
        let publication_after = WorkItemStore
            .get_publication(&db.pool, fixture.work_item_id)
            .await
            .expect("read immutable post-correction publication")
            .expect("publication remains");
        assert_eq!(manifest_after.id, manifest_before.id);
        assert_eq!(manifest_after.manifest_hash, manifest_before.manifest_hash);
        assert_eq!(manifest_after.payload, manifest_before.payload);
        assert_eq!(publication_after, publication_before);
    }

    sqlx::query("UPDATE audio_identities SET status='deprecated',updated_at=$2 WHERE id=$1")
        .bind(target_audio.into_uuid())
        .bind(now + Duration::seconds(6))
        .execute(&db.pool)
        .await
        .expect("deprecate AudioIdentity correction target");
    let active_audio = AudioIdentityId::new();
    sqlx::query(
        "INSERT INTO audio_identities (id,status,revision,created_at,updated_at)
         VALUES ($1,'provisional',1,$2,$2)",
    )
    .bind(active_audio.into_uuid())
    .bind(now + Duration::seconds(6))
    .execute(&db.pool)
    .await
    .expect("insert active AudioIdentity merge source");
    let deprecated_target_merge = AudioIdentityMerge {
        id: AudioIdentityMergeId::new(),
        source_id: active_audio,
        target_id: target_audio,
        reason: "deprecated AudioIdentity cannot receive a merge".to_owned(),
        occurred_at: now + Duration::seconds(7),
    };
    assert!(matches!(
        store
            .merge_audio_identity(
                &db.pool,
                &deprecated_target_merge,
                "catalog-admin",
                "deprecated-audio-rejection-v1",
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let deprecated_source_merge = AudioIdentityMerge {
        id: AudioIdentityMergeId::new(),
        source_id: target_audio,
        target_id: active_audio,
        reason: "deprecated AudioIdentity cannot be a merge source".to_owned(),
        occurred_at: now + Duration::seconds(8),
    };
    assert!(matches!(
        store
            .merge_audio_identity(
                &db.pool,
                &deprecated_source_merge,
                "catalog-admin",
                "deprecated-audio-rejection-v2",
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let deprecated_source_split = AudioIdentitySplit {
        id: AudioIdentitySplitId::new(),
        source_id: target_audio,
        new_identity_id: AudioIdentityId::new(),
        track_ids: [target_track].into_iter().collect(),
        reason: "deprecated AudioIdentity cannot be split".to_owned(),
        occurred_at: now + Duration::seconds(9),
    };
    assert!(matches!(
        store
            .split_audio_identity(
                &db.pool,
                &deprecated_source_split,
                "catalog-admin",
                "deprecated-audio-rejection-v1",
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let rejected_audio_state: (Uuid, i64, i64, i64) = sqlx::query_as(
        "SELECT track.audio_identity_id,
                (SELECT count(*) FROM audio_identity_merge_events WHERE id=$2),
                (SELECT count(*) FROM audio_identity_split_events WHERE id=$3),
                (SELECT count(*) FROM audio_identity_merge_events WHERE id=$4)
         FROM tracks track WHERE track.id=$1",
    )
    .bind(target_track.into_uuid())
    .bind(deprecated_target_merge.id.into_uuid())
    .bind(deprecated_source_split.id.into_uuid())
    .bind(deprecated_source_merge.id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query deprecated Audio correction rollback");
    assert_eq!(rejected_audio_state, (target_audio.into_uuid(), 0, 0, 0));

    db.drop().await;
}

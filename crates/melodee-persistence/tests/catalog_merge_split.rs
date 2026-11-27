//! Audited Artist merge/split correction and immutable-media proofs.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use std::collections::BTreeSet;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::{
    Artist, ArtistAlias, ArtistAliasId, ArtistAliasType, ArtistId, ArtistMerge, ArtistMergeId,
    ArtistPublicId, ArtistRelationship, ArtistRelationshipId, ArtistRelationshipKind, ArtistSplit,
    ArtistSplitId, ArtistStatus, ArtistType, Confidence, DateRange, ExternalIdentity,
    ExternalIdentityId, ExternalIdentityOwner, GroupMembership, GroupMembershipId, MembershipRole,
    NewArtist, SplitAssignment, TrackArtistId,
};
use melodee_persistence::{
    CatalogCorrectionDimension, CatalogImportRequest, CatalogStore, CatalogStoreError,
    MAX_CATALOG_CORRECTION_RELEASES, WorkItemStore, WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn test_artist(name: &str, artist_type: ArtistType, now: OffsetDateTime) -> Artist {
    Artist::new(NewArtist {
        id: ArtistId::new(),
        public_api_id: ArtistPublicId::from_uuid(Uuid::now_v7()),
        canonical_name: name.to_owned(),
        normalized_name: name.to_uppercase().replace(' ', ""),
        sort_name: name.to_owned(),
        artist_type,
        disambiguation: None,
        active_dates: None,
        status: ArtistStatus::Verified,
        created_at: now,
    })
    .expect("valid test Artist")
}

async fn insert_native_user(db: &TestDb, username: &str, now: OffsetDateTime) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users (
             id, public_api_id, username, normalized_username, email, normalized_email,
             password_hash, password_hash_algorithm, stream_signing_key, created_at, updated_at
         ) VALUES ($1,$2,$3,lower($3),$4,lower($4),$5,'bcrypt',$6,$7,$7)",
    )
    .bind(id)
    .bind(Uuid::now_v7())
    .bind(username)
    .bind(format!("{username}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .bind("artist-merge-parity-stream-key")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert native user");
    id
}

#[tokio::test]
async fn artist_merge_and_scrobble_wait_in_parent_order_and_preserve_popularity() {
    use melodee_persistence::{NativeScrobbleResult, NativeUserStateStore};
    let db = TestDb::create("artist_merge_scrobble_order").await;
    let fixture = seed_published_release(
        &db,
        90,
        CatalogFixturePresentation {
            release_title: "Concurrent Artist Correction",
            release_artist: "Merge Destination",
            track_title: "Concurrent Play",
            track_artist: "Merge Source",
            release_date: "2009-09-09",
        },
    )
    .await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim fixture");
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("merge-race", "test"),
            now,
        )
        .await
        .expect("import fixture");
    let release = fixture.release_id.into_uuid();
    let (track, public, source, source_public): (Uuid,Uuid,Uuid,Uuid) = sqlx::query_as("SELECT t.id,t.public_api_id,a.artist_id,artist.public_api_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' JOIN artists artist ON artist.id=a.artist_id WHERE t.release_id=$1")
        .bind(release).fetch_one(&db.pool).await.expect("read Track credit");
    let target: Uuid = sqlx::query_scalar(
        "SELECT artist_id FROM release_artists WHERE release_id=$1 AND role='primary'",
    )
    .bind(release)
    .fetch_one(&db.pool)
    .await
    .expect("read target");
    assert_ne!(source, target);
    let user = insert_native_user(&db, "artist-merge-race", now).await;
    let mut holder = db.pool.begin().await.expect("begin holder");
    sqlx::query("SELECT id FROM artists WHERE id=$1 FOR UPDATE")
        .bind(source)
        .execute(&mut *holder)
        .await
        .expect("lock source Artist");
    let merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: ArtistId::from_uuid(source),
        source_public_api_id: ArtistPublicId::from_uuid(source_public),
        target_artist_id: ArtistId::from_uuid(target),
        actor: "test".into(),
        reason: "race".into(),
        occurred_at: now,
    };
    let merge_pool = db.pool.clone();
    let play_pool = db.pool.clone();
    let probes = melodee_persistence::metrics::snapshot()
        .into_iter()
        .find(|metric| metric.name == "scrobble_wait_probe")
        .expect("probe metric")
        .count;
    let merge_task =
        tokio::spawn(async move { CatalogStore.merge_artist(&merge_pool, &merge, "race").await });
    let play_task = tokio::spawn(async move {
        NativeUserStateStore::record_scrobble(
            &play_pool,
            user,
            public,
            "artist-merge-race",
            now,
            now,
            Some(30),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'")
                .fetch_one(&db.pool).await.expect("count waiters");
            let observed = melodee_persistence::metrics::snapshot().into_iter()
                .find(|metric| metric.name == "scrobble_wait_probe").expect("probe metric").count;
            if waiting >= 1 && observed > probes { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("merge waits and scrobble probes parent");
    holder.commit().await.expect("release parent");
    let (merged, played) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        (
            merge_task.await.expect("merge join"),
            play_task.await.expect("play join"),
        )
    })
    .await
    .expect("operations finish");
    merged.expect("merge succeeds");
    assert_eq!(
        played.expect("play succeeds"),
        NativeScrobbleResult::Applied
    );
    let (actual, expected): (i64,i64) = sqlx::query_as("SELECT c.popularity,(SELECT count(*) FROM native_track_plays p JOIN track_artists a ON a.track_id=p.track_id WHERE a.role='primary' AND a.artist_id=$1) FROM native_recommendation_candidates c WHERE c.entity_type='artist' AND c.entity_id=$1")
        .bind(target).fetch_one(&db.pool).await.expect("compare Artist popularity");
    assert_eq!(actual, expected);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM native_track_plays WHERE track_id=$1")
            .bind(track)
            .fetch_one(&db.pool)
            .await
            .expect("count plays"),
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn merge_then_selective_split_preserves_public_ids_v3_and_every_media_path() {
    let db = TestDb::create("catalog_artist_merge_split").await;
    let fixture = seed_published_release(
        &db,
        91,
        CatalogFixturePresentation {
            release_title: "Immutable Paths",
            release_artist: "Correct Identity",
            track_title: "Selected Split",
            track_artist: "Duplicate Identity",
            release_date: "2009-09-09",
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
            .expect("claim merge/split fixture")
            .len(),
        1
    );
    store
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("merge-split-import-v1", "merge-split-test"),
            now,
        )
        .await
        .expect("import merge/split fixture");

    let work_before = WorkItemStore
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-correction work item");
    let manifest_before = WorkItemStore
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-correction v3")
        .expect("v3 exists");
    let publication_before = WorkItemStore
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-correction publication")
        .expect("publication exists");
    let media_before: Vec<(Uuid, String, String, i64)> = sqlx::query_as(
        "SELECT id,canonical_relative_path,byte_hash,byte_size FROM media_assets ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("snapshot canonical media rows");
    let track_paths_before: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id,source_path FROM tracks ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .expect("snapshot Track source paths");
    let initial_overlay: Uuid =
        sqlx::query_scalar("SELECT current_overlay_id FROM releases WHERE id=$1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query initial overlay");

    let target: (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT billed.id,billed.artist_id,artist.public_api_id
         FROM release_artists billed JOIN artists artist ON artist.id=billed.artist_id
         WHERE billed.release_id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query merge target");
    let source: (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT billed.id,billed.artist_id,artist.public_api_id
         FROM track_artists billed
         JOIN tracks track ON track.id=billed.track_id
         JOIN artists artist ON artist.id=billed.artist_id
         WHERE track.release_id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query merge source");
    assert_ne!(source.1, target.1);
    let peer_artist_id = ArtistId::new();
    let peer_artist = Artist::new(NewArtist {
        id: peer_artist_id,
        public_api_id: ArtistPublicId::from_uuid(Uuid::now_v7()),
        canonical_name: "Relationship Peer".to_owned(),
        normalized_name: "RELATIONSHIPPEER".to_owned(),
        sort_name: "Relationship Peer".to_owned(),
        artist_type: ArtistType::Ensemble,
        disambiguation: None,
        active_dates: None,
        status: ArtistStatus::Verified,
        created_at: now,
    })
    .expect("valid relationship peer");
    store
        .create_artist(&db.pool, &peer_artist)
        .await
        .expect("create relationship peer");
    let redirected_alias = ArtistAliasId::new();
    store
        .add_artist_alias(
            &db.pool,
            &ArtistAlias {
                id: redirected_alias,
                artist_id: ArtistId::from_uuid(source.1),
                value: "Redirected Locked Alias".to_owned(),
                normalized_value: "REDIRECTEDLOCKEDALIAS".to_owned(),
                alias_type: ArtistAliasType::Search,
                locale: None,
                evidence_source: "merge-redirect-fixture".to_owned(),
                confidence: Confidence::new(10_000).expect("valid confidence"),
                locked: true,
            },
            now,
        )
        .await
        .expect("seed source alias redirect");
    let redirected_external = ExternalIdentityId::new();
    store
        .add_artist_external_identity(
            &db.pool,
            &ExternalIdentity {
                id: redirected_external,
                owner: ExternalIdentityOwner::Artist(ArtistId::from_uuid(source.1)),
                provider: "redirect-fixture".to_owned(),
                external_id: "redirected-provider-id".to_owned(),
                confidence: Confidence::new(10_000).expect("valid confidence"),
                evidence_source: "merge-redirect-fixture".to_owned(),
                verified_at: Some(now),
                locked: true,
                provider_guarantees_uniqueness: true,
            },
            now,
        )
        .await
        .expect("seed source external identity redirect");
    let relationship_id = ArtistRelationshipId::new();
    store
        .add_artist_relationship(
            &db.pool,
            &ArtistRelationship {
                id: relationship_id,
                subject_artist_id: ArtistId::from_uuid(source.1),
                object_artist_id: peer_artist_id,
                kind: ArtistRelationshipKind::new("associated_with".to_owned())
                    .expect("valid relationship kind"),
                active_dates: None,
                confidence: Confidence::new(8_000).expect("valid confidence"),
                evidence_source: "merge-redirect-fixture".to_owned(),
            },
            now,
        )
        .await
        .expect("seed source relationship redirect");
    let membership_id = GroupMembershipId::new();
    store
        .add_group_membership(
            &db.pool,
            &GroupMembership {
                id: membership_id,
                member_artist_id: ArtistId::from_uuid(source.1),
                group_artist_id: peer_artist_id,
                active_dates: DateRange::new(None, None).expect("valid unbounded membership"),
                role: MembershipRole::new("member".to_owned()).expect("valid role"),
                confidence: Confidence::new(8_000).expect("valid confidence"),
                evidence_source: "merge-redirect-fixture".to_owned(),
                excluded_tracks: BTreeSet::new(),
            },
            now,
        )
        .await
        .expect("seed source membership redirect");
    let merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: ArtistId::from_uuid(source.1),
        source_public_api_id: ArtistPublicId::from_uuid(source.2),
        target_artist_id: ArtistId::from_uuid(target.1),
        actor: "catalog-admin".to_owned(),
        reason: "duplicate observation confirmed".to_owned(),
        occurred_at: now + Duration::seconds(1),
    };
    store
        .merge_artist(&db.pool, &merge, "manual-identity-correction-v1")
        .await
        .expect("merge duplicate Artist");

    let merged_source: (Uuid, String, Option<Uuid>) = sqlx::query_as(
        "SELECT public_api_id,status,merged_into_artist_id FROM artists WHERE id=$1",
    )
    .bind(source.1)
    .fetch_one(&db.pool)
    .await
    .expect("read merge tombstone");
    assert_eq!(
        merged_source,
        (source.2, "merged".to_owned(), Some(target.1))
    );
    let merged_track_owner: Uuid =
        sqlx::query_scalar("SELECT artist_id FROM track_artists WHERE id=$1")
            .bind(source.0)
            .fetch_one(&db.pool)
            .await
            .expect("merge retargeted Track billing");
    assert_eq!(merged_track_owner, target.1);
    let redirected: (Uuid, bool, Uuid, bool, Uuid, Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT
            (SELECT artist_id FROM artist_aliases WHERE id=$1),
            (SELECT is_locked FROM artist_aliases WHERE id=$1),
            (SELECT artist_id FROM artist_external_identities WHERE id=$2),
            (SELECT is_locked FROM artist_external_identities WHERE id=$2),
            (SELECT source_artist_id FROM artist_relationships WHERE id=$3),
            (SELECT target_artist_id FROM artist_relationships WHERE id=$3),
            (SELECT member_artist_id FROM group_memberships WHERE id=$4),
            (SELECT group_artist_id FROM group_memberships WHERE id=$4)",
    )
    .bind(redirected_alias.into_uuid())
    .bind(redirected_external.into_uuid())
    .bind(relationship_id.into_uuid())
    .bind(membership_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query merge redirects");
    assert_eq!(
        redirected,
        (
            target.1,
            true,
            target.1,
            true,
            target.1,
            peer_artist_id.into_uuid(),
            target.1,
            peer_artist_id.into_uuid(),
        ),
        "merge redirects locked evidence and relationship endpoints without losing locks"
    );
    let post_merge_overlay: Uuid =
        sqlx::query_scalar("SELECT current_overlay_id FROM releases WHERE id=$1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query merge overlay");
    assert_ne!(post_merge_overlay, initial_overlay);

    let selected_alias = ArtistAliasId::new();
    let retained_alias = ArtistAliasId::new();
    for (id, value) in [
        (selected_alias, "Selected Alias"),
        (retained_alias, "Retained Alias"),
    ] {
        store
            .add_artist_alias(
                &db.pool,
                &ArtistAlias {
                    id,
                    artist_id: ArtistId::from_uuid(target.1),
                    value: value.to_owned(),
                    normalized_value: value.to_uppercase().replace(' ', ""),
                    alias_type: ArtistAliasType::Search,
                    locale: None,
                    evidence_source: "merge-split-fixture".to_owned(),
                    confidence: Confidence::new(10_000).expect("valid confidence"),
                    locked: true,
                },
                now + Duration::seconds(2),
            )
            .await
            .expect("add split alias fixture");
    }
    let selected_external = ExternalIdentityId::new();
    let retained_external = ExternalIdentityId::new();
    for (id, external_id) in [
        (selected_external, "selected-provider-id"),
        (retained_external, "retained-provider-id"),
    ] {
        store
            .add_artist_external_identity(
                &db.pool,
                &ExternalIdentity {
                    id,
                    owner: ExternalIdentityOwner::Artist(ArtistId::from_uuid(target.1)),
                    provider: "fixture-provider".to_owned(),
                    external_id: external_id.to_owned(),
                    confidence: Confidence::new(9_000).expect("valid confidence"),
                    evidence_source: "merge-split-fixture".to_owned(),
                    verified_at: Some(now),
                    locked: true,
                    provider_guarantees_uniqueness: true,
                },
                now + Duration::seconds(2),
            )
            .await
            .expect("add split provider fixture");
    }

    let new_artist_id = ArtistId::new();
    let new_public_id = ArtistPublicId::from_uuid(Uuid::now_v7());
    let new_artist = Artist::new(NewArtist {
        id: new_artist_id,
        public_api_id: new_public_id,
        canonical_name: "Restored Track Identity".to_owned(),
        normalized_name: "RESTOREDTRACKIDENTITY".to_owned(),
        sort_name: "Restored Track Identity".to_owned(),
        artist_type: ArtistType::Unknown,
        disambiguation: Some("selective split fixture".to_owned()),
        active_dates: None,
        status: ArtistStatus::Verified,
        created_at: now + Duration::seconds(3),
    })
    .expect("valid split Artist");
    let invalid_new_artist = test_artist(
        "Invalid Split Destination",
        ArtistType::Unknown,
        now + Duration::seconds(3),
    );
    let invalid_split = ArtistSplit {
        id: ArtistSplitId::new(),
        source_artist_id: peer_artist_id,
        new_artist_id: invalid_new_artist.id(),
        assignments: vec![SplitAssignment::TrackArtist(TrackArtistId::from_uuid(
            source.0,
        ))],
        actor: "catalog-admin".to_owned(),
        reason: "assignment is not currently owned by the claimed source".to_owned(),
        occurred_at: now + Duration::seconds(3),
    };
    assert!(matches!(
        store
            .split_artist(
                &db.pool,
                &invalid_split,
                &invalid_new_artist,
                "invalid-selective-split-v1",
            )
            .await,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    let rejected_split_state: (i64, i64, Uuid) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM artists WHERE id=$1),
            (SELECT count(*) FROM artist_split_events WHERE id=$2),
            (SELECT artist_id FROM track_artists WHERE id=$3)",
    )
    .bind(invalid_new_artist.id().into_uuid())
    .bind(invalid_split.id.into_uuid())
    .bind(source.0)
    .fetch_one(&db.pool)
    .await
    .expect("invalid split is rolled back atomically");
    assert_eq!(rejected_split_state, (0, 0, target.1));
    let split = ArtistSplit {
        id: ArtistSplitId::new(),
        source_artist_id: ArtistId::from_uuid(target.1),
        new_artist_id,
        assignments: vec![
            SplitAssignment::TrackArtist(TrackArtistId::from_uuid(source.0)),
            SplitAssignment::Alias(selected_alias),
            SplitAssignment::ExternalIdentity(selected_external),
        ],
        actor: "catalog-admin".to_owned(),
        reason: "move only the selected Track observation and evidence".to_owned(),
        occurred_at: now + Duration::seconds(3),
    };
    store
        .split_artist(
            &db.pool,
            &split,
            &new_artist,
            "manual-identity-correction-v2",
        )
        .await
        .expect("selectively split Artist assignments");

    let owners: (Uuid, Uuid, Uuid, Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT
            (SELECT artist_id FROM release_artists WHERE id=$1),
            (SELECT artist_id FROM track_artists WHERE id=$2),
            (SELECT artist_id FROM artist_aliases WHERE id=$3),
            (SELECT artist_id FROM artist_aliases WHERE id=$4),
            (SELECT artist_id FROM artist_external_identities WHERE id=$5),
            (SELECT artist_id FROM artist_external_identities WHERE id=$6)",
    )
    .bind(target.0)
    .bind(source.0)
    .bind(selected_alias.into_uuid())
    .bind(retained_alias.into_uuid())
    .bind(selected_external.into_uuid())
    .bind(retained_external.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query selective split owners");
    assert_eq!(
        owners,
        (
            target.1,
            new_artist_id.into_uuid(),
            new_artist_id.into_uuid(),
            target.1,
            new_artist_id.into_uuid(),
            target.1,
        ),
        "only explicitly selected assignments move"
    );
    let public_ids: BTreeSet<Uuid> =
        sqlx::query_scalar("SELECT public_api_id FROM artists WHERE id = ANY($1)")
            .bind([source.1, target.1, new_artist_id.into_uuid()])
            .fetch_all(&db.pool)
            .await
            .expect("query all preserved public identities")
            .into_iter()
            .collect();
    assert_eq!(
        public_ids,
        [source.2, target.2, new_public_id.into_uuid()]
            .into_iter()
            .collect()
    );

    let overlay_chain: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id,predecessor_overlay_id FROM catalog_semantic_overlays
         WHERE release_id=$1 ORDER BY created_at,id",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("query correction overlay chain");
    assert_eq!(overlay_chain.len(), 3);
    assert_eq!(overlay_chain[0], (initial_overlay, None));
    assert_eq!(
        overlay_chain[1],
        (post_merge_overlay, Some(initial_overlay))
    );
    assert_eq!(overlay_chain[2].1, Some(post_merge_overlay));
    let correction_events: (i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM artist_merge_events WHERE id=$1),
            (SELECT count(*) FROM artist_split_events WHERE id=$2)",
    )
    .bind(merge.id.into_uuid())
    .bind(split.id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query immutable correction events");
    assert_eq!(correction_events, (1, 1));

    let work_after = WorkItemStore
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-correction work item");
    let manifest_after = WorkItemStore
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-correction v3")
        .expect("v3 remains");
    let publication_after = WorkItemStore
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-correction publication")
        .expect("publication remains");
    let media_after: Vec<(Uuid, String, String, i64)> = sqlx::query_as(
        "SELECT id,canonical_relative_path,byte_hash,byte_size FROM media_assets ORDER BY id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("query canonical media after corrections");
    let track_paths_after: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id,source_path FROM tracks ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .expect("query Track paths after corrections");
    assert_eq!(media_after, media_before);
    assert_eq!(track_paths_after, track_paths_before);
    assert_eq!(manifest_after.id, manifest_before.id);
    assert_eq!(manifest_after.work_item_id, manifest_before.work_item_id);
    assert_eq!(
        manifest_after.schema_version,
        manifest_before.schema_version
    );
    assert_eq!(manifest_after.manifest_hash, manifest_before.manifest_hash);
    assert_eq!(manifest_after.payload, manifest_before.payload);
    assert_eq!(
        manifest_after.source_snapshot_id,
        manifest_before.source_snapshot_id
    );
    assert_eq!(
        manifest_after.source_signature,
        manifest_before.source_signature
    );
    assert_eq!(manifest_after.generated_at, manifest_before.generated_at);
    assert_eq!(publication_after, publication_before);
    assert_eq!(work_after.state, work_before.state);
    assert_eq!(work_after.source_path, work_before.source_path);
    assert_eq!(work_after.current_path, work_before.current_path);
    assert_eq!(work_after.target_path, work_before.target_path);

    db.drop().await;
}

#[tokio::test]
async fn legacy_only_merge_is_audited_without_overlay_but_missing_import_fails_closed() {
    let db = TestDb::create("catalog_legacy_only_corrections").await;
    let store = CatalogStore;
    let now = OffsetDateTime::now_utc();

    let legacy_source = test_artist("Legacy Source", ArtistType::Unknown, now);
    let legacy_target = test_artist("Legacy Target", ArtistType::Unknown, now);
    store
        .create_artist(&db.pool, &legacy_source)
        .await
        .expect("create legacy source");
    store
        .create_artist(&db.pool, &legacy_target)
        .await
        .expect("create legacy target");
    let legacy_release = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO releases (
            id,public_api_id,title,normalized_title,sort_title,display_artist,
            created_at,updated_at
         ) VALUES ($1,$1,'Legacy-only Release','LEGACYONLYRELEASE',
                   'Legacy-only Release','Legacy Source',$2,$2)",
    )
    .bind(legacy_release)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert legacy-only Release");
    let legacy_billing = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO release_artists (
            id,release_id,artist_id,role,presentation_order,display_value,join_phrase,created_at
         ) VALUES ($1,$2,$3,'primary',1,'Legacy Source','',$4)",
    )
    .bind(legacy_billing)
    .bind(legacy_release)
    .bind(legacy_source.id().into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert legacy-only billing");
    let legacy_merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: legacy_source.id(),
        source_public_api_id: legacy_source.public_api_id(),
        target_artist_id: legacy_target.id(),
        actor: "migration-admin".to_owned(),
        reason: "reconcile legacy-only duplicate".to_owned(),
        occurred_at: now + Duration::seconds(1),
    };
    store
        .merge_artist(&db.pool, &legacy_merge, "legacy-correction-v1")
        .await
        .expect("legacy-only merge emits revision evidence without synthetic overlay");
    let legacy_result: (Uuid, i64, Option<Uuid>, i64, i64, bool) = sqlx::query_as(
        "SELECT
            (SELECT artist_id FROM release_artists WHERE id=$1),
            release.revision,release.current_overlay_id,
            (SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=release.id),
            (SELECT count(*) FROM catalog_projection_outbox
             WHERE entity_type='release' AND entity_id=release.id AND entity_revision=2),
            (SELECT (payload->>'legacy_only')::boolean FROM catalog_audit_events
             WHERE entity_type='release' AND entity_id=release.id
             ORDER BY created_at DESC,event_id DESC LIMIT 1)
         FROM releases release WHERE id=$2",
    )
    .bind(legacy_billing)
    .bind(legacy_release)
    .fetch_one(&db.pool)
    .await
    .expect("query legacy-only correction evidence");
    assert_eq!(
        legacy_result,
        (legacy_target.id().into_uuid(), 2, None, 0, 1, true)
    );

    let published = seed_published_release(&db, 92, CatalogFixturePresentation::default()).await;
    let published_source = test_artist("Published Source", ArtistType::Unknown, now);
    let published_target = test_artist("Published Target", ArtistType::Unknown, now);
    store
        .create_artist(&db.pool, &published_source)
        .await
        .expect("create published source");
    store
        .create_artist(&db.pool, &published_target)
        .await
        .expect("create published target");
    sqlx::query(
        "INSERT INTO releases (
            id,public_api_id,title,normalized_title,sort_title,display_artist,
            created_at,updated_at
         ) VALUES ($1,$1,'Published without import','PUBLISHEDWITHOUTIMPORT',
                   'Published without import','Published Source',$2,$2)",
    )
    .bind(published.release_id.into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert catalog row missing import");
    let published_billing = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO release_artists (
            id,release_id,artist_id,role,presentation_order,display_value,join_phrase,created_at
         ) VALUES ($1,$2,$3,'primary',1,'Published Source','',$4)",
    )
    .bind(published_billing)
    .bind(published.release_id.into_uuid())
    .bind(published_source.id().into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert published billing missing import");
    let rejected_merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: published_source.id(),
        source_public_api_id: published_source.public_api_id(),
        target_artist_id: published_target.id(),
        actor: "catalog-admin".to_owned(),
        reason: "must not correct a missing catalog import".to_owned(),
        occurred_at: now + Duration::seconds(2),
    };
    assert!(matches!(
        store
            .merge_artist(&db.pool, &rejected_merge, "invalid-missing-import-v1")
            .await,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    let rejected_state: (String, Option<Uuid>, Uuid, i64, i64) = sqlx::query_as(
        "SELECT artist.status,artist.merged_into_artist_id,
                (SELECT artist_id FROM release_artists WHERE id=$2),
                release.revision,
                (SELECT count(*) FROM artist_merge_events WHERE id=$3)
         FROM artists artist CROSS JOIN releases release
         WHERE artist.id=$1 AND release.id=$4",
    )
    .bind(published_source.id().into_uuid())
    .bind(published_billing)
    .bind(rejected_merge.id.into_uuid())
    .bind(published.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("missing-import conflict rolled back atomically");
    assert_eq!(
        rejected_state,
        (
            "verified".to_owned(),
            None,
            published_source.id().into_uuid(),
            1,
            0,
        )
    );

    let cycle_source = test_artist("Cycle Source", ArtistType::Unknown, now);
    let cycle_target = test_artist("Cycle Target", ArtistType::Unknown, now);
    store
        .create_artist(&db.pool, &cycle_source)
        .await
        .expect("create cycle source");
    store
        .create_artist(&db.pool, &cycle_target)
        .await
        .expect("create cycle target");
    store
        .add_artist_relationship(
            &db.pool,
            &ArtistRelationship {
                id: ArtistRelationshipId::new(),
                subject_artist_id: cycle_source.id(),
                object_artist_id: cycle_target.id(),
                kind: ArtistRelationshipKind::new("associated_with".to_owned())
                    .expect("valid relationship"),
                active_dates: None,
                confidence: Confidence::new(10_000).expect("valid confidence"),
                evidence_source: "cycle-fixture".to_owned(),
            },
            now,
        )
        .await
        .expect("seed merge self-link conflict");
    assert!(
        store
            .merge_artist(
                &db.pool,
                &ArtistMerge {
                    id: ArtistMergeId::new(),
                    source_artist_id: cycle_source.id(),
                    source_public_api_id: cycle_source.public_api_id(),
                    target_artist_id: cycle_target.id(),
                    actor: "catalog-admin".to_owned(),
                    reason: "would collapse a relationship to self".to_owned(),
                    occurred_at: now + Duration::seconds(3),
                },
                "cycle-check-v1",
            )
            .await
            .is_err(),
        "merge rejects a relationship/member self-cycle"
    );

    db.drop().await;
}

#[tokio::test]
async fn batch_merge_coalesces_user_provider_image_and_discovery_data() {
    let db = TestDb::create("catalog_artist_merge_complete_parity").await;
    let store = CatalogStore;
    let now = OffsetDateTime::now_utc();
    let target = test_artist("Canonical Artist", ArtistType::Person, now);
    let source_one = test_artist("First Duplicate", ArtistType::Unknown, now);
    let source_two = test_artist("Second Duplicate", ArtistType::Group, now);
    for artist in [&target, &source_one, &source_two] {
        store
            .create_artist(&db.pool, artist)
            .await
            .expect("create Artist merge fixture");
    }

    let user_one = insert_native_user(&db, "merge-user-one", now).await;
    let user_two = insert_native_user(&db, "merge-user-two", now).await;
    for (user_id, artist_id, starred, hated, rating, pinned, play_count) in [
        (user_one, target.id(), false, false, 2_i16, false, 10_i64),
        (user_one, source_one.id(), true, true, 4, false, 5),
        (user_one, source_two.id(), false, false, 3, true, 7),
        (user_two, source_two.id(), true, false, 5, true, 11),
    ] {
        sqlx::query(
            "INSERT INTO native_artist_user_states (
                 user_id,artist_id,starred,hated,rating,pinned,play_count,
                 last_played_at,created_at,updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$8)",
        )
        .bind(user_id)
        .bind(artist_id.into_uuid())
        .bind(starred)
        .bind(hated)
        .bind(rating)
        .bind(pinned)
        .bind(play_count)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("seed Artist user state");
    }

    for (artist_id, bytes) in [
        (source_one.id(), vec![0x89_u8, 0x50, 0x4e, 0x47, 1]),
        (source_two.id(), vec![0x89_u8, 0x50, 0x4e, 0x47, 2]),
    ] {
        sqlx::query(
            "INSERT INTO catalog_entity_images (
                 entity_type,artist_id,image_mime_type,asset_key,uploaded_by,created_at,updated_at
             ) VALUES ('artist',$1,'image/png',$2,'merge-test',$3,$3)",
        )
        .bind(artist_id.into_uuid())
        .bind(
            melodee_persistence::asset_files::put(&db.pool, &bytes)
                .await
                .expect("store fixture"),
        )
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("seed operator image");
    }

    for (artist_id, provider, confidence, marker) in [
        (target.id(), "spotify", 5_000, 1_u8),
        (source_one.id(), "spotify", 9_000, 2_u8),
        (source_two.id(), "deezer", 8_000, 3_u8),
    ] {
        sqlx::query(
            "INSERT INTO catalog_provider_images (
                 id,entity_type,entity_id,provider,source_url,image_mime_type,
                 asset_key,confidence,observed_at
             ) VALUES ($1,'artist',$2,$3,$4,'image/png',$5,$6,$7)",
        )
        .bind(Uuid::now_v7())
        .bind(artist_id.into_uuid())
        .bind(provider)
        .bind(format!("https://example.test/{provider}/{marker}"))
        .bind(
            melodee_persistence::asset_files::put(&db.pool, &[0x89, 0x50, 0x4e, 0x47, marker])
                .await
                .expect("store fixture"),
        )
        .bind(confidence)
        .bind(now + Duration::seconds(i64::from(marker)))
        .execute(&db.pool)
        .await
        .expect("seed provider image");
    }

    for (artist_id, provider, external_id, query) in [
        (target.id(), "spotify", "shared", "duplicate"),
        (source_one.id(), "spotify", "shared", "duplicate"),
        (source_one.id(), "wikidata", "Q123", "first"),
        (source_two.id(), "deezer", "456", "second"),
    ] {
        sqlx::query(
            "INSERT INTO provider_identity_candidates (
                 id,artist_id,provider,external_id,query,candidate_rank,
                 confidence,payload,observed_at
             ) VALUES ($1,$2,$3,$4,$5,1,8000,'{}'::jsonb,$6)",
        )
        .bind(Uuid::now_v7())
        .bind(artist_id.into_uuid())
        .bind(provider)
        .bind(external_id)
        .bind(query)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("seed provider identity candidate");
    }

    for (artist_id, provider, request_key) in [
        (target.id(), "spotify", "shared-task"),
        (source_one.id(), "spotify", "shared-task"),
        (source_one.id(), "wikidata", "source-only-task"),
    ] {
        sqlx::query(
            "INSERT INTO provider_enrichment_tasks (
                 id,entity_type,entity_id,provider,purpose,request_key,status,
                 attempt_count,next_attempt_at,created_at,updated_at
             ) VALUES ($1,'artist',$2,$3,'artist_metadata',$4,'queued',0,$5,$5,$5)",
        )
        .bind(Uuid::now_v7())
        .bind(artist_id.into_uuid())
        .bind(provider)
        .bind(request_key)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("seed provider enrichment task");
    }

    let chart_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_charts (
             id,public_api_id,slug,title,created_at,updated_at
         ) VALUES ($1,$2,'merge-parity','Merge parity',$3,$3)",
    )
    .bind(chart_id)
    .bind(Uuid::now_v7())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed native chart");
    sqlx::query(
        "INSERT INTO native_chart_items (
             chart_id,rank,artist_name,release_title,linked_artist_id,created_at
         ) VALUES ($1,1,'Second Duplicate','Fixture Release',$2,$3)",
    )
    .bind(chart_id)
    .bind(source_two.id().into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed chart Artist link");

    let preview = store
        .preview_artist_merge(&db.pool, &[source_one.id(), source_two.id()], target.id())
        .await
        .expect("preview complete Artist merge");
    assert_eq!(
        preview.target.public_api_id,
        target.public_api_id().into_uuid()
    );
    assert_eq!(preview.sources.len(), 2);
    assert_eq!(preview.impact.user_states, 3);
    assert_eq!(preview.impact.user_state_conflicts, 1);
    assert_eq!(preview.impact.operator_images, 2);
    assert_eq!(preview.impact.provider_images, 2);
    assert_eq!(preview.impact.provider_image_conflicts, 1);
    assert_eq!(preview.impact.provider_candidates, 3);
    assert_eq!(preview.impact.provider_tasks, 2);
    assert_eq!(preview.impact.chart_links, 1);
    assert!(
        preview
            .metadata_conflicts
            .contains(&"canonicalName".to_owned())
    );
    assert!(
        preview
            .metadata_conflicts
            .contains(&"artistType".to_owned())
    );

    let merges = [&source_one, &source_two]
        .into_iter()
        .map(|source| ArtistMerge {
            id: ArtistMergeId::new(),
            source_artist_id: source.id(),
            source_public_api_id: source.public_api_id(),
            target_artist_id: target.id(),
            actor: "catalog-admin".to_owned(),
            reason: "C# parity complete-data merge".to_owned(),
            occurred_at: now + Duration::seconds(10),
        })
        .collect::<Vec<_>>();
    store
        .merge_artists(&db.pool, &merges, "admin-batch-merge-v1")
        .await
        .expect("apply complete-data batch merge");

    let statuses: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='verified'),
                count(*) FILTER (WHERE status='merged')
           FROM artists WHERE id=ANY($1)",
    )
    .bind([
        target.id().into_uuid(),
        source_one.id().into_uuid(),
        source_two.id().into_uuid(),
    ])
    .fetch_one(&db.pool)
    .await
    .expect("read merged Artist statuses");
    assert_eq!(statuses, (1, 2));

    let folded: (bool, bool, i16, bool, i64) = sqlx::query_as(
        "SELECT starred,hated,rating,pinned,play_count
           FROM native_artist_user_states WHERE user_id=$1 AND artist_id=$2",
    )
    .bind(user_one)
    .bind(target.id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read folded user state");
    assert_eq!(folded, (true, true, 4, true, 22));
    let state_counts: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE artist_id=$1),
                count(*) FILTER (WHERE artist_id=ANY($2))
           FROM native_artist_user_states",
    )
    .bind(target.id().into_uuid())
    .bind([source_one.id().into_uuid(), source_two.id().into_uuid()])
    .fetch_one(&db.pool)
    .await
    .expect("count coalesced user states");
    assert_eq!(state_counts, (2, 0));

    let image_counts: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE artist_id=$1),
                count(*) FILTER (WHERE artist_id=ANY($2))
           FROM catalog_entity_images WHERE entity_type='artist'",
    )
    .bind(target.id().into_uuid())
    .bind([source_one.id().into_uuid(), source_two.id().into_uuid()])
    .fetch_one(&db.pool)
    .await
    .expect("count target and recoverable source images");
    assert_eq!(image_counts, (1, 1));
    let provider_state: (i64, i32, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM catalog_provider_images
              WHERE entity_type='artist' AND entity_id=$1),
            (SELECT confidence FROM catalog_provider_images
              WHERE entity_type='artist' AND entity_id=$1 AND provider='spotify'),
            (SELECT count(*) FROM provider_identity_candidates WHERE artist_id=$1),
            (SELECT count(*) FROM provider_enrichment_tasks WHERE entity_id=$1),
            (SELECT count(*) FROM provider_enrichment_tasks
              WHERE entity_id=ANY($2) AND status='failed'
                AND last_error_category='artist_merged')",
    )
    .bind(target.id().into_uuid())
    .bind([source_one.id().into_uuid(), source_two.id().into_uuid()])
    .fetch_one(&db.pool)
    .await
    .expect("read coalesced provider state");
    assert_eq!(provider_state, (2, 9_000, 3, 2, 1));
    let chart_owner: Uuid =
        sqlx::query_scalar("SELECT linked_artist_id FROM native_chart_items WHERE chart_id=$1")
            .bind(chart_id)
            .fetch_one(&db.pool)
            .await
            .expect("read retargeted chart link");
    assert_eq!(chart_owner, target.id().into_uuid());
    let merge_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artist_merge_events
          WHERE source_artist_id=ANY($1) AND target_artist_id=$2
            AND moved_relationships ? 'coalesced'",
    )
    .bind([source_one.id().into_uuid(), source_two.id().into_uuid()])
    .bind(target.id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("count complete-data merge events");
    assert_eq!(merge_events, 2);

    db.drop().await;
}

#[tokio::test]
async fn batch_merge_rolls_back_every_source_when_a_late_source_conflicts() {
    let db = TestDb::create("catalog_artist_merge_batch_rollback").await;
    let store = CatalogStore;
    let now = OffsetDateTime::now_utc();
    let target = test_artist("Batch Target", ArtistType::Person, now);
    let first = test_artist("Batch First", ArtistType::Person, now);
    let second = test_artist("Batch Second", ArtistType::Person, now);
    for artist in [&target, &first, &second] {
        store
            .create_artist(&db.pool, artist)
            .await
            .expect("create rollback Artist fixture");
    }
    let (early, blocked) = if first.id().into_uuid() < second.id().into_uuid() {
        (&first, &second)
    } else {
        (&second, &first)
    };
    store
        .add_artist_relationship(
            &db.pool,
            &ArtistRelationship {
                id: ArtistRelationshipId::new(),
                subject_artist_id: blocked.id(),
                object_artist_id: target.id(),
                kind: ArtistRelationshipKind::new("associated_with".to_owned())
                    .expect("valid relationship kind"),
                active_dates: None,
                confidence: Confidence::new(10_000).expect("valid confidence"),
                evidence_source: "batch-rollback".to_owned(),
            },
            now,
        )
        .await
        .expect("seed late batch conflict");
    let merges = [early, blocked]
        .into_iter()
        .map(|source| ArtistMerge {
            id: ArtistMergeId::new(),
            source_artist_id: source.id(),
            source_public_api_id: source.public_api_id(),
            target_artist_id: target.id(),
            actor: "catalog-admin".to_owned(),
            reason: "prove all-or-nothing batch".to_owned(),
            occurred_at: now + Duration::seconds(1),
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        store
            .merge_artists(&db.pool, &merges, "admin-batch-rollback-v1")
            .await,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    let state: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='verified'),
                (SELECT count(*) FROM artist_merge_events WHERE source_artist_id=ANY($1))
           FROM artists WHERE id=ANY($1)",
    )
    .bind([first.id().into_uuid(), second.id().into_uuid()])
    .fetch_one(&db.pool)
    .await
    .expect("read rolled-back batch state");
    assert_eq!(state, (2, 0));

    db.drop().await;
}

#[tokio::test]
async fn batch_merge_enforces_the_aggregate_release_bound_before_writing() {
    let db = TestDb::create("catalog_artist_merge_batch_release_bound").await;
    let store = CatalogStore;
    let now = OffsetDateTime::now_utc();
    let target = test_artist("Bounded Batch Target", ArtistType::Person, now);
    let source_one = test_artist("Bounded Batch One", ArtistType::Person, now);
    let source_two = test_artist("Bounded Batch Two", ArtistType::Person, now);
    for artist in [&target, &source_one, &source_two] {
        store
            .create_artist(&db.pool, artist)
            .await
            .expect("create aggregate bound Artist fixture");
    }

    let scope_size = MAX_CATALOG_CORRECTION_RELEASES + 1;
    let release_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let credit_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let source_ids = (0..scope_size)
        .map(|index| {
            if index % 2 == 0 {
                source_one.id().into_uuid()
            } else {
                source_two.id().into_uuid()
            }
        })
        .collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO releases (
             id,public_api_id,title,normalized_title,sort_title,display_artist,
             created_at,updated_at
         )
         SELECT release_id,release_id,'Bounded Batch','BOUNDEDBATCH',
                'Bounded Batch','Bounded Batch Artist',$2,$2
           FROM unnest($1::uuid[]) release_id",
    )
    .bind(&release_ids)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("set-wise seed aggregate-bound Releases");
    sqlx::query(
        "INSERT INTO release_artists (
             id,release_id,artist_id,observed_artist_id,role,presentation_order,
             display_value,join_phrase,created_at
         )
         SELECT fixture.credit_id,fixture.release_id,fixture.artist_id,NULL,
                'primary',1,'Bounded Batch Artist','',$4
           FROM unnest($1::uuid[],$2::uuid[],$3::uuid[])
                fixture(credit_id,release_id,artist_id)",
    )
    .bind(&credit_ids)
    .bind(&release_ids)
    .bind(&source_ids)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("set-wise attach aggregate-bound Artist credits");

    let merges = [&source_one, &source_two]
        .into_iter()
        .map(|source| ArtistMerge {
            id: ArtistMergeId::new(),
            source_artist_id: source.id(),
            source_public_api_id: source.public_api_id(),
            target_artist_id: target.id(),
            actor: "catalog-admin".to_owned(),
            reason: "prove aggregate batch correction bound".to_owned(),
            occurred_at: now + Duration::seconds(1),
        })
        .collect::<Vec<_>>();
    match store
        .merge_artists(&db.pool, &merges, "admin-batch-bound-v1")
        .await
    {
        Err(CatalogStoreError::CorrectionScopeExceeded { dimension, limit }) => {
            assert_eq!(dimension, CatalogCorrectionDimension::Releases);
            assert_eq!(limit, MAX_CATALOG_CORRECTION_RELEASES);
        }
        other => panic!("expected aggregate Release correction bound, got {other:?}"),
    }
    let unchanged: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='verified'),
                (SELECT count(*) FROM artist_merge_events WHERE source_artist_id=ANY($1)),
                (SELECT count(*) FROM release_artists WHERE artist_id=ANY($1))
           FROM artists WHERE id=ANY($1)",
    )
    .bind([source_one.id().into_uuid(), source_two.id().into_uuid()])
    .fetch_one(&db.pool)
    .await
    .expect("read aggregate-bound rollback state");
    assert_eq!(
        unchanged,
        (
            2,
            0,
            i64::try_from(scope_size).expect("scope size fits i64"),
        )
    );

    db.drop().await;
}

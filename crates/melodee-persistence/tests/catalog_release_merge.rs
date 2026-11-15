//! Multi-source Release merge: track, billing, artwork, and reversibility proofs.
//!
//! These tests exercise the administrator-facing Release merge described by
//! ADR-0198: several source Releases selected from any Artist fold into one
//! target, its Tracks/observations/contributions and artwork move, the
//! surviving Release keeps the target's Release artist, and the recorded merge
//! event lets a split restore exactly what moved.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, PublishedCatalogFixture, seed_published_release};
use common::{TestDb, test_config};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, CatalogStoreError, ReleaseMerge, WorkerId,
};
use time::OffsetDateTime;
use uuid::Uuid;

struct SeededRelease {
    internal_id: Uuid,
    public_id: Uuid,
}

async fn import_release(
    db: &TestDb,
    store: &CatalogStore,
    worker: &WorkerId,
    fixture: &PublishedCatalogFixture,
) {
    let now = OffsetDateTime::now_utc();
    store
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            worker,
            &CatalogImportRequest::new("release-merge-admin-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import Release fixture");
}

async fn seed_release(
    db: &TestDb,
    store: &CatalogStore,
    ordinal: u32,
    title: &str,
    artist: &str,
) -> SeededRelease {
    let fixture = seed_published_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            release_artist: artist,
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let worker = WorkerId::new();
    let claimed = store
        .claim_cataloging(&db.pool, &worker, &test_config(), OffsetDateTime::now_utc())
        .await
        .expect("claim Release fixture");
    assert_eq!(claimed.len(), 1);
    import_release(db, store, &worker, &fixture).await;
    let public_id: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read seeded Release public id");
    SeededRelease {
        internal_id: fixture.release_id.into_uuid(),
        public_id,
    }
}

fn merge(source: &SeededRelease, target: &SeededRelease) -> ReleaseMerge {
    ReleaseMerge {
        source_release_id: source.internal_id,
        target_release_id: target.internal_id,
        actor: "release-merge-admin".to_owned(),
        reason: "administrator confirmed duplicate Release".to_owned(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

#[tokio::test]
async fn batch_merge_folds_every_source_and_keeps_the_target_release_artist() {
    let db = TestDb::create("catalog_release_batch_merge").await;
    let store = CatalogStore;
    let first = seed_release(&db, &store, 30, "Merge Source One", "Wrong Artist").await;
    let second = seed_release(&db, &store, 31, "Merge Source Two", "Other Wrong Artist").await;
    let target = seed_release(&db, &store, 32, "Canonical Release", "Right Artist").await;

    let target_billing_before: Vec<Uuid> = sqlx::query_scalar(
        "SELECT artist_id FROM release_artists WHERE release_id=$1 ORDER BY presentation_order",
    )
    .bind(target.internal_id)
    .fetch_all(&db.pool)
    .await
    .expect("read target Release billing");

    let preview = store
        .preview_release_merge(
            &db.pool,
            &[first.internal_id, second.internal_id],
            target.internal_id,
        )
        .await
        .expect("preview multi-source Release merge");
    assert_eq!(preview.sources.len(), 2);
    assert_eq!(preview.target.public_api_id, target.public_id);
    assert_eq!(preview.impact.moved_tracks, 2);
    assert_eq!(preview.target_track_count_after, 3);
    assert!(
        preview
            .metadata_conflicts
            .contains(&"releaseArtist".to_owned()),
        "the sources bill different Artists than the target: {:?}",
        preview.metadata_conflicts
    );
    assert!(preview.metadata_conflicts.contains(&"title".to_owned()));

    let event_ids = store
        .merge_releases(&db.pool, &[merge(&first, &target), merge(&second, &target)])
        .await
        .expect("apply multi-source Release merge");
    assert_eq!(event_ids.len(), 2);

    let statuses: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged')
           FROM releases WHERE id=ANY($1)",
    )
    .bind([first.internal_id, second.internal_id, target.internal_id])
    .fetch_one(&db.pool)
    .await
    .expect("read merged Release statuses");
    assert_eq!(statuses, (1, 2));

    let merged_into: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, merged_into_release_id FROM releases WHERE id=ANY($1)")
            .bind([first.internal_id, second.internal_id])
            .fetch_all(&db.pool)
            .await
            .expect("read tombstones");
    assert!(
        merged_into
            .iter()
            .all(|(_, into)| *into == Some(target.internal_id))
    );

    let ordinals: Vec<i32> =
        sqlx::query_scalar("SELECT ordinal FROM tracks WHERE release_id=$1 ORDER BY ordinal")
            .bind(target.internal_id)
            .fetch_all(&db.pool)
            .await
            .expect("read surviving Track ordinals");
    assert_eq!(ordinals, vec![1, 2, 3]);

    // The surviving Release is credited to the target's Artist: the merge
    // never rewrites Release-level billing to a source Artist.
    let target_billing_after: Vec<Uuid> = sqlx::query_scalar(
        "SELECT artist_id FROM release_artists WHERE release_id=$1 ORDER BY presentation_order",
    )
    .bind(target.internal_id)
    .fetch_all(&db.pool)
    .await
    .expect("read surviving Release billing");
    assert_eq!(target_billing_after, target_billing_before);
    let source_billing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_artists WHERE release_id=ANY($1)")
            .bind([first.internal_id, second.internal_id])
            .fetch_one(&db.pool)
            .await
            .expect("count source Release billing evidence");
    assert_eq!(source_billing, 2);

    let audit_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_audit_events
          WHERE entity_id = ANY($1) AND event_type IN ('release_merged','release_merged_into')",
    )
    .bind([first.internal_id, second.internal_id, target.internal_id])
    .fetch_one(&db.pool)
    .await
    .expect("read Release merge audit events");
    assert_eq!(audit_events, 4);

    db.drop().await;
}

#[tokio::test]
async fn batch_merge_adopts_missing_artwork_and_split_restores_it() {
    let db = TestDb::create("catalog_release_merge_artwork").await;
    let store = CatalogStore;
    let source = seed_release(&db, &store, 40, "Artwork Source", "Artist A").await;
    let target = seed_release(&db, &store, 41, "Artwork Target", "Artist B").await;
    let now = OffsetDateTime::now_utc();

    sqlx::query(
        "INSERT INTO catalog_entity_images (
             entity_type,release_id,image_mime_type,image_bytes,uploaded_by,created_at,updated_at
         ) VALUES ('release',$1,'image/png',$2,'merge-test',$3,$3)",
    )
    .bind(source.internal_id)
    .bind(vec![0x89_u8, 0x50, 0x4e, 0x47, 1])
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed source operator image");
    for (release_id, provider, marker, confidence) in [
        (source.internal_id, "spotify", 2_u8, 9_000_i32),
        (target.internal_id, "deezer", 3_u8, 5_000),
    ] {
        sqlx::query(
            "INSERT INTO catalog_provider_images (
                 id,entity_type,entity_id,provider,source_url,image_mime_type,
                 image_bytes,confidence,observed_at
             ) VALUES ($1,'release',$2,$3,$4,'image/png',$5,$6,$7)",
        )
        .bind(Uuid::now_v7())
        .bind(release_id)
        .bind(provider)
        .bind(format!("https://example.test/{provider}/{marker}"))
        .bind(vec![0x89, 0x50, 0x4e, 0x47, marker])
        .bind(confidence)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("seed provider image");
    }

    let preview = store
        .preview_release_merge(&db.pool, &[source.internal_id], target.internal_id)
        .await
        .expect("preview artwork adoption");
    assert_eq!(preview.impact.operator_images_adopted, 1);
    assert_eq!(preview.impact.provider_images_adopted, 1);
    assert_eq!(preview.impact.provider_image_conflicts, 0);

    let event_id = store
        .merge_releases(&db.pool, &[merge(&source, &target)])
        .await
        .expect("merge Release artwork")[0]
        .merge_event_id;

    let operator_owner: Uuid = sqlx::query_scalar(
        "SELECT release_id FROM catalog_entity_images WHERE entity_type='release'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read adopted operator image owner");
    assert_eq!(operator_owner, target.internal_id);
    let providers: Vec<String> = sqlx::query_scalar(
        "SELECT provider FROM catalog_provider_images
          WHERE entity_type='release' AND entity_id=$1 ORDER BY provider",
    )
    .bind(target.internal_id)
    .fetch_all(&db.pool)
    .await
    .expect("read adopted provider artwork");
    assert_eq!(providers, vec!["deezer".to_owned(), "spotify".to_owned()]);

    store
        .split_release_merge(
            &db.pool,
            event_id,
            "release-merge-admin",
            "split after review",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("split Release merge");

    let restored_owner: Uuid = sqlx::query_scalar(
        "SELECT release_id FROM catalog_entity_images WHERE entity_type='release'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read restored operator image owner");
    assert_eq!(restored_owner, source.internal_id);
    let source_providers: Vec<String> = sqlx::query_scalar(
        "SELECT provider FROM catalog_provider_images
          WHERE entity_type='release' AND entity_id=$1 ORDER BY provider",
    )
    .bind(source.internal_id)
    .fetch_all(&db.pool)
    .await
    .expect("read restored source provider artwork");
    assert_eq!(source_providers, vec!["spotify".to_owned()]);
    let target_providers: Vec<String> = sqlx::query_scalar(
        "SELECT provider FROM catalog_provider_images
          WHERE entity_type='release' AND entity_id=$1",
    )
    .bind(target.internal_id)
    .fetch_all(&db.pool)
    .await
    .expect("read target provider artwork after split");
    assert_eq!(target_providers, vec!["deezer".to_owned()]);

    db.drop().await;
}

#[tokio::test]
async fn split_keeps_a_replaced_target_cover_after_an_adoption() {
    let db = TestDb::create("catalog_release_merge_cover_guard").await;
    let store = CatalogStore;
    let source = seed_release(&db, &store, 50, "Cover Source", "Artist C").await;
    let target = seed_release(&db, &store, 51, "Cover Target", "Artist D").await;
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO catalog_entity_images (
             entity_type,release_id,image_mime_type,image_bytes,uploaded_by,created_at,updated_at
         ) VALUES ('release',$1,'image/png',$2,'merge-test',$3,$3)",
    )
    .bind(source.internal_id)
    .bind(vec![0x89_u8, 0x50, 0x4e, 0x47, 7])
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed source operator image");

    let event_id = store
        .merge_releases(&db.pool, &[merge(&source, &target)])
        .await
        .expect("merge Release cover")[0]
        .merge_event_id;

    // An operator upload after the merge is newer curation and must win.
    sqlx::query(
        "UPDATE catalog_entity_images SET image_bytes=$2, uploaded_by='operator', updated_at=$3
          WHERE entity_type='release' AND release_id=$1",
    )
    .bind(target.internal_id)
    .bind(vec![0x89_u8, 0x50, 0x4e, 0x47, 9])
    .bind(now + time::Duration::seconds(5))
    .execute(&db.pool)
    .await
    .expect("replace target cover after merge");

    store
        .split_release_merge(
            &db.pool,
            event_id,
            "release-merge-admin",
            "split after review",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("split Release merge");

    let owner: Uuid = sqlx::query_scalar(
        "SELECT release_id FROM catalog_entity_images WHERE entity_type='release'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read cover owner after split");
    assert_eq!(owner, target.internal_id);
    let uploader: String = sqlx::query_scalar(
        "SELECT uploaded_by FROM catalog_entity_images WHERE entity_type='release'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read cover uploader after split");
    assert_eq!(uploader, "operator");

    db.drop().await;
}

#[tokio::test]
async fn merge_detaches_source_billing_observation_links_and_split_restores_them() {
    let db = TestDb::create("catalog_release_merge_billing").await;
    let store = CatalogStore;
    let source = seed_release(&db, &store, 60, "Billing Source", "Artist E").await;
    let target = seed_release(&db, &store, 61, "Billing Target", "Artist F").await;

    let observation_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM observed_artists
          WHERE release_id=$1 AND track_id IS NOT NULL ORDER BY id LIMIT 1",
    )
    .bind(source.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read a source observation");
    let artist_id: Uuid =
        sqlx::query_scalar("SELECT artist_id FROM release_artists WHERE release_id=$1 LIMIT 1")
            .bind(source.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("read the source Release billing Artist");
    let release_artist_id = Uuid::now_v7();
    let presentation_order: i32 = sqlx::query_scalar(
        "SELECT COALESCE(max(presentation_order), 0) + 1 FROM release_artists WHERE release_id=$1",
    )
    .bind(source.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("allocate Release billing order");
    sqlx::query(
        "INSERT INTO release_artists (
             id, release_id, artist_id, observed_artist_id, role,
             presentation_order, display_value, join_phrase, created_at
         ) VALUES ($1,$2,$3,$4,'primary',$5,'Artist E Link',' & ',$6)",
    )
    .bind(release_artist_id)
    .bind(source.internal_id)
    .bind(artist_id)
    .bind(observation_id)
    .bind(presentation_order)
    .bind(OffsetDateTime::now_utc())
    .execute(&db.pool)
    .await
    .expect("seed source Release billing observation link");

    let expected_detach: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_artists
          WHERE release_id=$1 AND observed_artist_id IS NOT NULL",
    )
    .bind(source.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("count source billing observation links");
    assert!(expected_detach >= 1);

    let preview = store
        .preview_release_merge(&db.pool, &[source.internal_id], target.internal_id)
        .await
        .expect("preview billing detachment");
    assert_eq!(
        preview.impact.detached_release_artist_observations,
        expected_detach
    );

    let event_id = store
        .merge_releases(&db.pool, &[merge(&source, &target)])
        .await
        .expect("merge Release billing")[0]
        .merge_event_id;

    let detached: Option<Uuid> =
        sqlx::query_scalar("SELECT observed_artist_id FROM release_artists WHERE id=$1")
            .bind(release_artist_id)
            .fetch_one(&db.pool)
            .await
            .expect("read detached billing link");
    assert_eq!(detached, None);
    let moved_observation_owner: Uuid =
        sqlx::query_scalar("SELECT release_id FROM observed_artists WHERE id=$1")
            .bind(observation_id)
            .fetch_one(&db.pool)
            .await
            .expect("read moved observation owner");
    assert_eq!(moved_observation_owner, target.internal_id);

    store
        .split_release_merge(
            &db.pool,
            event_id,
            "release-merge-admin",
            "split after review",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("split Release merge");

    let restored: Option<Uuid> =
        sqlx::query_scalar("SELECT observed_artist_id FROM release_artists WHERE id=$1")
            .bind(release_artist_id)
            .fetch_one(&db.pool)
            .await
            .expect("read restored billing link");
    assert_eq!(restored, Some(observation_id));

    db.drop().await;
}

#[tokio::test]
async fn batch_merge_rolls_back_every_source_when_a_source_is_not_active() {
    let db = TestDb::create("catalog_release_merge_rollback").await;
    let store = CatalogStore;
    let merged_already = seed_release(&db, &store, 70, "Already Merged", "Artist G").await;
    let pending = seed_release(&db, &store, 71, "Pending Source", "Artist H").await;
    let target = seed_release(&db, &store, 72, "Rollback Target", "Artist I").await;

    let first_event = store
        .merge_release(
            &db.pool,
            merged_already.internal_id,
            target.internal_id,
            "release-merge-admin",
            "first merge",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("merge one source first");
    assert_ne!(first_event, Uuid::nil());
    let target_tracks_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(target.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("count target Tracks before failed batch");

    let error = store
        .merge_releases(
            &db.pool,
            &[merge(&merged_already, &target), merge(&pending, &target)],
        )
        .await
        .expect_err("a merged source must reject the whole batch");
    assert!(matches!(error, CatalogStoreError::MutationConflict));

    let pending_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(pending.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("read pending source status");
    assert_eq!(pending_status, "active");
    let pending_tracks: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
        .bind(pending.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("count pending source Tracks");
    assert_eq!(pending_tracks, 1);
    let target_tracks_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(target.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("count target Tracks after failed batch");
    assert_eq!(target_tracks_after, target_tracks_before);

    db.drop().await;
}

#[tokio::test]
async fn preview_rejects_duplicate_sources_and_target_in_sources() {
    let db = TestDb::create("catalog_release_merge_validation").await;
    let store = CatalogStore;
    let source = seed_release(&db, &store, 80, "Validation Source", "Artist J").await;
    let target = seed_release(&db, &store, 81, "Validation Target", "Artist K").await;

    let duplicate = store
        .preview_release_merge(
            &db.pool,
            &[source.internal_id, source.internal_id],
            target.internal_id,
        )
        .await;
    assert!(matches!(duplicate, Err(CatalogStoreError::InvalidRequest)));

    let self_target = store
        .preview_release_merge(&db.pool, &[target.internal_id], target.internal_id)
        .await;
    assert!(matches!(
        self_target,
        Err(CatalogStoreError::InvalidRequest)
    ));

    db.drop().await;
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
    .bind("release-merge-parity-stream-key")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert native user");
    id
}

#[tokio::test]
async fn merge_moves_user_release_state_and_keeps_immutable_novelty_evidence() {
    let db = TestDb::create("catalog_release_merge_user_state").await;
    let store = CatalogStore;
    let source = seed_release(&db, &store, 90, "State Source", "Artist L").await;
    let target = seed_release(&db, &store, 91, "State Target", "Artist M").await;
    let now = OffsetDateTime::now_utc();
    let source_only_user = insert_native_user(&db, "merge-state-source", now).await;
    let both_users = insert_native_user(&db, "merge-state-both", now).await;

    for (user_id, release_id, starred, rating, play_count) in [
        (source_only_user, source.internal_id, true, 4_i16, 7_i64),
        (both_users, source.internal_id, true, 2, 3),
        (both_users, target.internal_id, false, 5, 11),
    ] {
        sqlx::query(
            "INSERT INTO native_release_user_states (
                 user_id,release_id,starred,hated,rating,pinned,play_count,
                 last_played_at,created_at,updated_at
             ) VALUES ($1,$2,$3,false,$4,false,$5,$6,$6,$6)",
        )
        .bind(user_id)
        .bind(release_id)
        .bind(starred)
        .bind(rating)
        .bind(play_count)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("seed per-user Release state");
    }

    let (source_track, audio_identity_id, media_asset_id): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT id, audio_identity_id, playable_media_asset_id FROM tracks
          WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(source.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read source Track");
    let rule_set_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO policy_rule_sets (id,name,version,rules,created_by,created_at)
         VALUES ($1,'release-merge-test',1,'[]'::jsonb,
                 '{\"authority\":\"administrator\",\"actor\":\"merge-test\"}'::jsonb,$2)",
    )
    .bind(rule_set_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed policy rule set");
    let evaluation_id = Uuid::now_v7();
    // The novelty detail and aggregate share a deferred FK, so they must be
    // written in one transaction (detail first, then the proven aggregate).
    let mut novelty_tx = db.pool.begin().await.expect("begin novelty setup");
    sqlx::query(
        "INSERT INTO track_novelty (
             id, release_id, track_id, audio_identity_id, media_asset_id, evaluation_id,
             category, confidence, evidence_kind, detail, evaluated_at, created_at
         ) VALUES ($1,$2,$3,$4,$5,$6,'new_track',9000,'no_catalog_match',
                   'no catalog match', $7, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(source.internal_id)
    .bind(source_track)
    .bind(audio_identity_id)
    .bind(media_asset_id)
    .bind(evaluation_id)
    .bind(now)
    .execute(&mut *novelty_tx)
    .await
    .expect("seed Track novelty row");
    sqlx::query(
        "INSERT INTO release_novelty_evaluations (
             id, release_id, policy_rule_set_id, exact_duplicate_count,
             known_audio_identity_count, alternate_recording_count,
             new_track_count, uncertain_count, total_track_count,
             evaluated_at, evaluator_version, created_at
         ) VALUES ($1,$2,$3,0,0,0,1,0,1,$4,'release-merge-test-v1',$4)",
    )
    .bind(evaluation_id)
    .bind(source.internal_id)
    .bind(rule_set_id)
    .bind(now)
    .execute(&mut *novelty_tx)
    .await
    .expect("seed Release novelty aggregate");
    novelty_tx.commit().await.expect("commit novelty setup");

    let preview = store
        .preview_release_merge(&db.pool, &[source.internal_id], target.internal_id)
        .await
        .expect("preview user-state merge");
    assert_eq!(preview.impact.moved_user_states, 1);
    assert_eq!(preview.impact.retained_user_states, 1);

    let event_id = store
        .merge_releases(&db.pool, &[merge(&source, &target)])
        .await
        .expect("merge Release user state")[0]
        .merge_event_id;

    let moved_state: (bool, i16, i64) = sqlx::query_as(
        "SELECT starred, rating, play_count FROM native_release_user_states
          WHERE user_id=$1 AND release_id=$2",
    )
    .bind(source_only_user)
    .bind(target.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read moved user state");
    assert_eq!(moved_state, (true, 4, 7));
    let retained_state: (i16, i64) = sqlx::query_as(
        "SELECT rating, play_count FROM native_release_user_states
          WHERE user_id=$1 AND release_id=$2",
    )
    .bind(both_users)
    .bind(target.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read retained target user state");
    assert_eq!(retained_state, (5, 11));
    let source_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_release_user_states WHERE release_id=$1")
            .bind(source.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("count source user state rows on tombstone");
    assert_eq!(source_rows, 1);
    // Policy novelty evidence is immutable: it keeps the Release whose
    // evaluation recorded it even though the Track now lives in the survivor.
    let novelty_owner: Uuid =
        sqlx::query_scalar("SELECT release_id FROM track_novelty WHERE track_id=$1")
            .bind(source_track)
            .fetch_one(&db.pool)
            .await
            .expect("read retained novelty owner");
    assert_eq!(novelty_owner, source.internal_id);

    store
        .split_release_merge(
            &db.pool,
            event_id,
            "release-merge-admin",
            "split after review",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("split Release merge");

    let restored_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_release_user_states WHERE release_id=$1")
            .bind(source.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("count restored source user state rows");
    assert_eq!(restored_rows, 2);
    let restored_user_state: (i16, i64) = sqlx::query_as(
        "SELECT rating, play_count FROM native_release_user_states
          WHERE user_id=$1 AND release_id=$2",
    )
    .bind(source_only_user)
    .bind(source.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read restored user state");
    assert_eq!(restored_user_state, (4, 7));
    let restored_novelty_owner: Uuid =
        sqlx::query_scalar("SELECT release_id FROM track_novelty WHERE track_id=$1")
            .bind(source_track)
            .fetch_one(&db.pool)
            .await
            .expect("read novelty owner after split");
    assert_eq!(restored_novelty_owner, source.internal_id);

    db.drop().await;
}

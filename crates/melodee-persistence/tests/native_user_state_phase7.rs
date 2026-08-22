//! user-state, playlist, and scrobble PostgreSQL acceptance.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "catalog_common/mod.rs"]
mod catalog_common;
#[path = "common/mod.rs"]
mod common;
#[path = "phase5_common/mod.rs"]
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use melodee_persistence::{
    NativePlaylistCreate, NativeScrobbleResult, NativeUserStateMutation, NativeUserStateStore,
    NativeUserStateStoreError,
};
use time::OffsetDateTime;
use uuid::Uuid;

async fn insert_user(db: &common::TestDb, username: &str) -> Uuid {
    let id = Uuid::now_v7();
    let public_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        r#"
        INSERT INTO native_users (
            id, public_api_id, username, normalized_username, email, normalized_email,
            password_hash, password_hash_algorithm, stream_signing_key,
            created_at, updated_at
        ) VALUES ($1, $2, $3, lower($3), $4, lower($4), $5, 'bcrypt', $6, $7, $7)
        "#,
    )
    .bind(id)
    .bind(public_id)
    .bind(username)
    .bind(format!("{username}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .bind("phase7-test-stream-key")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert native user");
    id
}

#[tokio::test]
async fn annotations_playlists_and_scrobbles_are_bounded_atomic_and_idempotent() {
    let db = common::TestDb::create("phase7_user_state").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7101,
        CatalogFixturePresentation {
            release_title: "Phase 7 One",
            release_artist: "Phase 7 Artist",
            track_title: "Phase 7 Track One",
            track_artist: "Phase 7 Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7102,
        CatalogFixturePresentation {
            release_title: "Phase 7 Two",
            release_artist: "Phase 7 Artist Two",
            track_title: "Phase 7 Track Two",
            track_artist: "Phase 7 Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let user_id = insert_user(&db, "phase7-user").await;
    let now = OffsetDateTime::now_utc();

    let (artist_public_id, track_public_id): (Uuid, Uuid) = sqlx::query_as(
        r#"
        SELECT a.public_api_id, t.public_api_id
        FROM tracks t
        JOIN track_artists ta ON ta.track_id = t.id AND ta.role = 'primary'
        JOIN artists a ON a.id = ta.artist_id
        WHERE t.release_id = $1
        "#,
    )
    .bind(first.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("first public IDs");
    let second_track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(second.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("second Track public ID");

    let artist_state = NativeUserStateStore::mutate_artist(
        &db.pool,
        user_id,
        artist_public_id,
        NativeUserStateMutation::Starred(true),
        now,
    )
    .await
    .expect("star Artist");
    assert!(artist_state.starred);
    let artist_state = NativeUserStateStore::mutate_artist(
        &db.pool,
        user_id,
        artist_public_id,
        NativeUserStateMutation::Hated(true),
        now,
    )
    .await
    .expect("hate Artist");
    assert!(artist_state.hated);
    assert!(!artist_state.starred);
    let release_state = NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        first.release_id().into_uuid(),
        NativeUserStateMutation::Rating(5),
        now,
    )
    .await
    .expect("rate Release");
    assert_eq!(release_state.rating, 5);
    let release_state = NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        first.release_id().into_uuid(),
        NativeUserStateMutation::Starred(true),
        now,
    )
    .await
    .expect("star Release");
    assert!(release_state.starred);
    let release_state = NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        first.release_id().into_uuid(),
        NativeUserStateMutation::Hated(true),
        now,
    )
    .await
    .expect("hate Release");
    assert!(release_state.hated);
    assert!(!release_state.starred);
    let track_state = NativeUserStateStore::mutate_track(
        &db.pool,
        user_id,
        track_public_id,
        NativeUserStateMutation::Hated(true),
        now,
    )
    .await
    .expect("hate Track");
    assert!(track_state.hated);
    let track_state = NativeUserStateStore::mutate_track(
        &db.pool,
        user_id,
        track_public_id,
        NativeUserStateMutation::Starred(true),
        now,
    )
    .await
    .expect("star Track");
    assert!(track_state.starred);
    assert!(!track_state.hated);
    assert!(matches!(
        NativeUserStateStore::mutate_track(
            &db.pool,
            user_id,
            track_public_id,
            NativeUserStateMutation::Rating(6),
            now,
        )
        .await,
        Err(NativeUserStateStoreError::InvalidRequest(_))
    ));

    let playlist = NativeUserStateStore::create_playlist(
        &db.pool,
        user_id,
        &NativePlaylistCreate {
            public_api_id: None,
            name: "Phase 7 Playlist".to_string(),
            description: Some("ordered acceptance".to_string()),
            is_public: false,
            track_public_ids: vec![track_public_id, second_track_public_id],
        },
        now,
    )
    .await
    .expect("create playlist");
    NativeUserStateStore::append_playlist_tracks(
        &db.pool,
        user_id,
        playlist.public_api_id,
        &[track_public_id],
        now,
    )
    .await
    .expect("append duplicate Track without legacy null failure");
    NativeUserStateStore::reorder_playlist_tracks(
        &db.pool,
        user_id,
        playlist.public_api_id,
        &[second_track_public_id, track_public_id],
        now,
    )
    .await
    .expect("set-based partial reorder");
    let items = NativeUserStateStore::playlist_items(&db.pool, user_id, playlist.public_api_id)
        .await
        .expect("list playlist items");
    assert_eq!(
        items
            .iter()
            .map(|item| item.track_public_id)
            .collect::<Vec<_>>(),
        vec![second_track_public_id, track_public_id]
    );

    let played_at = now - time::Duration::seconds(30);
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user_id,
            track_public_id,
            "phase7-event-1",
            played_at,
            now,
            Some(30),
        )
        .await
        .expect("record scrobble"),
        NativeScrobbleResult::Applied
    );
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user_id,
            track_public_id,
            "phase7-event-1",
            played_at,
            now,
            Some(30),
        )
        .await
        .expect("adopt exact replay"),
        NativeScrobbleResult::Adopted
    );
    let play_count: i64 = sqlx::query_scalar(
        "SELECT play_count FROM native_track_user_states s JOIN tracks t ON t.id = s.track_id WHERE s.user_id = $1 AND t.public_api_id = $2",
    )
    .bind(user_id)
    .bind(track_public_id)
    .fetch_one(&db.pool)
    .await
    .expect("read Track play count");
    assert_eq!(play_count, 1, "exact retry never double-counts");
    let popularity: i64 = sqlx::query_scalar(
        "SELECT popularity FROM native_recommendation_candidates candidate
         JOIN tracks track ON track.id=candidate.entity_id
         WHERE candidate.entity_type='track' AND track.public_api_id=$1",
    )
    .bind(track_public_id)
    .fetch_one(&db.pool)
    .await
    .expect("materialized recommendation popularity");
    assert_eq!(popularity, 1, "exact replay never double-counts popularity");

    let source_track_id: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE public_api_id=$1")
        .bind(track_public_id)
        .fetch_one(&db.pool)
        .await
        .expect("Track internal ID");
    let first_artist_id: Uuid = sqlx::query_scalar(
        "SELECT artist_id FROM track_artists WHERE track_id=$1 AND role='primary'",
    )
    .bind(source_track_id)
    .fetch_one(&db.pool)
    .await
    .expect("first primary Artist");
    let second_artist_id: Uuid = sqlx::query_scalar(
        "SELECT billing.artist_id FROM track_artists billing
         JOIN tracks track ON track.id=billing.track_id
         WHERE track.release_id=$1 AND billing.role='primary'",
    )
    .bind(second.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("second primary Artist");
    let (audio_identity_id, media_asset_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT audio_identity_id,playable_media_asset_id FROM tracks WHERE id=$1")
            .bind(source_track_id)
            .fetch_one(&db.pool)
            .await
            .expect("source Track media facts");
    let reassigned_track_id = Uuid::now_v7();
    let reassigned_track_public_id = Uuid::now_v7();
    let mut tx = db
        .pool
        .begin()
        .await
        .expect("reassignment fixture transaction");
    sqlx::query(
        "INSERT INTO tracks(
           id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,
           ordinal,title,normalized_title,sort_title,created_at,updated_at)
         VALUES($1,$2,$3,$4,$5,2,'Reassigned Track','REASSIGNED TRACK','Reassigned Track',now(),now())",
    )
    .bind(reassigned_track_id)
    .bind(reassigned_track_public_id)
    .bind(first.release_id().into_uuid())
    .bind(audio_identity_id)
    .bind(media_asset_id)
    .execute(&mut *tx)
    .await
    .expect("insert reassignment Track");
    sqlx::query(
        "INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at)
         VALUES($1,$2,'playable',now())",
    )
    .bind(reassigned_track_id)
    .bind(media_asset_id)
    .execute(&mut *tx)
    .await
    .expect("bind reassignment media");
    sqlx::query(
        "INSERT INTO track_artists(
           id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at)
         VALUES($1,$2,$3,'primary',1,'Phase 7 Artist','',now())",
    )
    .bind(Uuid::now_v7())
    .bind(reassigned_track_id)
    .bind(first_artist_id)
    .execute(&mut *tx)
    .await
    .expect("bind reassignment Artist");
    tx.commit().await.expect("commit reassignment Track");
    sqlx::query(
        "INSERT INTO native_track_plays(
           id,user_id,track_id,client_event_id,played_at,submitted_at,completed)
         VALUES($1,$2,$3,'relationship-reassignment',now(),now(),true)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(reassigned_track_id)
    .execute(&db.pool)
    .await
    .expect("record reassignment popularity");

    sqlx::query("UPDATE tracks SET release_id=$2 WHERE id=$1")
        .bind(reassigned_track_id)
        .bind(second.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("reassign Track Release");
    let release_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$2)",
    )
    .bind(first.release_id().into_uuid())
    .bind(second.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("reassigned Release popularity");
    assert_eq!(release_popularity, (1, 1));

    sqlx::query(
        "UPDATE track_artists SET artist_id=$2,display_value='Phase 7 Artist Two'
         WHERE track_id=$1 AND role='primary'",
    )
    .bind(reassigned_track_id)
    .bind(second_artist_id)
    .execute(&db.pool)
    .await
    .expect("reassign primary Artist");
    let artist_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$2)",
    )
    .bind(first_artist_id)
    .bind(second_artist_id)
    .fetch_one(&db.pool)
    .await
    .expect("reassigned Artist popularity");
    assert_eq!(artist_popularity, (1, 1));

    let concurrent_track_id = Uuid::now_v7();
    let concurrent_track_public_id = Uuid::now_v7();
    let mut fixture_tx = db.pool.begin().await.expect("concurrent Track fixture");
    sqlx::query(
        "INSERT INTO tracks(
           id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,
           ordinal,title,normalized_title,sort_title,created_at,updated_at)
         VALUES($1,$2,$3,$4,$5,3,'Concurrent Reassignment Track',
           'CONCURRENT REASSIGNMENT TRACK','Concurrent Reassignment Track',now(),now())",
    )
    .bind(concurrent_track_id)
    .bind(concurrent_track_public_id)
    .bind(first.release_id().into_uuid())
    .bind(audio_identity_id)
    .bind(media_asset_id)
    .execute(&mut *fixture_tx)
    .await
    .expect("insert concurrent Track");
    sqlx::query(
        "INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at)
         VALUES($1,$2,'playable',now())",
    )
    .bind(concurrent_track_id)
    .bind(media_asset_id)
    .execute(&mut *fixture_tx)
    .await
    .expect("bind concurrent Track media");
    sqlx::query(
        "INSERT INTO track_artists(
           id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at)
         VALUES($1,$2,$3,'primary',1,'Phase 7 Artist','',now())",
    )
    .bind(Uuid::now_v7())
    .bind(concurrent_track_id)
    .bind(first_artist_id)
    .execute(&mut *fixture_tx)
    .await
    .expect("bind concurrent Track Artist");
    fixture_tx.commit().await.expect("commit concurrent Track");

    let mut relationship_tx = db.pool.begin().await.expect("relationship transaction");
    sqlx::query("UPDATE tracks SET release_id=$2 WHERE id=$1")
        .bind(concurrent_track_id)
        .bind(second.release_id().into_uuid())
        .execute(&mut *relationship_tx)
        .await
        .expect("move concurrent Track Release");
    sqlx::query(
        "UPDATE track_artists SET artist_id=$2,display_value='Phase 7 Artist Two'
         WHERE track_id=$1 AND role='primary'",
    )
    .bind(concurrent_track_id)
    .bind(second_artist_id)
    .execute(&mut *relationship_tx)
    .await
    .expect("move concurrent primary Artist");

    let play_pool = db.pool.clone();
    let mut play_task = tokio::spawn(async move {
        sqlx::query(
            "INSERT INTO native_track_plays(
               id,user_id,track_id,client_event_id,played_at,submitted_at,completed)
             VALUES($1,$2,$3,'concurrent-relationship-reassignment',now(),now(),true)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(concurrent_track_id)
        .execute(&play_pool)
        .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut play_task)
            .await
            .is_err(),
        "play projection waits for the in-flight Track relationship transaction"
    );
    relationship_tx
        .commit()
        .await
        .expect("commit concurrent relationships");
    play_task
        .await
        .expect("join concurrent play")
        .expect("record concurrent play");

    let concurrent_release_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$2)",
    )
    .bind(first.release_id().into_uuid())
    .bind(second.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("concurrent Release popularity");
    assert_eq!(concurrent_release_popularity, (1, 2));
    let concurrent_artist_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$2)",
    )
    .bind(first_artist_id)
    .bind(second_artist_id)
    .fetch_one(&db.pool)
    .await
    .expect("concurrent Artist popularity");
    assert_eq!(concurrent_artist_popularity, (1, 2));
    assert!(matches!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user_id,
            second_track_public_id,
            "phase7-event-1",
            played_at,
            now,
            Some(30),
        )
        .await,
        Err(NativeUserStateStoreError::Conflict)
    ));

    NativeUserStateStore::delete_playlist(&db.pool, user_id, playlist.public_api_id)
        .await
        .expect("delete owned playlist");
    assert!(matches!(
        NativeUserStateStore::playlist(&db.pool, user_id, playlist.public_api_id).await,
        Err(NativeUserStateStoreError::NotFound)
    ));
    db.drop().await;
}

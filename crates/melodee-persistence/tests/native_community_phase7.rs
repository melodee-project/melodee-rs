//! Phase 7 request/share/preferences PostgreSQL integrity acceptance.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "catalog_common/mod.rs"]
mod catalog_common;
#[path = "common/mod.rs"]
mod common;
#[path = "phase5_common/mod.rs"]
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use melodee_persistence::{NativeShareImport, NativeShareStore, NativeShareStoreError};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn insert_user(db: &common::TestDb, username: &str) -> Uuid {
    let id = Uuid::now_v7();
    let public_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO native_users (id, public_api_id, username, normalized_username, email, \
         normalized_email, password_hash, password_hash_algorithm, stream_signing_key, \
         created_at, updated_at) \
         VALUES ($1, $2, $3, lower($3), $4, lower($4), $5, 'bcrypt', $6, $7, $7)",
    )
    .bind(id)
    .bind(public_id)
    .bind(username)
    .bind(format!("{username}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .bind("phase7-community-stream-key")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert user");
    id
}

#[tokio::test]
async fn community_schema_preserves_ids_parent_scope_and_replay_safe_preferences() {
    let db = common::TestDb::create("phase7_community").await;
    let catalog = phase5_common::seed_cataloged_release(
        &db,
        7201,
        CatalogFixturePresentation {
            release_title: "Community Release",
            release_artist: "Community Artist",
            track_title: "Community Track",
            track_artist: "Community Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let owner = insert_user(&db, "community-owner").await;
    let other = insert_user(&db, "community-other").await;
    let request_id = Uuid::now_v7();
    let request_public_id = Uuid::from_u128(0x1020_3040_5060_4070_8090_a0b0_c0d0_e0f0);
    sqlx::query(
        "INSERT INTO native_requests (id, public_api_id, created_by_user_id, category, \
         description, last_activity_at, last_activity_user_id, created_at, updated_at) \
         VALUES ($1, $2, $3, 5, 'Please add this', now(), $3, now(), now())",
    )
    .bind(request_id)
    .bind(request_public_id)
    .bind(owner)
    .execute(&db.pool)
    .await
    .expect("insert request preserving migrated public ID");
    assert!(
        sqlx::query("UPDATE native_requests SET public_api_id = $2 WHERE id = $1")
            .bind(request_id)
            .bind(Uuid::from_u128(0x2020_3040_5060_4070_8090_a0b0_c0d0_e0f0))
            .execute(&db.pool)
            .await
            .is_err(),
        "public request identity is immutable"
    );

    let second_request = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_requests (id, public_api_id, created_by_user_id, category, \
         description, last_activity_at, last_activity_user_id, created_at, updated_at) \
         VALUES ($1, $1, $2, 5, 'Second request', now(), $2, now(), now())",
    )
    .bind(second_request)
    .bind(other)
    .execute(&db.pool)
    .await
    .expect("insert second request");
    let parent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_request_comments (id, public_api_id, request_id, created_by_user_id, body, created_at) \
         VALUES ($1, $1, $2, $3, 'parent', now())",
    )
    .bind(parent)
    .bind(request_id)
    .bind(owner)
    .execute(&db.pool)
    .await
    .expect("insert parent comment");
    assert!(
        sqlx::query(
            "INSERT INTO native_request_comments (id, public_api_id, request_id, parent_comment_id, \
             created_by_user_id, body, created_at) VALUES ($1, $1, $2, $3, $4, 'invalid child', now())",
        )
        .bind(Uuid::now_v7())
        .bind(second_request)
        .bind(parent)
        .bind(other)
        .execute(&db.pool)
        .await
        .is_err(),
        "a comment cannot attach to a parent from another request"
    );

    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(catalog.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("track public ID");
    let share_id = Uuid::now_v7();
    let migrated_share_public_id = Uuid::from_u128(0x3020_3040_5060_4070_8090_a0b0_c0d0_e0f0);
    let now = OffsetDateTime::now_utc();
    let store = NativeShareStore::new();
    let secure_import = NativeShareImport {
        source_key: "legacy-share-41",
        internal_id: share_id,
        public_api_id: migrated_share_public_id,
        legacy_token: "0123456789abcdef0123456789abcdef",
        legacy_token_is_secure: true,
        owner_user_id: owner,
        share_type: 1,
        resource_public_api_id: track_public_id,
        description: Some("preserved secure share"),
        is_downloadable: false,
        expires_at: None,
        created_at: now,
        updated_at: now,
    };
    let preserved = store
        .import_share(&db.pool, &secure_import)
        .await
        .expect("secure migrated share");
    assert_eq!(preserved.public_api_id, migrated_share_public_id);
    assert_eq!(preserved.active_token, "0123456789abcdef0123456789abcdef");
    assert!(!preserved.rotated_weak_token);
    assert_eq!(
        store
            .import_share(&db.pool, &secure_import)
            .await
            .expect("exact replay"),
        preserved
    );
    let replay_conflicts = [
        NativeShareImport {
            owner_user_id: other,
            ..secure_import
        },
        NativeShareImport {
            description: Some("changed after import"),
            ..secure_import
        },
        NativeShareImport {
            is_downloadable: true,
            ..secure_import
        },
        NativeShareImport {
            expires_at: Some(now + Duration::hours(1)),
            ..secure_import
        },
        NativeShareImport {
            updated_at: now + Duration::seconds(1),
            ..secure_import
        },
        NativeShareImport {
            legacy_token_is_secure: false,
            ..secure_import
        },
    ];
    for conflicting in replay_conflicts {
        assert!(matches!(
            store.import_share(&db.pool, &conflicting).await,
            Err(NativeShareStoreError::ImportConflict)
        ));
    }

    let weak_import = NativeShareImport {
        source_key: "legacy-share-weak",
        internal_id: Uuid::now_v7(),
        public_api_id: Uuid::from_u128(0x4020_3040_5060_4070_8090_a0b0_c0d0_e0f0),
        legacy_token: "timestamp-short",
        legacy_token_is_secure: false,
        owner_user_id: owner,
        share_type: 1,
        resource_public_api_id: track_public_id,
        description: None,
        is_downloadable: false,
        expires_at: None,
        created_at: now,
        updated_at: now,
    };
    let rotated = store
        .import_share(&db.pool, &weak_import)
        .await
        .expect("weak bearer rotation");
    assert!(rotated.rotated_weak_token);
    assert_eq!(
        rotated.rotated_legacy_source_key.as_deref(),
        Some("legacy-share-weak")
    );
    assert_ne!(rotated.active_token, weak_import.legacy_token);
    assert_eq!(rotated.active_token.len(), 32);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_shares WHERE share_unique_id='timestamp-short'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("weak bearer inactive"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_migration_identity_bindings
             WHERE source_kind='share' AND source_key IN ('legacy-share-41','legacy-share-weak')",
        )
        .fetch_one(&db.pool)
        .await
        .expect("share bindings"),
        2
    );

    for _ in 0..2 {
        sqlx::query(
            "INSERT INTO native_playback_settings (user_id, crossfade_duration, gapless_playback, \
             volume_normalization, replay_gain, audio_quality, created_at, updated_at) \
             VALUES ($1, 3, true, false, 'track', 'lossless', now(), now()) \
             ON CONFLICT (user_id) DO UPDATE SET crossfade_duration = EXCLUDED.crossfade_duration, \
             replay_gain = EXCLUDED.replay_gain, audio_quality = EXCLUDED.audio_quality, updated_at = now()",
        )
        .bind(owner)
        .execute(&db.pool)
        .await
        .expect("exact settings retry");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_playback_settings WHERE user_id = $1",
        )
        .bind(owner)
        .fetch_one(&db.pool)
        .await
        .expect("settings count"),
        1,
        "exact retry does not duplicate per-user settings"
    );

    let release_request = Uuid::now_v7();
    let track_request = Uuid::now_v7();
    for (request_id, category, release_title, track_title) in [
        (release_request, 1_i16, Some("Wanted Release"), None),
        (track_request, 2_i16, None, Some("Wanted Track")),
    ] {
        sqlx::query(
            "INSERT INTO native_requests (
                id, public_api_id, created_by_user_id, category, description,
                artist_name, release_title, track_title, release_year,
                last_activity_at, last_activity_user_id, created_at, updated_at
             ) VALUES ($1, $1, $2, $3, 'Please add this catalog fact',
                       'Wanted Artist', $4, $5, 2026, now(), $2, now(), now())",
        )
        .bind(request_id)
        .bind(owner)
        .bind(category)
        .bind(release_title)
        .bind(track_title)
        .execute(&db.pool)
        .await
        .expect("insert request awaiting catalog match");
    }

    let matched = phase5_common::seed_cataloged_release(
        &db,
        7202,
        CatalogFixturePresentation {
            release_title: "Wanted Release",
            release_artist: "Wanted Artist",
            track_title: "Wanted Track",
            track_artist: "Wanted Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let matched_release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(matched.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("matched Release public ID");
    let matched_track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(matched.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("matched Track public ID");
    let release_completion: (i16, Option<Uuid>) = sqlx::query_as(
        "SELECT status, target_release_public_api_id FROM native_requests WHERE id = $1",
    )
    .bind(release_request)
    .fetch_one(&db.pool)
    .await
    .expect("release request completion");
    let track_completion: (i16, Option<Uuid>) = sqlx::query_as(
        "SELECT status, target_track_public_api_id FROM native_requests WHERE id = $1",
    )
    .bind(track_request)
    .fetch_one(&db.pool)
    .await
    .expect("track request completion");
    assert_eq!(release_completion, (3, Some(matched_release_public_id)));
    assert_eq!(track_completion, (3, Some(matched_track_public_id)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_request_comments
             WHERE request_id = ANY($1) AND is_system",
        )
        .bind(vec![release_request, track_request])
        .fetch_one(&db.pool)
        .await
        .expect("automatic completion comments"),
        2
    );

    phase5_common::seed_cataloged_release(
        &db,
        7203,
        CatalogFixturePresentation {
            release_title: "Wanted Release",
            release_artist: "Wanted Artist",
            track_title: "Wanted Track",
            track_artist: "Wanted Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_request_comments
             WHERE request_id = ANY($1) AND is_system",
        )
        .bind(vec![release_request, track_request])
        .fetch_one(&db.pool)
        .await
        .expect("idempotent completion comments"),
        2,
        "later matching imports cannot duplicate completion activity"
    );
    db.drop().await;
}

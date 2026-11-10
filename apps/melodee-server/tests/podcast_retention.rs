//! Podcast retention policy integration tests (legacy `PodcastCleanupJob`
//! parity: keep-for-N-days, keep-last-N, keep-unplayed-only).
//!
//! The worker pass runs against real PostgreSQL and a real owned storage root;
//! cleanup candidates are selected by settings, re-validated under a row lock,
//! and the owned file is removed only for the recorded path. Bookmarked
//! episodes are always retained.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::path::Path;

use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport, SettingsStore};
use time::OffsetDateTime;
use uuid::Uuid;

struct Fixture {
    db: common::TestDb,
    storage: tempfile::TempDir,
    channel_id: i64,
    user_id: Uuid,
}

async fn fixture(name: &str) -> Fixture {
    let db = common::TestDb::create(name).await;
    let now = OffsetDateTime::now_utc();
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "retention-user",
                normalized_username: "RETENTION-USER",
                email: "retention-user@example.test",
                normalized_email: "RETENTION-USER@EXAMPLE.TEST",
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "podcast-retention-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["HasStreamRole".to_string()],
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import user");
    let user_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM native_users WHERE normalized_username='RETENTION-USER'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("user id");
    let channel_id: i64 = sqlx::query_scalar(
        "INSERT INTO native_podcast_channels (
             public_api_id, owner_user_id, feed_url, title, last_sync_at, created_at, updated_at
         ) VALUES ($1, $2, 'https://example.test/feed.xml', 'Retention Channel', now(), now(), now())
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .fetch_one(&db.pool)
    .await
    .expect("channel");
    let storage = tempfile::tempdir().expect("podcast storage");
    std::fs::write(
        storage.path().join(".melodee-podcast-root"),
        b"melodee-podcast-root-v1\n",
    )
    .expect("ownership marker");
    Fixture {
        db,
        storage,
        channel_id,
        user_id,
    }
}

fn worker_config(storage: &Path) -> melodee_server::podcast_worker::PodcastConfig {
    melodee_server::podcast_worker::PodcastConfig {
        worker_enabled: true,
        storage_root: Some(storage.to_path_buf()),
        allow_loopback_http: true,
        ..melodee_server::podcast_worker::PodcastConfig::default()
    }
}

/// Inserts one downloaded episode with a real owned file and returns its
/// episode id and public id.
async fn seed_downloaded(
    fixture: &Fixture,
    title: &str,
    published_offset_days: i64,
    downloaded_offset_days: i64,
) -> (i64, Uuid) {
    let public_id = Uuid::now_v7();
    let path = fixture.storage.path().join(format!("{public_id}.media"));
    std::fs::write(&path, b"downloaded episode bytes").expect("episode file");
    let published_at = OffsetDateTime::now_utc() - time::Duration::days(published_offset_days);
    let downloaded_at = OffsetDateTime::now_utc() - time::Duration::days(downloaded_offset_days);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO native_podcast_episodes (
             public_api_id, channel_id, title, feed_item_id, enclosure_url,
             download_status, local_path, downloaded_bytes, downloaded_at,
             published_at, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, 'https://example.test/e.mp3',
                   'Downloaded', $5, 23, $6, $7, now(), now())
         RETURNING id",
    )
    .bind(public_id)
    .bind(fixture.channel_id)
    .bind(title)
    .bind(format!("item-{public_id}"))
    .bind(path.to_string_lossy().to_string())
    .bind(downloaded_at)
    .bind(published_at)
    .fetch_one(&fixture.db.pool)
    .await
    .expect("episode");
    (id, public_id)
}

async fn set_setting(fixture: &Fixture, key: &str, value: serde_json::Value) {
    SettingsStore::new()
        .set(
            &fixture.db.pool,
            key,
            &value,
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("setting");
}

async fn status(fixture: &Fixture, id: i64) -> (String, Option<String>) {
    sqlx::query_as("SELECT download_status, local_path FROM native_podcast_episodes WHERE id=$1")
        .bind(id)
        .fetch_one(&fixture.db.pool)
        .await
        .expect("episode row")
}

#[tokio::test]
async fn keep_for_n_days_removes_only_expired_downloads() {
    let fixture = fixture("podcast_retention_days").await;
    let (expired, expired_id) = seed_downloaded(&fixture, "Old Episode", 10, 10).await;
    let (fresh, fresh_id) = seed_downloaded(&fixture, "Fresh Episode", 1, 0).await;
    set_setting(
        &fixture,
        "podcast.retention.downloadedEpisodesInDays",
        serde_json::json!(3),
    )
    .await;

    let config = worker_config(fixture.storage.path());
    melodee_server::podcast_worker::run_podcast_once(&fixture.db.pool, &config)
        .await
        .expect("worker pass");

    let expired_status = status(&fixture, expired).await;
    assert_eq!(expired_status.0, "NotDownloaded");
    assert!(expired_status.1.is_none());
    assert!(
        !fixture
            .storage
            .path()
            .join(format!("{expired_id}.media"))
            .exists(),
        "expired owned file is removed"
    );
    let fresh_status = status(&fixture, fresh).await;
    assert_eq!(fresh_status.0, "Downloaded");
    assert!(fresh_status.1.is_some());
    assert!(
        fixture
            .storage
            .path()
            .join(format!("{fresh_id}.media"))
            .exists()
    );
    fixture.db.drop().await;
}

#[tokio::test]
async fn keep_last_n_removes_the_oldest_downloads() {
    let fixture = fixture("podcast_retention_last_n").await;
    let (oldest, oldest_id) = seed_downloaded(&fixture, "Oldest", 30, 0).await;
    let (newer, newer_id) = seed_downloaded(&fixture, "Newer", 20, 0).await;
    let (newest, newest_id) = seed_downloaded(&fixture, "Newest", 10, 0).await;
    set_setting(
        &fixture,
        "podcast.retention.keepLastNEpisodes",
        serde_json::json!(2),
    )
    .await;

    let config = worker_config(fixture.storage.path());
    melodee_server::podcast_worker::run_podcast_once(&fixture.db.pool, &config)
        .await
        .expect("worker pass");

    assert_eq!(status(&fixture, oldest).await.0, "NotDownloaded");
    assert!(
        !fixture
            .storage
            .path()
            .join(format!("{oldest_id}.media"))
            .exists()
    );
    assert_eq!(status(&fixture, newer).await.0, "Downloaded");
    assert_eq!(status(&fixture, newest).await.0, "Downloaded");
    assert!(
        fixture
            .storage
            .path()
            .join(format!("{newer_id}.media"))
            .exists()
    );
    assert!(
        fixture
            .storage
            .path()
            .join(format!("{newest_id}.media"))
            .exists()
    );
    fixture.db.drop().await;
}

#[tokio::test]
async fn keep_unplayed_only_removes_played_episodes() {
    let fixture = fixture("podcast_retention_unplayed").await;
    let (played, played_id) = seed_downloaded(&fixture, "Played Episode", 2, 0).await;
    let (unplayed, unplayed_id) = seed_downloaded(&fixture, "Unplayed Episode", 1, 0).await;
    sqlx::query(
        "INSERT INTO native_podcast_play_history (
             user_id, episode_id, played_at, client, is_now_playing
         ) VALUES ($1, $2, now() - interval '1 hour', 'fixture-client', false)",
    )
    .bind(fixture.user_id)
    .bind(played)
    .execute(&fixture.db.pool)
    .await
    .expect("play history");
    set_setting(
        &fixture,
        "podcast.retention.keepUnplayedOnly",
        serde_json::json!(true),
    )
    .await;

    let config = worker_config(fixture.storage.path());
    melodee_server::podcast_worker::run_podcast_once(&fixture.db.pool, &config)
        .await
        .expect("worker pass");

    assert_eq!(status(&fixture, played).await.0, "NotDownloaded");
    assert!(
        !fixture
            .storage
            .path()
            .join(format!("{played_id}.media"))
            .exists()
    );
    assert_eq!(status(&fixture, unplayed).await.0, "Downloaded");
    assert!(
        fixture
            .storage
            .path()
            .join(format!("{unplayed_id}.media"))
            .exists()
    );
    fixture.db.drop().await;
}

#[tokio::test]
async fn bookmarked_episodes_survive_every_retention_policy() {
    let fixture = fixture("podcast_retention_bookmark").await;
    let (bookmarked, bookmarked_id) = seed_downloaded(&fixture, "Bookmarked", 60, 60).await;
    sqlx::query(
        "INSERT INTO native_podcast_bookmarks (user_id, episode_id, position_seconds, created_at, updated_at)
         VALUES ($1, $2, 42, now(), now())",
    )
    .bind(fixture.user_id)
    .bind(bookmarked)
    .execute(&fixture.db.pool)
    .await
    .expect("bookmark");
    set_setting(
        &fixture,
        "podcast.retention.downloadedEpisodesInDays",
        serde_json::json!(1),
    )
    .await;
    set_setting(
        &fixture,
        "podcast.retention.keepLastNEpisodes",
        serde_json::json!(1),
    )
    .await;

    let config = worker_config(fixture.storage.path());
    melodee_server::podcast_worker::run_podcast_once(&fixture.db.pool, &config)
        .await
        .expect("worker pass");

    let row = status(&fixture, bookmarked).await;
    assert_eq!(row.0, "Downloaded");
    assert!(row.1.is_some());
    assert!(
        fixture
            .storage
            .path()
            .join(format!("{bookmarked_id}.media"))
            .exists(),
        "a bookmarked episode's file is never deleted"
    );
    fixture.db.drop().await;
}

#[tokio::test]
async fn per_user_quota_stops_new_claims() {
    let fixture = fixture("podcast_quota").await;
    // One downloaded episode already exceeds the configured per-user quota.
    let (_downloaded, _downloaded_id) = seed_downloaded(&fixture, "Existing", 1, 0).await;
    sqlx::query("UPDATE native_podcast_episodes SET downloaded_bytes=23 WHERE channel_id=$1")
        .bind(fixture.channel_id)
        .execute(&fixture.db.pool)
        .await
        .expect("downloaded bytes");
    let queued_public_id = Uuid::now_v7();
    let queued_id: i64 = sqlx::query_scalar(
        "INSERT INTO native_podcast_episodes (
             public_api_id, channel_id, title, feed_item_id, enclosure_url,
             download_status, created_at, updated_at
         ) VALUES ($1, $2, 'Queued Over Quota', 'quota-item',
                   'https://example.test/quota.mp3', 'Queued', now(), now())
         RETURNING id",
    )
    .bind(queued_public_id)
    .bind(fixture.channel_id)
    .fetch_one(&fixture.db.pool)
    .await
    .expect("queued episode");
    set_setting(
        &fixture,
        "podcast.quota.maxBytesPerUser",
        serde_json::json!(10),
    )
    .await;

    let config = worker_config(fixture.storage.path());
    let completed = melodee_server::podcast_worker::run_podcast_once(&fixture.db.pool, &config)
        .await
        .expect("worker pass");
    assert_eq!(completed, 0, "no claim while the user is over quota");
    assert_eq!(status(&fixture, queued_id).await.0, "Queued");
    fixture.db.drop().await;
}

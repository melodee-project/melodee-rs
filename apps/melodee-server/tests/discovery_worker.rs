//! Phase 9 WP-04 scheduled chart/recommendation aggregate maintenance
//! acceptance (PRD-WORK-003).

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::TestDb;
use melodee_server::discovery_worker::{
    DiscoveryWorkerConfig, run_discovery_once, start_discovery_loop,
};
use uuid::Uuid;

async fn insert_user(db: &TestDb, username: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = time::OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO native_users (\
           id, public_api_id, username, normalized_username, email, normalized_email, \
           password_hash, password_hash_algorithm, stream_signing_key, created_at, updated_at\
         ) VALUES ($1, $2, $3, lower($3), $4, lower($4), $5, 'bcrypt', $6, $7, $7)",
    )
    .bind(id)
    .bind(Uuid::now_v7())
    .bind(username)
    .bind(format!("{username}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .bind("phase9-discovery-test-stream-key")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert native user");
    id
}

async fn insert_play(db: &TestDb, user_id: Uuid, track_id: Uuid, event: &str) {
    sqlx::query(
        "INSERT INTO native_track_plays(\
           id,user_id,track_id,client_event_id,played_at,submitted_at,completed) \
         VALUES($1,$2,$3,$4,now(),now(),true)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(track_id)
    .bind(event)
    .execute(&db.pool)
    .await
    .expect("insert play fact");
}

struct Seeded {
    release_id: Uuid,
    track_id: Uuid,
    artist_id: Uuid,
}

async fn seed_release(db: &TestDb, ordinal: u32, title: &str, artist: &str) -> Seeded {
    let fixture = phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            release_artist: artist,
            track_title: title,
            track_artist: artist,
            release_date: "2026-05-06",
        },
    )
    .await;
    let release_id = fixture.release_id().into_uuid();
    let (track_id, artist_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT track.id, billing.artist_id FROM tracks track \
         JOIN track_artists billing ON billing.track_id=track.id AND billing.role='primary' \
         WHERE track.release_id=$1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("seeded Track and primary Artist");
    fixture.assert_media_paths_absent();
    Seeded {
        release_id,
        track_id,
        artist_id,
    }
}

async fn popularity(db: &TestDb, entity_type: &str, entity_id: Uuid) -> Option<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT popularity FROM native_recommendation_candidates \
         WHERE entity_type=$1 AND entity_id=$2",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_optional(&db.pool)
    .await
    .expect("read candidate popularity")
}

#[tokio::test]
async fn refresh_reconciles_drift_and_missing_rows_from_durable_play_facts() {
    let db = TestDb::create("discovery_worker_refresh").await;
    let first = seed_release(&db, 9101, "Discovery One", "Discovery Artist One").await;
    let second = seed_release(&db, 9102, "Discovery Two", "Discovery Artist Two").await;
    let user_id = insert_user(&db, "discovery-worker-user").await;
    insert_play(&db, user_id, first.track_id, "discovery-play-1").await;
    insert_play(&db, user_id, first.track_id, "discovery-play-2").await;
    insert_play(&db, user_id, second.track_id, "discovery-play-3").await;

    // Durable triggers keep the projection exact in the happy path.
    assert_eq!(popularity(&db, "track", first.track_id).await, Some(2));
    assert_eq!(popularity(&db, "release", first.release_id).await, Some(2));
    assert_eq!(popularity(&db, "artist", first.artist_id).await, Some(2));
    assert_eq!(popularity(&db, "track", second.track_id).await, Some(1));

    // Simulate drift from a restore/downgrade window: zeroed counters and a
    // candidate row that never materialized.
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=0")
        .execute(&db.pool)
        .await
        .expect("simulate counter drift");
    sqlx::query(
        "DELETE FROM native_recommendation_candidates WHERE entity_type='track' AND entity_id=$1",
    )
    .bind(second.track_id)
    .execute(&db.pool)
    .await
    .expect("simulate missing candidate row");

    let config = DiscoveryWorkerConfig::default();
    let stats = run_discovery_once(&db.pool, &config)
        .await
        .expect("maintenance pass reconciles projections");
    assert_eq!(
        stats.candidates_inserted, 1,
        "missing Track candidate re-added"
    );
    assert!(
        stats.candidates_corrected >= 6,
        "drifted counters corrected: {stats:?}"
    );
    assert!(!stats.truncated);

    assert_eq!(popularity(&db, "track", first.track_id).await, Some(2));
    assert_eq!(popularity(&db, "release", first.release_id).await, Some(2));
    assert_eq!(popularity(&db, "artist", first.artist_id).await, Some(2));
    assert_eq!(
        popularity(&db, "track", second.track_id).await,
        Some(1),
        "re-added candidate recomputed from durable play facts"
    );
    assert_eq!(popularity(&db, "release", second.release_id).await, Some(1));
    assert_eq!(popularity(&db, "artist", second.artist_id).await, Some(1));

    // Idempotent: a second pass over unchanged facts is a no-op.
    let repeat = run_discovery_once(&db.pool, &config)
        .await
        .expect("second maintenance pass");
    assert_eq!(repeat.candidates_inserted, 0);
    assert_eq!(repeat.candidates_corrected, 0);
    assert_eq!(repeat.chart_items_relinked, 0);
    db.drop().await;
}

#[tokio::test]
async fn refresh_converges_across_bounded_batches() {
    let db = TestDb::create("discovery_worker_bounded_batches").await;
    let first = seed_release(&db, 9111, "Bounded One", "Bounded Artist One").await;
    let second = seed_release(&db, 9112, "Bounded Two", "Bounded Artist Two").await;
    let user_id = insert_user(&db, "discovery-bounded-user").await;
    insert_play(&db, user_id, first.track_id, "bounded-play-1").await;
    insert_play(&db, user_id, second.track_id, "bounded-play-2").await;
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=0")
        .execute(&db.pool)
        .await
        .expect("simulate counter drift");

    let config = DiscoveryWorkerConfig {
        batch_size: 1,
        ..DiscoveryWorkerConfig::default()
    };
    let stats = run_discovery_once(&db.pool, &config)
        .await
        .expect("single-row batches still converge");
    assert!(stats.batches > 6, "one-row batches walk every candidate");
    assert!(!stats.truncated);
    assert_eq!(popularity(&db, "track", first.track_id).await, Some(1));
    assert_eq!(popularity(&db, "track", second.track_id).await, Some(1));

    // A pass capped below the work required reports truncation and converges
    // on a later pass instead of failing.
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=0")
        .execute(&db.pool)
        .await
        .expect("re-introduce counter drift");
    let capped = DiscoveryWorkerConfig {
        batch_size: 1,
        max_batches_per_pass: 1,
        ..DiscoveryWorkerConfig::default()
    };
    let truncated = run_discovery_once(&db.pool, &capped)
        .await
        .expect("capped pass remains accountable");
    assert!(truncated.truncated);
    let converged = run_discovery_once(&db.pool, &config)
        .await
        .expect("follow-up pass completes the reconciliation");
    assert!(!converged.truncated);
    assert_eq!(popularity(&db, "release", first.release_id).await, Some(1));
    assert_eq!(popularity(&db, "release", second.release_id).await, Some(1));
    db.drop().await;
}

#[tokio::test]
async fn scheduler_runs_periodically_and_stops_gracefully() {
    let db = TestDb::create("discovery_worker_scheduler").await;
    let seeded = seed_release(&db, 9121, "Periodic One", "Periodic Artist One").await;
    let user_id = insert_user(&db, "discovery-scheduler-user").await;
    insert_play(&db, user_id, seeded.track_id, "scheduler-play-1").await;
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=0")
        .execute(&db.pool)
        .await
        .expect("simulate counter drift");

    let config = DiscoveryWorkerConfig {
        enabled: true,
        poll_interval: std::time::Duration::from_millis(50),
        batch_size: 500,
        max_batches_per_pass: 100,
    };
    let handle = start_discovery_loop(config, db.pool.clone())
        .expect("enabled worker starts the maintenance loop");

    // The loop refreshes the drifted projection from durable play facts
    // without any explicit trigger.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if popularity(&db, "track", seeded.track_id).await == Some(1) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "scheduled maintenance did not reconcile the drifted projection"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let latest = handle.stop().await;
    let stats = latest.expect("stopped loop reports its last pass");
    assert!(stats.candidates_corrected >= 1);
    db.drop().await;
}

#[tokio::test]
async fn cancelled_loop_stops_scheduling_and_leaves_projections_untouched() {
    let db = TestDb::create("discovery_worker_cancellation").await;
    let seeded = seed_release(&db, 9131, "Cancelled One", "Cancelled Artist One").await;
    let config = DiscoveryWorkerConfig {
        enabled: true,
        poll_interval: std::time::Duration::from_millis(50),
        batch_size: 500,
        max_batches_per_pass: 100,
    };
    let handle = start_discovery_loop(config, db.pool.clone())
        .expect("enabled worker starts the maintenance loop");
    // Shutdown must be prompt even when the loop is mid-tick; every committed
    // batch is independently idempotent, so cancellation never leaves a
    // partial outcome to roll back.
    tokio::time::timeout(std::time::Duration::from_secs(10), handle.stop())
        .await
        .expect("cancellation completes promptly");

    // After the join resolves, no further pass may touch the projections.
    sqlx::query(
        "UPDATE native_recommendation_candidates SET popularity=42 \
         WHERE entity_type='track' AND entity_id=$1",
    )
    .bind(seeded.track_id)
    .execute(&db.pool)
    .await
    .expect("mark post-stop projection state");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        popularity(&db, "track", seeded.track_id).await,
        Some(42),
        "a stopped loop never reconciles again"
    );
    db.drop().await;
}

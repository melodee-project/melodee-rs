//! Phase 9 WP-05 worker-pool pause/resume runtime acceptance (ADR-0134 §8,
//! PRD-WORK-005): a durably paused pool claims nothing, in-flight leases are
//! never stranded or stolen, and resume restores claiming.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_persistence::{
    POOL_DISCOVERY, POOL_PODCAST, POOL_POLICY, PolicyStore, WorkerId, WorkerPoolStore,
};
use melodee_policy::{PolicyConfig, PolicyEngine, default_rule_set};
use melodee_server::discovery_worker::{DiscoveryWorkerConfig, run_discovery_once};
use melodee_server::podcast_worker::{PodcastConfig, run_podcast_once};
use melodee_server::policy_worker::run_policy_once;
use time::OffsetDateTime;
use uuid::Uuid;

async fn set_paused(db: &TestDb, pool: &str, paused: bool) {
    WorkerPoolStore
        .set_paused(
            &db.pool,
            pool,
            paused,
            "worker-pool-test",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("set pool pause state");
}

async fn policy_engine(db: &TestDb, config: &PolicyConfig) -> PolicyEngine {
    let proposed = default_rule_set(config).expect("valid default rules");
    let persisted = PolicyStore
        .ensure_rule_set(&db.pool, &proposed, OffsetDateTime::now_utc())
        .await
        .expect("persist default rules");
    PolicyEngine::with_rule_set(config.clone(), persisted).expect("load persisted rules")
}

async fn seed_queued_episode(db: &TestDb, feed_item_id: &str, title: &str) -> i64 {
    let owner = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,normalized_email,\
         password_hash,password_hash_algorithm,stream_signing_key,created_at,updated_at) \
         VALUES($1,$2,$3,upper($3),$4,upper($4),$5,'bcrypt','pool-test-key',now(),now())",
    )
    .bind(owner)
    .bind(Uuid::now_v7())
    .bind(format!("pool-owner-{feed_item_id}"))
    .bind(format!("pool-{feed_item_id}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .execute(&db.pool)
    .await
    .expect("insert owner");
    let channel = sqlx::query_scalar::<_, i64>(
        "INSERT INTO native_podcast_channels(public_api_id,owner_user_id,feed_url,title,created_at,updated_at) \
         VALUES($1,$2,$3,$4,now(),now()) RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(owner)
    .bind(format!("https://feeds.example.test/{feed_item_id}"))
    .bind(format!("Pool Show {feed_item_id}"))
    .fetch_one(&db.pool)
    .await
    .expect("insert channel");
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO native_podcast_episodes(public_api_id,channel_id,feed_item_id,title,\
         enclosure_url,download_status,created_at,updated_at) \
         VALUES($1,$2,$3,$4,'https://media.example.test/episode.mp3','Queued',now(),now()) RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(channel)
    .bind(feed_item_id)
    .bind(title)
    .fetch_one(&db.pool)
    .await
    .expect("insert episode")
}

fn podcast_config(storage: &std::path::Path) -> PodcastConfig {
    std::fs::write(
        storage.join(".melodee-podcast-root"),
        b"melodee-podcast-root-v1\n",
    )
    .expect("ownership marker");
    PodcastConfig {
        enabled: true,
        worker_enabled: true,
        storage_root: Some(storage.to_path_buf()),
        ..PodcastConfig::default()
    }
}

async fn episode_state(db: &TestDb, episode: i64) -> (String, i32, Option<Uuid>) {
    sqlx::query_as(
        "SELECT download_status,download_attempt_count,download_lease_owner \
         FROM native_podcast_episodes WHERE id=$1",
    )
    .bind(episode)
    .fetch_one(&db.pool)
    .await
    .expect("episode state")
}

#[tokio::test]
async fn paused_podcast_pool_claims_nothing_and_resume_recovers() {
    let db = TestDb::create("worker_pool_podcast").await;
    let storage = tempfile::tempdir().expect("podcast storage");
    let config = podcast_config(storage.path());
    let queued = seed_queued_episode(&db, "queued", "Queued Episode").await;

    // Paused: no claim, no refresh, no reconciliation side effects.
    set_paused(&db, POOL_PODCAST, true).await;
    let completed = run_podcast_once(&db.pool, &config)
        .await
        .expect("paused pass is a clean no-op");
    assert_eq!(completed, 0);
    assert_eq!(
        episode_state(&db, queued).await,
        ("Queued".to_owned(), 0, None),
        "a paused pool never claims"
    );

    // An in-flight lease from before the pause is neither stranded by the
    // pause nor stolen by a paused pass.
    let in_flight = seed_queued_episode(&db, "inflight", "In Flight Episode").await;
    let owner = Uuid::now_v7();
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Downloading',download_lease_owner=$2,\
         download_lease_expires_at=now()+interval '1 hour' WHERE id=$1",
    )
    .bind(in_flight)
    .bind(owner)
    .execute(&db.pool)
    .await
    .expect("in-flight lease");
    let completed = run_podcast_once(&db.pool, &config)
        .await
        .expect("paused pass leaves in-flight work alone");
    assert_eq!(completed, 0);
    assert_eq!(
        episode_state(&db, in_flight).await,
        ("Downloading".to_owned(), 0, Some(owner)),
        "a paused pass never steals or strands a live lease"
    );

    // Resume: claiming recovers. The unreachable enclosure fails the download
    // after the claim, and the durable failure bookkeeping proves the claim
    // happened (attempt recorded, lease released — never stranded).
    set_paused(&db, POOL_PODCAST, false).await;
    let _ = run_podcast_once(&db.pool, &config)
        .await
        .expect("resumed pass claims again");
    let (status, attempts, lease) = episode_state(&db, queued).await;
    assert_eq!(attempts, 1, "resumed pool claimed and accounted the work");
    assert_eq!(status, "Queued");
    assert_eq!(lease, None, "failure bookkeeping released the lease");
    assert_eq!(
        episode_state(&db, in_flight).await,
        ("Downloading".to_owned(), 0, Some(owner)),
        "the live lease is still untouched after resume"
    );

    db.drop().await;
}

#[tokio::test]
async fn paused_policy_pool_claims_nothing_and_in_flight_lease_is_intact() {
    let db = TestDb::create("worker_pool_policy").await;
    let config = PolicyConfig {
        batch_size: 4,
        concurrency: 2,
        ..PolicyConfig::default()
    };
    let engine = policy_engine(&db, &config).await;
    let first =
        phase5_common::seed_cataloged_release(&db, 41, CatalogFixturePresentation::default()).await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        42,
        CatalogFixturePresentation {
            release_title: "Paused Pool Second Release",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;

    // Simulate an in-flight claim from before the pause.
    let prior_worker = WorkerId::new();
    let claimed = PolicyStore
        .claim_cataloged(
            &db.pool,
            &prior_worker,
            &test_config(),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("pre-pause claim");
    assert_eq!(claimed.len(), 2);

    set_paused(&db, POOL_POLICY, true).await;
    let stats = run_policy_once(
        &engine,
        &config,
        &PolicyStore,
        &db.pool,
        &test_config(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("paused pass");
    assert_eq!(stats.claimed, 0, "a paused pool claims nothing");
    assert_eq!(stats.evaluated, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_decisions")
            .fetch_one(&db.pool)
            .await
            .expect("count decisions"),
        0
    );
    for fixture in [&first, &second] {
        let item = melodee_persistence::WorkItemStore
            .get(&db.pool, fixture.work_item_id())
            .await
            .expect("in-flight item");
        assert_eq!(
            item.lease_owner.as_deref(),
            Some(prior_worker.as_str()),
            "pause never strands or steals an in-flight lease"
        );
    }

    // The lease contract still governs in-flight work: the owner can finish
    // or release it while the pool is paused.
    PolicyStore
        .release_lease(
            &db.pool,
            first.work_item_id(),
            &prior_worker,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("release in-flight claim during pause");
    PolicyStore
        .release_lease(
            &db.pool,
            second.work_item_id(),
            &prior_worker,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("release second in-flight claim");

    // Resume: the next pass claims and evaluates again.
    set_paused(&db, POOL_POLICY, false).await;
    let stats = run_policy_once(
        &engine,
        &config,
        &PolicyStore,
        &db.pool,
        &test_config(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("resumed pass");
    assert_eq!(stats.claimed, 2);
    assert_eq!(stats.evaluated, 2);
    assert_eq!(stats.decisions_inserted + stats.decisions_adopted, 2);

    first.assert_media_paths_absent();
    second.assert_media_paths_absent();
    db.drop().await;
}

#[tokio::test]
async fn paused_discovery_pool_skips_reconciliation_and_resume_converges() {
    let db = TestDb::create("worker_pool_discovery").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        43,
        CatalogFixturePresentation {
            release_title: "Paused Pool Discovery Release",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let track_id: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
        .bind(fixture.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("seeded track");
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,normalized_email,\
         password_hash,password_hash_algorithm,stream_signing_key,created_at,updated_at) \
         VALUES($1,$2,'pool-discovery-user','POOL-DISCOVERY-USER','pool-discovery@example.test',\
         'POOL-DISCOVERY@EXAMPLE.TEST',$3,'bcrypt','pool-discovery-key',now(),now())",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .execute(&db.pool)
    .await
    .expect("insert listener");
    let listener: Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE username='pool-discovery-user'")
            .fetch_one(&db.pool)
            .await
            .expect("listener id");
    sqlx::query(
        "INSERT INTO native_track_plays(id,user_id,track_id,client_event_id,played_at,submitted_at,completed) \
         VALUES($1,$2,$3,'pool-discovery-play',now(),now(),true)",
    )
    .bind(Uuid::now_v7())
    .bind(listener)
    .bind(track_id)
    .execute(&db.pool)
    .await
    .expect("insert play");

    let config = DiscoveryWorkerConfig::default();
    async fn popularity(db: &TestDb, track_id: Uuid) -> Option<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT popularity FROM native_recommendation_candidates \
             WHERE entity_type='track' AND entity_id=$1",
        )
        .bind(track_id)
        .fetch_optional(&db.pool)
        .await
        .expect("popularity")
    }

    // Durable triggers keep the projection exact before any pass.
    assert_eq!(popularity(&db, track_id).await, Some(1));
    // Simulate restore-window drift a maintenance pass would repair.
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=0")
        .execute(&db.pool)
        .await
        .expect("simulate drift");

    set_paused(&db, POOL_DISCOVERY, true).await;
    let stats = run_discovery_once(&db.pool, &config)
        .await
        .expect("paused pass is a clean no-op");
    assert_eq!(
        stats,
        melodee_persistence::NativeDiscoveryMaintenanceStats::default()
    );
    assert_eq!(
        popularity(&db, track_id).await,
        Some(0),
        "paused pass reconciles nothing"
    );

    set_paused(&db, POOL_DISCOVERY, false).await;
    let stats = run_discovery_once(&db.pool, &config)
        .await
        .expect("resumed pass reconciles");
    assert!(
        stats.candidates_corrected >= 1,
        "resume converges: {stats:?}"
    );
    assert_eq!(popularity(&db, track_id).await, Some(1));

    fixture.assert_media_paths_absent();
    db.drop().await;
}

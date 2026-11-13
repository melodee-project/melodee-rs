//! persistence reads for the admin dashboard,
//! policy-rule, and worker-pool surfaces: rolling-window work-item counts,
//! the hourly completion series, unresolved-identity and pending-decision
//! attention counts, rule-set version history, durable rule match counts,
//! dry-run sample enrichment, and per-pool queue metrics.

#![allow(clippy::expect_used)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, seed_ready_item, test_config};
use melodee_domain::{PolicyActor, PolicyRuleSet, PolicyRuleSetId};
use melodee_persistence::{
    CatalogStore, POOL_DISCOVERY, POOL_PODCAST, POOL_POLICY, PolicyStore, WorkItemStore,
    WorkerPoolStore, WorkerPoolStoreError,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn window_counts_and_completed_hourly_track_the_rolling_window() {
    let db = TestDb::create("admin_window_counts").await;
    let store = WorkItemStore;
    let now = OffsetDateTime::now_utc();

    // Empty database: every aggregate is zero/empty.
    let counts = store
        .window_counts(&db.pool, now - time::Duration::hours(24))
        .await
        .expect("window counts");
    assert_eq!(counts.failed, 0);
    assert_eq!(counts.completed, 0);
    // The series is a zero-filled trailing window, so an idle hour is a data
    // point (0) rather than an absent bucket: that is what lets the dashboard
    // chart plot a real 24-hour axis.
    let idle_hourly = store
        .completed_hourly(&db.pool, now - time::Duration::hours(24))
        .await
        .expect("hourly");
    assert!(
        (24..=26).contains(&idle_hourly.len()),
        "expected a ~24h hour spine, got {}",
        idle_hourly.len()
    );
    assert!(idle_hourly.iter().all(|bucket| bucket.count == 0));

    let failed_id = seed_ready_item(&store, &db.pool, "admin/window/failed").await;
    let completed_id = seed_ready_item(&store, &db.pool, "admin/window/completed").await;
    let stale_id = seed_ready_item(&store, &db.pool, "admin/window/stale").await;
    // Test-only backdating of durable columns (the public transition path
    // always stamps "now").
    sqlx::query("UPDATE release_work_items SET state='failed', updated_at=$1 WHERE id=$2")
        .bind(now)
        .bind(failed_id)
        .execute(&db.pool)
        .await
        .expect("backdate failure");
    // completed_at is only consistent with a terminal state (schema
    // constraint); the test backdates both together.
    sqlx::query(
        "UPDATE release_work_items SET state='indexed', completed_at=$1, updated_at=$1 WHERE id=$2",
    )
    .bind(now - time::Duration::hours(2))
    .bind(completed_id)
    .execute(&db.pool)
    .await
    .expect("backdate completion");
    sqlx::query(
        "UPDATE release_work_items SET state='indexed', completed_at=$1, updated_at=$1 WHERE id=$2",
    )
    .bind(now - time::Duration::hours(48))
    .bind(stale_id)
    .execute(&db.pool)
    .await
    .expect("backdate stale completion");

    let counts = store
        .window_counts(&db.pool, now - time::Duration::hours(24))
        .await
        .expect("window counts");
    assert_eq!(counts.failed, 1, "only the in-window failure counts");
    assert_eq!(counts.completed, 1, "the 48h-old completion is excluded");

    let hourly = store
        .completed_hourly(&db.pool, now - time::Duration::hours(24))
        .await
        .expect("hourly");
    let bucket = now - time::Duration::hours(2);
    let populated = hourly
        .iter()
        .filter(|entry| entry.count != 0)
        .collect::<Vec<_>>();
    assert_eq!(
        populated.len(),
        1,
        "exactly one hour carries the completion"
    );
    assert_eq!(populated[0].count, 1);
    assert!(
        populated[0].hour <= bucket && populated[0].hour > bucket - time::Duration::hours(1),
        "bucket is the truncated hour of the completion"
    );

    db.drop().await;
}

#[tokio::test]
async fn policy_admin_reads_cover_counts_history_match_counts_and_samples() {
    let db = TestDb::create("admin_policy_reads").await;
    let store = PolicyStore;
    let now = OffsetDateTime::now_utc();

    // Two immutable versions of one named rule set.
    let version_one = phase5_common::policy_rule_set(PolicyRuleSetId::new(), now);
    let mut version_two = version_one.clone();
    version_two.id = PolicyRuleSetId::new();
    version_two.version = 2;
    version_two.created_by = PolicyActor::System {
        policy_rule_set_version: 2,
    };
    version_two.rules.retain(|rule| rule.id != "unknown");
    let version_two = PolicyRuleSet::new(version_two).expect("valid second version");
    store
        .ensure_rule_set(&db.pool, &version_one, now)
        .await
        .expect("persist version one");
    store
        .ensure_rule_set(&db.pool, &version_two, now)
        .await
        .expect("persist version two");

    let versions = store
        .list_rule_set_versions(&db.pool, &version_one.name)
        .await
        .expect("version history");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].version, 1);
    assert_eq!(versions[1].version, 2);
    assert!(
        versions[1].rules.len() < versions[0].rules.len(),
        "version two drops the unknown rule"
    );

    // A pending durable decision from the legal evaluation path.
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7,
        CatalogFixturePresentation {
            release_title: "Admin Read Fixture",
            release_artist: "Admin Artist",
            track_title: "Admin Track",
            track_artist: "Admin Artist",
            release_date: "2001-02-03",
        },
    )
    .await;
    let worker = melodee_persistence::WorkerId::new();
    let mut claims = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim");
    let claim = claims.pop().expect("one claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load context");
    store
        .commit_evaluation(
            &db.pool,
            &worker,
            &phase5_common::unknown_review_commit(&context, version_one.id, now),
        )
        .await
        .expect("commit evaluation");

    assert_eq!(
        store
            .count_pending_decisions(&db.pool)
            .await
            .expect("pending decisions"),
        1
    );
    let match_counts = store
        .rule_match_counts(&db.pool)
        .await
        .expect("rule match counts");
    assert_eq!(match_counts, vec![("unknown".to_string(), 1)]);

    let samples = store
        .test_sample_releases(&db.pool, &[fixture.release_id()])
        .await
        .expect("sample releases");
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].title, "Admin Read Fixture");
    assert_eq!(samples[0].artist.as_deref(), Some("Admin Artist"));

    // The sample read is bounded by the durable test-sample contract.
    let overbound: Vec<melodee_domain::ReleaseId> =
        (0..21).map(|_| melodee_domain::ReleaseId::new()).collect();
    assert!(
        matches!(
            store.test_sample_releases(&db.pool, &overbound).await,
            Err(melodee_persistence::PolicyStoreError::ScopeExceeded { .. })
        ),
        "more than MAX_POLICY_TEST_SAMPLES ids are rejected"
    );

    db.drop().await;
}

#[tokio::test]
async fn unresolved_identity_count_matches_the_review_queue_definition() {
    let db = TestDb::create("admin_identity_count").await;
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        CatalogStore
            .count_unresolved_observations(&db.pool)
            .await
            .expect("empty count"),
        0
    );

    let fixture = phase5_common::seed_cataloged_release(
        &db,
        8,
        CatalogFixturePresentation {
            release_title: "Identity Count Fixture",
            release_artist: "Counted Artist",
            track_title: "Counted Track",
            track_artist: "Counted Artist",
            release_date: "2002-03-04",
        },
    )
    .await;
    // The import auto-confirm leaves the queue empty; a raw observation
    // without an authoritative decision deepens it.
    assert_eq!(
        CatalogStore
            .count_unresolved_observations(&db.pool)
            .await
            .expect("post-import count"),
        0
    );
    sqlx::query(
        "INSERT INTO observed_artists (
            id, release_id, track_id, scope, source_ordinal, raw_value,
            normalized_value, display_value, source_pointer, source_payload,
            created_at
         ) VALUES ($1,$2,NULL,'release',9,' Counted Guest ','counted guest','Counted Guest','/fixture/guest',$3,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(fixture.release_id().into_uuid())
    .bind(serde_json::json!({ "value": "Counted Guest" }))
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed unresolved observation");
    assert_eq!(
        CatalogStore
            .count_unresolved_observations(&db.pool)
            .await
            .expect("seeded count"),
        1
    );

    db.drop().await;
}

#[tokio::test]
async fn worker_pool_metrics_reflect_durable_queues_only() {
    let db = TestDb::create("admin_pool_metrics").await;
    let now = OffsetDateTime::now_utc();

    // No durable queue: discovery maintenance reports no metrics; unknown
    // pools report none; invalid names are rejected.
    assert_eq!(
        WorkerPoolStore
            .metrics(&db.pool, POOL_DISCOVERY, now)
            .await
            .expect("discovery metrics"),
        None
    );
    assert!(
        matches!(
            WorkerPoolStore.metrics(&db.pool, "NotAPool", now).await,
            Err(WorkerPoolStoreError::InvalidPoolName)
        ),
        "invalid pool names are rejected"
    );

    // Empty durable queues report real zeros, not nulls.
    let podcast = WorkerPoolStore
        .metrics(&db.pool, POOL_PODCAST, now)
        .await
        .expect("podcast metrics")
        .expect("podcast has a durable queue");
    assert_eq!(podcast.queued, 0);
    assert_eq!(podcast.running, 0);
    assert_eq!(podcast.oldest_queued_at, None);

    // Policy queue: a cataloged work item is queued until claimed.
    let _fixture = phase5_common::seed_cataloged_release(
        &db,
        9,
        CatalogFixturePresentation {
            release_title: "Pool Metrics Fixture",
            release_artist: "Pool Artist",
            track_title: "Pool Track",
            track_artist: "Pool Artist",
            release_date: "2003-04-05",
        },
    )
    .await;
    let policy = WorkerPoolStore
        .metrics(&db.pool, POOL_POLICY, now)
        .await
        .expect("policy metrics")
        .expect("policy has a durable queue");
    assert_eq!(policy.queued, 1);
    assert_eq!(policy.running, 0);
    assert!(policy.oldest_queued_at.is_some());
    assert_eq!(policy.completed_last_hour, 0);

    // A committed policy evaluation lands in the hourly completion count via
    // the durable `policy.decision_applied` event.
    let store = PolicyStore;
    let worker = melodee_persistence::WorkerId::new();
    let mut claims = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim");
    let claim = claims.pop().expect("one claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load context");
    let rule_set = phase5_common::policy_rule_set(PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist rules");
    store
        .commit_evaluation(
            &db.pool,
            &worker,
            &phase5_common::unknown_review_commit(&context, rule_set.id, now),
        )
        .await
        .expect("commit evaluation");
    let policy = WorkerPoolStore
        .metrics(&db.pool, POOL_POLICY, now + time::Duration::minutes(1))
        .await
        .expect("policy metrics after commit")
        .expect("policy has a durable queue");
    assert_eq!(policy.queued, 0, "the committed item left the queue");
    assert_eq!(policy.completed_last_hour, 1);

    db.drop().await;
}

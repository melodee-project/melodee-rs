//! Bounded processed-outbox retention acceptance evidence (ADR-0214).
//!
//! The scenarios exercise the operator contract: only eligible successful
//! detail is deleted, the exact distinct-Track chart survives with raw rows
//! gone, replay safety is preserved, and rebuilds/acknowledgements fence the
//! maintenance decision.

#![allow(clippy::expect_used, clippy::panic)]

mod common;

use common::TestDb;
use melodee_persistence::{
    MAX_PROCESSED_OUTBOX_RETENTION_DAYS, OutboxRetentionConfig, SearchProjectionStore,
    finalize_closed_hours, prune_processed_outbox, retention_status,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn seed_processed_history(db: &TestDb, rows: i64) {
    // 100,000 successful rows spread across three closed hours ten days ago,
    // with distinct Track ids so an independent DISTINCT count can be
    // compared with the aggregate.
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, processed_at, created_at
         )
         SELECT 'tantivy.catalog', 'track',
                md5(source.i::text)::uuid, 'upsert', 1,
                now() - interval '10 days', 0,
                date_trunc('hour', now() - interval '10 days')
                    + ((source.i % 3) || ' hours')::interval,
                now() - interval '10 days'
           FROM generate_series(1, $1) AS source(i)",
    )
    .bind(rows)
    .execute(&db.pool)
    .await
    .expect("seed processed history");
}

async fn seed_controls(db: &TestDb) {
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, processed_at, created_at
         ) VALUES
           ('tantivy.catalog','track',uuidv7(),'upsert',1,now() - interval '1 hour',0,
            least(now(), date_trunc('hour', now()) + interval '1 second'),
            now() - interval '1 hour'),
           ('tantivy.catalog','track',uuidv7(),'upsert',1,now(),0,NULL,now()),
           ('tantivy.catalog','track',uuidv7(),'upsert',1,now(),3,NULL,now()),
           ('tantivy.catalog','track',uuidv7(),'upsert',1,now() - interval '10 days',1,NULL,now())",
    )
    .execute(&db.pool)
    .await
    .expect("seed recent control");
    sqlx::query(
        "UPDATE catalog_projection_outbox
            SET lease_owner = CASE WHEN id = $1 THEN 'maintenance-test' END,
                lease_expires_at = CASE WHEN id = $1 THEN now() + interval '5 minutes' END
          WHERE lease_owner IS NULL AND processed_at IS NULL AND failed_at IS NULL
            AND last_error = 'lease-control'",
    )
    .bind(0_i64)
    .execute(&db.pool)
    .await
    .expect("no-op lease control");
    // Mark one pending row as leased and one permanently failed.
    sqlx::query(
        "UPDATE catalog_projection_outbox
            SET lease_owner='maintenance-test', lease_expires_at=now() + interval '5 minutes'
          WHERE id = (SELECT id FROM catalog_projection_outbox
                       WHERE processed_at IS NULL AND failed_at IS NULL
                         AND lease_owner IS NULL
                       ORDER BY id DESC LIMIT 1)",
    )
    .execute(&db.pool)
    .await
    .expect("lease control");
    sqlx::query(
        "UPDATE catalog_projection_outbox
            SET failed_at = now(), failure_kind = 'document', last_error = 'permanent'
          WHERE id = (SELECT id FROM catalog_projection_outbox
                       WHERE processed_at IS NULL AND failed_at IS NULL
                         AND lease_owner IS NULL
                       ORDER BY id DESC LIMIT 1)",
    )
    .execute(&db.pool)
    .await
    .expect("failure control");
}

async fn independent_buckets(db: &TestDb) -> Vec<(OffsetDateTime, i64)> {
    sqlx::query_as(
        "SELECT date_trunc('hour', processed_at), count(DISTINCT entity_id)
           FROM catalog_projection_outbox
          WHERE projection_name='tantivy.catalog' AND entity_type='track'
            AND processed_at IS NOT NULL
            AND processed_at < date_trunc('hour', now())
          GROUP BY 1 ORDER BY 1",
    )
    .fetch_all(&db.pool)
    .await
    .expect("independent buckets")
}

async fn finalize_and_load_chart(db: &TestDb, since: OffsetDateTime) -> Vec<(OffsetDateTime, i64)> {
    SearchProjectionStore::processed_tracks_hourly(&db.pool, since)
        .await
        .expect("chart series")
        .into_iter()
        .map(|row| (row.hour, row.count))
        .collect()
}

/// All and only the eligible rows are deleted, the exact chart survives, and a
/// second pass is a no-op.
#[tokio::test]
async fn pruning_keeps_exact_hourly_totals_and_controls() {
    let db = TestDb::create("outbox_retention_prune").await;
    let now = OffsetDateTime::now_utc();
    seed_processed_history(&db, 100_000).await;
    seed_controls(&db).await;
    let controls = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_projection_outbox
          WHERE processed_at IS NULL OR processed_at >= now() - interval '1 hour'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("control count");
    // Acknowledge the backlog so the checkpoint high water covers it.
    sqlx::query(
        "INSERT INTO search_projection_checkpoint (
            projection_name, last_processed_outbox_id, updated_at
         ) VALUES ('tantivy.catalog',
                   (SELECT max(id) FROM catalog_projection_outbox), now())
         ON CONFLICT (projection_name) DO UPDATE
         SET last_processed_outbox_id = EXCLUDED.last_processed_outbox_id",
    )
    .execute(&db.pool)
    .await
    .expect("seed checkpoint");

    let since = (now - Duration::days(11))
        .replace_minute(0)
        .expect("bucket");
    let expected = independent_buckets(&db).await;
    let config = OutboxRetentionConfig::default();

    let preview = prune_processed_outbox(&db.pool, config, false, 32, None, now)
        .await
        .expect("dry run");
    assert!(preview.dry_run);
    assert_eq!(preview.rows_pruned, 0);
    assert_eq!(
        preview.status.eligible, 100_000,
        "the dry run reports exactly the eligible rows"
    );
    assert!(preview.status.protected > 0);
    assert_eq!(preview.status.failed, 1);

    // Each hourly pass is bounded; the schedule resumes until settled.
    let mut applied = prune_processed_outbox(&db.pool, config, true, 32, None, now)
        .await
        .expect("apply retention");
    while applied.status.eligible > 0 {
        let next = prune_processed_outbox(&db.pool, config, true, 32, None, now)
            .await
            .expect("resume retention");
        applied.rows_pruned += next.rows_pruned;
        applied.batches += next.batches;
        applied.status = next.status;
    }
    assert_eq!(applied.rows_pruned, 100_000);
    assert!(
        applied.batches > 1,
        "deletion stays in bounded batches: {}",
        applied.batches
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_projection_outbox WHERE processed_at IS NOT NULL
              AND processed_at < now() - interval '7 days'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("old rows removed"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_projection_outbox
              WHERE processed_at IS NULL OR processed_at >= now() - interval '1 hour'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("controls retained"),
        controls
    );
    assert!(applied.status.highest_pruned_id > 0);
    assert_eq!(applied.status.eligible, 0);

    // The chart is identical, now served from exact hourly totals.
    let chart = finalize_and_load_chart(&db, since).await;
    let bucket = |rows: &[(OffsetDateTime, i64)], hour: OffsetDateTime| {
        rows.iter()
            .find(|(value, _)| *value == hour)
            .map(|(_, count)| *count)
            .unwrap_or(0)
    };
    for (hour, count) in &expected {
        assert_eq!(
            bucket(&chart, *hour),
            *count,
            "finalized hour {hour} keeps its exact distinct-Track count (expected={expected:?})"
        );
    }
    let current_hour = now
        .replace_minute(0)
        .and_then(|value| value.replace_second(0))
        .and_then(|value| value.replace_nanosecond(0))
        .expect("current hour");
    assert!(
        bucket(&chart, current_hour) > 0,
        "the live partial hour is still served from raw detail"
    );

    let again = prune_processed_outbox(&db.pool, config, true, 32, None, now)
        .await
        .expect("second pass");
    assert_eq!(
        again.rows_pruned, 0,
        "a settled second pass changes nothing"
    );
    assert_eq!(
        finalize_closed_hours(&db.pool, now)
            .await
            .expect("refinalize"),
        0
    );
}

/// A partial pass resumes; two workers cannot double-delete or double-count.
#[tokio::test]
async fn interrupted_and_concurrent_pruning_converge() {
    let db = TestDb::create("outbox_retention_concurrent").await;
    let now = OffsetDateTime::now_utc();
    seed_processed_history(&db, 5_000).await;
    sqlx::query(
        "INSERT INTO search_projection_checkpoint (
            projection_name, last_processed_outbox_id, updated_at
         ) VALUES ('tantivy.catalog',
                   (SELECT max(id) FROM catalog_projection_outbox), now())",
    )
    .execute(&db.pool)
    .await
    .expect("seed checkpoint");
    let config = OutboxRetentionConfig::default();

    // One bounded batch, then a resume: the cursor and tombstone stay coherent.
    let first = prune_processed_outbox(&db.pool, config, true, 1, None, now)
        .await
        .expect("partial pass");
    assert_eq!(first.rows_pruned, 1_000);
    assert!(first.status.eligible > 0);
    let (left, right) = tokio::join!(
        prune_processed_outbox(&db.pool, config, true, 32, None, now),
        prune_processed_outbox(&db.pool, config, true, 32, None, now),
    );
    let left = left.expect("worker a");
    let right = right.expect("worker b");
    assert_eq!(
        first.rows_pruned + left.rows_pruned + right.rows_pruned,
        5_000,
        "every eligible row is deleted exactly once"
    );
    let status = retention_status(&db.pool, config, now, None)
        .await
        .expect("status");
    assert_eq!(status.eligible, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM search_outbox_hourly_totals
              WHERE projection_name='tantivy.catalog'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("hourly totals"),
        3
    );
}

/// A nonterminal rebuild pauses pruning; finishing it releases the pause.
#[tokio::test]
async fn rebuild_blocks_pruning_until_it_finishes() {
    let db = TestDb::create("outbox_retention_rebuild").await;
    let now = OffsetDateTime::now_utc();
    seed_processed_history(&db, 2_000).await;
    sqlx::query(
        "INSERT INTO search_projection_checkpoint (
            projection_name, last_processed_outbox_id, updated_at
         ) VALUES ('tantivy.catalog',
                   (SELECT max(id) FROM catalog_projection_outbox), now())",
    )
    .execute(&db.pool)
    .await
    .expect("seed checkpoint");
    let search_root = "/tmp/melodee-retention-rebuild";
    sqlx::query(
        "INSERT INTO search_rebuild_jobs (
            job_id, search_root, requested_at, requested_by, status, available_at
         ) VALUES (uuidv7(), $1, now(), 'test', 'requested', now())",
    )
    .bind(search_root)
    .execute(&db.pool)
    .await
    .expect("request rebuild");

    let config = OutboxRetentionConfig::default();
    let blocked = prune_processed_outbox(&db.pool, config, true, 32, Some(search_root), now)
        .await
        .expect("blocked pass");
    assert_eq!(blocked.rows_pruned, 0);
    assert_eq!(
        blocked.status.blocking_rebuild.as_deref(),
        Some("requested")
    );

    sqlx::query("UPDATE search_rebuild_jobs SET status='activated', finished_at=now()")
        .execute(&db.pool)
        .await
        .expect("finish rebuild");
    let released = prune_processed_outbox(&db.pool, config, true, 32, Some(search_root), now)
        .await
        .expect("released pass");
    assert_eq!(released.rows_pruned, 2_000);
    assert!(released.status.blocking_rebuild.is_none());
}

/// Pruning through an empty raw table keeps the chart, and a stale upsert for a
/// pruned revision is suppressed while the current revision still enqueues.
#[tokio::test]
async fn replay_after_prune_is_suppressed_and_chart_is_stable() {
    let db = TestDb::create("outbox_retention_replay").await;
    let now = OffsetDateTime::now_utc();
    let entity_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, processed_at, created_at
         ) VALUES
           ('tantivy.catalog','track',$1,'upsert',1,now() - interval '10 days',0,
            now() - interval '10 days', now() - interval '10 days')",
    )
    .bind(entity_id)
    .execute(&db.pool)
    .await
    .expect("seed processed row");
    sqlx::query(
        "INSERT INTO search_projection_checkpoint (
            projection_name, last_processed_outbox_id, updated_at
         ) VALUES ('tantivy.catalog',
                   (SELECT max(id) FROM catalog_projection_outbox), now())",
    )
    .execute(&db.pool)
    .await
    .expect("seed checkpoint");
    let since = (now - Duration::days(2)).replace_minute(0).expect("bucket");
    let before = finalize_and_load_chart(&db, since).await;
    let config = OutboxRetentionConfig::default();
    assert_eq!(
        prune_processed_outbox(&db.pool, config, true, 32, None, now)
            .await
            .expect("prune")
            .rows_pruned,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_projection_outbox")
            .fetch_one(&db.pool)
            .await
            .expect("empty raw table"),
        0
    );
    assert_eq!(
        finalize_and_load_chart(&db, since).await,
        before,
        "pruning through an empty raw table cannot lose the chart bucket"
    );

    // Stale replay of the pruned revision is suppressed by the tombstone.
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog','track',$1,'upsert',1,now(),0,now())
         ON CONFLICT (projection_name, entity_type, entity_id, entity_revision) DO NOTHING",
    )
    .bind(entity_id)
    .execute(&db.pool)
    .await
    .expect("stale replay");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_projection_outbox")
            .fetch_one(&db.pool)
            .await
            .expect("stale replay suppressed"),
        0
    );
    // The next genuine revision still enqueues, and so does the next import.
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog','track',$1,'upsert',2,now(),0,now())",
    )
    .bind(entity_id)
    .execute(&db.pool)
    .await
    .expect("current revision enqueues");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_projection_outbox")
            .fetch_one(&db.pool)
            .await
            .expect("current revision present"),
        1
    );
}

/// An acknowledgement that completes just after UTC midnight is counted in the
/// hour of its own `processed_at`, and finalization cannot lose it.
#[tokio::test]
async fn acknowledgements_crossing_midnight_keep_their_hour() {
    let db = TestDb::create("outbox_retention_midnight").await;
    // Both rows are processed a few minutes apart across a synthetic hour
    // boundary; finalization is anchored at the later time.
    let finalize_at = OffsetDateTime::now_utc();
    let late = finalize_at
        .replace_minute(0)
        .expect("hour")
        .replace_second(0)
        .expect("second")
        .replace_nanosecond(0)
        .expect("nanosecond")
        - Duration::hours(3);
    for offset_minutes in [58_i64, 61] {
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation, entity_revision,
                available_at, attempt_count, processed_at, created_at
             ) VALUES ('tantivy.catalog','track',uuidv7(),'upsert',1,$1,0,$2,$3)",
        )
        .bind(late + Duration::minutes(offset_minutes))
        .bind(late + Duration::minutes(offset_minutes))
        .bind(late)
        .execute(&db.pool)
        .await
        .expect("seed boundary row");
    }
    finalize_closed_hours(&db.pool, finalize_at)
        .await
        .expect("finalize closed hours");
    let buckets: Vec<(OffsetDateTime, i64)> = sqlx::query_as(
        "SELECT hour, indexed_tracks FROM search_outbox_hourly_totals
          ORDER BY hour",
    )
    .fetch_all(&db.pool)
    .await
    .expect("hourly totals");
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0].1, 1);
    assert_eq!(buckets[1].1, 1);
    assert_eq!(
        buckets[1].0 - buckets[0].0,
        Duration::hours(1),
        "the boundary row belongs to the later hour"
    );
}

/// The retention bounds are documented and enforced for configuration.
#[test]
fn retention_configuration_is_bounded() {
    assert_eq!(OutboxRetentionConfig::default().retention_days, 7);
    assert!(OutboxRetentionConfig::new(1).is_err());
    assert!(OutboxRetentionConfig::new(MAX_PROCESSED_OUTBOX_RETENTION_DAYS + 1).is_err());
    assert_eq!(
        OutboxRetentionConfig::new(30)
            .expect("valid")
            .retention_days,
        30
    );
}

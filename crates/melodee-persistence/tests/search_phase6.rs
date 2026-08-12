//! Phase 6 search leases, fencing, rebuild lifecycle, and schema integrity.

#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::{T0, TestDb};
use melodee_persistence::{
    SEARCH_PROJECTION_NAME, SearchDocumentCounts, SearchProjectionStore, SearchSnapshotReader,
    SearchStoreError,
};
use time::Duration as TimeDuration;
use uuid::Uuid;

#[tokio::test]
async fn outbox_claim_expiry_retry_and_ack_are_exclusive_and_fenced() {
    let db = TestDb::create("phase6_outbox_lease").await;
    let entity_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(entity_id)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed outbox");

    let first = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-a",
        8,
        Duration::from_secs(10),
        T0,
    )
    .await
    .expect("first claim");
    assert_eq!(first.len(), 1);
    let claimed_status = SearchProjectionStore::status(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "/tmp/phase6-outbox-test",
        T0 + TimeDuration::seconds(1),
    )
    .await
    .expect("claimed status");
    assert_eq!(claimed_status.pending, 0);
    assert_eq!(claimed_status.leased, 1);
    let none = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-b",
        8,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(1),
    )
    .await
    .expect("exclusive claim");
    assert!(none.is_empty());
    let ids = [first[0].id];
    let stale = SearchProjectionStore::acknowledge_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-a",
        &ids,
        T0 + TimeDuration::seconds(11),
    )
    .await;
    assert!(matches!(stale, Err(SearchStoreError::LeaseLost)));

    let reclaimed = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-b",
        8,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(11),
    )
    .await
    .expect("expired lease reclaim");
    assert_eq!(reclaimed.len(), 1);
    SearchProjectionStore::record_outbox_failure(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-b",
        &ids,
        "database",
        "transient test failure",
        Some(T0 + TimeDuration::seconds(15)),
        5,
        T0 + TimeDuration::seconds(12),
    )
    .await
    .expect("retry evidence");
    assert!(
        SearchProjectionStore::claim_outbox(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            "worker-c",
            8,
            Duration::from_secs(10),
            T0 + TimeDuration::seconds(14),
        )
        .await
        .expect("pre-backoff claim")
        .is_empty()
    );
    let retried = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-c",
        8,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(15),
    )
    .await
    .expect("retry claim");
    assert_eq!(retried[0].attempt_count, 1);
    SearchProjectionStore::acknowledge_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-c",
        &ids,
        T0 + TimeDuration::seconds(16),
    )
    .await
    .expect("fenced ack");
    let status = SearchProjectionStore::status(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "/tmp/phase6-outbox-test",
        T0 + TimeDuration::seconds(16),
    )
    .await
    .expect("status");
    assert_eq!(status.pending, 0);
    assert_eq!(status.failed, 0);
    assert_eq!(status.last_processed_outbox_id, ids[0]);

    assert!(
        SearchProjectionStore::claim_outbox(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            "idle-worker",
            8,
            Duration::from_secs(60),
            T0 + TimeDuration::seconds(17),
        )
        .await
        .expect("idle poll")
        .is_empty()
    );

    let permanent_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'delete', 1, $3, $3)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(permanent_id)
    .bind(T0 + TimeDuration::seconds(17))
    .execute(&db.pool)
    .await
    .expect("seed permanent-failure row");
    let permanent = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-d",
        8,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(17),
    )
    .await
    .expect("claim permanent row");
    SearchProjectionStore::record_outbox_failure(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-d",
        &[permanent[0].id],
        "invalid_catalog_row",
        "operator action is required",
        None,
        5,
        T0 + TimeDuration::seconds(18),
    )
    .await
    .expect("record permanent failure");
    let status = SearchProjectionStore::status(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "/tmp/phase6-outbox-test",
        T0 + TimeDuration::seconds(18),
    )
    .await
    .expect("failed status");
    assert_eq!(status.pending, 0);
    assert_eq!(status.failed, 1);
    assert_eq!(
        status.latest_failure_kind.as_deref(),
        Some("invalid_catalog_row")
    );
    assert_eq!(
        status.latest_failure_message.as_deref(),
        Some("operator action is required")
    );

    let exhausted_entity = Uuid::now_v7();
    let exhausted_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'delete', 1, $3, $3)
         RETURNING id",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(exhausted_entity)
    .bind(T0 + TimeDuration::seconds(19))
    .fetch_one(&db.pool)
    .await
    .expect("seed retry-exhaustion row");
    for (worker, claim_at, retry_at) in [
        (
            "worker-e",
            T0 + TimeDuration::seconds(19),
            T0 + TimeDuration::seconds(20),
        ),
        (
            "worker-f",
            T0 + TimeDuration::seconds(20),
            T0 + TimeDuration::seconds(21),
        ),
    ] {
        let claimed = SearchProjectionStore::claim_outbox(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            worker,
            8,
            Duration::from_secs(10),
            claim_at,
        )
        .await
        .expect("claim retry-exhaustion row");
        assert_eq!(claimed.len(), 1);
        SearchProjectionStore::record_outbox_failure(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            worker,
            &[exhausted_id],
            "index_io",
            "persistent retryable index failure",
            Some(retry_at),
            2,
            claim_at + TimeDuration::milliseconds(100),
        )
        .await
        .expect("record bounded retry failure");
    }
    let exhausted = sqlx::query_as::<_, (i32, Option<String>, bool, bool)>(
        "SELECT attempt_count, failure_kind, failed_at IS NOT NULL,
                lease_owner IS NULL AND lease_expires_at IS NULL
         FROM catalog_projection_outbox WHERE id = $1",
    )
    .bind(exhausted_id)
    .fetch_one(&db.pool)
    .await
    .expect("read exhausted row");
    assert_eq!(exhausted, (2, Some("index_io".to_string()), true, true));
    assert!(
        SearchProjectionStore::claim_outbox(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            "worker-g",
            8,
            Duration::from_secs(10),
            T0 + TimeDuration::seconds(30),
        )
        .await
        .expect("permanent row is not reclaimable")
        .is_empty()
    );
    db.drop().await;
}

#[tokio::test]
async fn rebuild_reclaims_expired_owner_and_atomically_retires_prior_generation() {
    let db = TestDb::create("phase6_rebuild_lifecycle").await;
    let root = "/tmp/melodee-phase6-lifecycle";
    let first_job = SearchProjectionStore::request_rebuild(&db.pool, root, "admin-a", T0)
        .await
        .expect("request first rebuild");
    assert!(matches!(
        SearchProjectionStore::request_rebuild(&db.pool, root, "admin-b", T0).await,
        Err(SearchStoreError::RebuildConflict(_))
    ));
    let first_claim = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        "worker-a",
        1,
        Duration::from_secs(10),
        T0,
    )
    .await
    .expect("claim first rebuild")
    .expect("job claimed");
    assert_eq!(first_claim.job_id, first_job.job_id);
    assert!(
        SearchProjectionStore::claim_rebuild(
            &db.pool,
            root,
            "worker-b",
            1,
            Duration::from_secs(10),
            T0 + TimeDuration::seconds(1),
        )
        .await
        .expect("exclusive rebuild claim")
        .is_none()
    );
    let reclaimed = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        "worker-b",
        1,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(11),
    )
    .await
    .expect("reclaim rebuild")
    .expect("expired rebuild reclaimed");
    assert_eq!(reclaimed.generation_id, first_claim.generation_id);
    assert_eq!(reclaimed.attempt_count, 2);
    let counts = SearchDocumentCounts {
        artists: 0,
        releases: 0,
        tracks: 0,
    };
    SearchProjectionStore::mark_generation_validated(
        &db.pool,
        reclaimed.job_id,
        "worker-b",
        &counts,
        &"a".repeat(64),
        0,
        T0 + TimeDuration::seconds(12),
    )
    .await
    .expect("validate first generation");
    let first_active = SearchProjectionStore::activate_generation(
        &db.pool,
        reclaimed.job_id,
        "worker-b",
        T0 + TimeDuration::seconds(13),
    )
    .await
    .expect("activate first generation");
    assert_eq!(first_active.status, "active");

    let second_job = SearchProjectionStore::request_rebuild(
        &db.pool,
        root,
        "admin-c",
        T0 + TimeDuration::seconds(14),
    )
    .await
    .expect("request second rebuild");
    let second = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        "worker-c",
        1,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(14),
    )
    .await
    .expect("claim second rebuild")
    .expect("second rebuild claimed");
    SearchProjectionStore::mark_generation_validated(
        &db.pool,
        second.job_id,
        "worker-c",
        &counts,
        &"b".repeat(64),
        0,
        T0 + TimeDuration::seconds(15),
    )
    .await
    .expect("validate second generation");
    let second_active = SearchProjectionStore::activate_generation(
        &db.pool,
        second_job.job_id,
        "worker-c",
        T0 + TimeDuration::seconds(16),
    )
    .await
    .expect("activate second generation");
    assert_ne!(first_active.generation_id, second_active.generation_id);
    assert_eq!(
        SearchProjectionStore::get_generation(&db.pool, first_active.generation_id)
            .await
            .expect("read retired generation")
            .status,
        "retired"
    );

    let failed_job = SearchProjectionStore::request_rebuild(
        &db.pool,
        root,
        "admin-d",
        T0 + TimeDuration::seconds(17),
    )
    .await
    .expect("request failure-evidence rebuild");
    let failed_claim = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        "worker-d",
        1,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(17),
    )
    .await
    .expect("claim failure-evidence rebuild")
    .expect("failure-evidence rebuild claimed");
    assert_eq!(failed_claim.job_id, failed_job.job_id);
    SearchProjectionStore::record_rebuild_failure(
        &db.pool,
        failed_job.job_id,
        "worker-d",
        "rebuild_contract",
        "operator must correct the generation root",
        T0 + TimeDuration::seconds(18),
    )
    .await
    .expect("record permanent rebuild failure");
    let failed_status = SearchProjectionStore::status(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        root,
        T0 + TimeDuration::seconds(18),
    )
    .await
    .expect("read failed rebuild status");
    assert_eq!(
        failed_status.latest_rebuild_status.as_deref(),
        Some("failed")
    );
    assert_eq!(
        failed_status.latest_rebuild_error.as_deref(),
        Some("operator must correct the generation root")
    );
    assert_eq!(
        SearchProjectionStore::get_generation(
            &db.pool,
            failed_claim.generation_id.expect("failed generation")
        )
        .await
        .expect("read failed generation")
        .status,
        "failed"
    );
    db.drop().await;
}

#[tokio::test]
async fn repeatable_read_snapshot_excludes_concurrent_write_that_catchup_observes() {
    let db = TestDb::create("phase6_snapshot_catchup").await;
    let mut snapshot = SearchSnapshotReader::begin(&db.pool)
        .await
        .expect("begin repeatable-read snapshot");
    assert_eq!(snapshot.source_high_water_mark(), 0);
    let artist_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO artists (
            id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, status, created_at, updated_at
         ) VALUES ($1, $2, 'Concurrent Artist', 'CONCURRENT ARTIST',
                   'Concurrent Artist', 'person', 'verified', $3, $3)",
    )
    .bind(artist_id)
    .bind(Uuid::now_v7())
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("insert concurrent catalog row");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(artist_id)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("insert concurrent outbox row");
    assert!(
        snapshot
            .artists_page(None, 10)
            .await
            .expect("snapshot page")
            .is_empty(),
        "the consistent snapshot must not partially see a later commit"
    );
    snapshot.finish().await.expect("finish snapshot");
    let latest = SearchProjectionStore::max_outbox_id(&db.pool)
        .await
        .expect("latest outbox");
    let catchup = SearchProjectionStore::load_outbox_after(&db.pool, 0, latest, 10)
        .await
        .expect("catch-up rows");
    assert_eq!(catchup.len(), 1);
    assert_eq!(catchup[0].entity_id, artist_id);
    db.drop().await;
}

#[tokio::test]
async fn search_schema_rejects_invalid_identity_counts_and_active_pointer() {
    let db = TestDb::create("phase6_search_schema").await;
    let invalid_id =
        Uuid::parse_str("123e4567-e89b-42d3-a456-426614174000").expect("valid UUIDv4 test vector");
    assert!(
        sqlx::query(
            "INSERT INTO search_generations (
                generation_id, search_root, schema_version, status, created_at
             ) VALUES ($1, '/tmp/invalid-v4', 1, 'building', $2)",
        )
        .bind(invalid_id)
        .bind(T0)
        .execute(&db.pool)
        .await
        .is_err()
    );

    let invalid_counts = Uuid::now_v7();
    assert!(
        sqlx::query(
            "INSERT INTO search_generations (
                generation_id, search_root, schema_version, status,
                document_counts, created_at
             ) VALUES ($1, '/tmp/invalid-counts', 1, 'building', '{}'::jsonb, $2)",
        )
        .bind(invalid_counts)
        .bind(T0)
        .execute(&db.pool)
        .await
        .is_err()
    );

    let invalid_live_counts = Uuid::now_v7();
    assert!(
        sqlx::query(
            "INSERT INTO search_generations (
                generation_id, search_root, schema_version, status,
                current_document_counts, created_at
             ) VALUES ($1, '/tmp/invalid-live-counts', 1, 'building',
                       '{\"artists\":-1,\"releases\":0,\"tracks\":0}'::jsonb, $2)",
        )
        .bind(invalid_live_counts)
        .bind(T0)
        .execute(&db.pool)
        .await
        .is_err()
    );

    let building = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO search_generations (
            generation_id, search_root, schema_version, status, created_at
         ) VALUES ($1, '/tmp/not-active', 1, 'building', $2)",
    )
    .bind(building)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed non-active generation");
    let mut pointer = db.pool.begin().await.expect("begin invalid pointer");
    sqlx::query(
        "INSERT INTO search_active_generation (
            search_root, generation_id, activated_at, updated_at
         ) VALUES ('/tmp/not-active', $1, $2, $2)",
    )
    .bind(building)
    .bind(T0)
    .execute(&mut *pointer)
    .await
    .expect("deferred pointer insert");
    assert!(pointer.commit().await.is_err());
    db.drop().await;
}

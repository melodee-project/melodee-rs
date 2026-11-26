//! search leases, fencing, rebuild lifecycle, and schema integrity.

#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;
use tokio::sync::mpsc;

use common::{T0, TestDb};
use melodee_persistence::{
    MAX_SEARCH_OUTBOX_ATTEMPTS, SEARCH_PROJECTION_NAME, SearchCatalogRecord, SearchDocumentCounts,
    SearchEntityType, SearchProjectionStore, SearchSnapshotReader, SearchStoreError,
};
use time::Duration as TimeDuration;
use time::OffsetDateTime;
use uuid::Uuid;

async fn wait_for_outbox_insert_waiter(db: &TestDb) {
    // The whole binary runs this test beside other PostgreSQL-heavy tests; the
    // waiter must be observable under that load, not only on an idle machine.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let waiters: i64 = sqlx::query_scalar(
                "SELECT count(*)
                 FROM pg_locks l
                 JOIN pg_class c ON c.oid = l.relation
                 WHERE c.relname = 'catalog_projection_outbox'
                   AND l.locktype = 'relation'
                   AND l.mode = 'RowExclusiveLock'
                   AND NOT l.granted",
            )
            .fetch_one(&db.pool)
            .await
            .expect("query lock waiter");
            if waiters > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("outbox insert observed waiting on activation lock");
}

async fn wait_for_outbox_fence_lock_granted(db: &TestDb) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            // PERF-V001: activation takes ACCESS EXCLUSIVE (not SHARE) so it
            // waits for every in-flight outbox-inserting transaction.
            let fence_locks: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks l
                 JOIN pg_class c ON c.oid = l.relation
                 WHERE c.relname = 'catalog_projection_outbox'
                   AND l.locktype = 'relation'
                   AND l.mode = 'AccessExclusiveLock'
                   AND l.granted",
            )
            .fetch_one(&db.pool)
            .await
            .expect("query lock waiter");
            if fence_locks > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("activation fence lock observed");
}

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
        true,
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
        true,
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
            true,
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
    let snapshot_generation = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO search_generations (
            generation_id, search_root, schema_version, status, created_at
         ) VALUES ($1, '/tmp/phase6-snapshot-catchup', 1, 'building', $2)",
    )
    .bind(snapshot_generation)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("create snapshot generation");
    let mut snapshot = SearchSnapshotReader::begin(&db.pool, snapshot_generation)
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
    assert!(
        sqlx::query(
            "INSERT INTO search_active_generation (
            search_root, generation_id, activated_at, updated_at
         ) VALUES ('/tmp/not-active', $1, $2, $2)",
        )
        .bind(building)
        .bind(T0)
        .execute(&db.pool)
        .await
        .is_err(),
        "insert invalid active pointer should fail when generation is not active"
    );

    let root = "/tmp/active-pointer-stability";
    let counts = SearchDocumentCounts {
        artists: 0,
        releases: 0,
        tracks: 0,
    };
    let _first_job = SearchProjectionStore::request_rebuild(&db.pool, root, "admin", T0)
        .await
        .expect("request initial active generation");
    let first_claim = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        "worker-active-setup",
        1,
        Duration::from_secs(10),
        T0,
    )
    .await
    .expect("claim initial active rebuild")
    .expect("initial rebuild should be claimed");
    SearchProjectionStore::mark_generation_validated(
        &db.pool,
        first_claim.job_id,
        "worker-active-setup",
        &counts,
        &"a".repeat(64),
        0,
        T0 + TimeDuration::seconds(1),
    )
    .await
    .expect("mark initial generation validated");
    let active = SearchProjectionStore::activate_generation(
        &db.pool,
        first_claim.job_id,
        "worker-active-setup",
        T0 + TimeDuration::seconds(2),
    )
    .await
    .expect("activate initial generation");
    let active_generation = active.generation_id;

    assert!(
        sqlx::query(
            "UPDATE search_generations
         SET status = 'retired', retired_at = $2
         WHERE generation_id = $1",
        )
        .bind(active_generation)
        .bind(T0 + TimeDuration::seconds(2))
        .execute(&db.pool)
        .await
        .is_err(),
        "attempt to retire active generation should fail while active pointer exists"
    );

    assert!(
        sqlx::query("DELETE FROM search_generations WHERE generation_id = $1")
            .bind(active_generation)
            .execute(&db.pool)
            .await
            .is_err(),
        "delete attempt should fail when generation is active pointer target"
    );

    assert!(
        sqlx::query(
            "DELETE FROM search_active_generation
             WHERE search_root = $1 AND generation_id = $2",
        )
        .bind(root)
        .bind(active_generation)
        .execute(&db.pool)
        .await
        .is_err(),
        "active pointer cannot be deleted while referenced generation is active"
    );

    let mut migration = db
        .pool
        .begin()
        .await
        .expect("begin generation transition tx");
    sqlx::query(
        "UPDATE search_generations
         SET status = 'retired', retired_at = $2
         WHERE generation_id = $1",
    )
    .bind(active_generation)
    .bind(T0 + TimeDuration::seconds(2))
    .execute(&mut *migration)
    .await
    .expect("mark generation as retired in same tx as pointer removal");
    sqlx::query(
        "DELETE FROM search_active_generation
         WHERE search_root = $1 AND generation_id = $2",
    )
    .bind(root)
    .bind(active_generation)
    .execute(&mut *migration)
    .await
    .expect("delete active pointer after marking generation non-active");
    migration
        .commit()
        .await
        .expect("commit non-active transition");

    let unpointed_active = Uuid::now_v7();
    assert!(
        sqlx::query(
            "INSERT INTO search_generations (
                generation_id, search_root, schema_version, status,
                activated_at, created_at
            ) VALUES ($1, '/tmp/unpointed-active', 1, 'active', $2, $2)",
        )
        .bind(unpointed_active)
        .bind(T0 + TimeDuration::seconds(4))
        .execute(&db.pool)
        .await
        .is_err(),
        "cannot insert a search_generation with active status unless search_active_generation points to it"
    );

    let second_active = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO search_generations (
            generation_id, search_root, schema_version, status, created_at
        ) VALUES ($1, $2, 1, 'building', $3)",
    )
    .bind(second_active)
    .bind(root)
    .bind(T0 + TimeDuration::seconds(5))
    .execute(&db.pool)
    .await
    .expect("seed second generation for second-active regression");
    assert!(
        sqlx::query(
            "UPDATE search_generations
             SET status = 'active', activated_at = $2
             WHERE generation_id = $1",
        )
        .bind(second_active)
        .bind(T0 + TimeDuration::seconds(6))
        .execute(&db.pool)
        .await
        .is_err(),
        "activating second generation while one active pointer target already exists should fail"
    );

    db.drop().await;
}

#[tokio::test]
async fn search_release_snapshot_splits_roles_for_primary_featured_performer_and_other() {
    let db = TestDb::create("phase6_release_snapshot_role_split").await;
    let release_id = Uuid::now_v7();
    let release_public_id = Uuid::now_v7();
    let primary_artist = Uuid::now_v7();
    let featured_artist = Uuid::now_v7();
    let guest_artist = Uuid::now_v7();
    let other_artist = Uuid::now_v7();
    let performer_artist = Uuid::now_v7();

    for (artist_id, title) in [
        (primary_artist, "Primary Artist"),
        (featured_artist, "Featured Artist"),
        (guest_artist, "Guest Artist"),
        (other_artist, "Other Artist"),
        (performer_artist, "Performer Artist"),
    ] {
        sqlx::query(
            "INSERT INTO artists (
                id, public_api_id, canonical_name, normalized_name, sort_name,
                artist_type, status, created_at, updated_at
             ) VALUES ($1, $2, $3, UPPER($3), $4, 'person', 'verified', $5, $5)",
        )
        .bind(artist_id)
        .bind(Uuid::now_v7())
        .bind(title)
        .bind(title)
        .bind(T0)
        .execute(&db.pool)
        .await
        .expect("seed artist");
    }

    sqlx::query(
        "INSERT INTO releases (
            id, public_api_id, title, normalized_title, sort_title,
            display_artist, status, revision, created_at, updated_at
         ) VALUES ($1, $2, 'Release Title', 'RELEASE TITLE', 'Release Title',
                   'Release Artist', 'active', 1, $3, $3)",
    )
    .bind(release_id)
    .bind(release_public_id)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed release");

    sqlx::query(
        "INSERT INTO release_artists (
            id, release_id, artist_id, role, presentation_order, display_value,
            join_phrase, created_at
         ) VALUES
         ($1, $2, $3, 'primary', 1, 'primary', '', $4),
         ($5, $2, $6, 'featured', 2, 'featured', '', $4),
         ($7, $2, $8, 'guest', 3, 'guest', '', $4),
         ($9, $2, $10, 'other', 4, 'other', '', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(release_id)
    .bind(primary_artist)
    .bind(T0)
    .bind(Uuid::now_v7())
    .bind(featured_artist)
    .bind(Uuid::now_v7())
    .bind(guest_artist)
    .bind(Uuid::now_v7())
    .bind(other_artist)
    .execute(&db.pool)
    .await
    .expect("seed release artists");

    sqlx::query(
        "INSERT INTO contributions (
            id, release_id, track_id, artist_id, observed_artist_id,
            role, role_detail, evidence_source, display_value,
            presentation_order, is_performance, created_at
         ) VALUES ($1, $2, NULL, $3, NULL,
                   'primary_artist', NULL, 'unit-test', NULL, 0, TRUE, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(release_id)
    .bind(performer_artist)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed release performer");

    let record =
        SearchProjectionStore::load_catalog_record(&db.pool, SearchEntityType::Release, release_id)
            .await
            .expect("load release record")
            .expect("release record exists");
    let SearchCatalogRecord::Release(snapshot) = record else {
        panic!("unexpected non-release record payload")
    };
    assert_eq!(
        snapshot.artist_ids,
        vec![primary_artist, featured_artist, guest_artist, other_artist,]
    );
    assert_eq!(snapshot.primary_artist_ids, vec![primary_artist]);
    assert_eq!(
        snapshot.featured_artist_ids,
        vec![featured_artist, guest_artist]
    );
    assert_eq!(snapshot.other_artist_ids, vec![other_artist]);
    assert_eq!(snapshot.performer_artist_ids, vec![performer_artist]);

    db.drop().await;
}

#[tokio::test]
async fn search_track_snapshot_splits_other_from_billing_and_residual() {
    let db = TestDb::create("phase6_track_snapshot_other_role_split").await;
    let release_id = Uuid::now_v7();
    let release_public_id = Uuid::now_v7();
    let release_other_artist = Uuid::now_v7();
    let track_other_artist = Uuid::now_v7();
    let residual_other_artist = Uuid::now_v7();
    let performance_other_artist = Uuid::now_v7();
    let track_id = Uuid::now_v7();
    let track_public_id = Uuid::now_v7();
    let track_audio_identity = Uuid::now_v7();
    let track_media_asset = Uuid::now_v7();

    for (artist_id, title) in [
        (release_other_artist, "Release Other"),
        (track_other_artist, "Track Other"),
        (residual_other_artist, "Residual Other"),
        (performance_other_artist, "Performance Other"),
    ] {
        sqlx::query(
            "INSERT INTO artists (
                id, public_api_id, canonical_name, normalized_name, sort_name,
                artist_type, status, created_at, updated_at
             ) VALUES ($1, $2, $3, UPPER($3), $4, 'person', 'verified', $5, $5)",
        )
        .bind(artist_id)
        .bind(Uuid::now_v7())
        .bind(title)
        .bind(title)
        .bind(T0)
        .execute(&db.pool)
        .await
        .expect("seed other artist");
    }

    sqlx::query(
        "INSERT INTO releases (
            id, public_api_id, title, normalized_title, sort_title,
            display_artist, status, revision, created_at, updated_at
         ) VALUES ($1, $2, 'Other Release', 'OTHER RELEASE', 'Other Release',
                   'Other Artist', 'active', 1, $3, $3)",
    )
    .bind(release_id)
    .bind(release_public_id)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed release");

    sqlx::query(
        "INSERT INTO audio_identities (id, status, revision, created_at, updated_at)
         VALUES ($1, 'verified', 1, $2, $2)",
    )
    .bind(track_audio_identity)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed audio identity");

    sqlx::query(
        "INSERT INTO media_assets (
            id, canonical_relative_path, byte_hash, byte_size, mime_type, created_at
         ) VALUES ($1, $2, $3, 0, 'audio/mpeg', $4)",
    )
    .bind(track_media_asset)
    .bind("phase6-track-other-asset.flac")
    .bind("0000000000000000000000000000000000000000000000000000000000000000")
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed media asset");

    let mut track_tx = db.pool.begin().await.expect("begin track setup");
    sqlx::query(
        "INSERT INTO tracks (
            id, public_api_id, release_id, audio_identity_id, playable_media_asset_id,
            ordinal, title, normalized_title, sort_title, display_artist, disc_number,
            track_number, duration_ms, source_path, revision, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, 1, 'Other Track', 'OTHER TRACK',
                  'Other Track', NULL, NULL, 1, 300000, 'other.m4a', 1, $6, $6)",
    )
    .bind(track_id)
    .bind(track_public_id)
    .bind(release_id)
    .bind(track_audio_identity)
    .bind(track_media_asset)
    .bind(T0)
    .execute(&mut *track_tx)
    .await
    .expect("seed track");

    sqlx::query(
        "INSERT INTO track_media_assets (
            track_id, media_asset_id, purpose, ordinal, created_at
         ) VALUES ($1, $2, 'playable', 1, $3)",
    )
    .bind(track_id)
    .bind(track_media_asset)
    .bind(T0)
    .execute(&mut *track_tx)
    .await
    .expect("seed track media asset link");
    track_tx.commit().await.expect("commit track setup");

    sqlx::query(
        "INSERT INTO track_artists (
            id, track_id, artist_id, role, presentation_order, display_value,
            join_phrase, created_at
         ) VALUES ($1, $2, $3, 'other', 1, 'other', '', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(track_id)
    .bind(track_other_artist)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed track other billing");

    sqlx::query(
        "INSERT INTO release_artists (
            id, release_id, artist_id, role, presentation_order, display_value,
            join_phrase, created_at
         ) VALUES ($1, $2, $3, 'other', 1, 'other', '', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(release_id)
    .bind(release_other_artist)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed release other billing");

    sqlx::query(
        "INSERT INTO artist_track_associations (
            artist_id, track_id, association_type, participation_kind, source_key,
            source_artist_id, confidence, is_performance, created_at
         ) VALUES ($1, $2, 'unit-test', 'direct', $3, NULL, 1, FALSE, $4)",
    )
    .bind(residual_other_artist)
    .bind(track_id)
    .bind(Uuid::now_v7())
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed residual association");

    sqlx::query(
        "INSERT INTO contributions (
            id, release_id, track_id, artist_id, observed_artist_id,
            role, role_detail, evidence_source, display_value,
            presentation_order, is_performance, created_at
         ) VALUES ($1, $2, $3, $4, NULL, 'composer', NULL, 'unit-test', NULL, 0, TRUE, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(release_id)
    .bind(track_id)
    .bind(performance_other_artist)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed track performance contribution");

    let record =
        SearchProjectionStore::load_catalog_record(&db.pool, SearchEntityType::Track, track_id)
            .await
            .expect("load track record")
            .expect("track record exists");
    let SearchCatalogRecord::Track(snapshot) = record else {
        panic!("unexpected non-track record payload")
    };
    let mut others = snapshot.other_artist_ids;
    others.sort();
    let mut expected = vec![
        release_other_artist,
        track_other_artist,
        residual_other_artist,
    ];
    expected.sort();
    assert_eq!(others, expected);
    assert_eq!(
        snapshot.performer_artist_ids,
        vec![performance_other_artist]
    );

    db.drop().await;
}

#[tokio::test]
async fn search_artist_snapshot_collects_redirect_chains_for_merged_sources() {
    let db = TestDb::create("phase6_artist_snapshot_merge_chain").await;
    let terminal = Uuid::now_v7();
    let intermediate = Uuid::now_v7();
    let source = Uuid::now_v7();
    let terminal_public = Uuid::now_v7();
    let intermediate_public = Uuid::now_v7();
    let source_public = Uuid::now_v7();

    sqlx::query(
        "INSERT INTO artists (
            id, public_api_id, merged_into_artist_id, canonical_name, normalized_name, sort_name,
            artist_type, status, revision, created_at, updated_at
         ) VALUES
         ($1, $2, NULL, 'Terminal', 'TERMINAL', 'Terminal', 'person', 'verified', 1, $7, $7),
         ($3, $4, $1, 'Intermediate', 'INTERMEDIATE', 'Intermediate', 'person', 'merged', 1, $7, $7),
         ($5, $6, $3, 'Source', 'SOURCE', 'Source', 'person', 'merged', 1, $7, $7)",
    )
    .bind(terminal)
    .bind(terminal_public)
    .bind(intermediate)
    .bind(intermediate_public)
    .bind(source)
    .bind(source_public)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed artists");

    let record =
        SearchProjectionStore::load_catalog_record(&db.pool, SearchEntityType::Artist, terminal)
            .await
            .expect("load terminal artist")
            .expect("terminal artist exists");
    let SearchCatalogRecord::Artist(snapshot) = record else {
        panic!("unexpected non-artist snapshot payload")
    };

    let mut redirect_ids = snapshot.redirect_ids;
    redirect_ids.sort();
    let mut expected = vec![intermediate, source, intermediate_public, source_public];
    expected.sort();
    assert_eq!(redirect_ids, expected);

    db.drop().await;
}

#[tokio::test]
async fn outbox_max_attempts_escalate_exactly_at_configured_limit() {
    let db = TestDb::create("phase6_outbox_attempt_limits").await;
    let outbox_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)
         RETURNING id",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(Uuid::now_v7())
    .bind(T0)
    .fetch_one(&db.pool)
    .await
    .expect("seed bounded-attempt outbox");
    let claimed = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-out",
        1,
        Duration::from_secs(10),
        T0,
    )
    .await
    .expect("claim bounded attempt row");
    assert_eq!(claimed.len(), 1);

    assert!(matches!(
        SearchProjectionStore::record_outbox_failure(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            "worker-out",
            &[claimed[0].id],
            "index_document",
            "invalid config",
            Some(T0 + TimeDuration::seconds(1)),
            0,
            true,
            T0,
        )
        .await,
        Err(SearchStoreError::InvalidRequest(_))
    ));
    assert!(matches!(
        SearchProjectionStore::record_outbox_failure(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            "worker-out",
            &[claimed[0].id],
            "index_document",
            "invalid config",
            Some(T0 + TimeDuration::seconds(1)),
            MAX_SEARCH_OUTBOX_ATTEMPTS + 1,
            true,
            T0,
        )
        .await,
        Err(SearchStoreError::InvalidRequest(_))
    ));

    let first_claim = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-out",
        1,
        Duration::from_secs(10),
        T0,
    )
    .await
    .expect("claim first retryable attempt");
    assert_eq!(first_claim.len(), 1);

    SearchProjectionStore::record_outbox_failure(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-out",
        &[first_claim[0].id],
        "index_document",
        "temporary boundary check",
        Some(T0 + TimeDuration::seconds(1)),
        2,
        true,
        T0,
    )
    .await
    .expect("record attempt one");

    let post_retry: (i32, Option<OffsetDateTime>, Option<String>) = sqlx::query_as(
        "SELECT attempt_count, failed_at, failure_kind
         FROM catalog_projection_outbox WHERE id = $1",
    )
    .bind(outbox_id)
    .fetch_one(&db.pool)
    .await
    .expect("read retriable attempt row");
    assert_eq!(post_retry.0, 1);
    assert!(post_retry.1.is_none());

    let retried_claim = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-out",
        1,
        Duration::from_secs(10),
        T0 + TimeDuration::seconds(1),
    )
    .await
    .expect("reclaim at configured retry boundary");
    assert_eq!(retried_claim.len(), 1);

    SearchProjectionStore::record_outbox_failure(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-out",
        &[retried_claim[0].id],
        "index_document",
        "second attempt should exhaust",
        None,
        2,
        true,
        T0 + TimeDuration::seconds(1),
    )
    .await
    .expect("record attempt two");

    let exhausted: (i32, Option<OffsetDateTime>, Option<String>) = sqlx::query_as(
        "SELECT attempt_count, failed_at, failure_kind
         FROM catalog_projection_outbox WHERE id = $1",
    )
    .bind(outbox_id)
    .fetch_one(&db.pool)
    .await
    .expect("read exhausted row");
    assert_eq!(exhausted.0, 2);
    assert!(exhausted.1.is_some());
    assert_eq!(exhausted.2.as_deref(), Some("index_document"));
    assert!(
        SearchProjectionStore::claim_outbox(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            "worker-out-2",
            1,
            Duration::from_secs(10),
            T0 + TimeDuration::seconds(2),
        )
        .await
        .expect("post-exhaustion claim")
        .is_empty()
    );

    db.drop().await;
}

#[tokio::test]
async fn outbox_status_pending_and_leased_counts_distinguish_valid_lease_state() {
    let db = TestDb::create("phase6_outbox_status_overlap").await;
    let now = OffsetDateTime::now_utc();

    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(Uuid::now_v7())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed leased row");
    let claimed = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "worker-owner",
        1,
        Duration::from_secs(10),
        now,
    )
    .await
    .expect("claim leased row");
    assert_eq!(claimed.len(), 1);

    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
        ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(Uuid::now_v7())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed pending row");

    let status = SearchProjectionStore::status(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        "/tmp/phase6-outbox-status-overlap",
        now,
    )
    .await
    .expect("status for overlap assertion");
    assert_eq!(status.pending, 1);
    assert_eq!(status.leased, 1);
    assert_eq!(status.failed, 0);
    let oldest_available_at = status
        .oldest_available_at
        .expect("pending rows must expose oldest available");
    assert!(oldest_available_at <= now + time::Duration::milliseconds(1));

    db.drop().await;
}

#[tokio::test]
async fn outbox_checkpoint_lease_survives_partial_failure() {
    let db = TestDb::create("phase6_outbox_partial_checkpoint").await;
    let now = T0 + TimeDuration::seconds(1);
    let mut catalog_insert = db.pool.begin().await.expect("begin catalog insert");
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(first)
    .bind(now)
    .execute(&mut *catalog_insert)
    .await
    .expect("seed first outbox");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(second)
    .bind(now)
    .execute(&mut *catalog_insert)
    .await
    .expect("seed second outbox");
    catalog_insert.commit().await.expect("commit seeded rows");
    let worker = "worker-partial";
    let claimed = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        worker,
        2,
        Duration::from_secs(10),
        now,
    )
    .await
    .expect("claim mixed batch");
    assert_eq!(claimed.len(), 2);
    SearchProjectionStore::record_outbox_failure(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        worker,
        &[claimed[0].id],
        "catalog_read",
        "fault injected",
        None,
        2,
        false,
        now + TimeDuration::seconds(1),
    )
    .await
    .expect("record partial catalog read failure");
    SearchProjectionStore::renew_outbox_lease(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        worker,
        &[claimed[1].id],
        Duration::from_secs(10),
        now + TimeDuration::seconds(1),
    )
    .await
    .expect("renew healthy sibling");
    db.drop().await;
}

#[tokio::test]
async fn outbox_insert_is_not_blocked_by_rebuild_row_lock_contention_before_share_lock() {
    let db = TestDb::create("phase6_outbox_activate_row_lock_barrier").await;
    let root = "/tmp/phase6-activation-row-lock";
    let worker = "activation-worker";
    let now = T0 + TimeDuration::seconds(1);
    let counts = SearchDocumentCounts {
        artists: 0,
        releases: 0,
        tracks: 0,
    };
    let _job = SearchProjectionStore::request_rebuild(&db.pool, root, "admin", now)
        .await
        .expect("request rebuild");
    let claimed = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        worker,
        1,
        Duration::from_secs(10),
        now,
    )
    .await
    .expect("claim rebuild")
    .expect("rebuild claimed");
    SearchProjectionStore::mark_generation_validated(
        &db.pool,
        claimed.job_id,
        worker,
        &counts,
        &"a".repeat(64),
        0,
        now,
    )
    .await
    .expect("mark generation validated");

    let job_id = claimed.job_id;
    let mut blocker = db.pool.begin().await.expect("begin blocker");
    sqlx::query_scalar::<_, Uuid>(
        "SELECT job_id
         FROM search_rebuild_jobs
         WHERE job_id = $1 AND lease_owner = $2
         FOR UPDATE",
    )
    .bind(job_id)
    .bind(worker)
    .fetch_one(&mut *blocker)
    .await
    .expect("pin rebuild job");

    let activate_db = db.pool.clone();
    let activation = tokio::spawn(async move {
        SearchProjectionStore::activate_generation(
            &activate_db,
            job_id,
            worker,
            now + TimeDuration::seconds(1),
        )
        .await
    });

    let insertion_id = Uuid::now_v7();
    let (insert_tx, mut insert_rx) = mpsc::channel::<Result<i64, String>>(1);
    let insert_db = db.pool.clone();
    tokio::spawn(async move {
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, created_at
             ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)
             RETURNING id",
        )
        .bind("legacy.phase.catalog")
        .bind(insertion_id)
        .bind(T0 + TimeDuration::seconds(2))
        .fetch_one(&insert_db)
        .await
        .map_err(|error| error.to_string());
        let _ = insert_tx.send(result).await;
    });

    let inserted = tokio::time::timeout(Duration::from_secs(1), insert_rx.recv())
        .await
        .expect("outbox insert must complete while activation waits on rebuild lock")
        .expect("outbox insert result delivered")
        .expect("insert should succeed");
    assert!(inserted > 0);

    blocker.commit().await.expect("release rebuild-row pin");
    activation
        .await
        .expect("activation task")
        .expect("activation must succeed");

    db.drop().await;
}

#[tokio::test]
async fn outbox_insert_waits_and_is_catchup_after_activate_generation_share_lock() {
    let db = TestDb::create("phase6_outbox_activate_share_lock").await;
    let root = "/tmp/phase6-activation-lock";
    let worker = "activation-worker";
    let bootstrap_worker = "activation-bootstrap";
    let now = T0 + TimeDuration::seconds(1);
    let counts = SearchDocumentCounts {
        artists: 0,
        releases: 0,
        tracks: 0,
    };

    let _bootstrap_job =
        SearchProjectionStore::request_rebuild(&db.pool, root, "admin-bootstrap", now)
            .await
            .expect("request rebuild");
    let bootstrap_claim = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        bootstrap_worker,
        1,
        Duration::from_secs(10),
        now,
    )
    .await
    .expect("claim bootstrap rebuild")
    .expect("bootstrap rebuild claimed");
    SearchProjectionStore::mark_generation_validated(
        &db.pool,
        bootstrap_claim.job_id,
        bootstrap_worker,
        &counts,
        &"b".repeat(64),
        0,
        now,
    )
    .await
    .expect("mark bootstrap generation valid");
    SearchProjectionStore::activate_generation(
        &db.pool,
        bootstrap_claim.job_id,
        bootstrap_worker,
        now + TimeDuration::seconds(1),
    )
    .await
    .expect("activate bootstrap generation");

    let _second_bootstrap_job = SearchProjectionStore::request_rebuild(
        &db.pool,
        root,
        "admin",
        now + TimeDuration::seconds(2),
    )
    .await
    .expect("request rebuild");
    let claimed = SearchProjectionStore::claim_rebuild(
        &db.pool,
        root,
        worker,
        1,
        Duration::from_secs(10),
        now + TimeDuration::seconds(2),
    )
    .await
    .expect("claim rebuild")
    .expect("rebuild claimed");
    SearchProjectionStore::mark_generation_validated(
        &db.pool,
        claimed.job_id,
        worker,
        &counts,
        &"a".repeat(64),
        0,
        now,
    )
    .await
    .expect("mark generation validated");

    let job_id = claimed.job_id;
    sqlx::query(
        "CREATE OR REPLACE FUNCTION phase6_activation_share_lock_barrier()
         RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(1718191919);
             RETURN NEW;
         END;
         $$;",
    )
    .execute(&db.pool)
    .await
    .expect("install activation advisory barrier function");
    sqlx::query(
        "CREATE TRIGGER phase6_activation_share_lock_barrier
         BEFORE INSERT OR UPDATE ON search_active_generation
         FOR EACH ROW
         EXECUTE FUNCTION phase6_activation_share_lock_barrier();",
    )
    .execute(&db.pool)
    .await
    .expect("install activation advisory barrier trigger");

    let mut barrier_lock = db
        .pool
        .begin()
        .await
        .expect("begin activation barrier holder");
    sqlx::query("SELECT pg_advisory_xact_lock(1718191919)")
        .execute(&mut *barrier_lock)
        .await
        .expect("hold advisory barrier");

    let activate_db = db.pool.clone();
    let activation = tokio::spawn(async move {
        SearchProjectionStore::activate_generation(
            &activate_db,
            job_id,
            worker,
            now + TimeDuration::seconds(1),
        )
        .await
    });

    wait_for_outbox_fence_lock_granted(&db).await;
    let insertion_id = Uuid::now_v7();
    let (insert_tx, mut insert_rx) = mpsc::channel::<Result<i64, String>>(1);
    let insert_db = db.pool.clone();
    tokio::spawn(async move {
        let result = sqlx::query_scalar::<_, i64>(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, created_at
             ) VALUES ($1, 'artist', $2, 'upsert', 1, $3, $3)
             RETURNING id",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(insertion_id)
        .bind(T0 + TimeDuration::seconds(2))
        .fetch_one(&insert_db)
        .await
        .map_err(|error| error.to_string());
        let _ = insert_tx.send(result).await;
    });

    wait_for_outbox_insert_waiter(&db).await;
    assert!(
        matches!(
            insert_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "outbox insert must remain blocked while activation holds the exclusive fence"
    );

    barrier_lock
        .commit()
        .await
        .expect("release activation advisory barrier");
    let activated = activation
        .await
        .expect("activation task")
        .expect("activation must succeed");
    let inserted = tokio::time::timeout(Duration::from_secs(5), insert_rx.recv())
        .await
        .expect("outbox insert should complete after activation")
        .expect("insert result delivered");
    let inserted = inserted.expect("insert should succeed");
    assert_eq!(activated.status, "active");
    let latest = SearchProjectionStore::max_outbox_id(&db.pool)
        .await
        .expect("latest outbox");
    assert_eq!(latest, inserted);
    assert!(latest > activated.caught_up_outbox_id);
    let catchup = SearchProjectionStore::load_outbox_after(
        &db.pool,
        activated.caught_up_outbox_id,
        latest,
        10,
    )
    .await
    .expect("load post-activation catchup");
    assert_eq!(catchup.len(), 1);
    assert_eq!(catchup[0].id, inserted);
    assert_eq!(catchup[0].entity_id, insertion_id);
    sqlx::query(
        "DROP TRIGGER IF EXISTS phase6_activation_share_lock_barrier ON search_active_generation",
    )
    .execute(&db.pool)
    .await
    .expect("remove activation barrier trigger");
    sqlx::query("DROP FUNCTION IF EXISTS phase6_activation_share_lock_barrier()")
        .execute(&db.pool)
        .await
        .expect("remove activation barrier function");

    db.drop().await;
}

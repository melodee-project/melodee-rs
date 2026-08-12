//! PostgreSQL snapshot -> owned Tantivy generation -> atomic activation.

#![allow(clippy::expect_used, clippy::panic)]

#[path = "../../melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::IngestionState;
use melodee_mql::parse_and_plan;
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, SEARCH_PROJECTION_NAME, SearchCatalogRecord,
    SearchProjectionStore, WorkerId,
};
use melodee_search::{
    ArtistDoc, ReleaseDoc, SearchConfig, SearchRoot, SearchStore, TrackDoc, apply_outbox_once,
    request_rebuild, run_rebuild_once,
};
use tempfile::TempDir;

#[tokio::test]
async fn populated_catalog_rebuilds_without_reading_media_and_activates_atomically() {
    let db = TestDb::create("phase6_rebuild").await;
    let fixture = seed_published_release(
        &db,
        601,
        CatalogFixturePresentation {
            release_title: "Searchable Reykjavík",
            release_artist: "Distinct Search Artist",
            track_title: "Projection Without Media",
            track_artist: "Featured Search Artist",
            release_date: "2006-07-08",
        },
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let claimed = catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim published fixture");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].state, IngestionState::Cataloging);
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase6.fixture-resolver.v1", "phase6-test"),
            now,
        )
        .await
        .expect("catalog fixture");
    assert!(!std::path::Path::new(&fixture.source_path).exists());
    assert!(!std::path::Path::new(&fixture.canonical_asset_path).exists());

    let directory = TempDir::new().expect("search temp root");
    let search_path = directory.path().join("search");
    let config = SearchConfig {
        search_root: search_path,
        enabled: true,
        ..SearchConfig::default()
    };
    let requested = request_rebuild(&config, &db.pool, "phase6-test")
        .await
        .expect("request rebuild");
    assert_eq!(requested.status, "requested");
    assert_eq!(
        run_rebuild_once(&config, &db.pool)
            .await
            .expect("run rebuild"),
        Some(requested.job_id)
    );

    let root = SearchRoot::open(&config.search_root).expect("owned search root");
    let active = melodee_persistence::SearchProjectionStore::active_generation(
        &db.pool,
        root.path().to_string_lossy().as_ref(),
    )
    .await
    .expect("read active generation")
    .expect("active generation exists");
    assert_eq!(active.status, "active");
    assert_eq!(active.document_counts.artists, 2);
    assert_eq!(active.document_counts.releases, 1);
    assert_eq!(active.document_counts.tracks, 1);

    let store = SearchStore::open(&root.generation_path(active.generation_id))
        .expect("open active generation");
    let hits = store
        .search(
            &parse_and_plan("title:\"Projection Without Media\"").expect("valid MQL"),
            20,
            0,
        )
        .expect("query rebuilt generation");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, "track");
    assert_eq!(hits[0].display, "Projection Without Media");
    let resolved =
        melodee_search::resolve_artist_ids_active(&config, &db.pool, "Distinct Search Artist")
            .await
            .expect("resolve admin Artist lookup while a writer handle is open");
    assert_eq!(resolved.len(), 1);
    drop(store);

    // Deterministic crash window: commit one claimed row to Tantivy, release
    // the lease without acknowledging PostgreSQL, then let the normal worker
    // replay the entire batch. Revision fencing must make the replay exact.
    let initial = apply_outbox_once(&config, &db.pool)
        .await
        .expect("acknowledge snapshot-covered initial outbox");
    assert_eq!(initial.applied, 0);
    assert_eq!(initial.stale_or_idempotent, initial.claimed);
    let crash_at = time::OffsetDateTime::now_utc();
    let mut correction = db.pool.begin().await.expect("begin catalog correction");
    let (revised_artist_id, revised_artist_revision) = sqlx::query_as::<_, (uuid::Uuid, i64)>(
        "UPDATE artists
             SET canonical_name = 'Distinct Search Artist Revised',
                 normalized_name = 'DISTINCT SEARCH ARTIST REVISED',
                 sort_name = 'Distinct Search Artist Revised',
                 revision = revision + 1, updated_at = $1
             WHERE canonical_name = 'Distinct Search Artist'
             RETURNING id, revision",
    )
    .bind(crash_at)
    .fetch_one(&mut *correction)
    .await
    .expect("apply authoritative catalog correction");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', $3, $4, $4)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(revised_artist_id)
    .bind(revised_artist_revision)
    .bind(crash_at)
    .execute(&mut *correction)
    .await
    .expect("atomically represented correction outbox fixture");
    correction
        .commit()
        .await
        .expect("commit catalog correction and outbox together");
    let crash_worker = "phase6-crash-window";
    let claimed = SearchProjectionStore::claim_outbox(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        crash_worker,
        256,
        std::time::Duration::from_secs(60),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("claim crash-window batch");
    assert_eq!(claimed.len(), 1);
    let first = &claimed[0];
    let record =
        SearchProjectionStore::load_catalog_record(&db.pool, first.entity_type, first.entity_id)
            .await
            .expect("load crash-window record")
            .expect("record exists");
    let generation_path = root.generation_path(active.generation_id);
    let mut crash_store = SearchStore::open(&generation_path).expect("open crash-window writer");
    let applied_before_crash = match record {
        SearchCatalogRecord::Artist(record) => crash_store
            .upsert_artist(&ArtistDoc::from(record))
            .expect("apply Artist before crash"),
        SearchCatalogRecord::Release(record) => crash_store
            .upsert_release(&ReleaseDoc::from(record))
            .expect("apply Release before crash"),
        SearchCatalogRecord::Track(record) => crash_store
            .upsert_track(&TrackDoc::from(record))
            .expect("apply Track before crash"),
        SearchCatalogRecord::NoDocument => false,
    };
    assert!(applied_before_crash);
    crash_store.commit().expect("durable commit before crash");
    drop(crash_store);
    SearchProjectionStore::release_outbox_lease(
        &db.pool,
        SEARCH_PROJECTION_NAME,
        crash_worker,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("simulate process loss by releasing lease");
    let replay = apply_outbox_once(&config, &db.pool)
        .await
        .expect("replay and acknowledge");
    assert_eq!(replay.acknowledged, claimed.len());
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.stale_or_idempotent, claimed.len());
    let idle = apply_outbox_once(&config, &db.pool)
        .await
        .expect("queue converged");
    assert_eq!(idle.claimed, 0);

    // A document-level Tantivy failure must not commit that row's scheduled
    // replacement or delay a healthy sibling from the same claimed batch.
    #[cfg(feature = "test-support")]
    {
        let artist_rows = sqlx::query_as::<_, (uuid::Uuid, i64)>(
            "SELECT id, revision FROM artists ORDER BY (id = $1) DESC, id LIMIT 2",
        )
        .bind(revised_artist_id)
        .fetch_all(&db.pool)
        .await
        .expect("load poison-batch Artists");
        assert_eq!(artist_rows.len(), 2);
        let poison_at = time::OffsetDateTime::now_utc();
        let mut poison_tx = db.pool.begin().await.expect("begin poison batch");
        let bad_revision = artist_rows[0].1 + 1;
        sqlx::query(
            "UPDATE artists SET sort_name = 'Injected Failure Candidate', revision = $2,
                            updated_at = $3 WHERE id = $1",
        )
        .bind(artist_rows[0].0)
        .bind(bad_revision)
        .bind(poison_at)
        .execute(&mut *poison_tx)
        .await
        .expect("update fault-injection Artist");
        let bad_outbox_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', $3, $4, $4)
         RETURNING id",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(artist_rows[0].0)
        .bind(bad_revision)
        .bind(poison_at)
        .fetch_one(&mut *poison_tx)
        .await
        .expect("seed poison outbox row");
        let good_revision = artist_rows[1].1 + 1;
        sqlx::query(
            "UPDATE artists SET canonical_name = 'Healthy Batch Sibling',
                            normalized_name = 'HEALTHY BATCH SIBLING',
                            sort_name = 'Healthy Batch Sibling', revision = $2,
                            updated_at = $3 WHERE id = $1",
        )
        .bind(artist_rows[1].0)
        .bind(good_revision)
        .bind(poison_at)
        .execute(&mut *poison_tx)
        .await
        .expect("update healthy sibling");
        let good_outbox_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', $3, $4, $4)
         RETURNING id",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(artist_rows[1].0)
        .bind(good_revision)
        .bind(poison_at)
        .fetch_one(&mut *poison_tx)
        .await
        .expect("seed healthy sibling outbox row");
        poison_tx.commit().await.expect("commit poison batch");

        melodee_search::worker::set_document_failure_for_test(Some(artist_rows[0].0));
        let isolated = apply_outbox_once(&config, &db.pool)
            .await
            .expect("isolate malformed search document");
        melodee_search::worker::set_document_failure_for_test(None);
        assert_eq!(isolated.claimed, 2);
        assert_eq!(isolated.applied, 1);
        assert_eq!(isolated.acknowledged, 1);
        assert_eq!(isolated.errors, 1);
        let outcomes = sqlx::query_as::<_, (i64, bool, bool, Option<String>)>(
            "SELECT id, processed_at IS NOT NULL, failed_at IS NOT NULL, failure_kind
         FROM catalog_projection_outbox WHERE id = ANY($1) ORDER BY id",
        )
        .bind(&[bad_outbox_id, good_outbox_id][..])
        .fetch_all(&db.pool)
        .await
        .expect("read isolated batch outcomes");
        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            outcomes[0],
            (
                bad_outbox_id,
                false,
                true,
                Some("index_document".to_string())
            )
        );
        assert_eq!(outcomes[1], (good_outbox_id, true, false, None));
        let failed_row_still_visible = melodee_search::query_active(
            &config,
            &db.pool,
            &format!("id:{}", artist_rows[0].0),
            10,
            0,
        )
        .await
        .expect("query failed row's prior document");
        assert_eq!(failed_row_still_visible.len(), 1);
        let healthy_sibling_visible = melodee_search::query_active(
            &config,
            &db.pool,
            "name:\"Healthy Batch Sibling\"",
            10,
            0,
        )
        .await
        .expect("query healthy sibling");
        assert_eq!(healthy_sibling_visible.len(), 1);
        let healthy_diagnostic = melodee_search::diagnostic(&config, &db.pool)
            .await
            .expect("diagnose live counts after isolated batch");
        assert!(healthy_diagnostic.generation_error.is_none());

        let recovery_at = time::OffsetDateTime::now_utc();
        let recovered_revision = bad_revision + 1;
        let mut recovery_tx = db.pool.begin().await.expect("begin poison recovery");
        sqlx::query(
            "UPDATE artists SET sort_name = canonical_name, revision = $2,
                            updated_at = $3 WHERE id = $1",
        )
        .bind(artist_rows[0].0)
        .bind(recovered_revision)
        .bind(recovery_at)
        .execute(&mut *recovery_tx)
        .await
        .expect("repair authoritative Artist sort name");
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', $3, $4, $4)",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(artist_rows[0].0)
        .bind(recovered_revision)
        .bind(recovery_at)
        .execute(&mut *recovery_tx)
        .await
        .expect("seed repaired Artist outbox row");
        recovery_tx.commit().await.expect("commit poison recovery");
        let recovered_poison = apply_outbox_once(&config, &db.pool)
            .await
            .expect("index repaired Artist revision");
        assert_eq!(recovered_poison.applied, 1);
        assert_eq!(recovered_poison.acknowledged, 1);
        assert_eq!(recovered_poison.errors, 0);
    }

    // Snapshot counts stay sealed while live counts follow incremental
    // deletes/upserts, preventing healthy catch-up from becoming a permanent
    // diagnostic mismatch.
    let live_revision = sqlx::query_scalar::<_, i64>("SELECT revision FROM artists WHERE id = $1")
        .bind(revised_artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("read current Artist revision");
    let count_at = time::OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'delete', $3, $4, $4)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(revised_artist_id)
    .bind(live_revision + 1)
    .bind(count_at)
    .execute(&db.pool)
    .await
    .expect("seed live-count delete");
    let deleted = apply_outbox_once(&config, &db.pool)
        .await
        .expect("apply live-count delete");
    assert_eq!(deleted.applied, 1);
    let after_delete =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read live counts after delete")
            .expect("active generation");
    assert_eq!(after_delete.document_counts.artists, 2);
    assert_eq!(after_delete.current_document_counts.artists, 1);
    assert!(
        melodee_search::diagnostic(&config, &db.pool)
            .await
            .expect("diagnose after live delete")
            .generation_error
            .is_none()
    );

    let restored_revision = live_revision + 2;
    let restore_at = time::OffsetDateTime::now_utc();
    let mut restore_tx = db.pool.begin().await.expect("begin live-count restore");
    sqlx::query("UPDATE artists SET revision = $2, updated_at = $3 WHERE id = $1")
        .bind(revised_artist_id)
        .bind(restored_revision)
        .bind(restore_at)
        .execute(&mut *restore_tx)
        .await
        .expect("advance restored Artist revision");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', $3, $4, $4)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(revised_artist_id)
    .bind(restored_revision)
    .bind(restore_at)
    .execute(&mut *restore_tx)
    .await
    .expect("seed live-count restore");
    restore_tx
        .commit()
        .await
        .expect("commit live-count restore");
    assert_eq!(
        apply_outbox_once(&config, &db.pool)
            .await
            .expect("apply live-count restore")
            .applied,
        1
    );
    let after_restore =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read live counts after restore")
            .expect("active generation");
    assert_eq!(after_restore.current_document_counts.artists, 2);
    assert!(
        melodee_search::diagnostic(&config, &db.pool)
            .await
            .expect("diagnose after live restore")
            .generation_error
            .is_none()
    );

    let final_store = SearchStore::open(&generation_path).expect("open converged generation");
    let final_stats = final_store.stats().expect("converged counts");
    assert_eq!(final_stats.artists, 2);
    assert_eq!(final_stats.releases, 1);
    assert_eq!(final_stats.tracks, 1);
    let corrected_hits = final_store
        .search(
            &parse_and_plan("artist:\"Distinct Search Artist Revised\"")
                .expect("valid corrected Artist query"),
            20,
            0,
        )
        .expect("query corrected Artist");
    assert!(
        corrected_hits.iter().any(|hit| {
            hit.kind == "artist" && hit.internal_id == revised_artist_id.to_string()
        })
    );
    drop(final_store);

    // Corrupt only the disposable active projection, observe a diagnostic,
    // then rebuild from PostgreSQL without either media path becoming real.
    std::fs::remove_file(generation_path.join("meta.json")).expect("corrupt disposable index");
    let unhealthy = melodee_search::diagnostic(&config, &db.pool)
        .await
        .expect("diagnostic remains available");
    assert!(unhealthy.generation.is_none());
    assert!(unhealthy.generation_error.is_some());
    let recovery = request_rebuild(&config, &db.pool, "phase6-corruption-recovery")
        .await
        .expect("request recovery rebuild");
    assert_eq!(
        run_rebuild_once(&config, &db.pool)
            .await
            .expect("recover from PostgreSQL"),
        Some(recovery.job_id)
    );
    let recovered =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read recovered active generation")
            .expect("recovered active exists");
    assert_ne!(recovered.generation_id, active.generation_id);
    assert_eq!(recovered.document_counts, active.document_counts);
    let recovered_hits = melodee_search::query_active(
        &config,
        &db.pool,
        "title:\"Projection Without Media\"",
        20,
        0,
    )
    .await
    .expect("query recovered generation");
    assert_eq!(recovered_hits, hits);
    assert!(
        !generation_path.exists(),
        "retired owned generation is removed"
    );
    assert!(!std::path::Path::new(&fixture.source_path).exists());
    assert!(!std::path::Path::new(&fixture.canonical_asset_path).exists());

    // Simulate a crash after filesystem retirement but before its DB receipt.
    sqlx::query("UPDATE search_generations SET removed_at = NULL WHERE generation_id = $1")
        .bind(active.generation_id)
        .execute(&db.pool)
        .await
        .expect("reopen retirement crash window");

    let cancelled = request_rebuild(&config, &db.pool, "phase6-cancel-test")
        .await
        .expect("request cancellable rebuild");
    let cancel = std::sync::atomic::AtomicBool::new(true);
    assert_eq!(
        melodee_search::rebuild::run_rebuild_once_with_cancel(&config, &db.pool, &cancel)
            .await
            .expect("cancel at safe boundary"),
        Some(cancelled.job_id)
    );
    let released = SearchProjectionStore::get_rebuild_job(&db.pool, cancelled.job_id)
        .await
        .expect("read cancelled job");
    assert_eq!(released.status, "running");
    assert!(released.lease_owner.is_none());
    assert_eq!(
        run_rebuild_once(&config, &db.pool)
            .await
            .expect("direct retry after cancellation"),
        Some(cancelled.job_id)
    );
    let adopted_removal = sqlx::query_scalar::<_, Option<time::OffsetDateTime>>(
        "SELECT removed_at FROM search_generations WHERE generation_id = $1",
    )
    .bind(active.generation_id)
    .fetch_one(&db.pool)
    .await
    .expect("read adopted retirement receipt");
    assert!(adopted_removal.is_some());

    db.drop().await;
}

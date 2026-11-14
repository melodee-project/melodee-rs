//! PostgreSQL snapshot -> owned Tantivy generation -> atomic activation.

#![allow(clippy::expect_used, clippy::panic)]

#[path = "../../melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::{
    Artist, ArtistAlias, ArtistAliasId, ArtistAliasType, ArtistId, ArtistMerge, ArtistMergeId,
    ArtistPublicId, ArtistStatus, ArtistType, Confidence, IngestionState, NewArtist,
};
use melodee_mql::parse_and_plan;
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, SEARCH_PROJECTION_NAME, SearchCatalogRecord,
    SearchEntityType, SearchOperation, SearchProjectionStore, WorkerId,
};
use melodee_search::{
    ArtistDoc, ReleaseDoc, SearchConfig, SearchRoot, SearchStore, TrackDoc, apply_outbox_once,
    request_rebuild, resolve_artist_ids_active, run_rebuild_once,
};
use tempfile::TempDir;

async fn activate_empty_generation(db: &TestDb, directory: &TempDir) -> SearchConfig {
    let config = SearchConfig {
        search_root: directory.path().join("search"),
        enabled: true,
        ..SearchConfig::default()
    };
    request_rebuild(&config, &db.pool, "empty-catalog-test")
        .await
        .expect("request empty rebuild");
    assert!(
        run_rebuild_once(&config, &db.pool)
            .await
            .expect("run empty rebuild")
            .is_some()
    );
    config
}

#[tokio::test]
async fn empty_catalog_activates_queryable_zero_document_generation() {
    let db = TestDb::create("phase6_empty_rebuild").await;
    let directory = TempDir::new().expect("search temp root");
    let config = activate_empty_generation(&db, &directory).await;
    let root = SearchRoot::open(&config.search_root).expect("owned search root");
    let active =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read active generation")
            .expect("empty active generation");
    assert_eq!(active.document_counts.artists, 0);
    assert_eq!(active.document_counts.releases, 0);
    assert_eq!(active.document_counts.tracks, 0);
    assert_eq!(active.current_document_counts, active.document_counts);
    let media_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_assets")
        .fetch_one(&db.pool)
        .await
        .expect("count media assets");
    assert_eq!(media_count, 0);

    let store = SearchStore::open_readonly(&root.generation_path(active.generation_id))
        .expect("open empty generation read-only");
    let stats = store.stats().expect("read empty generation stats");
    assert_eq!((stats.artists, stats.releases, stats.tracks), (0, 0, 0));
    assert!(
        store
            .search(
                &parse_and_plan("artist:anything").expect("valid MQL"),
                10,
                0
            )
            .expect("query empty generation")
            .is_empty()
    );
    drop(store);

    let healthy = melodee_search::diagnostic(&config, &db.pool)
        .await
        .expect("diagnose empty generation");
    assert!(healthy.generation.is_some());
    assert!(healthy.generation_error.is_none());

    let sentinel_root = config.search_root.display().to_string();
    std::fs::remove_file(
        root.generation_path(active.generation_id)
            .join("generation.json"),
    )
    .expect("remove disposable manifest");
    let missing_manifest = melodee_search::diagnostic(&config, &db.pool)
        .await
        .expect("missing manifest is a diagnostic result");
    assert!(missing_manifest.generation.is_none());
    assert_eq!(
        missing_manifest.generation_error_code.as_deref(),
        Some("search.filesystem")
    );
    let serialized = serde_json::to_string(&missing_manifest).expect("serialize diagnostic");
    assert!(!serialized.contains(&sentinel_root));
    assert!(!serialized.contains("generation.json"));
    db.drop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn lease_theft_fences_commit_and_scheduler_converges_on_next_pass() {
    let db = TestDb::create("phase6_outbox_heartbeat").await;
    let directory = TempDir::new().expect("search temp root");
    let mut config = activate_empty_generation(&db, &directory).await;
    config.lease_seconds = 1;
    config.poll_interval_ms = 10;
    let barrier = melodee_search::SearchApplyTestBarrier::new();
    config.apply_test_barrier = Some(barrier.clone());

    let entity_id = uuid::Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();
    let outbox_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'delete', 1, $3, $3)
         RETURNING id",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(entity_id)
    .bind(now)
    .fetch_one(&db.pool)
    .await
    .expect("seed deletion outbox row");

    let scheduler = melodee_search::start_search_loop(config.clone(), db.pool.clone());
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        barrier.wait_until_reached(),
    )
    .await
    .expect("application reached pre-commit barrier");
    let stale_owner = sqlx::query_scalar::<_, String>(
        "SELECT lease_owner FROM catalog_projection_outbox WHERE id = $1",
    )
    .bind(outbox_id)
    .fetch_one(&db.pool)
    .await
    .expect("read original lease owner");
    let thief = "phase6-lease-thief";
    let mut theft = db.pool.begin().await.expect("begin lease theft");
    let stolen_expiry = now + time::Duration::seconds(60);
    let stolen_row = sqlx::query(
        "UPDATE catalog_projection_outbox
         SET lease_owner = $2, lease_expires_at = $3
         WHERE id = $1 AND lease_owner = $4",
    )
    .bind(outbox_id)
    .bind(thief)
    .bind(stolen_expiry)
    .bind(&stale_owner)
    .execute(&mut *theft)
    .await
    .expect("steal row lease");
    assert_eq!(stolen_row.rows_affected(), 1);
    let stolen_checkpoint = sqlx::query(
        "UPDATE search_projection_checkpoint
         SET lease_owner = $2, lease_expires_at = $3
         WHERE projection_name = $1 AND lease_owner = $4",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(thief)
    .bind(stolen_expiry)
    .bind(&stale_owner)
    .execute(&mut *theft)
    .await
    .expect("steal checkpoint lease");
    assert_eq!(stolen_checkpoint.rows_affected(), 1);
    theft.commit().await.expect("commit lease theft");

    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        barrier.wait_until_cancelled(),
    )
    .await
    .expect("failed heartbeat cancelled blocking apply before commit");
    let stale_state = sqlx::query_as::<_, (bool, bool)>(
        "SELECT processed_at IS NOT NULL, failed_at IS NOT NULL
         FROM catalog_projection_outbox WHERE id = $1",
    )
    .bind(outbox_id)
    .fetch_one(&db.pool)
    .await
    .expect("read fenced row state");
    assert_eq!(stale_state, (false, false));

    let mut release_thief = db.pool.begin().await.expect("begin thief abandonment");
    sqlx::query(
        "UPDATE catalog_projection_outbox
         SET lease_owner = NULL, lease_expires_at = NULL
         WHERE id = $1 AND lease_owner = $2",
    )
    .bind(outbox_id)
    .bind(thief)
    .execute(&mut *release_thief)
    .await
    .expect("release stolen row lease");
    sqlx::query(
        "UPDATE search_projection_checkpoint
         SET lease_owner = NULL, lease_expires_at = NULL
         WHERE projection_name = $1 AND lease_owner = $2",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(thief)
    .execute(&mut *release_thief)
    .await
    .expect("release stolen checkpoint lease");
    release_thief.commit().await.expect("abandon stolen lease");

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let processed = sqlx::query_scalar::<_, bool>(
                "SELECT processed_at IS NOT NULL FROM catalog_projection_outbox WHERE id = $1",
            )
            .bind(outbox_id)
            .fetch_one(&db.pool)
            .await
            .expect("poll outbox completion");
            if processed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("scheduler resumed after pass-local lease loss");
    scheduler.stop().await;

    let root = SearchRoot::open(&config.search_root).expect("owned root");
    let active =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read active generation")
            .expect("active generation");
    let mut writer = SearchStore::open(&root.generation_path(active.generation_id))
        .expect("open converged writer");
    assert!(
        !writer
            .delete("artist", &entity_id.to_string(), 1)
            .expect("revision fence")
    );
    db.drop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn scheduler_stop_interrupts_long_idle_poll() {
    let db = TestDb::create("phase6_scheduler_stop").await;
    let directory = TempDir::new().expect("search temp root");
    let mut config = activate_empty_generation(&db, &directory).await;
    config.poll_interval_ms = 60_000;
    let probe = melodee_search::SearchSchedulerTestProbe::new();
    config.scheduler_test_probe = Some(probe.clone());
    let scheduler = melodee_search::start_search_loop(config, db.pool.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), probe.wait_until_idle())
        .await
        .expect("scheduler reached idle poll");
    tokio::time::timeout(std::time::Duration::from_secs(1), scheduler.stop())
        .await
        .expect("stop interrupts sixty-second poll");
    db.drop().await;
}

#[tokio::test]
async fn postgres_snapshots_preserve_release_and_track_artist_roles_in_index() {
    let db = TestDb::create("phase6_role_snapshot_index").await;
    let fixture = seed_published_release(
        &db,
        602,
        CatalogFixturePresentation {
            release_title: "Role Snapshot Release",
            release_artist: "Fixture Release Artist",
            track_title: "Role Snapshot Track",
            track_artist: "Fixture Track Artist",
            release_date: "2006-07-08",
        },
    )
    .await;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim fixture");
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase6.role-fixture.v1", "phase6-test"),
            now,
        )
        .await
        .expect("catalog role fixture");
    let release_id = sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM releases LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .expect("release id");
    let track_id = sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM tracks LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .expect("track id");

    let role_artists = [
        ("Release Primary Probe", "release", "primary", true),
        ("Track Primary Probe", "track", "primary", true),
        ("Release Featured Probe", "release", "featured", true),
        ("Track Featured Probe", "track", "featured", true),
        ("Release Other Probe", "release", "other", true),
        ("Track Other Probe", "track", "other", true),
        ("Release Performer Probe", "release", "vocalist", true),
        ("Track Performer Probe", "track", "vocalist", true),
        ("Non Performance Probe", "track", "producer", false),
    ]
    .map(|(name, scope, role, performance)| (uuid::Uuid::now_v7(), name, scope, role, performance));
    for (artist_id, name, _, _, _) in role_artists {
        sqlx::query(
            "INSERT INTO artists (
                id, public_api_id, canonical_name, normalized_name, sort_name,
                artist_type, status, created_at, updated_at
             ) VALUES ($1, $2, $3, UPPER($3), $3,
                       'person', 'verified', $4, $4)",
        )
        .bind(artist_id)
        .bind(uuid::Uuid::now_v7())
        .bind(name)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("seed role Artist");
    }
    for (artist_id, name, scope, role, performance) in role_artists {
        match (scope, role) {
            ("release", "primary" | "featured" | "other") => {
                sqlx::query(
                    "INSERT INTO release_artists (
                        id, release_id, artist_id, role, presentation_order,
                        display_value, join_phrase, created_at
                     ) VALUES ($1, $2, $3, $4,
                         (SELECT COALESCE(max(presentation_order), 0) + 1
                          FROM release_artists WHERE release_id = $2),
                         $5, '', $6)",
                )
                .bind(uuid::Uuid::now_v7())
                .bind(release_id)
                .bind(artist_id)
                .bind(role)
                .bind(name)
                .bind(now)
                .execute(&db.pool)
                .await
                .expect("seed ReleaseArtist role");
            }
            ("track", "primary" | "featured" | "other") => {
                sqlx::query(
                    "INSERT INTO track_artists (
                        id, track_id, artist_id, role, presentation_order,
                        display_value, join_phrase, created_at
                     ) VALUES ($1, $2, $3, $4,
                         (SELECT COALESCE(max(presentation_order), 0) + 1
                          FROM track_artists WHERE track_id = $2),
                         $5, '', $6)",
                )
                .bind(uuid::Uuid::now_v7())
                .bind(track_id)
                .bind(artist_id)
                .bind(role)
                .bind(name)
                .bind(now)
                .execute(&db.pool)
                .await
                .expect("seed TrackArtist role");
            }
            _ => {
                sqlx::query(
                    "INSERT INTO contributions (
                        id, release_id, track_id, artist_id, observed_artist_id,
                        role, evidence_source, presentation_order,
                        is_performance, created_at
                     ) VALUES ($1, $2, $3, $4, NULL, $5, 'phase6-test', 0, $6, $7)",
                )
                .bind(uuid::Uuid::now_v7())
                .bind(release_id)
                .bind((scope == "track").then_some(track_id))
                .bind(artist_id)
                .bind(role)
                .bind(performance)
                .bind(now)
                .execute(&db.pool)
                .await
                .expect("seed Contribution role");
            }
        }
    }

    let directory = TempDir::new().expect("index directory");
    let mut store = SearchStore::create(directory.path()).expect("create role index");
    for (artist_id, _, _, _, _) in role_artists {
        let record = SearchProjectionStore::load_catalog_record(
            &db.pool,
            melodee_persistence::SearchEntityType::Artist,
            artist_id,
        )
        .await
        .expect("load Artist snapshot")
        .expect("Artist snapshot");
        let SearchCatalogRecord::Artist(record) = record else {
            panic!("expected Artist snapshot")
        };
        store
            .upsert_artist(&ArtistDoc::from(record))
            .expect("index Artist snapshot");
    }
    let release = SearchProjectionStore::load_catalog_record(
        &db.pool,
        melodee_persistence::SearchEntityType::Release,
        release_id,
    )
    .await
    .expect("load Release snapshot")
    .expect("Release snapshot");
    let SearchCatalogRecord::Release(release) = release else {
        panic!("expected Release snapshot")
    };
    store
        .upsert_release(&ReleaseDoc::from(release))
        .expect("index Release snapshot");
    let track = SearchProjectionStore::load_catalog_record(
        &db.pool,
        melodee_persistence::SearchEntityType::Track,
        track_id,
    )
    .await
    .expect("load Track snapshot")
    .expect("Track snapshot");
    let SearchCatalogRecord::Track(track) = track else {
        panic!("expected Track snapshot")
    };
    store
        .upsert_track(&TrackDoc::from(track))
        .expect("index Track snapshot");
    store.commit().expect("commit role snapshots");

    for (expression, release_matches, track_matches) in [
        ("primaryArtist:\"Release Primary Probe\"", true, true),
        ("primaryArtist:\"Track Primary Probe\"", false, true),
        ("featuredArtist:\"Release Featured Probe\"", true, true),
        ("featuredArtist:\"Track Featured Probe\"", false, true),
        ("other:\"Release Other Probe\"", true, true),
        ("other:\"Track Other Probe\"", false, true),
        ("performer:\"Release Performer Probe\"", true, true),
        ("performer:\"Track Performer Probe\"", false, true),
        ("performer:\"Non Performance Probe\"", false, false),
    ] {
        let hits = store
            .search(&parse_and_plan(expression).expect("valid role MQL"), 20, 0)
            .expect("query indexed PG snapshots");
        assert_eq!(
            hits.iter()
                .any(|hit| hit.kind == "release" && hit.internal_id == release_id.to_string()),
            release_matches,
            "Release mismatch for {expression}: {hits:?}"
        );
        assert_eq!(
            hits.iter()
                .any(|hit| hit.kind == "track" && hit.internal_id == track_id.to_string()),
            track_matches,
            "Track mismatch for {expression}: {hits:?}"
        );
    }
    db.drop().await;
}

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

#[tokio::test]
async fn public_identity_correction_merges_aliases_into_real_search_workers() {
    let db = TestDb::create("phase6_identity_correction").await;
    let fixture = seed_published_release(
        &db,
        602,
        CatalogFixturePresentation {
            release_title: "Correction Proof Release",
            release_artist: "Correction Source Artist",
            track_title: "Correction Proof Track",
            track_artist: "Correction Track Artist",
            release_date: "2008-08-08",
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
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase6.identity-correction.v1", "phase6-test"),
            now,
        )
        .await
        .expect("catalog fixture");
    assert!(!std::path::Path::new(&fixture.source_path).exists());
    assert!(!std::path::Path::new(&fixture.canonical_asset_path).exists());

    let source_artist: (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT id, public_api_id FROM artists WHERE canonical_name = $1")
            .bind("Correction Source Artist")
            .fetch_one(&db.pool)
            .await
            .expect("read imported source Artist");
    let target_artist = Artist::new(NewArtist {
        id: ArtistId::new(),
        public_api_id: ArtistPublicId::from_uuid(uuid::Uuid::now_v7()),
        canonical_name: "Correction Target Artist".to_owned(),
        normalized_name: "CORRECTIONTARGETARTIST".to_owned(),
        sort_name: "Correction Target Artist".to_owned(),
        artist_type: ArtistType::Person,
        disambiguation: None,
        active_dates: None,
        status: ArtistStatus::Verified,
        created_at: now,
    })
    .expect("valid target Artist");
    catalog
        .create_artist(&db.pool, &target_artist)
        .await
        .expect("create correction target Artist");
    let alias = ArtistAlias {
        id: ArtistAliasId::new(),
        artist_id: ArtistId::from_uuid(source_artist.0),
        value: "Correction Alias".to_owned(),
        normalized_value: "CORRECTIONALIAS".to_owned(),
        alias_type: ArtistAliasType::Search,
        locale: None,
        evidence_source: "phase6-identity-correction".to_owned(),
        confidence: Confidence::new(10_000).expect("valid confidence"),
        locked: true,
    };
    catalog
        .add_artist_alias(&db.pool, &alias, now)
        .await
        .expect("seed alias on source Artist");

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

    let source_hits = melodee_search::query_active(
        &config,
        &db.pool,
        "artist:\"Correction Source Artist\"",
        20,
        0,
    )
    .await
    .expect("query source Artist before correction");
    assert!(
        source_hits
            .iter()
            .any(|hit| hit.kind == "artist" && hit.internal_id == source_artist.0.to_string()),
        "source artist should be searchable before correction: {source_hits:?}"
    );
    let alias_hits =
        melodee_search::query_active(&config, &db.pool, "alias:Correction Alias", 20, 0)
            .await
            .expect("query alias before correction");
    assert!(
        alias_hits
            .iter()
            .any(|hit| hit.kind == "artist" && hit.internal_id == source_artist.0.to_string()),
        "alias should resolve to source artist before correction: {alias_hits:?}"
    );
    assert_eq!(
        resolve_artist_ids_active(&config, &db.pool, "Correction Alias")
            .await
            .expect("resolve alias before correction"),
        vec![source_artist.0.to_string()]
    );
    assert_eq!(
        resolve_artist_ids_active(&config, &db.pool, "Correction Target Artist")
            .await
            .expect("resolve target before correction"),
        vec![target_artist.id().into_uuid().to_string()]
    );

    let merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: ArtistId::from_uuid(source_artist.0),
        source_public_api_id: ArtistPublicId::from_uuid(source_artist.1),
        target_artist_id: target_artist.id(),
        actor: "catalog-admin".to_owned(),
        reason: "merge duplicate observations through the public correction API".to_owned(),
        occurred_at: now,
    };
    catalog
        .merge_artist(&db.pool, &merge, "manual-identity-correction-v1")
        .await
        .expect("merge source Artist into target");
    let pending_after_merge: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM catalog_projection_outbox
         WHERE projection_name = $1 AND processed_at IS NULL AND failed_at IS NULL",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .fetch_one(&db.pool)
    .await
    .expect("count pending search outbox after merge");
    assert!(pending_after_merge > 0);

    let mut total_applied = 0_u64;
    for _ in 0..8 {
        let pass = apply_outbox_once(&config, &db.pool)
            .await
            .expect("apply correction outbox");
        total_applied += pass.applied as u64;
        if pass.claimed == 0 {
            break;
        }
    }
    assert!(total_applied > 0);
    let drained = apply_outbox_once(&config, &db.pool)
        .await
        .expect("confirm outbox drain");
    assert_eq!(drained.claimed, 0);

    let merged_source_hits = melodee_search::query_active(
        &config,
        &db.pool,
        "artist:\"Correction Source Artist\"",
        20,
        0,
    )
    .await
    .expect("query redirected source Artist");
    assert!(
        merged_source_hits
            .iter()
            .any(|hit| hit.kind == "artist" && hit.internal_id == target_artist.id().to_string()),
        "merged source artist should resolve to the canonical target: {merged_source_hits:?}"
    );
    assert!(
        merged_source_hits
            .iter()
            .all(|hit| hit.internal_id != source_artist.0.to_string()),
        "merged source tombstone should not remain searchable: {merged_source_hits:?}"
    );
    let merged_target_hits = melodee_search::query_active(
        &config,
        &db.pool,
        "artist:\"Correction Target Artist\"",
        20,
        0,
    )
    .await
    .expect("query merged target Artist");
    assert!(
        merged_target_hits
            .iter()
            .any(|hit| hit.kind == "artist" && hit.internal_id == target_artist.id().to_string()),
        "target artist should remain searchable after merge: {merged_target_hits:?}"
    );
    let merged_alias_hits =
        melodee_search::query_active(&config, &db.pool, "alias:Correction Alias", 20, 0)
            .await
            .expect("query alias after merge");
    assert!(
        merged_alias_hits
            .iter()
            .any(|hit| hit.kind == "artist" && hit.internal_id == target_artist.id().to_string()),
        "alias should move to target artist after merge: {merged_alias_hits:?}"
    );
    assert_eq!(
        resolve_artist_ids_active(&config, &db.pool, "Correction Alias")
            .await
            .expect("resolve alias after merge"),
        vec![target_artist.id().into_uuid().to_string()]
    );
    assert_eq!(
        resolve_artist_ids_active(&config, &db.pool, "Correction Source Artist")
            .await
            .expect("resolve source after merge"),
        vec![target_artist.id().into_uuid().to_string()]
    );
    assert_eq!(
        resolve_artist_ids_active(&config, &db.pool, "Correction Target Artist")
            .await
            .expect("resolve target after merge"),
        vec![target_artist.id().into_uuid().to_string()]
    );
    assert_eq!(
        resolve_artist_ids_active(&config, &db.pool, &source_artist.0.to_string())
            .await
            .expect("resolve source internal id after merge"),
        vec![target_artist.id().into_uuid().to_string()]
    );
    assert_eq!(
        resolve_artist_ids_active(&config, &db.pool, &source_artist.1.to_string())
            .await
            .expect("resolve source public id after merge"),
        vec![target_artist.id().into_uuid().to_string()]
    );
    let redirected_id_hits =
        melodee_search::query_active(&config, &db.pool, &format!("id:{}", source_artist.0), 20, 0)
            .await
            .expect("query redirected source internal id");
    assert!(
        redirected_id_hits
            .iter()
            .any(|hit| hit.kind == "artist" && hit.internal_id == target_artist.id().to_string()),
        "source internal id should redirect to target artist: {redirected_id_hits:?}"
    );
    let redirected_public_hits = melodee_search::query_active(
        &config,
        &db.pool,
        &format!("publicId:{}", source_artist.1),
        20,
        0,
    )
    .await
    .expect("query redirected source public id");
    assert!(
        redirected_public_hits
            .iter()
            .any(|hit| hit.kind == "artist" && hit.internal_id == target_artist.id().to_string()),
        "source public id should redirect to target artist: {redirected_public_hits:?}"
    );
    assert!(!std::path::Path::new(&fixture.source_path).exists());
    assert!(!std::path::Path::new(&fixture.canonical_asset_path).exists());

    db.drop().await;
}

#[tokio::test]
async fn outbox_apply_pass_survives_repeated_process_termination() {
    let db = TestDb::create("phase6_outbox_termination").await;
    let fixture = seed_published_release(
        &db,
        701,
        CatalogFixturePresentation {
            release_title: "Termination Proof Release",
            release_artist: "Termination Source Artist",
            track_title: "Termination Proof Track",
            track_artist: "Termination Track Artist",
            release_date: "2008-08-08",
        },
    )
    .await;

    let directory = TempDir::new().expect("search temp root");
    let search_path = directory.path().join("search");
    let config = SearchConfig {
        search_root: search_path,
        enabled: true,
        // Short lease so a terminated worker's claimed outbox items expire
        // quickly and are reclaimed by the next pass.
        lease_seconds: 1,
        claim_batch_size: 256,
        poll_interval_ms: 1_000,
        ..SearchConfig::default()
    };

    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let claimed = catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim published fixture");
    assert_eq!(claimed.len(), 1);
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase6.outbox-termination.v1", "phase6-test"),
            now,
        )
        .await
        .expect("catalog fixture");

    // Request and run a rebuild to create an active owned generation.
    let requested = request_rebuild(&config, &db.pool, "phase6-outbox-termination")
        .await
        .expect("request rebuild");
    assert_eq!(requested.status, "requested");
    let _rebuild_job = run_rebuild_once(&config, &db.pool)
        .await
        .expect("run rebuild");

    // Read the seeded catalog identities so we can seed outbox rows.
    let artist_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM artists WHERE canonical_name = $1")
            .bind("Termination Source Artist")
            .fetch_one(&db.pool)
            .await
            .expect("read source artist id");
    let release_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM releases WHERE title = $1")
        .bind("Termination Proof Release")
        .fetch_one(&db.pool)
        .await
        .expect("read release id");
    let track_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE title = $1")
        .bind("Termination Proof Track")
        .fetch_one(&db.pool)
        .await
        .expect("read track id");

    // Seed outbox rows that the apply pass must process. These mirror the
    // rows a catalog correction would emit after the initial rebuild. Use a
    // higher revision than the initial snapshot so the upsert actually writes
    // into the owned generation.
    let mut seed_tx = db.pool.begin().await.expect("begin outbox seed");
    sqlx::query("UPDATE artists SET revision = 2, updated_at = $1 WHERE id = $2")
        .bind(now)
        .bind(artist_id)
        .execute(&mut *seed_tx)
        .await
        .expect("bump artist revision");
    sqlx::query("UPDATE releases SET revision = 2, updated_at = $1 WHERE id = $2")
        .bind(now)
        .bind(release_id)
        .execute(&mut *seed_tx)
        .await
        .expect("bump release revision");
    sqlx::query("UPDATE tracks SET revision = 2, updated_at = $1 WHERE id = $2")
        .bind(now)
        .bind(track_id)
        .execute(&mut *seed_tx)
        .await
        .expect("bump track revision");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'artist', $2, 'upsert', $3, $4, $4)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(artist_id)
    .bind(2i64)
    .bind(now)
    .execute(&mut *seed_tx)
    .await
    .expect("seed artist outbox row");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'release', $2, 'upsert', $3, $4, $4)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(release_id)
    .bind(2i64)
    .bind(now)
    .execute(&mut *seed_tx)
    .await
    .expect("seed release outbox row");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'track', $2, 'upsert', $3, $4, $4)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(track_id)
    .bind(2i64)
    .bind(now)
    .execute(&mut *seed_tx)
    .await
    .expect("seed track outbox row");
    seed_tx.commit().await.expect("commit outbox seed");

    let root = SearchRoot::open(&config.search_root).expect("open search root");
    let active =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read active generation")
            .expect("active generation");
    let generation_path = root.generation_path(active.generation_id);
    let rebuild_counts = active.document_counts;

    // Repeatedly simulate whole-process termination during the outbox apply
    // pass: claim a batch, apply rows to Tantivy, then abandon the lease
    // without acknowledging. The lease expires and the next pass reclaims
    // the items, proving exactly-once reclamation across repeated
    // terminations.
    let round_count = 5;
    for round in 0..round_count {
        let round_now = time::OffsetDateTime::now_utc();
        let items = SearchProjectionStore::claim_outbox(
            &db.pool,
            SEARCH_PROJECTION_NAME,
            &format!("search-termination-worker-{round}"),
            config.claim_batch_size,
            std::time::Duration::from_secs(config.lease_seconds),
            round_now,
        )
        .await
        .expect("claim outbox batch");
        assert!(
            !items.is_empty(),
            "round {round}: expected pending outbox items to be reclaimable"
        );
        let mut store = SearchStore::open(&generation_path).expect("open generation for apply");
        for item in &items {
            let record = SearchProjectionStore::load_catalog_record(
                &db.pool,
                item.entity_type,
                item.entity_id,
            )
            .await
            .expect("load catalog record");
            match (item.operation, record) {
                (SearchOperation::Upsert, Some(SearchCatalogRecord::Artist(inner))) => {
                    store
                        .upsert_artist(&ArtistDoc::from(inner.clone()))
                        .expect("upsert artist");
                }
                (SearchOperation::Upsert, Some(SearchCatalogRecord::Release(inner))) => {
                    store
                        .upsert_release(&ReleaseDoc::from(inner.clone()))
                        .expect("upsert release");
                }
                (SearchOperation::Upsert, Some(SearchCatalogRecord::Track(inner))) => {
                    store
                        .upsert_track(&TrackDoc::from(inner.clone()))
                        .expect("upsert track");
                }
                (SearchOperation::Delete, _) | (SearchOperation::Upsert, None) => {
                    if item.entity_type == SearchEntityType::AudioIdentity {
                        continue;
                    }
                    store
                        .delete(
                            item.entity_type.as_str(),
                            &item.entity_id.to_string(),
                            item.entity_revision,
                        )
                        .expect("delete");
                }
                (SearchOperation::Upsert, Some(SearchCatalogRecord::NoDocument)) => {}
            }
        }
        store.commit().expect("commit applied rows");
        // Simulate whole-process termination: drop the worker, do NOT release
        // the outbox lease. The lease expires and the items are reclaimable.
        drop(store);
        tokio::time::sleep(std::time::Duration::from_secs(config.lease_seconds + 1)).await;
    }

    // Final pass: reclaim expired leases and re-apply exactly once.
    // The outbox holds the three seeded rows plus any rows left over from
    // the initial rebuild load, so assert it reclaims at least the seeded
    // rows and then fully drains the queue.
    let final_pass = apply_outbox_once(&config, &db.pool)
        .await
        .expect("final outbox pass");
    assert!(
        final_pass.claimed >= 3,
        "final pass should reclaim expired-lease outbox items: {final_pass:?}"
    );
    let drained = apply_outbox_once(&config, &db.pool)
        .await
        .expect("confirm outbox drain");
    assert_eq!(
        drained.claimed, 0,
        "outbox queue should be drained after final pass"
    );

    // Verify convergence: no duplicates, no lost documents, query results
    // correct. The owned index must contain the same document counts as the
    // post-rebuild baseline even though the outbox apply pass was terminated
    // and reclaimed repeatedly, proving exactly-once reclamation.
    let store = SearchStore::open(&generation_path).expect("open converged generation");
    let stats = store.stats().expect("converged counts");
    assert_eq!(
        (
            stats.artists as u64,
            stats.releases as u64,
            stats.tracks as u64
        ),
        (
            rebuild_counts.artists,
            rebuild_counts.releases,
            rebuild_counts.tracks
        ),
        "document counts must not change across termination rounds"
    );
    // Each entity must be queryable exactly once (no duplicate documents).
    let artist_hits = store
        .search(
            &parse_and_plan("artist:\"Termination Source Artist\"").expect("valid MQL"),
            20,
            0,
        )
        .expect("query source artist");
    let artist_matches: Vec<_> = artist_hits
        .iter()
        .filter(|hit| hit.kind == "artist")
        .filter(|hit| hit.internal_id == artist_id.to_string())
        .cloned()
        .collect();
    assert_eq!(
        artist_matches.len(),
        1,
        "source artist should be present exactly once: {artist_hits:?}"
    );
    let release_hits = store
        .search(
            &parse_and_plan("title:\"Termination Proof Release\"").expect("valid MQL"),
            20,
            0,
        )
        .expect("query release");
    let release_matches: Vec<_> = release_hits
        .iter()
        .filter(|hit| hit.kind == "release")
        .filter(|hit| hit.internal_id == release_id.to_string())
        .cloned()
        .collect();
    assert_eq!(
        release_matches.len(),
        1,
        "release should be present exactly once: {release_hits:?}"
    );
    let track_hits = store
        .search(
            &parse_and_plan("title:\"Termination Proof Track\"").expect("valid MQL"),
            20,
            0,
        )
        .expect("query track");
    let track_matches: Vec<_> = track_hits
        .iter()
        .filter(|hit| hit.kind == "track")
        .filter(|hit| hit.internal_id == track_id.to_string())
        .cloned()
        .collect();
    assert_eq!(
        track_matches.len(),
        1,
        "track should be present exactly once: {track_hits:?}"
    );
    drop(store);
    db.drop().await;
}

/// `SPEC.md` §11.2 ends the Release lifecycle at `Indexed`, and
/// `IngestionState::is_terminal` names it as the successful terminal state. The
/// search worker owns the only durable evidence that indexing finished, so it
/// must also perform `Indexing -> Indexed`; otherwise every accepted Release
/// parks in `indexing` forever (never terminal, never claimable, invisible to
/// the terminal-state History view).
#[tokio::test]
async fn projection_completion_advances_release_to_terminal_indexed_state() {
    use melodee_persistence::WorkItemStore;

    let db = TestDb::create("phase6_lifecycle_completion").await;
    let fixture = seed_published_release(
        &db,
        611,
        CatalogFixturePresentation {
            release_title: "Lifecycle Completion Release",
            release_artist: "Lifecycle Artist",
            track_title: "Lifecycle Track",
            track_artist: "Lifecycle Artist",
            release_date: "2007-08-09",
        },
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim published fixture");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase6.lifecycle.v1", "phase6-test"),
            now,
        )
        .await
        .expect("catalog fixture");

    // Simulate the policy worker accepting the Release: `Accept` targets
    // `Indexing` and the projection rows are already enqueued by the import.
    sqlx::query(
        "UPDATE release_work_items
            SET state = 'indexing', state_reason = 'policy.disposition.accept', updated_at = $2
          WHERE id = $1",
    )
    .bind(fixture.work_item_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("park fixture in indexing");

    let directory = TempDir::new().expect("search temp root");
    let config = activate_empty_generation(&db, &directory).await;

    // Nothing is projected yet, so completion must not fire.
    let completed = WorkItemStore
        .complete_indexed(
            &db.pool,
            time::OffsetDateTime::now_utc(),
            100,
            "test.pending",
        )
        .await
        .expect("completion attempt with pending projection");
    assert!(
        completed.is_empty(),
        "a Release with pending projection rows must not complete"
    );

    // Drain the projection: this is the durable evidence that indexing finished.
    let pass = apply_outbox_once(&config, &db.pool)
        .await
        .expect("apply projection");
    assert!(pass.acknowledged > 0, "fixture projection was applied");

    let state: (String, bool) = sqlx::query_as(
        "SELECT state, completed_at IS NOT NULL FROM release_work_items WHERE id = $1",
    )
    .bind(fixture.work_item_id)
    .fetch_one(&db.pool)
    .await
    .expect("read work item");
    assert_eq!(
        state.0, "indexed",
        "drained projection completes the lifecycle"
    );
    assert!(state.1, "the terminal state records completed_at");

    // The terminal hop is auditable exactly like a worker transition, so
    // history, timelines and the activity series can see it.
    let events: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT event_type, from_state::text, to_state::text
           FROM release_work_item_events
          WHERE work_item_id = $1 AND event_type IN ('transitioned', 'completed')
          ORDER BY id DESC LIMIT 2",
    )
    .bind(fixture.work_item_id)
    .fetch_all(&db.pool)
    .await
    .expect("read completion events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "completed");
    assert_eq!(events[1].0, "transitioned");
    assert_eq!(events[1].1.as_deref(), Some("indexing"));
    assert_eq!(events[1].2.as_deref(), Some("indexed"));

    // Idempotent: an already-terminal Release is never re-completed.
    let again = WorkItemStore
        .complete_indexed(&db.pool, time::OffsetDateTime::now_utc(), 100, "test.again")
        .await
        .expect("second completion pass");
    assert!(again.is_empty(), "completion must be idempotent");

    db.drop().await;
}

/// A pending projection row for any entity of the Release (its own row, a
/// Track, or a billed Artist) blocks completion until it is applied — otherwise
/// the UI would report `Indexed` while the index is still missing documents.
#[tokio::test]
async fn projection_completion_waits_for_every_entity_of_the_release() {
    use melodee_persistence::WorkItemStore;

    let db = TestDb::create("phase6_lifecycle_blocks").await;
    let fixture = seed_published_release(
        &db,
        612,
        CatalogFixturePresentation {
            release_title: "Blocking Projection Release",
            release_artist: "Blocking Artist",
            track_title: "Blocking Track",
            track_artist: "Blocking Artist",
            release_date: "2008-09-10",
        },
    )
    .await;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim published fixture");
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase6.blocks.v1", "phase6-test"),
            now,
        )
        .await
        .expect("catalog fixture");
    sqlx::query(
        "UPDATE release_work_items
            SET state = 'indexing', state_reason = 'policy.disposition.accept', updated_at = $2
          WHERE id = $1",
    )
    .bind(fixture.work_item_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("park fixture in indexing");

    let directory = TempDir::new().expect("search temp root");
    let config = activate_empty_generation(&db, &directory).await;
    apply_outbox_once(&config, &db.pool)
        .await
        .expect("drain initial projection");

    // A later Track revision is queued but not yet applied.
    let (track_id, revision): (uuid::Uuid, i64) = sqlx::query_as(
        "UPDATE tracks SET revision = revision + 1 WHERE release_id = (
             SELECT release_id FROM catalog_imports WHERE work_item_id = $1
         ) RETURNING id, revision",
    )
    .bind(fixture.work_item_id)
    .fetch_one(&db.pool)
    .await
    .expect("queue track revision");
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, created_at
         ) VALUES ($1, 'track', $2, 'upsert', $3, $4, $4)",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .bind(track_id)
    .bind(revision)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("enqueue pending track row");

    let blocked = WorkItemStore
        .complete_indexed(
            &db.pool,
            time::OffsetDateTime::now_utc(),
            100,
            "test.blocked",
        )
        .await
        .expect("completion attempt while a Track is pending");
    assert!(
        blocked.is_empty(),
        "a pending Track projection must block completion"
    );

    apply_outbox_once(&config, &db.pool)
        .await
        .expect("apply the pending track row");
    let state: String = sqlx::query_scalar("SELECT state FROM release_work_items WHERE id = $1")
        .bind(fixture.work_item_id)
        .fetch_one(&db.pool)
        .await
        .expect("read work item");
    assert_eq!(
        state, "indexed",
        "completion follows the last applied entity"
    );

    db.drop().await;
}

/// Guard against unbounded index fragmentation.
///
/// Each projection batch opens its own writer and commits, so a busy catalog
/// produces many small commits. Tantivy's log-merge policy consolidates them
/// asynchronously; this test pins the property that matters — after N batches
/// the generation still holds fewer segments than batches, so fragmentation
/// cannot grow without bound. (Measured on this workload: 30 batches leave 3
/// segments.)
#[tokio::test]
async fn repeated_projection_passes_do_not_abandon_one_segment_per_batch() {
    let db = TestDb::create("phase6_segment_growth").await;
    let fixture = seed_published_release(
        &db,
        613,
        CatalogFixturePresentation {
            release_title: "Segment Growth Release",
            release_artist: "Segment Artist",
            track_title: "Segment Track",
            track_artist: "Segment Artist",
            release_date: "2009-10-11",
        },
    )
    .await;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim published fixture");
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase6.segments.v1", "phase6-test"),
            now,
        )
        .await
        .expect("catalog fixture");

    let directory = TempDir::new().expect("search temp root");
    let config = activate_empty_generation(&db, &directory).await;
    apply_outbox_once(&config, &db.pool)
        .await
        .expect("initial projection");

    let passes = 30;
    for index in 0..passes {
        let (artist_id, revision): (uuid::Uuid, i64) = sqlx::query_as(
            "UPDATE artists
                SET revision = revision + 1, updated_at = $1
              RETURNING id, revision",
        )
        .bind(time::OffsetDateTime::now_utc())
        .fetch_one(&db.pool)
        .await
        .expect("queue artist revision");
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, created_at
             ) VALUES ($1, 'artist', $2, 'upsert', $3, $4, $4)",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(artist_id)
        .bind(revision)
        .bind(time::OffsetDateTime::now_utc())
        .execute(&db.pool)
        .await
        .expect("enqueue artist revision");
        let pass = apply_outbox_once(&config, &db.pool)
            .await
            .expect("apply revision batch");
        assert_eq!(pass.acknowledged, 1, "pass {index} applied its revision");
    }

    let root = SearchRoot::open(&config.search_root).expect("owned search root");
    let active =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read active generation")
            .expect("an active generation exists");
    let generation_path = root.generation_path(active.generation_id);
    let segments = std::fs::read_dir(&generation_path)
        .expect("read generation")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "term"))
        .count();
    assert!(
        segments < passes,
        "projection passes must consolidate segments, found {segments} after {passes} passes"
    );

    db.drop().await;
}

/// 0044 may already be applied while an existing generation still stores provisional.
/// 0045 must make the ordinary outbox worker replace that revision without a rebuild.
#[tokio::test]
async fn migration_0045_refreshes_artist_status_in_an_existing_generation() {
    use melodee_persistence::{MIGRATOR, migrate};
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;
    let db = TestDb::create_unmigrated("search_status_upgrade").await;
    Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .migrations
                .iter()
                .filter(|migration| migration.version <= 44)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .expect("pre-repair schema");
    let artist:uuid::Uuid=sqlx::query_scalar("INSERT INTO artists(id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,revision,created_at,updated_at) VALUES(uuidv7(),uuidv7(),'Status Artist','status artist','Status Artist','unknown','active',1,now(),now()) RETURNING id")
        .fetch_one(&db.pool).await.expect("Artist after status rename");
    let directory = TempDir::new().expect("search temp root");
    let config = activate_empty_generation(&db, &directory).await;
    let root = SearchRoot::open(&config.search_root).expect("root");
    let active =
        SearchProjectionStore::active_generation(&db.pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("generation lookup")
            .expect("active");
    let Some(SearchCatalogRecord::Artist(record)) =
        SearchProjectionStore::load_catalog_record(&db.pool, SearchEntityType::Artist, artist)
            .await
            .expect("record")
    else {
        panic!("missing Artist");
    };
    let mut stale = ArtistDoc::from(record);
    stale.status = "provisional".to_owned();
    stale.revision = 2;
    sqlx::query("UPDATE artists SET revision=2 WHERE id=$1")
        .bind(artist)
        .execute(&db.pool)
        .await
        .expect("historical revision");
    let mut store = SearchStore::open(&root.generation_path(active.generation_id)).expect("index");
    store
        .upsert_artist(&stale)
        .expect("historical index document");
    store.commit().expect("historical index commit");
    assert_eq!(
        store
            .search(&parse_and_plan("status:provisional").expect("query"), 10, 0)
            .expect("old status")
            .len(),
        1
    );
    assert!(
        store
            .search(&parse_and_plan("status:active").expect("query"), 10, 0)
            .expect("new status before upgrade")
            .is_empty()
    );
    drop(store);
    migrate(&db.pool).await.expect("upgrade");
    let pass = apply_outbox_once(&config, &db.pool)
        .await
        .expect("apply status refresh");
    assert_eq!(pass.applied, 1);
    let store = SearchStore::open_readonly(&root.generation_path(active.generation_id))
        .expect("same index");
    assert_eq!(
        store
            .search(&parse_and_plan("status:active").expect("query"), 10, 0)
            .expect("updated status")
            .len(),
        1
    );
    assert!(
        store
            .search(&parse_and_plan("status:provisional").expect("query"), 10, 0)
            .expect("obsolete status")
            .is_empty()
    );
    drop(store);
    db.drop().await;
}

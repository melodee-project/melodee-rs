//! Production catalog scheduler integration acceptance.

#![allow(clippy::expect_used)]

#[path = "../../melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use catalog_common::{
    CatalogFixturePresentation, seed_legacy_repeated_track_artist_identity, seed_published_release,
    seed_published_release_with_musicbrainz_ids, seed_published_release_with_release_artists,
};
use common::{TestDb, test_config};
use melodee_catalog::{CatalogConfig, import_once, start_catalog_loop};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkEngineConfig, WorkerId};
use uuid::Uuid;

#[tokio::test]
async fn scheduler_drains_full_catalog_batches_before_the_next_idle_poll() {
    let db = TestDb::create("catalog_drain").await;
    for (ordinal, release_title) in [(1, "First"), (2, "Second"), (3, "Third")] {
        seed_published_release(
            &db,
            ordinal,
            CatalogFixturePresentation {
                release_title,
                release_artist: "Drain fixture",
                track_title: "Fixture track",
                track_artist: "Drain fixture",
                release_date: "2026-01-01",
            },
        )
        .await;
    }
    let handle = start_catalog_loop(
        CatalogConfig {
            concurrency: 1,
            poll_interval: time::Duration::hours(1),
            ..CatalogConfig::default()
        },
        db.pool.clone(),
        CatalogStore,
        WorkEngineConfig::default(),
    );
    let drained = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let imported: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM release_work_items WHERE state='cataloged'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("count real imports");
            if imported == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    handle.stop().await;
    db.drop().await;
    drained.expect("one-slot catalog worker drained all three claims within its first tick");
}

#[tokio::test]
async fn successful_import_reuses_one_artist_per_trusted_artist_id() {
    // ADR-0189: the same trusted provider Artist id is the same Artist, so the
    // second Release reuses the first Artist row instead of requiring a merge.
    let db = TestDb::create("catalog_scheduler_strong_artist_id").await;
    let shared_artist_id = "aa1b1c1d-0000-7000-8000-000000000071";
    for (ordinal, release_title) in [(108, "Trusted One"), (109, "Trusted Two")] {
        seed_published_release_with_musicbrainz_ids(
            &db,
            ordinal,
            CatalogFixturePresentation {
                release_title,
                release_artist: "Trusted Artist",
                track_title: "Fixture Track",
                track_artist: "Trusted Artist",
                release_date: "2026-01-01",
            },
            serde_json::json!({
                "artist": shared_artist_id,
                "release_artist": shared_artist_id
            }),
        )
        .await;
    }

    let stats = import_once(
        &CatalogConfig {
            concurrency: 2,
            batch_size: 2,
            ..CatalogConfig::default()
        },
        &CatalogStore,
        &db.pool,
        &WorkEngineConfig::default(),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("run strong-ID catalog import pass");
    assert_eq!(stats.completed, 2);
    assert_eq!(stats.failed, 0);

    let statuses: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(DISTINCT artist_id) FROM release_artists credit
                   JOIN artists artist ON artist.id=credit.artist_id
                  WHERE artist.normalized_name='trusted artist')
           FROM artists WHERE normalized_name='trusted artist'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read strong-ID import result");
    assert_eq!(
        statuses,
        (1, 0, 1),
        "one trusted provider id yields one Artist and no merge history"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artist_merge_events WHERE actor='catalog-reconciler'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count reconciler merges"),
        0
    );
    db.drop().await;
}

#[tokio::test]
async fn scheduler_repairs_legacy_duplicates_sharing_a_trusted_artist_id() {
    // A catalogue written before ADR-0189 can still hold two rows for one
    // trusted provider Artist id; the startup sweep must converge them.
    let db = TestDb::create("catalog_scheduler_legacy_strong_artist_id").await;
    let shared_artist_id = "aa1b1c1d-0000-7000-8000-000000000072";
    for (ordinal, release_title) in [(108, "Trusted One"), (109, "Trusted Two")] {
        seed_published_release_with_musicbrainz_ids(
            &db,
            ordinal,
            CatalogFixturePresentation {
                release_title,
                release_artist: "Trusted Artist",
                track_title: "Fixture Track",
                track_artist: "Trusted Artist",
                release_date: "2026-01-01",
            },
            serde_json::json!({
                "artist": shared_artist_id,
                "release_artist": shared_artist_id
            }),
        )
        .await;
    }
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let claimed = catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim legacy strong-ID fixtures");
    assert_eq!(claimed.len(), 2);
    let mut claimed = claimed.into_iter();
    let first_item = claimed.next().expect("first claimable work item").id;
    let second_item = claimed.next().expect("second claimable work item").id;
    catalog
        .import_manifest_v3(
            &db.pool,
            first_item,
            &worker,
            &CatalogImportRequest::new("older-binary-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import first legacy strong-ID fixture");
    sqlx::query(
        "UPDATE artists SET disambiguation='legacy duplicate'
          WHERE normalized_name='trusted artist'",
    )
    .execute(&db.pool)
    .await
    .expect("force a legacy second row");
    catalog
        .import_manifest_v3(
            &db.pool,
            second_item,
            &worker,
            &CatalogImportRequest::new("older-binary-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import second legacy strong-ID fixture");
    sqlx::query("UPDATE artists SET disambiguation=NULL WHERE normalized_name='trusted artist'")
        .execute(&db.pool)
        .await
        .expect("clear the temporary hint");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='trusted artist' AND status='active'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count legacy same-name rows"),
        2
    );

    let handle = start_catalog_loop(
        CatalogConfig {
            poll_interval: time::Duration::hours(1),
            ..CatalogConfig::default()
        },
        db.pool.clone(),
        catalog,
        WorkEngineConfig::default(),
    );
    let _ = handle.stop().await;

    let statuses: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_merge_events
                  WHERE actor='catalog-reconciler'
                    AND moved_relationships #>> '{reconciliation,evidence,predicate}'
                        = 'embedded_external_id')
           FROM artists WHERE normalized_name='trusted artist'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read startup strong-ID reconciliation result");
    assert_eq!(statuses, (1, 1, 1));
    db.drop().await;
}

#[tokio::test]
async fn successful_import_reuses_one_artist_for_repeated_release_credits() {
    // ADR-0189: C# reuses an Artist by normalized name at import, so two Releases
    // by "4Trey" produce one Artist with credits from both and no merge history.
    let db = TestDb::create("catalog_scheduler_cross_release_artist").await;
    for (ordinal, release_title) in [(101, "The Aim"), (102, "Play Your Roll")] {
        seed_published_release(
            &db,
            ordinal,
            CatalogFixturePresentation {
                release_title,
                release_artist: "4Trey",
                track_title: "Fixture Track",
                track_artist: "4Trey",
                release_date: "2026-01-01",
            },
        )
        .await;
    }

    let stats = import_once(
        &CatalogConfig {
            concurrency: 2,
            batch_size: 2,
            ..CatalogConfig::default()
        },
        &CatalogStore,
        &db.pool,
        &WorkEngineConfig::default(),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("run production catalog import pass");
    assert_eq!(stats.completed, 2);
    assert_eq!(stats.failed, 0);

    let statuses: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_merge_events
                  WHERE actor='catalog-reconciler'),
                (SELECT count(*) FROM artist_match_decisions
                  WHERE outcome='proposed' AND authority='automated'),
                (SELECT count(*) FROM release_artists credit
                   JOIN artists artist ON artist.id=credit.artist_id
                  WHERE artist.normalized_name='4trey')
           FROM artists WHERE normalized_name='4trey'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read scheduler reuse result");
    assert_eq!(
        statuses,
        (1, 0, 0, 0, 2),
        "one Artist carries both Release credits without merge or proposal history"
    );
    assert_eq!(
        CatalogStore
            .count_unresolved_observations(&db.pool)
            .await
            .expect("count unresolved observations"),
        0
    );

    db.drop().await;
}

#[tokio::test]
async fn scheduler_startup_repairs_older_repeated_primary_artist_identities() {
    let db = TestDb::create("catalog_startup_cross_release_artist").await;
    let release_titles = [
        "Along Came a Spider",
        "Alice Cooper Goes to Hell",
        "Brutal Planet",
        "Constrictor",
        "Detroit Stories",
        "Dirty Diamonds",
        "Dragontown",
        "Easy Action",
        "Flush the Fashion",
        "From the Inside",
        "Pretties for You",
        "Lace and Whiskey",
        "Raise Your Fist and Yell",
        "Special Forces",
    ];
    let mut fixtures = Vec::with_capacity(release_titles.len());
    for (offset, release_title) in release_titles.iter().enumerate() {
        let ordinal = 103_u32 + u32::try_from(offset).expect("fixture offset fits u32");
        fixtures.push(
            seed_published_release(
                &db,
                ordinal,
                CatalogFixturePresentation {
                    release_title,
                    release_artist: "Alice Cooper",
                    track_title: release_title,
                    track_artist: "Alice Cooper",
                    release_date: "2000-01-01",
                },
            )
            .await,
        );
    }
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let work_config = WorkEngineConfig {
        claim_batch_size: 14,
        ..WorkEngineConfig::default()
    };
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &work_config, now)
            .await
            .expect("claim old-binary fixtures")
            .len(),
        14
    );
    // Reproduce a pre-ADR-0189 catalogue: a disambiguation hint present while each
    // Release imports suppresses name reuse, leaving one row per Release.
    for fixture in &fixtures {
        catalog
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("older-binary-v1", "catalog-test"),
                now,
            )
            .await
            .expect("import without scheduler reconciliation");
        sqlx::query(
            "UPDATE artists SET disambiguation='legacy duplicate'
              WHERE normalized_name='alice cooper'",
        )
        .execute(&db.pool)
        .await
        .expect("keep legacy duplicates apart");
    }
    sqlx::query("UPDATE artists SET disambiguation=NULL WHERE normalized_name='alice cooper'")
        .execute(&db.pool)
        .await
        .expect("clear the temporary hints");
    sqlx::query("UPDATE releases SET status='deprecated' WHERE id=ANY($1)")
        .bind(
            fixtures
                .iter()
                .map(|fixture| fixture.release_id.into_uuid())
                .collect::<Vec<_>>(),
        )
        .execute(&db.pool)
        .await
        .expect("model retained deprecated Releases from the live catalog");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='alice cooper' AND status='active'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count pre-startup duplicates"),
        14
    );

    let handle = start_catalog_loop(
        CatalogConfig {
            poll_interval: time::Duration::hours(1),
            ..CatalogConfig::default()
        },
        db.pool.clone(),
        catalog,
        work_config,
    );
    let _ = handle.stop().await;

    let statuses: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_merge_events
                  WHERE moved_relationships #>> '{reconciliation,evidence,predicate}'
                        = 'repeated_credit')
           FROM artists WHERE normalized_name='alice cooper'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read startup reconciliation result");
    assert_eq!(statuses, (1, 13, 13));
    db.drop().await;
}

#[tokio::test]
async fn scheduler_startup_reconciles_repeated_track_credits_left_by_an_older_binary() {
    let db = TestDb::create("catalog_startup_same_release_track_artist").await;
    let fixture = seed_published_release_with_release_artists(
        &db,
        107,
        CatalogFixturePresentation {
            release_title: "Woounded, Vol. 2",
            release_artist: "9000 Rondae",
            track_title: "Face Hot",
            track_artist: "9000 Rondae/Glockboyz Teejaee",
            release_date: "2023-01-01",
        },
        &["9000 Rondae"],
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim older-binary Track Artist fixture");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("older-binary-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import without scheduler reconciliation");
    seed_legacy_repeated_track_artist_identity(&db, fixture.release_id, "glockboyz teejaee").await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='glockboyz teejaee' AND status='active'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count pre-startup repeated Track Artist identities"),
        2
    );

    let handle = start_catalog_loop(
        CatalogConfig {
            poll_interval: time::Duration::hours(1),
            ..CatalogConfig::default()
        },
        db.pool.clone(),
        catalog,
        WorkEngineConfig::default(),
    );
    let _ = handle.stop().await;

    let statuses: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_merge_events
                  WHERE actor='catalog-reconciler'
                    AND moved_relationships #>> '{reconciliation,evidence,predicate}'
                        = 'same_release_track_credit')
           FROM artists WHERE normalized_name='glockboyz teejaee'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read startup same-Release reconciliation result");
    assert_eq!(statuses, (1, 1, 1));
    db.drop().await;
}

async fn seed_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed")
        .fetch_one(pool)
        .await
        .expect("count identity seeds")
}

/// Failed discovery must not acknowledge durable work. Once the underlying
/// problem is repaired, the same seed is processed on the next drain.
#[tokio::test]
async fn identity_drain_preserves_seeds_when_candidate_discovery_fails() {
    let db = TestDb::create("catalog_identity_discovery_failure").await;
    sqlx::query(
        "INSERT INTO catalog_identity_reconciliation_seed (normalized_name, created_at)
         VALUES ('retry after discovery failure', now())",
    )
    .execute(&db.pool)
    .await
    .expect("insert seed");
    sqlx::query(
        "ALTER TABLE artist_external_identities RENAME TO unavailable_artist_external_identities",
    )
    .execute(&db.pool)
    .await
    .expect("inject candidate discovery failure");

    assert!(melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await);
    assert_eq!(seed_count(&db.pool).await, 1);

    sqlx::query(
        "ALTER TABLE unavailable_artist_external_identities RENAME TO artist_external_identities",
    )
    .execute(&db.pool)
    .await
    .expect("restore candidate discovery");
    assert!(melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await);
    assert_eq!(seed_count(&db.pool).await, 0);
    db.drop().await;
}

/// PERF-004 / RM-09: an ordinary import persists durable affected-identity
/// seeds in its authoritative transaction; the coalesced drain consumes them
/// exactly once and is idempotent.
#[tokio::test]
async fn import_persists_identity_seeds_and_the_coalesced_drain_clears_them() {
    let db = TestDb::create("catalog_identity_seeds").await;
    seed_published_release(
        &db,
        181,
        CatalogFixturePresentation {
            release_title: "Seed Release",
            release_artist: "Seed Artist",
            track_title: "Seed Track",
            track_artist: "Seed Artist",
            release_date: "2026-01-01",
        },
    )
    .await;
    import_once(
        &CatalogConfig {
            concurrency: 1,
            ..CatalogConfig::default()
        },
        &CatalogStore,
        &db.pool,
        &WorkEngineConfig::default(),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("import seed fixture");

    assert!(
        seed_count(&db.pool).await > 0,
        "the import transaction persists affected-identity seeds"
    );
    let watermark = CatalogStore
        .identity_reconciliation_seed_watermark(&db.pool)
        .await
        .expect("read seed watermark");
    assert!(watermark.is_some());

    assert!(
        melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await,
        "the drain runs when the durable queue is non-empty"
    );
    assert_eq!(
        seed_count(&db.pool).await,
        0,
        "the drain clears the captured watermark"
    );
    assert!(
        !melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await,
        "an empty queue does no work"
    );
    db.drop().await;
}

/// PERF-004 / RM-09: durable seeds converge a legacy duplicate that the import
/// path deliberately left apart, proving equivalence with the global algorithm
/// without a per-album scan.
#[tokio::test]
async fn coalesced_drain_converges_legacy_duplicate_artist_identities() {
    let db = TestDb::create("catalog_identity_drain").await;
    let mut fixtures = Vec::new();
    for (offset, title) in ["Legacy One", "Legacy Two", "Legacy Three"]
        .into_iter()
        .enumerate()
    {
        fixtures.push(
            seed_published_release(
                &db,
                u32::try_from(190 + offset).expect("fixture ordinal"),
                CatalogFixturePresentation {
                    release_title: title,
                    release_artist: "Drain Duplicate",
                    track_title: title,
                    track_artist: "Drain Duplicate",
                    release_date: "2001-01-01",
                },
            )
            .await,
        );
    }
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let work_config = WorkEngineConfig {
        claim_batch_size: 3,
        ..WorkEngineConfig::default()
    };
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &work_config, now)
            .await
            .expect("claim legacy fixtures")
            .len(),
        3
    );
    for fixture in &fixtures {
        catalog
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("legacy-seeds-v1", "catalog-test"),
                now,
            )
            .await
            .expect("import legacy fixture");
        sqlx::query(
            "UPDATE artists SET disambiguation='legacy duplicate'
              WHERE normalized_name='drain duplicate'",
        )
        .execute(&db.pool)
        .await
        .expect("keep legacy duplicates apart");
    }
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artists
          WHERE normalized_name='drain duplicate' AND status='active'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count pre-drain duplicates");
    assert_eq!(before, 3);
    sqlx::query("UPDATE artists SET disambiguation=NULL WHERE normalized_name='drain duplicate'")
        .execute(&db.pool)
        .await
        .expect("remove legacy blocker");

    assert!(
        melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await,
        "the durable seeds trigger a closure drain"
    );
    let (active, merged): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged')
           FROM artists WHERE normalized_name='drain duplicate'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count post-drain identities");
    assert_eq!(
        (active, merged),
        (1, 2),
        "the closure converges the legacy duplicates the import left apart"
    );
    assert!(
        seed_count(&db.pool).await > 0,
        "merges enqueue their affected names for a later drain"
    );
    assert!(melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await);
    assert_eq!(seed_count(&db.pool).await, 0);
    db.drop().await;
}

/// PERF-004 / RM-09: seed ids are allocated before commit, so a lower id can
/// become visible after a drain captured its batch. Deleting an id range would
/// discard that seed without ever reconciling it. The drain must delete only
/// the ids it examined.
#[tokio::test]
async fn late_committing_lower_id_seed_is_not_deleted_by_a_coalesced_drain() {
    let db = TestDb::create("catalog_identity_late_seed").await;
    let mut late = db.pool.begin().await.expect("begin late seed transaction");
    let late_id: i64 = sqlx::query_scalar(
        "INSERT INTO catalog_identity_reconciliation_seed (normalized_name, created_at)
         VALUES ('late committing seed', now()) RETURNING id",
    )
    .fetch_one(&mut *late)
    .await
    .expect("allocate lower seed id in the uncommitted transaction");
    let committed_id: i64 = sqlx::query_scalar(
        "INSERT INTO catalog_identity_reconciliation_seed (normalized_name, created_at)
         VALUES ('committed seed', now()) RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .expect("commit a higher seed id");
    assert!(
        committed_id > late_id,
        "the uncommitted seed holds the lower allocated id"
    );

    assert!(
        melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await,
        "the drain examines the visible seed"
    );

    late.commit()
        .await
        .expect("commit the late lower-id seed after the capture");
    let remaining: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM catalog_identity_reconciliation_seed ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .expect("read remaining seeds");
    assert!(
        remaining.contains(&late_id),
        "the late lower-id seed must remain durable for a later drain; got {remaining:?}"
    );

    assert!(
        melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await,
        "the next drain examines the late seed"
    );
    assert_eq!(seed_count(&db.pool).await, 0);
    db.drop().await;
}

/// PERF-004 / RM-09: two imports committing concurrently each persist their
/// own affected-identity seeds in their authoritative transaction; coalescing
/// must not lose either Release's obligation, and one drain clears both.
#[tokio::test]
async fn concurrent_imports_persist_independent_identity_seeds() {
    let db = TestDb::create("catalog_identity_concurrent_imports").await;
    for (ordinal, release_title, artist) in [
        (211_u32, "Concurrent Seed One", "Concurrent Seed Artist One"),
        (212_u32, "Concurrent Seed Two", "Concurrent Seed Artist Two"),
    ] {
        seed_published_release(
            &db,
            ordinal,
            CatalogFixturePresentation {
                release_title,
                release_artist: artist,
                track_title: "Concurrent Seed Track",
                track_artist: artist,
                release_date: "2026-02-02",
            },
        )
        .await;
    }
    let config = CatalogConfig {
        concurrency: 2,
        ..CatalogConfig::default()
    };
    let work = WorkEngineConfig::default();
    let now = time::OffsetDateTime::now_utc();
    let (first, second) = tokio::join!(
        import_once(&config, &CatalogStore, &db.pool, &work, now),
        import_once(&config, &CatalogStore, &db.pool, &work, now),
    );
    first.expect("first concurrent import");
    second.expect("second concurrent import");

    let cataloged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM releases
          WHERE title IN ('Concurrent Seed One', 'Concurrent Seed Two')",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count concurrent imports");
    assert_eq!(cataloged, 2, "both concurrent imports must commit");

    let seeded_releases: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT release_id FROM catalog_identity_reconciliation_seed
          WHERE release_id IS NOT NULL",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read per-Release durable seeds");
    assert_eq!(
        seeded_releases.len(),
        2,
        "each concurrent import keeps its own durable Release obligation"
    );

    assert!(
        melodee_catalog::reconcile_affected_identities_once(&CatalogStore, &db.pool).await,
        "one coalesced drain consumes both concurrent obligations"
    );
    assert_eq!(seed_count(&db.pool).await, 0);
    db.drop().await;
}

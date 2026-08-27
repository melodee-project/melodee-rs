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

#[tokio::test]
async fn successful_import_auto_merges_only_shared_trusted_artist_ids() {
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

    let statuses: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='provisional'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_merge_events
                  WHERE actor='catalog-reconciler'
                    AND moved_relationships #>> '{reconciliation,evidence,predicate}'
                        = 'embedded_external_id')
           FROM artists WHERE normalized_name='trusted artist'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read strong-ID reconciliation result");
    assert_eq!(statuses, (1, 1, 1));
    db.drop().await;
}

#[tokio::test]
async fn successful_import_proposes_cross_release_exact_names_without_merging() {
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

    let statuses: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='provisional'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_merge_events
                  WHERE actor='catalog-reconciler'),
                (SELECT count(*) FROM artist_match_decisions
                  WHERE outcome='proposed' AND authority='automated')
           FROM artists WHERE normalized_name='4trey'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read scheduler reconciliation result");
    assert_eq!(statuses, (2, 0, 0, 4));
    assert_eq!(
        CatalogStore
            .count_unresolved_observations(&db.pool)
            .await
            .expect("count review proposals"),
        4
    );

    db.drop().await;
}

#[tokio::test]
async fn scheduler_startup_proposes_older_same_name_identities_without_merging() {
    let db = TestDb::create("catalog_startup_cross_release_artist").await;
    let fixtures = [
        seed_published_release(
            &db,
            103,
            CatalogFixturePresentation {
                release_title: "Alice Cooper Goes to Hell",
                release_artist: "Alice Cooper",
                track_title: "Go to Hell",
                track_artist: "Alice Cooper",
                release_date: "1976-06-25",
            },
        )
        .await,
        seed_published_release(
            &db,
            104,
            CatalogFixturePresentation {
                release_title: "Brutal Planet",
                release_artist: "Alice Cooper",
                track_title: "Brutal Planet",
                track_artist: "Alice Cooper",
                release_date: "2000-06-06",
            },
        )
        .await,
    ];
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim old-binary fixtures")
            .len(),
        2
    );
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
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='alice cooper' AND status='provisional'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count pre-startup duplicates"),
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
        "SELECT count(*) FILTER (WHERE status='provisional'),
                count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_match_decisions
                  WHERE outcome='proposed' AND authority='automated')
           FROM artists WHERE normalized_name='alice cooper'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read startup reconciliation result");
    assert_eq!(statuses, (2, 0, 4));
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
              WHERE normalized_name='glockboyz teejaee' AND status='provisional'",
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
        "SELECT count(*) FILTER (WHERE status='provisional'),
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

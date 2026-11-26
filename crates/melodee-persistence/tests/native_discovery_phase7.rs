//! chart ingestion/linkage acceptance.

#![allow(clippy::expect_used)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, NativeChartImport, NativeDiscoveryStore, WorkerId,
};
use time::OffsetDateTime;

#[tokio::test]
async fn bounded_chart_import_is_atomic_and_links_only_unique_catalog_identity() {
    let db = TestDb::create("native_chart_import").await;
    let fixture = seed_published_release(
        &db,
        701,
        CatalogFixturePresentation {
            release_title: "CSV, Release",
            release_artist: "Chart Artist",
            track_title: "Chart Track",
            track_artist: "Track Performer",
            release_date: "2024-02-03",
        },
    )
    .await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let claimed = CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim catalog fixture");
    assert_eq!(claimed.len(), 1);
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase7.chart-import.v1", "chart-test"),
            now,
        )
        .await
        .expect("import catalog fixture");

    let request = NativeChartImport {
        slug: "fixture-100".to_owned(),
        title: "Fixture 100".to_owned(),
        source_name: Some("Acceptance fixture".to_owned()),
        source_url: Some("https://charts.example.test/fixture".to_owned()),
        chart_year: Some(2024),
        is_visible: true,
        is_generated_playlist_enabled: true,
    };
    let result = NativeDiscoveryStore
        .import_chart_csv(
            &db.pool,
            &request,
            b"Rank,ArtistName,ReleaseTitle,ReleaseYear\n1,Chart Artist,\"CSV, Release\",2024\n2,Unknown,Missing,2024\n",
        )
        .await
        .expect("import bounded chart");
    assert_eq!(result.item_count, 2);
    assert_eq!(result.linked_count, 1);
    let rows = sqlx::query_as::<_, (i32, Option<uuid::Uuid>)>(
        "SELECT rank,linked_release_id FROM native_chart_items WHERE chart_id=$1 ORDER BY rank",
    )
    .bind(result.chart_id)
    .fetch_all(&db.pool)
    .await
    .expect("chart rows");
    assert_eq!(rows[0].1, Some(fixture.release_id.into_uuid()));
    assert_eq!(rows[1].1, None);

    let before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM native_charts")
        .fetch_one(&db.pool)
        .await
        .expect("count charts");
    let invalid = NativeChartImport {
        slug: "invalid-duplicate-rank".to_owned(),
        ..request
    };
    assert!(
        NativeDiscoveryStore
            .import_chart_csv(&db.pool, &invalid, b"1,One,One,2024\n1,Two,Two,2024\n",)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM native_charts")
            .fetch_one(&db.pool)
            .await
            .expect("count charts after rejected input"),
        before,
        "validation must occur before the chart transaction writes"
    );
    db.drop().await;
}

#[tokio::test]
async fn discovery_maintenance_relinks_chart_items_only_on_unique_catalog_identity() {
    let db = TestDb::create("native_discovery_maintenance_relink").await;
    // Import a chart whose items have no catalog counterpart yet; both rows
    // stay unlinked at import time.
    let request = NativeChartImport {
        slug: "maintenance-relink".to_owned(),
        title: "Maintenance Relink".to_owned(),
        source_name: None,
        source_url: None,
        chart_year: Some(2026),
        is_visible: true,
        is_generated_playlist_enabled: false,
    };
    let imported = NativeDiscoveryStore
        .import_chart_csv(
            &db.pool,
            &request,
            b"Rank,ArtistName,ReleaseTitle,ReleaseYear\n1,Late Artist,Late Release,2026\n2,Duplicate Artist,Duplicate Release,2026\n",
        )
        .await
        .expect("import unlinked chart");
    assert_eq!(imported.linked_count, 0);

    // The catalog later gains a unique match for rank 1 and two ambiguous
    // matches for rank 2.
    let matched = seed_published_release(
        &db,
        702,
        CatalogFixturePresentation {
            release_title: "Late Release",
            release_artist: "Late Artist",
            track_title: "Late Track",
            track_artist: "Late Artist",
            release_date: "2026-01-02",
        },
    )
    .await;
    let worker = WorkerId::new();
    let claimed = CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), OffsetDateTime::now_utc())
        .await
        .expect("claim matched fixture");
    assert_eq!(claimed.len(), 1);
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            matched.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase7.chart-relink.v1", "chart-relink"),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("catalog matched fixture");
    for ordinal in [703, 704] {
        let duplicate = seed_published_release(
            &db,
            ordinal,
            CatalogFixturePresentation {
                release_title: "Duplicate Release",
                release_artist: "Duplicate Artist",
                track_title: "Duplicate Track",
                track_artist: "Duplicate Artist",
                release_date: "2026-03-04",
            },
        )
        .await;
        let claimed = CatalogStore
            .claim_cataloging(&db.pool, &worker, &test_config(), OffsetDateTime::now_utc())
            .await
            .expect("claim duplicate fixture");
        assert_eq!(claimed.len(), 1);
        CatalogStore
            .import_manifest_v3(
                &db.pool,
                duplicate.work_item_id,
                &worker,
                &CatalogImportRequest::new("phase7.chart-relink.v1", "chart-relink"),
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("catalog duplicate fixture");
    }

    let stats = NativeDiscoveryStore
        .refresh_discovery_aggregates(&db.pool, 500, 100)
        .await
        .expect("maintenance pass");
    assert_eq!(stats.chart_items_relinked, 1);
    assert!(!stats.truncated);
    let rows = sqlx::query_as::<_, (i32, Option<uuid::Uuid>)>(
        "SELECT rank,linked_release_id FROM native_chart_items WHERE chart_id=$1 ORDER BY rank",
    )
    .bind(imported.chart_id)
    .fetch_all(&db.pool)
    .await
    .expect("relinked chart rows");
    assert_eq!(
        rows[0].1,
        Some(matched.release_id.into_uuid()),
        "unique late-arriving catalog identity links"
    );
    assert_eq!(
        rows[1].1, None,
        "ambiguous observations stay unlinked; names are never identity"
    );

    // A second pass converges to a no-op: maintenance is idempotent.
    let second = NativeDiscoveryStore
        .refresh_discovery_aggregates(&db.pool, 500, 100)
        .await
        .expect("idempotent second pass");
    assert_eq!(second.chart_items_relinked, 0);
    assert_eq!(second.candidates_inserted, 0);
    assert_eq!(second.candidates_corrected, 0);
    db.drop().await;
}

/// PERF-012 / RM-06 permanent regression.
///
/// Reproduces the starvation shape: far more Track candidates than one pass can
/// visit, plus Release, Artist, and chart work. The old implementation reset
/// every popularity cursor to `Uuid::nil()` and shared one batch budget, so the
/// Track prefix consumed the whole pass and the other classes and charts were
/// never reached. This test proves (a) bounded per-invocation work, (b) no
/// class starvation, (c) eventual coverage of every stable candidate across
/// sweeps, and (d) wraparound reaching a row inserted behind the cursor.
#[tokio::test]
async fn discovery_progress_is_durable_fair_and_eventually_complete() {
    let db = TestDb::create("native_discovery_fairness").await;
    let fixture = seed_published_release(
        &db,
        771,
        CatalogFixturePresentation {
            release_title: "Fairness Chart Release",
            release_artist: "Fairness Chart Artist",
            track_title: "Fairness Chart Track",
            track_artist: "Fairness Chart Artist",
            release_date: "2023-05-06",
        },
    )
    .await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        CatalogStore
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim fairness fixture")
            .len(),
        1
    );
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase7.discovery-fairness.v1", "fairness"),
            now,
        )
        .await
        .expect("import fairness fixture");

    // 100,000 Track candidates with a deliberately wrong popularity so every
    // visited row is observable, plus independent Release and Artist classes.
    sqlx::query(
        "INSERT INTO native_recommendation_candidates (entity_type, entity_id, popularity, updated_at)
         SELECT 'track', gen_random_uuid(), 1, now() FROM generate_series(1, 100000)",
    )
    .execute(&db.pool)
    .await
    .expect("seed 100K Track candidates");
    sqlx::query(
        "INSERT INTO native_recommendation_candidates (entity_type, entity_id, popularity, updated_at)
         SELECT 'release', gen_random_uuid(), 1, now() FROM generate_series(1, 25)
         UNION ALL
         SELECT 'artist', gen_random_uuid(), 1, now() FROM generate_series(1, 25)",
    )
    .execute(&db.pool)
    .await
    .expect("seed Release/Artist candidates");

    // An unlinked chart item that this catalog can resolve, so chart progress is
    // observable even while 100K Track candidates remain.
    let chart = NativeDiscoveryStore
        .import_chart_csv(
            &db.pool,
            &NativeChartImport {
                slug: "fairness-chart".to_owned(),
                title: "Fairness Chart".to_owned(),
                source_name: None,
                source_url: None,
                chart_year: Some(2023),
                is_visible: true,
                is_generated_playlist_enabled: true,
            },
            b"Rank,ArtistName,ReleaseTitle,ReleaseYear\n1,Fairness Chart Artist,Fairness Chart Release,2023\n",
        )
        .await
        .expect("import fairness chart");
    assert_eq!(chart.linked_count, 1, "the fixture release links at import");

    // Release and Artist classes are corrected in the very first pass even
    // though tens of thousands of Track candidates are outstanding.
    let first = NativeDiscoveryStore
        .refresh_discovery_aggregates(&db.pool, 500, 100)
        .await
        .expect("first bounded discovery pass");
    assert!(
        first.batches <= 100,
        "one pass must stay within its budget, got {}",
        first.batches
    );
    let corrected_release_artist: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM native_recommendation_candidates
          WHERE entity_type IN ('release','artist') AND popularity <> 0",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count uncorrected Release/Artist candidates");
    assert_eq!(
        corrected_release_artist, 0,
        "Release and Artist classes must not be starved by the Track backlog"
    );

    // Drive the Track class to completion. 100,000 / (20 batches * 500) = 10
    // sweeps; allow a generous bound and then assert every candidate was
    // visited (popularity corrected to the authoritative zero).
    let mut passes = 1;
    loop {
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM native_recommendation_candidates
              WHERE entity_type = 'track' AND popularity <> 0",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count uncorrected Track candidates");
        if remaining == 0 {
            break;
        }
        assert!(
            passes < 40,
            "discovery never reached the end of its Track sweep"
        );
        NativeDiscoveryStore
            .refresh_discovery_aggregates(&db.pool, 500, 100)
            .await
            .expect("subsequent bounded discovery pass");
        passes += 1;
    }
    assert!(
        passes >= 10,
        "the budget must bound per-pass work (covered in {passes} passes)"
    );

    // The cursor has wrapped; a row inserted behind the previous cursor is
    // reached on the next sweep rather than being skipped forever.
    let behind = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001")
        .expect("valid behind-cursor UUID");
    sqlx::query(
        "INSERT INTO native_recommendation_candidates (entity_type, entity_id, popularity, updated_at)
         VALUES ('track', $1, 1, now())",
    )
    .bind(behind)
    .execute(&db.pool)
    .await
    .expect("insert candidate behind the cursor");
    let after_wrap = NativeDiscoveryStore
        .refresh_discovery_aggregates(&db.pool, 500, 100)
        .await
        .expect("pass after wraparound");
    assert!(
        after_wrap.passes_completed >= 1,
        "the Track class reports a completed sweep"
    );
    let behind_popularity: i64 = sqlx::query_scalar(
        "SELECT popularity FROM native_recommendation_candidates
          WHERE entity_type = 'track' AND entity_id = $1",
    )
    .bind(behind)
    .fetch_one(&db.pool)
    .await
    .expect("read behind-cursor candidate");
    assert_eq!(
        behind_popularity, 0,
        "rows inserted behind the cursor are visited after wraparound"
    );

    db.drop().await;
}

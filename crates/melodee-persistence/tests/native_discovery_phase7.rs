//! Phase 7 chart ingestion/linkage acceptance.

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

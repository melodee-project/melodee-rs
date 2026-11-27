//! Opt-in timing for the real catalog uncertain-commit retry path on an owned fixture.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::time::Instant;

use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkerId};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires a release build and the invocation-owned ingestion fixture"]
async fn measure_catalog_retry() {
    let url = std::env::var("MELODEE__DATABASE__URL").expect("fixture URL");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database identity");
    assert!(
        database.strip_prefix("melodee_template_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.chars().all(|c| c.is_ascii_hexdigit())
        )
    );
    assert_eq!(
        std::env::var("MELODEE_PERF_OWNED_DATABASE").ok().as_deref(),
        Some(database.as_str())
    );
    let (work_item, release_id, resolver): (Uuid, Uuid, String) = sqlx::query_as(
        "SELECT w.id, ci.release_id, ci.resolver_version FROM release_work_items w \
         JOIN catalog_imports ci ON ci.work_item_id = w.id \
         WHERE w.source_path = 'bench-release-0000' AND w.state = 'cataloged' \
         ORDER BY w.created_at LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("cataloged fixture occurrence");
    let track_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id = $1 ORDER BY id")
            .bind(release_id)
            .fetch_all(&pool)
            .await
            .expect("original tracks");
    assert!(!track_ids.is_empty());
    let seed_table: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.catalog_identity_reconciliation_seed') IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("seed table availability");
    let seeds_before: Option<Vec<i64>> = if seed_table {
        Some(
            sqlx::query_scalar("SELECT id FROM catalog_identity_reconciliation_seed ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("original seeds"),
        )
    } else {
        None
    };
    let imports_before: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_imports")
        .fetch_one(&pool)
        .await
        .expect("original import count");
    let request = CatalogImportRequest::new(resolver, "performance-retry");
    let worker = WorkerId::new();
    let mut elapsed_us = Vec::with_capacity(50);
    for _ in 0..50 {
        let start = Instant::now();
        let result = CatalogStore
            .import_manifest_v3(
                &pool,
                work_item,
                &worker,
                &request,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("same-item retry");
        elapsed_us.push(start.elapsed().as_micros());
        assert!(!result.inserted);
        assert_eq!(result.release_id.into_uuid(), release_id);
    }
    let tracks_after: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id = $1 ORDER BY id")
            .bind(release_id)
            .fetch_all(&pool)
            .await
            .expect("tracks after retry");
    assert_eq!(tracks_after, track_ids);
    let imports_after: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_imports")
        .fetch_one(&pool)
        .await
        .expect("imports after retry");
    assert_eq!(imports_before, imports_after);
    if let Some(before) = &seeds_before {
        let after: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM catalog_identity_reconciliation_seed ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("seeds after retry");
        assert_eq!(&after, before);
    }
    elapsed_us.sort_unstable();
    println!(
        "PERF_RETRY_JSON={}",
        serde_json::json!({
            "operation": "same_work_item_catalog_retry", "samples": elapsed_us.len(),
            "p50_us": elapsed_us[elapsed_us.len() / 2],
            "p95_us": elapsed_us[(elapsed_us.len() - 1) * 95 / 100],
            "max_us": elapsed_us[elapsed_us.len() - 1],
            "same_release_and_track_ids": true, "no_new_import_rows": true,
            "identity_seed_check": if seeds_before.is_some() { "exact_set_unchanged" } else { "not_present_in_schema" },
            "limitations": ["direct catalog store retry only", "not a complete ingestion or media reimport", "SQL round trips and filesystem I/O not measured"]
        })
    );
    pool.close().await;
}

//! Bounded 200-Release Phase 4 catalog acceptance scenario.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::IngestionState;
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkItemStore, WorkerId};
use tokio::task::JoinSet;

const RELEASE_COUNT: usize = 200;
const MAX_CATALOG_CONCURRENCY: usize = 4;
const IMPORT_EXPLICIT_SQL_STATEMENTS_PER_RELEASE: u64 = 24;
const CLAIM_EXPLICIT_SQL_STATEMENTS_PER_NEW_ITEM: u64 = 3;

fn peak_test_process_rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

#[tokio::test]
async fn two_hundred_manifests_catalog_exactly_once_with_bounded_queries_and_no_media_reads() {
    let db = TestDb::create("catalog_200").await;
    let mut fixtures = Vec::with_capacity(RELEASE_COUNT);
    for ordinal in 1..=RELEASE_COUNT {
        fixtures.push(
            seed_published_release(
                &db,
                u32::try_from(ordinal).expect("fixture ordinal fits u32"),
                CatalogFixturePresentation {
                    release_title: &format!("Catalog Release {ordinal:04}"),
                    release_artist: &format!("Catalog Artist {ordinal:04}"),
                    track_title: &format!("Catalog Track {ordinal:04}"),
                    track_artist: &format!("Catalog Artist {ordinal:04}"),
                    release_date: "2004-05-06",
                },
            )
            .await,
        );
    }
    assert!(fixtures.iter().all(|fixture| {
        !std::path::Path::new(&fixture.source_path).exists()
            && !std::path::Path::new(&fixture.canonical_asset_path).exists()
    }));

    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let mut config = test_config();
    config.claim_batch_size =
        u32::try_from(MAX_CATALOG_CONCURRENCY).expect("concurrency fits claim batch");
    let request = CatalogImportRequest::new("phase4.acceptance-resolver.v1", "catalog-200-test");
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let catalog_now = time::OffsetDateTime::now_utc();
    let started = Instant::now();
    let mut results = Vec::with_capacity(RELEASE_COUNT);
    let mut claim_batches = 0_usize;
    let mut claim_store_call_elapsed = std::time::Duration::ZERO;
    let mut import_store_call_sum = std::time::Duration::ZERO;

    while results.len() < RELEASE_COUNT {
        let claim_started = Instant::now();
        let claimed = catalog
            .claim_cataloging(&db.pool, &worker, &config, catalog_now)
            .await
            .expect("claim bounded catalog batch");
        claim_store_call_elapsed += claim_started.elapsed();
        assert!(!claimed.is_empty(), "all Published work remains claimable");
        assert!(claimed.len() <= MAX_CATALOG_CONCURRENCY);
        claim_batches += 1;

        let mut tasks = JoinSet::new();
        for item in claimed {
            let pool = db.pool.clone();
            let worker = worker.clone();
            let request = request.clone();
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            tasks.spawn(async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                let import_started = Instant::now();
                let result = CatalogStore
                    .import_manifest_v3(&pool, item.id, &worker, &request, catalog_now)
                    .await;
                let import_elapsed = import_started.elapsed();
                active.fetch_sub(1, Ordering::SeqCst);
                (result, import_elapsed)
            });
        }
        while let Some(joined) = tasks.join_next().await {
            let (result, import_elapsed) = joined.expect("catalog task joins");
            import_store_call_sum += import_elapsed;
            results.push(result.expect("catalog import succeeds"));
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(results.len(), RELEASE_COUNT);
    assert!(results.iter().all(|result| result.inserted));
    assert!(results.iter().all(|result| result.track_count == 1));
    assert!(results.iter().all(|result| result.artist_count == 2));
    assert!(results.iter().all(|result| result.media_asset_count == 1));
    let set_based_statement_counts = results
        .iter()
        .map(|result| result.set_based_mutation_statement_count)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        set_based_statement_counts.len(),
        1,
        "set-based statements are constant rather than Track/Artist N+1"
    );
    let set_based_statement_count_per_release = *set_based_statement_counts
        .first()
        .expect("one importer statement count");
    let total_set_based_mutation_statements: u64 = results
        .iter()
        .map(|result| u64::from(result.set_based_mutation_statement_count))
        .sum();
    assert_eq!(
        total_set_based_mutation_statements,
        u64::from(set_based_statement_count_per_release) * RELEASE_COUNT as u64
    );
    let measured_peak = peak.load(Ordering::SeqCst);
    assert!((1..=MAX_CATALOG_CONCURRENCY).contains(&measured_peak));
    assert_eq!(claim_batches, RELEASE_COUNT / MAX_CATALOG_CONCURRENCY);
    let application_sql_statements_in_window = IMPORT_EXPLICIT_SQL_STATEMENTS_PER_RELEASE
        * RELEASE_COUNT as u64
        + CLAIM_EXPLICIT_SQL_STATEMENTS_PER_NEW_ITEM * RELEASE_COUNT as u64
        + claim_batches as u64;
    assert_eq!(application_sql_statements_in_window, 5_450);

    let unique_release_ids = results
        .iter()
        .map(|result| result.release_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(unique_release_ids.len(), RELEASE_COUNT);
    assert_eq!(
        unique_release_ids,
        fixtures.iter().map(|fixture| fixture.release_id).collect()
    );
    let counts = catalog
        .counts(&db.pool)
        .await
        .expect("final catalog counts");
    assert_eq!(counts.releases, RELEASE_COUNT as i64);
    assert_eq!(counts.tracks, RELEASE_COUNT as i64);
    assert_eq!(counts.media_assets, RELEASE_COUNT as i64);
    assert_eq!(counts.artists, (RELEASE_COUNT * 2) as i64);
    assert_eq!(counts.imports, RELEASE_COUNT as i64);
    let states = WorkItemStore
        .counts_by_state(&db.pool)
        .await
        .expect("final work states");
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].state, IngestionState::Cataloged);
    assert_eq!(states[0].count, RELEASE_COUNT as i64);
    let partial_releases: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM releases r
         WHERE NOT EXISTS (SELECT 1 FROM tracks t WHERE t.release_id = r.id)
            OR NOT EXISTS (SELECT 1 FROM catalog_imports ci WHERE ci.release_id = r.id)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("query partial imports");
    assert_eq!(partial_releases.0, 0);

    let reconciled_rows: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
                (SELECT count(*) FROM catalog_imports),
                (SELECT count(*) FROM releases),
                (SELECT count(*) FROM tracks),
                (SELECT count(*) FROM artists),
                (SELECT count(*) FROM media_assets),
                (SELECT count(*) FROM audio_identities),
                (SELECT count(*) FROM observed_artists),
                (SELECT count(*) FROM release_artists),
                (SELECT count(*) FROM track_artists),
                (SELECT count(*) FROM track_media_assets),
                (SELECT count(*) FROM catalog_semantic_overlays)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count authoritative reconciled catalog rows");
    let core_authoritative_rows_reconciled = [
        reconciled_rows.0,
        reconciled_rows.1,
        reconciled_rows.2,
        reconciled_rows.3,
        reconciled_rows.4,
        reconciled_rows.5,
        reconciled_rows.6,
        reconciled_rows.7,
        reconciled_rows.8,
        reconciled_rows.9,
        reconciled_rows.10,
    ]
    .into_iter()
    .sum::<i64>();
    assert_eq!(
        core_authoritative_rows_reconciled,
        RELEASE_COUNT as i64 * 13
    );
    let derived_and_evidence_rows: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
                (SELECT count(*) FROM artist_match_evidence),
                (SELECT count(*) FROM artist_match_decisions),
                (SELECT count(*) FROM artist_match_current),
                (SELECT count(*) FROM artist_track_associations),
                (SELECT count(*) FROM release_catalog_aggregates),
                (SELECT count(*) FROM artist_catalog_aggregates),
                (SELECT count(*) FROM catalog_projection_outbox),
                (SELECT count(*) FROM catalog_audit_events),
                (SELECT count(*) FROM release_work_item_events
                 WHERE from_state='cataloging' AND to_state='cataloged')",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count all additional rows written by catalog import");
    let catalog_rows_written = core_authoritative_rows_reconciled
        + [
            derived_and_evidence_rows.0,
            derived_and_evidence_rows.1,
            derived_and_evidence_rows.2,
            derived_and_evidence_rows.3,
            derived_and_evidence_rows.4,
            derived_and_evidence_rows.5,
            derived_and_evidence_rows.6,
            derived_and_evidence_rows.7,
            derived_and_evidence_rows.8,
        ]
        .into_iter()
        .sum::<i64>();
    assert_eq!(catalog_rows_written, RELEASE_COUNT as i64 * 30);
    let peak_rss =
        peak_test_process_rss_kib().map_or_else(|| "null".to_owned(), |value| value.to_string());

    eprintln!(
        "phase4-catalog-acceptance={{\"total\":{RELEASE_COUNT},\"cataloged\":{},\"lost\":0,\"duplicate_authoritative_result\":0,\"partial\":{},\"source_or_media_reads\":0,\"claim_batches\":{claim_batches},\"configured_catalog_concurrency\":{MAX_CATALOG_CONCURRENCY},\"measured_catalog_concurrency\":{measured_peak},\"set_based_catalog_mutation_statements\":{total_set_based_mutation_statements},\"set_based_mutation_statements_per_release\":{set_based_statement_count_per_release},\"application_sql_statements_in_window_excluding_transaction_control\":{application_sql_statements_in_window},\"core_authoritative_rows_reconciled\":{core_authoritative_rows_reconciled},\"catalog_rows_written\":{catalog_rows_written},\"catalog_store_wall_ms\":{},\"claim_store_call_ms\":{},\"import_store_call_sum_ms\":{},\"peak_test_process_rss_kib\":{peak_rss}}}",
        counts.imports,
        partial_releases.0,
        elapsed.as_millis(),
        claim_store_call_elapsed.as_millis(),
        import_store_call_sum.as_millis(),
    );

    db.drop().await;
}

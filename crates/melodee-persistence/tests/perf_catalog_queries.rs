//! R2.7 performance evidence: representative PostgreSQL catalog browse
//! queries at production-shaped cardinality, including planner behavior for
//! broad low-selectivity searches. Follows the `perf_claims` pattern:
//! measurements are evidence, not user-facing performance claims; no
//! thresholds are asserted (PRD-PERF-001/003). Set
//! `MELODEE_WRITE_PERF_EVIDENCE=1` to refresh
//! `design/evidence/r2/catalog-query-benchmark.json`; normal correctness
//! gates never rewrite tracked evidence.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::TestDb;
use melodee_persistence::{CatalogArtistSort, CatalogReleaseSort, CatalogStore, CatalogTrackSort};
use serde::Serialize;
use sqlx::Acquire as _;
use sqlx::PgPool;
use std::time::Instant;

const ARTISTS: i64 = 2_000;
const RELEASES: i64 = 10_000;
const TRACKS_PER_RELEASE: i64 = 12;
const REPS: u32 = 3;

#[derive(Serialize)]
struct QueryMeasurement {
    query: &'static str,
    description: &'static str,
    matched_rows: i64,
    samples_ms: Vec<u128>,
    min_ms: u128,
    plan: Option<Vec<String>>,
}

#[derive(Serialize)]
struct BenchmarkReport {
    scenario: &'static str,
    artists: i64,
    releases: i64,
    tracks: i64,
    measurements: Vec<QueryMeasurement>,
}

/// Seeds a production-shaped catalog in dependency order with set-based
/// inserts. 40% of artist/release/track titles carry the token `Rock` so
/// the broad ILIKE queries are genuinely low-selectivity. All statements run
/// on one pinned connection because the staging tables are per-connection
/// TEMPORARY tables.
async fn seed_catalog(pool: &PgPool) {
    let mut seed_connection = pool.acquire().await.expect("seed connection");
    let cxn = &mut *seed_connection;
    sqlx::query(
        "INSERT INTO artists (
             id, public_api_id, canonical_name, normalized_name, sort_name,
             artist_type, status, created_at, updated_at
         )
         SELECT uuidv7(), uuidv7(),
                CASE WHEN g % 5 < 2 THEN 'Rock Artist ' ELSE 'Artist ' END || g,
                lower(CASE WHEN g % 5 < 2 THEN 'Rock Artist ' ELSE 'Artist ' END || g),
                CASE WHEN g % 5 < 2 THEN 'Rock Artist ' ELSE 'Artist ' END || g,
                'group', 'verified', now(), now()
         FROM generate_series(1, $1) AS g",
    )
    .bind(ARTISTS)
    .execute(&mut *cxn)
    .await
    .expect("seed artists");

    // One recorded alias on every tenth artist so the alias EXISTS leg of
    // the artist filter carries realistic cardinality.
    sqlx::query(
        "INSERT INTO artist_aliases (id, artist_id, alias, normalized_alias, created_at)
         SELECT uuidv7(), a.id, 'Rock Alias ' || s.g, lower('Rock Alias ' || s.g), now()
         FROM generate_series(1, $1) AS s(g)
         JOIN artists a ON a.canonical_name = 'Artist ' || (s.g * 10)",
    )
    .bind(ARTISTS / 10)
    .execute(&mut *cxn)
    .await
    .expect("seed artist aliases");

    sqlx::query(
        "INSERT INTO releases (
             id, public_api_id, title, normalized_title, sort_title,
             display_artist, status, created_at, updated_at
         )
         SELECT uuidv7(), uuidv7(),
                CASE WHEN g % 5 < 2 THEN 'Rock Release ' ELSE 'Release ' END || g,
                lower(CASE WHEN g % 5 < 2 THEN 'Rock Release ' ELSE 'Release ' END || g),
                CASE WHEN g % 5 < 2 THEN 'Rock Release ' ELSE 'Release ' END || g,
                'Artist ' || ((g - 1) % $2 + 1),
                'active', now(), now()
         FROM generate_series(1, $1) AS g",
    )
    .bind(RELEASES)
    .bind(ARTISTS)
    .execute(&mut *cxn)
    .await
    .expect("seed releases");

    sqlx::query(
        "INSERT INTO release_catalog_aggregates (
             release_id, track_count, duration_ms, media_bytes, revision, updated_at
         )
         SELECT id, $2, $2 * 240000, $2 * 40000000, 1, now() FROM releases",
    )
    .bind(RELEASES)
    .bind(TRACKS_PER_RELEASE)
    .execute(&mut *cxn)
    .await
    .expect("seed release aggregates");

    sqlx::query(
        "INSERT INTO artist_catalog_aggregates (
             artist_id, release_count, billed_track_count, performance_track_count,
             all_credit_track_count, revision, updated_at
         )
         SELECT id, $2, $2 * $3, $2 * $3, $2 * $3, 1, now() FROM artists",
    )
    .bind(ARTISTS)
    .bind(RELEASES / ARTISTS)
    .bind(TRACKS_PER_RELEASE)
    .execute(&mut *cxn)
    .await
    .expect("seed artist aggregates");

    // Deterministic per-release ordinal for relationship seeding.
    sqlx::query(
        "CREATE TEMPORARY TABLE bench_releases AS
         SELECT id, public_api_id, title, display_artist,
                row_number() OVER (ORDER BY id) AS n
         FROM releases",
    )
    .execute(&mut *cxn)
    .await
    .expect("stage bench releases");

    sqlx::query(
        "INSERT INTO release_artists (
             id, release_id, artist_id, role, presentation_order,
             display_value, join_phrase, created_at
         )
         SELECT uuidv7(), r.id, a.id, 'primary', 1, a.canonical_name, '', now()
         FROM bench_releases r
         CROSS JOIN LATERAL (SELECT ((r.n - 1) % $2 + 1) AS m) AS mm
         JOIN artists a ON a.canonical_name =
             CASE WHEN mm.m % 5 < 2 THEN 'Rock Artist ' ELSE 'Artist ' END || mm.m",
    )
    .bind(RELEASES)
    .bind(ARTISTS)
    .execute(&mut *cxn)
    .await
    .expect("seed release artists");

    // Stage one row per track with pre-generated identities so the
    // deferred circular FK (Track names its playable MediaAsset; the link
    // table proves it) can be satisfied set-based inside one transaction.
    sqlx::query(
        "CREATE TEMPORARY TABLE bench_tracks AS
         SELECT uuidv7() AS track_id, uuidv7() AS public_api_id,
                uuidv7() AS audio_identity_id, uuidv7() AS media_asset_id,
                r.id AS release_id, r.title AS release_title,
                r.display_artist, s.t AS ordinal,
                (r.n - 1) * $1 + s.t AS n
         FROM bench_releases r
         CROSS JOIN generate_series(1, $1) AS s(t)",
    )
    .bind(TRACKS_PER_RELEASE)
    .execute(&mut *cxn)
    .await
    .expect("stage bench tracks");

    let mut tx = seed_connection.begin().await.expect("seed transaction");
    sqlx::query(
        "INSERT INTO audio_identities (id, status, created_at, updated_at)
         SELECT audio_identity_id, 'verified', now(), now() FROM bench_tracks",
    )
    .execute(&mut *tx)
    .await
    .expect("seed audio identities");
    sqlx::query(
        "INSERT INTO media_assets (
             id, canonical_relative_path, byte_hash, byte_size, mime_type,
             codec, sample_rate_hz, channels, duration_ms, created_at
         )
         SELECT media_asset_id, 'bench/track-' || n || '.flac',
                repeat(md5(n::text), 2), 40000000, 'audio/flac',
                'flac', 44100, 2, 240000, now()
         FROM bench_tracks",
    )
    .execute(&mut *tx)
    .await
    .expect("seed media assets");
    sqlx::query(
        "INSERT INTO tracks (
             id, public_api_id, release_id, audio_identity_id,
             playable_media_asset_id, ordinal, title, normalized_title,
             sort_title, display_artist, disc_number, track_number,
             duration_ms, created_at, updated_at
         )
         SELECT track_id, public_api_id, release_id, audio_identity_id,
                media_asset_id, ordinal,
                CASE WHEN ordinal % 5 < 2 THEN 'Rock Track ' ELSE 'Track ' END || n,
                lower(CASE WHEN ordinal % 5 < 2 THEN 'Rock Track ' ELSE 'Track ' END || n),
                CASE WHEN ordinal % 5 < 2 THEN 'Rock Track ' ELSE 'Track ' END || n,
                display_artist, 1, ordinal, 240000, now(), now()
         FROM bench_tracks",
    )
    .execute(&mut *tx)
    .await
    .expect("seed tracks");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal, created_at)
         SELECT track_id, media_asset_id, 'playable', 1, now() FROM bench_tracks",
    )
    .execute(&mut *tx)
    .await
    .expect("seed track media assets");
    tx.commit().await.expect("commit seed transaction");

    sqlx::query("ANALYZE artists")
        .execute(pool)
        .await
        .expect("analyze artists");
    sqlx::query("ANALYZE releases")
        .execute(pool)
        .await
        .expect("analyze releases");
    sqlx::query("ANALYZE tracks")
        .execute(pool)
        .await
        .expect("analyze tracks");
}

async fn measure<F, Fut>(
    pool: &PgPool,
    query: &'static str,
    description: &'static str,
    plan_sql: Option<&str>,
    run: F,
) -> QueryMeasurement
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = i64>,
{
    let mut samples_ms = Vec::new();
    let mut matched_rows = 0_i64;
    for _ in 0..REPS {
        let started = Instant::now();
        matched_rows = run().await;
        samples_ms.push(started.elapsed().as_millis());
    }
    let plan = match plan_sql {
        Some(sql) => Some(
            // Test-owned constant SQL; the wrapper is sqlx 0.9's explicit
            // acknowledgement for non-literal query text.
            sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql))
                .fetch_all(pool)
                .await
                .expect("explain"),
        ),
        None => None,
    };
    QueryMeasurement {
        query,
        description,
        matched_rows,
        min_ms: *samples_ms.iter().min().unwrap_or(&0),
        samples_ms,
        plan,
    }
}

#[tokio::test]
async fn catalog_browse_queries_at_production_shaped_cardinality() {
    let db = TestDb::create("perf_catalog_queries").await;
    let pool = &db.pool;
    seed_catalog(pool).await;

    let store = CatalogStore;
    let mut measurements = Vec::new();

    measurements.push(
        measure(
            pool,
            "list_catalog_releases",
            "unfiltered first page (hot browse path)",
            None,
            || {
                Box::pin(async {
                    let (rows, total) = store
                        .list_catalog_releases(
                            pool,
                            None,
                            None,
                            50,
                            0,
                            CatalogReleaseSort::TitleAsc,
                        )
                        .await
                        .expect("releases page");
                    assert_eq!(50, rows.len() as i64);
                    total
                })
            },
        )
        .await,
    );

    measurements.push(
        measure(
            pool,
            "list_catalog_releases(broad)",
            "broad low-selectivity ILIKE '%rock%' matching 40% of releases",
            Some(
                "EXPLAIN SELECT r.public_api_id, r.title
                   FROM releases r
                  WHERE (r.title ILIKE '%rock%' ESCAPE '\\')
                  ORDER BY r.sort_title ASC, r.id ASC
                  LIMIT 50 OFFSET 0",
            ),
            || {
                Box::pin(async {
                    let (_rows, total) = store
                        .list_catalog_releases(
                            pool,
                            Some("%rock%"),
                            None,
                            50,
                            0,
                            CatalogReleaseSort::TitleAsc,
                        )
                        .await
                        .expect("broad releases");
                    total
                })
            },
        )
        .await,
    );

    measurements.push(
        measure(
            pool,
            "list_catalog_releases(narrow)",
            "narrow ILIKE matching one release",
            None,
            || {
                Box::pin(async {
                    let (_rows, total) = store
                        .list_catalog_releases(
                            pool,
                            Some("%Release 9999%"),
                            None,
                            50,
                            0,
                            CatalogReleaseSort::TitleAsc,
                        )
                        .await
                        .expect("narrow releases");
                    total
                })
            },
        )
        .await,
    );

    measurements.push(
        measure(
            pool,
            "list_catalog_artists(broad)",
            "broad low-selectivity ILIKE across canonical/sort name and alias EXISTS leg",
            Some(
                "EXPLAIN SELECT a.public_api_id, a.canonical_name
                   FROM artists a
                  WHERE (a.canonical_name ILIKE '%rock%' ESCAPE '\\'
                         OR a.sort_name ILIKE '%rock%' ESCAPE '\\'
                         OR EXISTS (SELECT 1 FROM artist_aliases alias
                                    WHERE alias.artist_id = a.id
                                      AND alias.alias ILIKE '%rock%' ESCAPE '\\'))
                  ORDER BY a.sort_name ASC, a.id ASC
                  LIMIT 50 OFFSET 0",
            ),
            || {
                Box::pin(async {
                    let (_rows, total) = store
                        .list_catalog_artists(
                            pool,
                            Some("%rock%"),
                            50,
                            0,
                            CatalogArtistSort::NameAsc,
                        )
                        .await
                        .expect("broad artists");
                    total
                })
            },
        )
        .await,
    );

    let some_release_public_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT public_api_id FROM releases ORDER BY id LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .expect("release public id");
    measurements.push(
        measure(
            pool,
            "list_catalog_tracks(by_release)",
            "track page scoped to one release public id (join + lookup)",
            None,
            || {
                Box::pin(async {
                    let (_rows, total) = store
                        .list_catalog_tracks(
                            pool,
                            None,
                            Some(some_release_public_id),
                            50,
                            0,
                            CatalogTrackSort::TitleAsc,
                        )
                        .await
                        .expect("tracks by release");
                    total
                })
            },
        )
        .await,
    );

    measurements.push(
        measure(
            pool,
            "list_catalog_tracks(broad)",
            "broad low-selectivity ILIKE '%rock%' matching 40% of tracks",
            Some(
                "EXPLAIN SELECT t.public_api_id, t.title
                   FROM tracks t JOIN releases r ON r.id = t.release_id
                  WHERE (t.title ILIKE '%rock%' ESCAPE '\\')
                  ORDER BY t.sort_title ASC, t.id ASC
                  LIMIT 50 OFFSET 0",
            ),
            || {
                Box::pin(async {
                    let (_rows, total) = store
                        .list_catalog_tracks(
                            pool,
                            Some("%rock%"),
                            None,
                            50,
                            0,
                            CatalogTrackSort::TitleAsc,
                        )
                        .await
                        .expect("broad tracks");
                    total
                })
            },
        )
        .await,
    );

    let report = BenchmarkReport {
        scenario: "catalog-browse-production-shaped",
        artists: ARTISTS,
        releases: RELEASES,
        tracks: RELEASES * TRACKS_PER_RELEASE,
        measurements,
    };
    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    eprintln!("r2.7-catalog-benchmark={json}");

    if std::env::var_os("MELODEE_WRITE_PERF_EVIDENCE").is_some() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../design/evidence/r2/catalog-query-benchmark.json"
        );
        std::fs::write(path, format!("{json}\n")).expect("write evidence");
    }

    db.drop().await;
}

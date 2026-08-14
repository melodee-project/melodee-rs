//! Repeatable Phase 7 native-API scale evidence.
//!
//! The default smoke protects result bounds and catastrophic regressions. The
//! ignored current-catalog run records the proposed Phase 0 browse/random
//! budgets without turning an unratified proposal into a product requirement.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{sync::Arc, time::Instant};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::native::{
    NativeClientBinding, NativeFuture, NativePrincipal, NativeState, NativeStreamUrlSigner,
    PostgresNativeUserStateReader,
};
use serde::Serialize;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const DEFAULT_TRACKS: i64 = 2_000;
const CURRENT_CATALOG_TRACKS: i64 = 560_000;
const REQUESTS_PER_ROUTE: usize = 20;
const MAX_RESPONSE_BYTES: usize = 1_048_576;

#[derive(Debug)]
struct TestSigner;

impl NativeStreamUrlSigner for TestSigner {
    fn stream_url(
        &self,
        track_public_id: Uuid,
        user_public_id: Uuid,
        client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        let binding = client_binding.to_owned();
        Box::pin(async move {
            Ok(format!(
                "http://127.0.0.1/song/stream/{track_public_id}/{user_public_id}/signed-{binding}"
            ))
        })
    }
}

#[derive(Debug, Serialize)]
struct RouteMeasurement {
    route: String,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    response_bytes: usize,
}

#[derive(Debug, Serialize)]
struct Phase7PerformanceReport {
    tracks: i64,
    releases: i64,
    artists: i64,
    requests_per_route: usize,
    seed_wall_ms: u128,
    peak_test_process_rss_kib: Option<u64>,
    measurement_scope: &'static str,
    authentication_scope: &'static str,
    stream_signing_scope: &'static str,
    media_io_scope: &'static str,
    measurements: Vec<RouteMeasurement>,
}

#[tokio::test]
async fn native_api_scale_smoke_is_bounded_and_correct() {
    run_scale(DEFAULT_TRACKS, false).await;
}

#[tokio::test]
#[ignore = "current-catalog Phase 7 performance evidence; run explicitly with --ignored"]
async fn native_api_current_catalog_scale_evidence() {
    run_scale(CURRENT_CATALOG_TRACKS, true).await;
}

async fn run_scale(track_count: i64, enforce_proposed_budgets: bool) {
    let db = common::TestDb::create("phase7_native_performance").await;
    let seed_started = Instant::now();
    let (artist_count, release_count) = seed_catalog(&db.pool, track_count).await;
    let seed_wall_ms = seed_started.elapsed().as_millis();
    let (state, principal) = state(&db).await;

    let (exact_track, exact_track_title): (Uuid, String) = sqlx::query_as(
        "SELECT public_api_id,title FROM tracks ORDER BY ordinal DESC, public_api_id LIMIT 1",
    )
    .fetch_one(&db.pool)
    .await
    .expect("exact benchmark Track");
    for required_index in [
        "artists_native_search_name_idx",
        "releases_native_search_title_idx",
        "tracks_native_search_title_idx",
        "tracks_native_random_hash_idx",
    ] {
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
                .bind(required_index)
                .fetch_one(&db.pool)
                .await
                .expect("native search index check"),
            "required native browse/search index is missing: {required_index}"
        );
    }
    let exact_search_term = exact_track_title
        .split_whitespace()
        .next_back()
        .expect("benchmark Track title suffix");
    let exact_search = format!("/api/v1/search/songs?q={exact_search_term}&page=1&pageSize=25");
    let routes = [
        "/api/v1/artists?page=1&pageSize=25&orderBy=Name&orderDirection=asc".to_string(),
        "/api/v1/albums?page=1&pageSize=25&orderBy=ReleaseDate&orderDirection=desc".to_string(),
        "/api/v1/songs?page=1&pageSize=25".to_string(),
        exact_search.clone(),
        "/api/v1/songs/random?count=25".to_string(),
        format!("/api/v1/songs/{exact_track}"),
    ];

    let mut measurements = Vec::new();
    for route in routes {
        // One unrecorded request warms PostgreSQL buffers and prepared paths.
        let _ = request(&state, principal, &route).await;
        let mut samples = Vec::with_capacity(REQUESTS_PER_ROUTE);
        let mut expected_body = None;
        let mut response_bytes = 0;
        for _ in 0..REQUESTS_PER_ROUTE {
            let started = Instant::now();
            let body = request(&state, principal, &route).await;
            samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            assert!(!body.is_empty(), "{route} returned an empty body");
            assert!(
                body.len() <= MAX_RESPONSE_BYTES,
                "{route} returned an unbounded response"
            );
            response_bytes = body.len();
            if route != "/api/v1/songs/random?count=25" {
                if let Some(expected) = expected_body.as_ref() {
                    assert_eq!(&body, expected, "{route} was not deterministic");
                } else {
                    expected_body = Some(body.clone());
                }
            }
            let value: serde_json::Value =
                serde_json::from_slice(&body).expect("native benchmark response JSON");
            if route.starts_with("/api/v1/artists?")
                || route.starts_with("/api/v1/albums?")
                || route == "/api/v1/songs?page=1&pageSize=25"
            {
                assert_eq!(
                    value["data"].as_array().map(Vec::len),
                    Some(25),
                    "{route} page size changed"
                );
            }
            if route == exact_search {
                assert_eq!(value["data"].as_array().map(Vec::len), Some(1));
                assert_eq!(value["data"][0]["id"], exact_track.to_string());
            }
            if route == "/api/v1/songs/random?count=25" {
                assert_eq!(value.as_array().map(Vec::len), Some(25));
            }
            if route.ends_with(&exact_track.to_string()) {
                assert_eq!(value["id"], exact_track.to_string());
            }
        }
        samples.sort_by(f64::total_cmp);
        let p50 = samples[(samples.len() - 1) / 2];
        let p95 = samples[((samples.len() * 95).div_ceil(100)).saturating_sub(1)];
        let max = *samples.last().expect("latency samples");
        if enforce_proposed_budgets {
            if route == "/api/v1/songs/random?count=25" {
                assert!(
                    p95 <= 500.0,
                    "proposed random p95 budget missed for {route}: {p95:.3}ms"
                );
            } else {
                assert!(
                    p95 <= 50.0,
                    "proposed browse/search p95 budget missed for {route}: {p95:.3}ms"
                );
            }
        } else {
            assert!(p95 <= 2_000.0, "smoke route stalled: {route} {p95:.3}ms");
        }
        measurements.push(RouteMeasurement {
            route,
            p50_ms: p50,
            p95_ms: p95,
            max_ms: max,
            response_bytes,
        });
    }

    let report = Phase7PerformanceReport {
        tracks: track_count,
        releases: release_count,
        artists: artist_count,
        requests_per_route: REQUESTS_PER_ROUTE,
        seed_wall_ms,
        peak_test_process_rss_kib: peak_rss_kib(),
        measurement_scope: "in-process native router with a preinstalled authenticated principal",
        authentication_scope: "access-token cryptography and public-ID lookup are outside the measured interval",
        stream_signing_scope: "bounded deterministic test signer; production HMAC signing is outside the measured interval",
        media_io_scope: "measured metadata routes do not open media; filesystem reads are not instrumented by this harness",
        measurements,
    };
    println!(
        "phase7-native-api-performance={}",
        serde_json::to_string(&report).expect("performance report JSON")
    );
    db.drop().await;
}

async fn request(state: &NativeState, principal: NativePrincipal, route: &str) -> Vec<u8> {
    let mut request = Request::builder()
        .uri(route)
        .body(Body::empty())
        .expect("benchmark request");
    request.extensions_mut().insert(principal);
    request
        .extensions_mut()
        .insert(NativeClientBinding("127.0.0.1".to_string()));
    let response = melodee_server::native::router()
        .with_state(state.clone())
        .oneshot(request)
        .await
        .expect("native route response");
    assert_eq!(response.status(), axum::http::StatusCode::OK, "GET {route}");
    response
        .into_body()
        .collect()
        .await
        .expect("native route body")
        .to_bytes()
        .to_vec()
}

async fn state(db: &common::TestDb) -> (NativeState, NativePrincipal) {
    let now = OffsetDateTime::now_utc();
    let roles = vec!["user".to_string()];
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "phase7-benchmark",
                normalized_username: "PHASE7BENCHMARK",
                email: "phase7-benchmark@example.test",
                normalized_email: "PHASE7-BENCHMARK@EXAMPLE.TEST",
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "phase7-benchmark-stream-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &roles,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("benchmark user");
    let principal = NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: false,
        is_editor: false,
    };
    let state = NativeState {
        pool: db.pool.clone(),
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search: None,
        public_origin: "http://127.0.0.1".to_string(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("bounded podcast discovery client"),
        stream_url_signer: Arc::new(TestSigner),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "phase7-benchmark-jwt-key-at-least-32-characters".to_string(),
                90,
            )
            .expect("benchmark auth configuration"),
        ),
    };
    (state, principal)
}

async fn seed_catalog(pool: &sqlx::PgPool, track_count: i64) -> (i64, i64) {
    assert!(track_count >= 25);
    let artist_count = (track_count / 16).clamp(1, 35_000);
    let release_count = (track_count * 5 / 56).clamp(1, 50_000);
    let mut tx = pool.begin().await.expect("benchmark seed transaction");
    sqlx::query(
        "CREATE TEMP TABLE phase7_bench_artist AS
           SELECT g, uuidv7() id, uuidv7() public_id FROM generate_series(1, $1) g",
    )
    .bind(artist_count)
    .execute(&mut *tx)
    .await
    .expect("benchmark Artist identity map");
    sqlx::query(
        "CREATE TEMP TABLE phase7_bench_release AS
           SELECT g, uuidv7() id, uuidv7() public_id,
                  ((g - 1) % $1 + 1)::bigint artist_g
           FROM generate_series(1, $2) g",
    )
    .bind(artist_count)
    .bind(release_count)
    .execute(&mut *tx)
    .await
    .expect("benchmark Release identity map");
    sqlx::query(
        "CREATE TEMP TABLE phase7_bench_track AS
           SELECT g, uuidv7() id, uuidv7() public_id,
                  ((g - 1) % $1 + 1)::bigint release_g,
                  ((g - 1) / $1 + 1)::integer ordinal
           FROM generate_series(1, $2) g",
    )
    .bind(release_count)
    .bind(track_count)
    .execute(&mut *tx)
    .await
    .expect("benchmark Track identity map");

    let audio_identity = Uuid::now_v7();
    let media_asset = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO artists
           (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at)
         SELECT id,public_id,format('Benchmark Artist %s',lpad(g::text,5,'0')),
                format('benchmark artist %s',lpad(g::text,5,'0')),
                format('benchmark artist %s',lpad(g::text,5,'0')),
                'person','verified',TIMESTAMPTZ '2026-08-12 00:00:00Z',TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_artist",
    )
    .execute(&mut *tx)
    .await
    .expect("benchmark Artists");
    sqlx::query(
        "INSERT INTO releases
           (id,public_api_id,title,normalized_title,sort_title,display_artist,release_date,status,created_at,updated_at)
         SELECT id,public_id,format('Benchmark Release %s',lpad(g::text,5,'0')),
                format('benchmark release %s',lpad(g::text,5,'0')),
                format('benchmark release %s',lpad(g::text,5,'0')),
                format('Benchmark Artist %s',lpad(artist_g::text,5,'0')),
                DATE '2000-01-01' + ((g - 1) % 9000)::integer,'active',
                TIMESTAMPTZ '2026-08-12 00:00:00Z',TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_release",
    )
    .execute(&mut *tx)
    .await
    .expect("benchmark Releases");
    sqlx::query(
        "INSERT INTO audio_identities (id,status,created_at,updated_at)
         VALUES ($1,'verified',TIMESTAMPTZ '2026-08-12 00:00:00Z',TIMESTAMPTZ '2026-08-12 00:00:00Z')",
    )
    .bind(audio_identity)
    .execute(&mut *tx)
    .await
    .expect("benchmark AudioIdentity");
    sqlx::query(
        "INSERT INTO media_assets
           (id,canonical_relative_path,byte_hash,byte_size,mime_type,container,codec,bitrate_bps,duration_ms,created_at)
         VALUES ($1,'phase7-benchmark/shared.flac',repeat('a',64),1048576,'audio/flac','flac','flac',
                 1000000,180000,TIMESTAMPTZ '2026-08-12 00:00:00Z')",
    )
    .bind(media_asset)
    .execute(&mut *tx)
    .await
    .expect("benchmark MediaAsset");
    sqlx::query(
        "INSERT INTO tracks
           (id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,ordinal,title,
            normalized_title,sort_title,display_artist,disc_number,track_number,duration_ms,revision,
            created_at,updated_at)
         SELECT track.id,track.public_id,release.id,$1,$2,track.ordinal,
                format('Benchmark Track %s',lpad(track.g::text,8,'0')),
                format('benchmark track %s',lpad(track.g::text,8,'0')),
                format('benchmark track %s',lpad(track.g::text,8,'0')),
                format('Benchmark Artist %s',lpad(release.artist_g::text,5,'0')),
                1,track.ordinal,180000,1,TIMESTAMPTZ '2026-08-12 00:00:00Z',TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_track track JOIN phase7_bench_release release ON release.g=track.release_g",
    )
    .bind(audio_identity)
    .bind(media_asset)
    .execute(&mut *tx)
    .await
    .expect("benchmark Tracks");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id,media_asset_id,purpose,ordinal,created_at)
         SELECT id,$1,'playable',1,TIMESTAMPTZ '2026-08-12 00:00:00Z' FROM phase7_bench_track",
    )
    .bind(media_asset)
    .execute(&mut *tx)
    .await
    .expect("benchmark playable links");
    sqlx::query(
        "INSERT INTO release_artists
           (id,release_id,artist_id,role,presentation_order,display_value,join_phrase,created_at)
         SELECT uuidv7(),release.id,artist.id,'primary',1,
                format('Benchmark Artist %s',lpad(artist.g::text,5,'0')),'',
                TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_release release JOIN phase7_bench_artist artist ON artist.g=release.artist_g",
    )
    .execute(&mut *tx)
    .await
    .expect("benchmark ReleaseArtists");
    sqlx::query(
        "INSERT INTO track_artists
           (id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at)
         SELECT uuidv7(),track.id,artist.id,'primary',1,
                format('Benchmark Artist %s',lpad(artist.g::text,5,'0')),'',
                TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_track track
         JOIN phase7_bench_release release ON release.g=track.release_g
         JOIN phase7_bench_artist artist ON artist.g=release.artist_g",
    )
    .execute(&mut *tx)
    .await
    .expect("benchmark TrackArtists");
    sqlx::query(
        "INSERT INTO artist_track_associations
           (artist_id,track_id,association_type,participation_kind,source_key,confidence,is_performance,created_at)
         SELECT artist.id,track.id,'primary','direct',track.id,10000,true,
                TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_track track
         JOIN phase7_bench_release release ON release.g=track.release_g
         JOIN phase7_bench_artist artist ON artist.g=release.artist_g",
    )
    .execute(&mut *tx)
    .await
    .expect("benchmark Track associations");
    sqlx::query(
        "INSERT INTO release_catalog_aggregates
           (release_id,track_count,duration_ms,media_bytes,revision,updated_at)
         SELECT release.id,count(track.id),count(track.id)*180000,count(track.id)*1048576,1,
                TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_release release JOIN phase7_bench_track track ON track.release_g=release.g
         GROUP BY release.id",
    )
    .execute(&mut *tx)
    .await
    .expect("benchmark Release aggregates");
    sqlx::query(
        "INSERT INTO artist_catalog_aggregates
           (artist_id,release_count,billed_track_count,performance_track_count,all_credit_track_count,
            revision,updated_at)
         SELECT artist.id,count(DISTINCT release.id),count(track.id),count(track.id),count(track.id),1,
                TIMESTAMPTZ '2026-08-12 00:00:00Z'
         FROM phase7_bench_artist artist
         JOIN phase7_bench_release release ON release.artist_g=artist.g
         JOIN phase7_bench_track track ON track.release_g=release.g
         GROUP BY artist.id",
    )
    .execute(&mut *tx)
    .await
    .expect("benchmark Artist aggregates");
    tx.commit().await.expect("benchmark seed commit");
    sqlx::query(
        "ANALYZE artists, releases, tracks, release_artists, track_artists,
                 artist_track_associations",
    )
    .execute(pool)
    .await
    .expect("benchmark statistics");
    (artist_count, release_count)
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmHWM:")?.trim();
        value.split_whitespace().next()?.parse().ok()
    })
}

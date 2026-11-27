//! Stats aggregation API integration tests: the administrator gate, the shape of
//! the library/listening/database panels, RFC 3339 timestamps, and the bounded
//! aggregate guarantees (fixed-size breakdowns, zero-filled day spine).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use common::TestDb;
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_system::AdminSystemState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-stats-jwt-key-at-least-32-bytes";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("stats-admin", true, admin_public),
        ("stats-plain", false, plain_public),
    ] {
        let email = format!("{username}@example.test");
        AuthStore::new()
            .import_user(
                &db.pool,
                &NativeUserImport {
                    id: Uuid::now_v7(),
                    public_api_id: public_id,
                    username,
                    normalized_username: &username.to_uppercase(),
                    email: &email,
                    normalized_email: &email.to_uppercase(),
                    password_hash: &bcrypt::hash("Mel0deeR0cks!", 4).expect("fixture hash"),
                    password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                    stream_signing_key: "test-stream-signing-key",
                    is_admin,
                    is_editor: false,
                    enabled: true,
                    locked: false,
                    blacklisted: false,
                    preferred_language: None,
                    preferred_theme: None,
                    auth_revision: 1,
                    roles: &["HasStreamRole".to_string()],
                    created_at: now,
                    updated_at: now,
                },
            )
            .await
            .expect("import user");
    }
    Fixture {
        admin_public,
        plain_public,
    }
}

fn token_for(public_api_id: Uuid, username: &str) -> String {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    issue_access_token(
        &melodee_auth::SecretString::new(JWT_KEY.to_string()),
        &NativeAccessClaims {
            aud: "MelodeeClient".to_string(),
            email: format!("{username}@example.test"),
            exp: now + 900,
            sid: public_api_id,
            iat: now,
            iss: "MelodeeApi".to_string(),
            nbf: now,
            role: vec![],
            unique_name: username.to_string(),
            theme: String::new(),
        },
    )
    .expect("issue token")
}

fn build_app(pool: sqlx::PgPool) -> axum::Router {
    let admin = AdminAccessState {
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        scripting: Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
        pool,
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth config"),
        ),
    };
    melodee_server::admin_stats::router().with_state(AdminSystemState::new(admin, None))
}

async fn call(app: axum::Router, path: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// Seeds a published *and catalog-imported* Release, so the Stats aggregates see
/// real catalog rows rather than a pending publication plan.
async fn seed_release(db: &TestDb, ordinal: u32, title: &str) {
    phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            release_artist: "Stats Artist",
            track_title: "Stats Track",
            track_artist: "Stats Artist",
            release_date: "1997-03-04",
        },
    )
    .await;
}

#[tokio::test]
async fn stats_require_an_administrator() {
    let db = TestDb::create("stats_admin_gate").await;
    let fixture = seed_users(&db).await;
    let app = build_app(db.pool.clone());

    let (status, _) = call(app.clone(), "/api/admin/v1/stats", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let plain = token_for(fixture.plain_public, "stats-plain");
    let (status, _) = call(app.clone(), "/api/admin/v1/stats", Some(&plain)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    db.drop().await;
}

#[tokio::test]
async fn library_asset_totals_and_bounded_formats_match_independent_sql() {
    let db = TestDb::create("stats_asset_groups").await;
    let fixture = seed_users(&db).await;
    let app = build_app(db.pool.clone());
    let admin = token_for(fixture.admin_public, "stats-admin");

    let (status, empty) = call(app.clone(), "/api/admin/v1/stats", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["library"]["mediaAssets"], 0);
    assert_eq!(empty["library"]["totalBytes"], 0);
    assert_eq!(empty["library"]["losslessTracks"], 0);
    assert_eq!(empty["library"]["formats"], serde_json::json!([]));

    // More than twelve tied groups exercise SQL ordering and truncation;
    // NULL, mixed-case lossless and lossy codecs exercise display semantics.
    sqlx::query("INSERT INTO media_assets(id,canonical_relative_path,byte_hash,byte_size,mime_type,container,codec,created_at)
        SELECT uuidv7(),'stats-fixture-'||n,repeat('a',64),n,'audio/test',
               CASE WHEN n=1 THEN NULL ELSE 'format-'||n END,
               CASE WHEN n=2 THEN NULL WHEN n=3 THEN 'FLAC' WHEN n=4 THEN 'flac' ELSE 'codec-'||n END,now()
          FROM generate_series(1,18) n")
        .execute(&db.pool).await.expect("insert varied unreferenced assets");
    sqlx::query("INSERT INTO media_assets(id,canonical_relative_path,byte_hash,byte_size,mime_type,container,codec,created_at)
        SELECT uuidv7(),'stats-repeated-'||n,repeat('b',64),n*7,'audio/test','shared','FLAC',now()
          FROM generate_series(1,3) n")
        .execute(&db.pool).await.expect("insert repeated format group");
    let expected_totals: (i64, i64, i64) = sqlx::query_as("SELECT count(*),COALESCE(sum(byte_size),0)::bigint,count(*) FILTER (WHERE codec IS NOT NULL AND lower(codec) IN ('flac','alac','wav','pcm_s16le','pcm_s24le','ape','wv')) FROM media_assets")
        .fetch_one(&db.pool).await.expect("independent asset totals");
    let expected_rows: Vec<(Option<String>, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT container,codec,count(*),COALESCE(sum(byte_size),0)::bigint FROM media_assets GROUP BY container,codec ORDER BY count(*) DESC,container,codec LIMIT 12",
    ).fetch_all(&db.pool).await.expect("independent ordered formats");
    let expected_formats: Vec<Value> = expected_rows.into_iter().map(|(container, codec, tracks, bytes)| {
        let codec = codec.unwrap_or_else(|| "unknown".to_owned());
        let lossless = matches!(codec.to_ascii_lowercase().as_str(), "flac"|"alac"|"wav"|"pcm_s16le"|"pcm_s24le"|"ape"|"wv");
        serde_json::json!({"container":container.unwrap_or_else(|| "unknown".to_owned()),"codec":codec,"tracks":tracks,"bytes":bytes,"lossless":lossless})
    }).collect();
    let (status, body) = call(app, "/api/admin/v1/stats", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["library"]["mediaAssets"], expected_totals.0);
    assert_eq!(body["library"]["totalBytes"], expected_totals.1);
    assert_eq!(body["library"]["losslessTracks"], expected_totals.2);
    assert_eq!(
        body["library"]["formats"],
        serde_json::json!(expected_formats)
    );
    db.drop().await;
}

#[tokio::test]
async fn stats_report_library_listening_and_database_panels() {
    let db = TestDb::create("stats_panels").await;
    let fixture = seed_users(&db).await;
    seed_release(&db, 41, "Stats Release One").await;
    seed_release(&db, 42, "Stats Release Two").await;
    sqlx::query("UPDATE media_assets SET codec = NULL")
        .execute(&db.pool)
        .await
        .expect("clear codecs");
    sqlx::query("UPDATE media_assets SET codec = 'FLAC' WHERE id = (SELECT min(id::text)::uuid FROM media_assets)")
        .execute(&db.pool).await.expect("set lossless codec");
    let expected_bytes: i64 = sqlx::query_scalar("SELECT sum(byte_size)::bigint FROM media_assets")
        .fetch_one(&db.pool)
        .await
        .expect("asset bytes");
    let expected_duration: i64 = sqlx::query_scalar("SELECT sum(duration_ms)::bigint FROM tracks")
        .fetch_one(&db.pool)
        .await
        .expect("track duration");
    sqlx::query("UPDATE release_publication_receipts SET published_at = now() - interval '2 hours'
                    WHERE work_item_id=(SELECT min(work_item_id::text)::uuid FROM release_publication_receipts)")
        .execute(&db.pool).await.expect("recent publication");
    sqlx::query("INSERT INTO release_work_items(id,api_key,source_path,current_path,state,state_reason,correlation_id,discovered_at,updated_at,created_at)
        SELECT uuidv7(),uuidv7(),state,state,state,'stats fixture',uuidv7(),now(),now(),now()
        FROM unnest(ARRAY['staged','needs_review']) AS state")
        .execute(&db.pool).await.expect("seed two queue states");
    let app = build_app(db.pool.clone());
    let admin = token_for(fixture.admin_public, "stats-admin");

    let (status, body) = call(app, "/api/admin/v1/stats", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Timestamps are RFC 3339 strings: the UI parses them with `new Date(...)`.
    let generated_at = body["generatedAt"].as_str().expect("generatedAt string");
    assert!(
        generated_at.ends_with('Z') && generated_at.contains('T'),
        "{generated_at}"
    );

    // Library panel.
    let library = &body["library"];
    assert_eq!(library["releases"], 2);
    assert_eq!(library["tracks"], 2);
    assert_eq!(library["mediaAssets"], 2);
    assert_eq!(library["totalBytes"], expected_bytes);
    assert_eq!(library["totalDurationMs"], expected_duration);
    assert_eq!(library["losslessTracks"], 1);
    let formats = library["formats"].as_array().expect("formats");
    assert!(!formats.is_empty(), "the fixture publishes one media asset");
    assert!(formats.len() <= 12, "the breakdown is bounded");
    // Release years come from the observed date text, so the decade is present.
    let decades = library["decades"].as_array().expect("decades");
    assert!(
        decades.iter().any(|entry| entry["decade"] == 1990),
        "1997 must land in the 1990s bucket: {decades:?}"
    );

    let processing = &body["processing"];
    assert_eq!(processing["importedLastHour"], 2);
    assert_eq!(processing["importedLastDay"], 2);
    assert_eq!(processing["waitingForPublication"], 1);
    assert_eq!(processing["awaitingReview"], 1);
    assert_eq!(processing["publishedLastHour"], 0);
    assert_eq!(processing["publishedLastDay"], 1);

    // Listening panel: a zero-filled day spine over the trailing window.
    let listening = &body["listening"];
    let days = listening["playsPerDay"].as_array().expect("playsPerDay");
    assert!(
        days.len() >= 14,
        "expected a two-week day spine, got {}",
        days.len()
    );
    let day = days[0]["day"].as_str().expect("day string");
    assert!(
        day.len() == 10 && day.contains('-'),
        "day must be a date: {day}"
    );
    assert_eq!(listening["totalPlays"], 0);

    // Database panel: sizes for the fixed relation list, honest counters.
    let database = &body["database"];
    assert!(database["totalBytes"].as_i64().unwrap_or_default() > 0);
    let tables = database["tables"].as_array().expect("tables");
    assert_eq!(tables.len(), 14);
    assert!(
        tables
            .windows(2)
            .all(|rows| rows[0]["bytes"].as_i64() >= rows[1]["bytes"].as_i64())
    );
    // The fixture publishes a media asset, which migration 0054 records in the
    // asset registry. Assert the panel reports the real registry aggregate
    // (honest, not invented) instead of assuming an empty store.
    let (registry_count, registry_bytes): (i64, i64) =
        sqlx::query_as("SELECT count(*), COALESCE(sum(size_bytes), 0) FROM asset_files")
            .fetch_one(&db.pool)
            .await
            .expect("read asset registry aggregate");
    assert_eq!(database["assetFileCount"].as_i64(), Some(registry_count));
    assert_eq!(database["assetFileBytes"].as_i64(), Some(registry_bytes));
    // The catalog import enqueues search-projection rows, and no search worker runs
    // in this test, so the honest value is whatever the outbox holds — assert it is
    // reported (not invented) rather than pinned to zero.
    assert!(
        database["pendingProjection"].as_i64().unwrap_or(-1) >= 0,
        "pendingProjection must be a count, got {}",
        database["pendingProjection"]
    );

    db.drop().await;
}

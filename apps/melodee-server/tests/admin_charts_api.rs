//! Admin chart curation API integration tests.
//!
//! Covers the operator surface added for legacy `ChartEditor` /
//! `ChartItemResolveDialog` / ranked-and-missing-report parity: administrator
//! gating, create/list/detail, validation, item replacement, catalog linkage,
//! ranked/missing reports, metadata patching (including explicit null clears),
//! and deletion.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-charts-jwt-key-at-least-32-bytes";

struct Fixture {
    admin_token: String,
    plain_token: String,
    artist_public_id: Uuid,
    release_public_id: Uuid,
}

async fn fixture(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let mut tokens = Vec::new();
    for (username, is_admin) in [("chart-admin", true), ("chart-plain", false)] {
        let public_id = Uuid::now_v7();
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
        let issued = OffsetDateTime::now_utc().unix_timestamp();
        tokens.push(
            issue_access_token(
                &SecretString::new(JWT_KEY.to_string()),
                &NativeAccessClaims {
                    aud: "MelodeeClient".to_string(),
                    email,
                    exp: issued + 900,
                    sid: public_id,
                    iat: issued,
                    iss: "MelodeeApi".to_string(),
                    nbf: issued,
                    role: vec![],
                    unique_name: username.to_string(),
                    theme: String::new(),
                },
            )
            .expect("issue token"),
        );
    }
    let artist_public_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name,
             artist_type, status, created_at, updated_at)
         VALUES ($1,$2,'Chart Artist','CHART ARTIST','Chart Artist','group','verified',now(),now())",
    )
    .bind(Uuid::now_v7())
    .bind(artist_public_id)
    .execute(&db.pool)
    .await
    .expect("artist");
    let release_public_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title,
             display_artist, release_date, status, created_at, updated_at)
         VALUES ($1,$2,'Chart Release','CHART RELEASE','Chart Release','Chart Artist',
             DATE '2020-01-01','active',now(),now())",
    )
    .bind(Uuid::now_v7())
    .bind(release_public_id)
    .execute(&db.pool)
    .await
    .expect("release");
    let mut tokens = tokens.into_iter();
    Fixture {
        admin_token: tokens.next().expect("admin token"),
        plain_token: tokens.next().expect("plain token"),
        artist_public_id,
        release_public_id,
    }
}

fn router(pool: sqlx::PgPool) -> axum::Router {
    melodee_server::admin_charts::router().with_state(AdminAccessState {
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
    })
}

async fn call(
    router: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = if let Some(body) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).expect("serialize")))
            .expect("request")
    } else {
        builder.body(Body::empty()).expect("request")
    };
    let response = router.oneshot(request).await.expect("response");
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

fn chart_body() -> Value {
    serde_json::json!({
        "slug": "chart-2026-01",
        "title": "January 2026",
        "sourceName": "Fixture Source",
        "sourceUrl": "https://example.test/chart",
        "chartYear": 2026,
        "description": "curated test chart",
        "tags": ["test", "curated"],
        "visible": true,
        "generatedPlaylistEnabled": false,
        "items": [
            {"rank": 1, "artistName": "Chart Artist", "releaseTitle": "Chart Release", "releaseYear": 2020},
            {"rank": 2, "artistName": "Unknown Stranger", "releaseTitle": "Nowhere", "releaseYear": 1999}
        ]
    })
}

#[tokio::test]
async fn chart_routes_require_administrator() {
    let db = common::TestDb::create("admin_charts_gate").await;
    let fixture = fixture(&db).await;
    let app = router(db.pool.clone());
    for (method, path) in [
        ("GET", "/api/admin/v1/charts"),
        ("POST", "/api/admin/v1/charts"),
        ("GET", &format!("/api/admin/v1/charts/{}", Uuid::now_v7())),
    ] {
        let (status, body) = call(app.clone(), method, path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_eq!(body["code"], "unauthenticated");
        let (status, _) = call(app.clone(), method, path, None, Some(&fixture.plain_token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    db.drop().await;
}

#[tokio::test]
async fn create_list_detail_patch_and_delete_round_trip() {
    let db = common::TestDb::create("admin_charts_crud").await;
    let fixture = fixture(&db).await;
    let app = router(db.pool.clone());

    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/charts",
        Some(chart_body()),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let chart_id = created["publicApiId"]
        .as_str()
        .expect("chart id")
        .to_string();
    assert_eq!(created["itemCount"], 2);
    assert_eq!(created["missingCount"], 2);
    assert_eq!(created["items"].as_array().expect("items").len(), 2);

    // Duplicate slug is a conflict, not a 500.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/charts",
        Some(chart_body()),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "chart_exists");

    let (status, listed) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/charts",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let charts = listed.as_array().expect("charts array");
    assert!(charts.iter().any(|chart| chart["slug"] == "chart-2026-01"));

    // PATCH: explicit null clears the description, omitted fields are kept.
    let (status, patched) = call(
        app.clone(),
        "PATCH",
        &format!("/api/admin/v1/charts/{chart_id}"),
        Some(serde_json::json!({"description": null, "visible": false})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert!(patched["description"].is_null());
    assert_eq!(patched["visible"], false);
    assert_eq!(patched["title"], "January 2026");

    let (status, _) = call(
        app.clone(),
        "DELETE",
        &format!("/api/admin/v1/charts/{chart_id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/charts/{chart_id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    db.drop().await;
}

#[tokio::test]
async fn item_replacement_validation_and_catalog_linkage() {
    let db = common::TestDb::create("admin_charts_items").await;
    let fixture = fixture(&db).await;
    let app = router(db.pool.clone());
    let (_, created) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/charts",
        Some(chart_body()),
        Some(&fixture.admin_token),
    )
    .await;
    let chart_id = created["publicApiId"]
        .as_str()
        .expect("chart id")
        .to_string();

    // Duplicate ranks are rejected without touching the stored list.
    let (status, body) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/charts/{chart_id}/items"),
        Some(serde_json::json!([
            {"rank": 1, "artistName": "A", "releaseTitle": "B"},
            {"rank": 1, "artistName": "C", "releaseTitle": "D"}
        ])),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation_error");

    // Unknown catalog ids are rejected as not-found.
    let (status, body) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/charts/{chart_id}/items"),
        Some(serde_json::json!([
            {"rank": 1, "artistName": "A", "releaseTitle": "B",
             "artistId": Uuid::now_v7().to_string()}
        ])),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "catalog_entity_not_found");

    // Link rank 1 to the seeded Artist/Release.
    let (status, body) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/charts/{chart_id}/items/1/link"),
        Some(serde_json::json!({
            "artistId": fixture.artist_public_id,
            "releaseId": fixture.release_public_id,
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["missingCount"], 1);

    // Ranked report carries the catalog linkage; the missing report only has
    // the unresolved item.
    let (status, ranked) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/charts/{chart_id}/ranked"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ranked = ranked.as_array().expect("ranked array");
    assert_eq!(ranked.len(), 2);
    assert_eq!(
        ranked[0]["linkedReleaseId"],
        fixture.release_public_id.to_string()
    );
    assert_eq!(ranked[0]["missing"], false);
    assert_eq!(ranked[1]["missing"], true);

    let (status, missing) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/charts/{chart_id}/missing"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let missing = missing.as_array().expect("missing array");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0]["rank"], 2);

    // Clearing the link restores the missing count.
    let (status, body) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/charts/{chart_id}/items/1/link"),
        Some(serde_json::json!({"artistId": null, "releaseId": null})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["missingCount"], 2);

    // Replacing the list is a full replacement.
    let (status, body) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/charts/{chart_id}/items"),
        Some(serde_json::json!([
            {"rank": 1, "artistName": "Only Item", "releaseTitle": "Only Release"}
        ])),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["itemCount"], 1);
    assert_eq!(body["items"][0]["artistName"], "Only Item");
    db.drop().await;
}

#[tokio::test]
async fn chart_validation_bounds_are_enforced() {
    let db = common::TestDb::create("admin_charts_validation").await;
    let fixture = fixture(&db).await;
    let app = router(db.pool.clone());
    for (label, body) in [
        ("empty slug", serde_json::json!({"slug": "", "title": "T"})),
        (
            "whitespace slug",
            serde_json::json!({"slug": "has space", "title": "T"}),
        ),
        (
            "empty title",
            serde_json::json!({"slug": "ok", "title": "  "}),
        ),
        (
            "too many tags",
            serde_json::json!({
                "slug": "ok", "title": "T",
                "tags": (0..51).map(|index| format!("t{index}")).collect::<Vec<_>>(),
            }),
        ),
        (
            "negative rank",
            serde_json::json!({
                "slug": "ok", "title": "T",
                "items": [{"rank": 0, "artistName": "A", "releaseTitle": "B"}]
            }),
        ),
    ] {
        let (status, response) = call(
            app.clone(),
            "POST",
            "/api/admin/v1/charts",
            Some(body),
            Some(&fixture.admin_token),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {response}");
    }
    db.drop().await;
}

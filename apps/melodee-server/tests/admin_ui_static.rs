//! Integration tests for admin SPA static serving (ADR-0138, Phase 10 WP-03).
//!
//! Proves: the built SPA is served at `/admin`, client-side deep links fall
//! back to `index.html`, the mount never shadows the API surface, and a
//! missing dist directory is not fatal (the server still serves APIs while
//! `/admin` 404s). Uses a temporary directory with a minimal `index.html` —
//! no database required.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use http_body_util::BodyExt;
use melodee_server::admin_ui::nest_admin_spa;
use tower::ServiceExt;

const INDEX_HTML: &str = "<!doctype html><html><body>melodee-admin-spa</body></html>";

/// A router shaped like the production composition: an API route plus the
/// SPA nested at `/admin` (when the dist directory exists).
fn app(dist: &std::path::Path) -> Router {
    nest_admin_spa(
        Router::new().route("/api/healthz", get(|| async { "ok" })),
        dist,
    )
}

async fn get_text(app: Router, path: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn make_dist() -> tempfile::TempDir {
    let dist = tempfile::tempdir().expect("tempdir");
    std::fs::write(dist.path().join("index.html"), INDEX_HTML).expect("index.html");
    std::fs::create_dir_all(dist.path().join("assets")).expect("assets dir");
    std::fs::write(dist.path().join("assets").join("app.js"), "console.log(1);").expect("app.js");
    dist
}

#[tokio::test]
async fn spa_shell_and_assets_are_served_under_admin() {
    let dist = make_dist();

    let (status, body) = get_text(app(dist.path()), "/admin").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX_HTML);

    let (status, body) = get_text(app(dist.path()), "/admin/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX_HTML);

    let (status, body) = get_text(app(dist.path()), "/admin/assets/app.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "console.log(1);");
}

#[tokio::test]
async fn deep_links_fall_back_to_index_html() {
    let dist = make_dist();
    let (status, body) = get_text(app(dist.path()), "/admin/some/deep/link").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX_HTML);

    let (status, body) = get_text(app(dist.path()), "/admin/processing/queue").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX_HTML);
}

#[tokio::test]
async fn admin_mount_does_not_shadow_api_routes() {
    let dist = make_dist();
    let (status, body) = get_text(app(dist.path()), "/api/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn missing_dist_is_not_fatal_and_admin_falls_through() {
    let missing =
        std::env::temp_dir().join(format!("melodee-missing-dist-{}", uuid::Uuid::now_v7()));
    assert!(!missing.exists());

    // The router still builds and APIs still serve.
    let (status, body) = get_text(app(&missing), "/api/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");

    // /admin falls through to the outer fallback: not found, not a panic.
    let (status, _) = get_text(app(&missing), "/admin").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get_text(app(&missing), "/admin/processing/queue").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

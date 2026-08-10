//! Focused route tests for distinct liveness and PostgreSQL readiness.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use http_body_util::BodyExt;
use melodee_server::system::{ReadinessState, healthz, readyz};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

async fn request_json(router: Router, path: &str) -> (StatusCode, Value) {
    let response = router
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test]
async fn healthz_is_liveness_only() {
    let router = Router::new().route("/healthz", get(healthz));
    let (status, body) = request_json(router, "/healthz").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["service"], "melodee-server");
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn readyz_returns_ready_when_postgres_answers() {
    let db = common::TestDb::create("server_readyz").await;
    let router = Router::new().route(
        "/readyz",
        get(readyz).with_state(ReadinessState::new(db.pool.clone())),
    );

    let (status, body) = request_json(router, "/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
    db.drop().await;
}

#[tokio::test]
async fn readyz_fails_closed_with_stable_body_when_pool_is_closed() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://melodee:secret@127.0.0.1:9/unreachable")
        .expect("syntactically valid database URL");
    pool.close().await;
    let router = Router::new().route(
        "/readyz",
        get(readyz).with_state(ReadinessState::new(pool).with_timeout(Duration::from_millis(50))),
    );

    let (status, body) = request_json(router, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        serde_json::json!({
            "service": "melodee-server",
            "version": env!("CARGO_PKG_VERSION"),
            "status": "not_ready"
        })
    );
    assert!(!body.to_string().contains("secret"));
}

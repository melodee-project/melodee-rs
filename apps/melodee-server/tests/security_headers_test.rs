//! Integration tests for security features.

#![allow(clippy::unwrap_used)]

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Request, header},
    middleware::Next,
    response::Response,
    routing::get,
};
use melodee_server::security_headers;
use tower::ServiceExt;

#[test]
fn admin_surface_scoping() {
    for path in [
        "/admin",
        "/admin/catalog/releases",
        "/api/admin/v1/system/health",
        "/api/admin/v1/work-items/123",
    ] {
        assert!(security_headers::is_admin_surface(path), "{path}");
    }

    for path in [
        "/api/v1/search",
        "/rest/getSong.view",
        "/Items/track",
        "/song/stream/track/user/token",
    ] {
        assert!(!security_headers::is_admin_surface(path), "{path}");
    }
}

async fn add_custom_header(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("X-Custom-Header", HeaderValue::from_static("test-value"));
    response
}

#[test]
fn header_policy_defensive_only() {
    let mut headers = HeaderMap::new();
    security_headers::apply_admin_headers(&mut headers);

    assert_eq!(
        headers.get(header::X_CONTENT_TYPE_OPTIONS),
        Some(&HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        headers.get(header::X_FRAME_OPTIONS),
        Some(&HeaderValue::from_static("DENY"))
    );
    assert_eq!(
        headers.get(header::REFERRER_POLICY),
        Some(&HeaderValue::from_static("no-referrer"))
    );

    let csp = headers.get(header::CONTENT_SECURITY_POLICY).unwrap();
    assert_eq!(
        *csp,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'"
        )
    );

    let pp = headers.get("permissions-policy").unwrap();
    assert_eq!(
        *pp,
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()")
    );
}

#[tokio::test]
async fn admin_routes_get_security_headers() {
    let app = Router::new()
        .route("/admin", get(|| async { "admin" }))
        .route("/api/admin/v1/system/info", get(|| async { "system" }))
        .route("/api/v1/ping", get(|| async { "compatibility" }))
        .route("/rest/getSong.view", get(|| async { "jellyfin" }))
        .route("/song/stream", get(|| async { "stream" }))
        .layer(axum::middleware::from_fn(
            melodee_server::security_headers::middleware,
        ));

    // Test admin route gets headers
    let mut request = Request::new(Body::empty());
    *request.uri_mut() = axum::http::Uri::from_static("/admin");
    let response = app.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers().get(header::X_FRAME_OPTIONS),
        Some(&HeaderValue::from_static("DENY"))
    );
    assert!(
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_some()
    );

    // Test admin API route gets headers
    let mut request = Request::new(Body::empty());
    *request.uri_mut() = axum::http::Uri::from_static("/api/admin/v1/system/info");
    let response = app.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers().get(header::X_FRAME_OPTIONS),
        Some(&HeaderValue::from_static("DENY"))
    );

    // Test compatibility routes do NOT get security headers
    let mut request = Request::new(Body::empty());
    *request.uri_mut() = axum::http::Uri::from_static("/api/v1/ping");
    let response = app.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(response.headers().get(header::X_FRAME_OPTIONS).is_none());

    let mut request = Request::new(Body::empty());
    *request.uri_mut() = axum::http::Uri::from_static("/rest/getSong.view");
    let response = app.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(response.headers().get(header::X_FRAME_OPTIONS).is_none());

    let mut request = Request::new(Body::empty());
    *request.uri_mut() = axum::http::Uri::from_static("/song/stream");
    let response = app.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(response.headers().get(header::X_FRAME_OPTIONS).is_none());
}
#[tokio::test]
async fn header_preserves_content_headers() {
    let app = Router::new()
        .route(
            "/admin/catalog/releases/123",
            get(|| async {
                axum::response::Json(serde_json::json!({
                    "title": "Test Release",
                    "trackCount": 10
                }))
            }),
        )
        .layer(axum::middleware::from_fn(
            melodee_server::security_headers::middleware,
        ));

    let mut request = Request::new(Body::empty());
    *request.uri_mut() = axum::http::Uri::from_static("/admin/catalog/releases/123");
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(response.headers().get(header::CONTENT_TYPE).is_some());
    assert!(response.headers().get(header::X_FRAME_OPTIONS).is_some());
    assert!(
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_some()
    );
}

#[tokio::test]
async fn header_application_order_independent() {
    let app = Router::new()
        .route("/admin", get(|| async { "admin" }))
        .layer(axum::middleware::from_fn(
            melodee_server::security_headers::middleware,
        ))
        .layer(axum::middleware::from_fn(add_custom_header));

    let mut request = Request::new(Body::empty());
    *request.uri_mut() = axum::http::Uri::from_static("/admin");
    let response = app.oneshot(request).await.unwrap();

    assert!(response.headers().get("X-Custom-Header").is_some());
    assert!(response.headers().get(header::X_FRAME_OPTIONS).is_some());
    assert!(
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_some()
    );
}

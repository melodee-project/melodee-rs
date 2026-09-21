//! Browser password-reset page security and accessibility evidence (MEL-3).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use melodee_server::password_reset;
use tower::ServiceExt;

async fn page(uri: &str, language: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(language) = language {
        builder = builder.header(header::ACCEPT_LANGUAGE, language);
    }
    password_reset::router()
        .oneshot(builder.body(Body::empty()).expect("page request"))
        .await
        .expect("page response")
}

#[tokio::test]
async fn emitted_reset_url_is_served_without_reflecting_the_token() {
    let token = "browser-round-trip-token";
    let response = page(
        &format!("/reset-password?token={token}"),
        Some("en-US,en;q=0.8"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL),
        Some(&axum::http::HeaderValue::from_static("no-store, private"))
    );
    assert_eq!(
        response.headers().get(header::REFERRER_POLICY),
        Some(&axum::http::HeaderValue::from_static("no-referrer"))
    );
    assert_eq!(
        response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
        Some(&axum::http::HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        response.headers().get(header::X_FRAME_OPTIONS),
        Some(&axum::http::HeaderValue::from_static("DENY"))
    );
    let csp = response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .expect("CSP");
    assert!(csp.contains("default-src 'none'"));
    assert!(csp.contains("connect-src 'self'"));
    assert!(csp.contains("form-action 'none'"));
    assert!(csp.contains("script-src 'nonce-"));

    let body = response
        .into_body()
        .collect()
        .await
        .expect("page body")
        .to_bytes();
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 page");
    assert!(body.contains("<form id=\"reset-form\""));
    assert!(body.contains("autocomplete=\"new-password\""));
    assert!(body.contains("aria-live=\"polite\""));
    assert!(body.contains("credentials: 'omit'"));
    assert!(body.contains("cache: 'no-store'"));
    assert!(body.contains("history.replaceState"));
    assert!(!body.contains(token));
    assert!(!body.contains("<img"));
    assert!(!body.contains("<link"));
    assert!(!body.contains("localStorage"));
    assert!(!body.contains("sessionStorage"));
    assert!(!body.contains("http://"));
    assert!(!body.contains("https://"));
}

#[tokio::test]
async fn missing_and_malformed_links_get_the_same_bounded_page() {
    for uri in [
        "/reset-password",
        "/reset-password?token=not%20a%20reset%20token",
        "/reset-password?token=too-long-token-with-invalid-punctuation%21",
    ] {
        let response = page(uri, Some("en-US")).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&axum::http::HeaderValue::from_static("no-store, private")),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn supported_rtl_locale_is_localized_and_has_no_cacheable_token_surface() {
    let response = page("/reset-password?token=opaque", Some("ar-SA")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("page body")
        .to_bytes();
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 page");
    assert!(body.contains("<html lang=\"ar-SA\" dir=\"rtl\">"));
    assert!(body.contains("إعادة تعيين كلمة المرور"));
    assert!(!body.contains("opaque"));
}

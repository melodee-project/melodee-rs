//! Opt-in browser transport for OpenSubsonic only (ADR-0180).

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, Method, header},
    middleware::{self, Next},
    response::Response,
};
use tower_http::cors::CorsLayer;

pub const ENV_ALLOWED_ORIGINS: &str = "MELODEE__OPENSUBSONIC__CORS__ALLOWED_ORIGINS";

#[derive(Debug, thiserror::Error)]
#[error(
    "{ENV_ALLOWED_ORIGINS} must contain at most 100 exact HTTP(S) origins (no wildcard, credentials, trailing slash, path, query, or fragment)"
)]
pub struct CorsConfigError;

fn policy(raw: &str) -> Result<CorsLayer, CorsConfigError> {
    if raw.len() > 32 * 1024 {
        return Err(CorsConfigError);
    }
    let mut origins = Vec::new();
    for origin in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if origin.len() > 1024 || origin.ends_with('/') || origin.contains(['*', '#', '@']) {
            return Err(CorsConfigError);
        }
        let url = reqwest::Url::parse(origin).map_err(|_| CorsConfigError)?;
        // Compare the browser's serialized origin, so invalid ports and
        // non-origin URL components cannot silently become inert entries.
        if !matches!(url.scheme(), "http" | "https") || url.origin().ascii_serialization() != origin
        {
            return Err(CorsConfigError);
        }
        origins.push(HeaderValue::from_str(origin).map_err(|_| CorsConfigError)?);
    }
    if origins.len() > 100 {
        return Err(CorsConfigError);
    }
    Ok(CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::HEAD, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::RANGE,
            header::IF_NONE_MATCH,
            header::IF_MODIFIED_SINCE,
        ])
        .expose_headers([
            header::ACCEPT_RANGES,
            header::CONTENT_RANGE,
            header::CONTENT_LENGTH,
            header::CONTENT_DISPOSITION,
            header::ETAG,
            header::LAST_MODIFIED,
            header::HeaderName::from_static("content-duration"),
            header::HeaderName::from_static("x-total-count"),
        ]))
}

async fn browser_stream_headers(request: Request, next: Next) -> Response {
    let browser = request.headers().contains_key(header::ORIGIN);
    let mut response = next.run(request).await;
    if browser {
        // Legacy streams emit '*'. Browser requests must use the explicit
        // outer policy, including when their origin is absent from its list.
        // Non-browser captured stream responses retain their existing headers.
        response
            .headers_mut()
            .remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    }
    response
}

pub(super) fn apply(router: Router, origins: &str) -> Result<Router, CorsConfigError> {
    Ok(router
        .layer(middleware::from_fn(browser_stream_headers))
        // Outermost: preflight never enters query parsing or authentication.
        .layer(policy(origins)?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use axum::{body::Body, routing::get};
    use tower::ServiceExt;

    #[test]
    fn origin_configuration_is_bounded_and_exact() {
        for origin in [
            "*",
            "null",
            "https://*.example",
            "https://user:pass@example.test",
            "https://example.test/",
            "https://example.test/path",
            "https://example.test?q=x",
            "https://example.test#fragment",
            "file://example.test",
            "https://",
            "https://:8080",
            "https://example.test:invalid",
        ] {
            assert!(policy(origin).is_err(), "accepted invalid origin: {origin}");
        }
        assert!(policy(&"x".repeat(32 * 1024 + 1)).is_err());
        assert!(policy(&["https://example.test"; 101].join(",")).is_err());
        assert!(
            policy("http://localhost:3000, https://feishin.vercel.app, http://[::1]:3000").is_ok()
        );
        assert!(policy("").is_ok());
    }

    #[tokio::test]
    async fn browser_policy_overrides_stream_wildcard_without_changing_non_browser_headers() {
        let router = || {
            Router::new().route(
                "/rest/stream",
                get(|| async { ([(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")], "audio") }),
            )
        };
        for configured in ["", "https://feishin.vercel.app"] {
            let app = apply(router(), configured).expect("policy");
            for origin in [
                None,
                Some("https://untrusted.example"),
                Some("https://feishin.vercel.app"),
            ] {
                let mut request = Request::builder().uri("/rest/stream");
                if let Some(origin) = origin {
                    request = request.header(header::ORIGIN, origin);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::empty()).expect("request"))
                    .await
                    .expect("response");
                let expected = match origin {
                    None => Some("*"),
                    Some(value) if value == configured => Some(value),
                    _ => None,
                };
                assert_eq!(
                    response
                        .headers()
                        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                        .map(|value| value.to_str().expect("origin")),
                    expected
                );
                assert!(
                    !response
                        .headers()
                        .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                );
            }
        }
    }
}

//! Security headers for the browser-facing administration surface.
//!
//! Native v1, OpenSubsonic, and Jellyfin responses are frozen compatibility
//! products. Their headers therefore remain untouched here; the new Admin API
//! and SPA have their own browser security policy and can adopt defensive
//! headers without changing those captured contracts.

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Request, header},
    middleware::Next,
    response::Response,
};

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'";

/// Returns whether a request belongs to the browser-facing administration
/// surface. The exact prefix keeps compatibility adapters and media responses
/// outside this policy.
#[must_use]
pub fn is_admin_surface(path: &str) -> bool {
    path == "/admin"
        || path.starts_with("/admin/")
        || path == "/api/admin/v1"
        || path.starts_with("/api/admin/v1/")
}

/// Applies the admin browser policy to a response without changing any
/// endpoint-specific cache or content headers.
pub fn apply_admin_headers(headers: &mut HeaderMap) {
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
}

/// Adds browser security headers only to Admin API and Admin SPA responses.
pub async fn middleware(request: Request<Body>, next: Next) -> Response {
    let admin_surface = is_admin_surface(request.uri().path());
    let mut response = next.run(request).await;
    if admin_surface {
        apply_admin_headers(response.headers_mut());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use tower::ServiceExt;

    #[test]
    fn admin_paths_are_scoped_without_touching_compatibility_products() {
        assert!(is_admin_surface("/admin"));
        assert!(is_admin_surface("/admin/catalog/releases"));
        assert!(is_admin_surface("/api/admin/v1/system/health"));
        assert!(!is_admin_surface("/api/v1/search"));
        assert!(!is_admin_surface("/rest/getSong.view"));
        assert!(!is_admin_surface("/Items/track"));
        assert!(!is_admin_surface("/song/stream/track/user/token"));
    }

    #[test]
    fn policy_sets_only_defensive_browser_headers() {
        let mut headers = HeaderMap::new();
        apply_admin_headers(&mut headers);
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(
            headers["permissions-policy"],
            "camera=(), geolocation=(), microphone=()"
        );
        assert!(
            headers[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .is_ok_and(|value| value.contains("object-src 'none'"))
        );
    }

    #[tokio::test]
    async fn middleware_applies_policy_to_admin_responses_only() {
        let app = Router::new()
            .route("/admin", get(|| async { "admin" }))
            .route("/api/v1/ping", get(|| async { "compatibility" }))
            .layer(axum::middleware::from_fn(middleware));

        let mut admin_request = Request::new(Body::empty());
        *admin_request.uri_mut() = axum::http::Uri::from_static("/admin");
        let admin = app
            .clone()
            .oneshot(admin_request)
            .await
            .unwrap_or_else(|infallible| match infallible {});
        assert_eq!(admin.headers()[header::X_FRAME_OPTIONS], "DENY");

        let mut compatibility_request = Request::new(Body::empty());
        *compatibility_request.uri_mut() = axum::http::Uri::from_static("/api/v1/ping");
        let compatibility = app
            .oneshot(compatibility_request)
            .await
            .unwrap_or_else(|infallible| match infallible {});
        assert!(
            !compatibility
                .headers()
                .contains_key(header::X_FRAME_OPTIONS)
        );
    }
}

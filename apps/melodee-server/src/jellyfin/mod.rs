//! Jellyfin compatibility adapter (ADR-0102, ADR-0131, PRD-API-JF-001..006).
//!
//! A music-focused subset, not a complete Jellyfin server. Protocol-owned
//! DTOs (PascalCase Jellyfin shapes) stay inside this module; the core domain
//! keeps Release/Track. No wire DTO is shared with native-v1 or OpenSubsonic.

pub mod auth;
pub mod discovery;
pub mod error;

use std::sync::Arc;

use axum::{Router, extract::Request, middleware::Next, response::Response};

/// Shared state for the Jellyfin adapter.
#[derive(Clone)]
pub struct JfState {
    pub pool: sqlx::PgPool,
    pub server_name: String,
    pub server_version: String,
    pub server_id: String,
    pub local_address: String,
    pub public_origin: String,
    /// HMAC-SHA256 pepper for Jellyfin access token digests (ADR-0131).
    /// Required when the adapter is enabled; never logged or serialized.
    pub token_pepper: SecretPepper,
    pub token_expiry_hours: i64,
    pub max_active_tokens: i32,
    pub direct_play: Option<crate::native_stream::DirectPlayService>,
    pub client_addresses: crate::native_stream::ClientAddressResolver,
    pub search: Option<melodee_search::SearchConfig>,
}

/// Wraps the token pepper so it is never accidentally logged or serialized.
#[derive(Clone)]
pub struct SecretPepper(Arc<Vec<u8>>);

impl SecretPepper {
    #[must_use]
    pub fn new(pepper: Vec<u8>) -> Self {
        assert!(
            pepper.len() >= 32,
            "Jellyfin token pepper must be >= 32 bytes"
        );
        Self(Arc::new(pepper))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretPepper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretPepper(<redacted>)")
    }
}

/// Jellyfin path prefixes that, combined with an `api_key` query param, trigger
/// a marker rewrite.
const JELLYFIN_PREFIXES: &[&str] = &[
    "/Audio/",
    "/Items/",
    "/Users/",
    "/UserViews",
    "/System/",
    "/Sessions/",
    "/Artists/",
    "/Playlists/",
    "/Images/",
];

fn is_jellyfin_marker(request: &Request) -> bool {
    let auth = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .or_else(|| request.headers().get("X-Emby-Authorization"));
    if auth
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("MediaBrowser"))
    {
        return true;
    }
    if request.headers().contains_key("X-MediaBrowser-Token")
        || request.headers().contains_key("X-Emby-Token")
    {
        return true;
    }
    let path = request.uri().path();
    if (path.contains("api_key=")
        || request
            .uri()
            .query()
            .is_some_and(|q| q.contains("api_key=")))
        && JELLYFIN_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
    {
        return true;
    }
    false
}

/// The captured JellyfinRoutingMiddleware: detected requests are rewritten to
/// `/api/jf{path}` before routing. Allowlisted paths rewrite regardless of
/// headers; other paths rewrite only when a marker is present. `/api/`,
/// `/rest/`, `/song/` prefixes always pass through. `GET/HEAD /` rewrites only
/// The captured JellyfinRoutingMiddleware: allowlisted pre-auth paths and
/// all registered Jellyfin routes are always accessible (the handler enforces
/// auth with 401 for missing tokens). Only the server root `/` is gated by a
/// marker (a no-marker root falls through to the admin SPA). Non-Jellyfin
/// paths naturally 404 from the router fallback.
pub async fn routing_middleware(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();

    // Pass-through prefixes never reach Jellyfin controllers.
    if path.starts_with("/api/") || path.starts_with("/rest/") || path.starts_with("/song/") {
        return next.run(request).await;
    }

    // The server root is only a Jellyfin request when a marker is present;
    // otherwise it falls through to the admin SPA.
    if path == "/" && !is_jellyfin_marker(&request) {
        return next.run(request).await;
    }

    next.run(request).await
}

/// Builds the Jellyfin router with the routing middleware applied. The
/// canonical `/api/jf/*` routes are mounted; the middleware rewrites detected
/// root-form requests to canonical paths before routing.
pub fn configured_router(state: JfState) -> Router {
    Router::new()
        .merge(discovery::router())
        .merge(auth::router())
        .layer(axum::middleware::from_fn(routing_middleware))
        .with_state(state)
}

//! Jellyfin adapter (PRD-API-JF-001..005).
//!
//! Scope locked by SPEC §7.2/§7.5/§10 and ADR-0102: a music-focused Jellyfin
//! surface. It does not become a general Emby/Jellyfin media server, does not
//! add video, does not invent libraries Melodee does not manage, and does not
//! claim full ecosystem support. Served endpoint classes are exactly the ones
//! pinned by the frozen Jellyfin captures (routing/discovery, auth,
//! browse/playlists/images/userstate/streaming contract surfaces) plus the
//! documented intentional gaps and rejects.
//!
//! Protocol-owned PascalCase DTOs, N-format GUIDs, `yyyy-MM-ddTHH:mm:ss.fffffffZ`
//! dates, null-serialization, envelope and ETag behavior stay inside this adapter
//! and are not shared with OpenSubsonic or native-v1 (ADR-0102). Item/collection
//! ETags use the exact legacy MD5 preimages from the frozen baseline so cache
//! identifiers are byte-identical at fixed data.
//!
//! Routing and auth placement are governed by ADR-0131 and the frozen route
//! inventory (`compatibility/jellyfin/contracts/route-inventory.json`): routes
//! are mounted in both
//! root form (`/System/Info`, `/Items/...`) and canonical form (`/api/jf/...`),
//! matching the frozen middleware that rewrote detected Jellyfin requests to
//! `/api/jf{path}`; the routing boundary itself is captured-contract, not
//! implementation detail.

pub mod audio;
pub mod auth;
pub mod browse;
pub mod discovery;
pub mod error;
pub mod images;
pub mod model;
pub mod playlists;
pub mod sessions;
pub mod userstate;

use std::sync::Arc;

use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use time::OffsetDateTime;
use uuid::Uuid;

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
    /// Unlocked Storage libraries exposed as UserViews. The Rust catalog has
    /// no library table (unlike the legacy fixture), so libraries are server
    /// configuration — the same pattern as OpenSubsonic music folders.
    pub libraries: Vec<Arc<JfLibrary>>,
}

/// One unlocked Storage library exposed as a Jellyfin `UserView`.
#[derive(Debug, Clone)]
pub struct JfLibrary {
    pub id: Uuid,
    pub name: String,
    pub created_at: OffsetDateTime,
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

/// Jellyfin path prefixes that, combined with an `api_key` query param, count
/// as a Jellyfin marker.
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
    if request
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|part| part.starts_with("api_key=")))
        && (path == "/"
            || JELLYFIN_PREFIXES
                .iter()
                .any(|prefix| path.starts_with(prefix)))
    {
        return true;
    }
    false
}

/// The captured JellyfinRoutingMiddleware semantics (ADR-0131): the frozen
/// middleware rewrote detected root-form requests to `/api/jf{path}`; this
/// adapter mounts both path forms directly, which is behaviorally identical
/// for clients. The one case still gated here is the root probe: `/` with a
/// Jellyfin marker returns the captured 200 empty body (RootController); `/`
/// without a marker falls through to the non-Jellyfin surface.
///
/// Because axum route-local layers only run for matched routes, this
/// middleware is applied at the outer router in `main.rs` so the bare-root
/// probe works exactly as captured.
pub async fn routing_middleware(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();

    // Pass-through prefixes never reach Jellyfin controllers.
    if path.starts_with("/api/") || path.starts_with("/rest/") || path.starts_with("/song/") {
        return next.run(request).await;
    }

    // The server root is a Jellyfin request (captured 200 empty) only when a
    // marker is present; otherwise it falls through to the outer surface.
    if path == "/" {
        if is_jellyfin_marker(&request) {
            return StatusCode::OK.into_response();
        }
        return next.run(request).await;
    }

    next.run(request).await
}

/// Static Jellyfin route segments in canonical (PascalCase) form. The legacy
/// ASP.NET router matched templates case-insensitively while preserving the
/// captured parameter values — real clients (Feishin) call lowercase paths
/// like `/users/authenticatebyname`, which a case-sensitive router 404s.
const CANONICAL_SEGMENTS: &[&str] = &[
    "AlbumArtists",
    "Artists",
    "Audio",
    "AuthenticateByName",
    "Capabilities",
    "Download",
    "FavoriteItems",
    "File",
    "Filters",
    "Full",
    "Genres",
    "Images",
    "Info",
    "InstantMix",
    "Items",
    "Login",
    "Logout",
    "Me",
    "Move",
    "MusicGenres",
    "Ping",
    "PlayedItems",
    "PlaybackInfo",
    "Playing",
    "Playlists",
    "Progress",
    "Public",
    "Refresh",
    "Sessions",
    "Similar",
    "Songs",
    "Stopped",
    "System",
    "UserViews",
    "Users",
    "Views",
];

/// Case-insensitively canonicalizes the static segments of a Jellyfin path,
/// leaving dynamic values (ids, image types, resource names) untouched.
/// Segments that match no template segment pass through unchanged. Returns
/// `None` when the path is already canonical.
fn canonicalized_uri(uri: &axum::http::Uri) -> Option<axum::http::Uri> {
    let path = uri.path();
    let canonical: Vec<&str> = path
        .split('/')
        .map(|segment| {
            CANONICAL_SEGMENTS
                .iter()
                .copied()
                .find(|canonical| segment.eq_ignore_ascii_case(canonical))
                .unwrap_or(segment)
        })
        .collect();
    let canonical_path = canonical.join("/");
    if canonical_path == path {
        return None;
    }
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    format!("{canonical_path}{query}").parse().ok()
}

/// Case-insensitive route matching fallback (real-client compatibility).
///
/// The legacy ASP.NET router matched templates case-insensitively while
/// preserving captured parameter values; real Jellyfin clients (Feishin) call
/// lowercase paths like `/users/authenticatebyname`. axum route layers never
/// run for unmatched paths, so this fallback — installed on the outer router
/// in `main.rs` — canonicalizes the static segments of paths under the
/// Jellyfin-owned roots and re-dispatches into the app. Paths that do not
/// canonicalize keep the captured behavior (404, empty body).
///
/// The JF-owned roots below never collide with native-v1 (`/api/v1/...`),
/// OpenSubsonic (`/rest/...`, `/song/...`), or admin (`/api/admin/...`)
/// prefixes, which stay case-sensitive as captured.
const JF_CASE_ROOTS: &[&str] = &[
    "/system/",
    "/users/",
    "/userviews",
    "/items/",
    "/audio/",
    "/artists",
    "/genres",
    "/musicgenres",
    "/playlists",
    "/sessions",
    "/songs/",
    "/images/",
    "/api/jf",
];

/// Builds the fallback service: canonicalize-and-replay for Jellyfin-cased
/// paths, captured empty 404 otherwise.
pub fn case_rewriting_fallback(
    app: Router,
) -> impl tower::Service<
    Request,
    Response = Response,
    Error = std::convert::Infallible,
    Future = impl std::future::Future<Output = Result<Response, std::convert::Infallible>>,
> + Clone
+ Send
+ 'static {
    tower::service_fn(move |request: Request| {
        let app = app.clone();
        async move {
            let path_is_root = request.uri().path() == "/";
            let is_jf = {
                let path = request.uri().path();
                let lower = path.to_ascii_lowercase();
                JF_CASE_ROOTS
                    .iter()
                    .any(|root| lower == *root || lower.starts_with(root))
            };
            if is_jf && let Some(uri) = canonicalized_uri(request.uri()) {
                let mut request = request;
                *request.uri_mut() = uri;
                let response = tower::ServiceExt::oneshot(app, request)
                    .await
                    .unwrap_or_else(|never| match never {});
                return Ok(response);
            }
            // The bare root replays unchanged so the Jellyfin routing
            // middleware (which wrapped the previous default fallback) still
            // decides the captured marker probe: marker → 200 empty, no
            // marker → fall through to the non-Jellyfin surface.
            if path_is_root {
                let response = tower::ServiceExt::oneshot(app, request)
                    .await
                    .unwrap_or_else(|never| match never {});
                return Ok(response);
            }
            Ok(StatusCode::NOT_FOUND.into_response())
        }
    })
}

/// Builds the Jellyfin router: all routes mounted in root form and canonical
/// `/api/jf` form, plus the canonical root probe (`/api/jf`, `/api/jf/`).
pub fn configured_router(state: JfState) -> Router {
    let routes = Router::<JfState>::new()
        .merge(discovery::router())
        .merge(auth::router())
        .merge(browse::router())
        .merge(audio::router())
        .merge(images::router())
        .merge(playlists::router())
        .merge(sessions::router())
        .merge(userstate::router());
    Router::new()
        .merge(routes.clone())
        .nest("/api/jf", routes)
        .route("/api/jf", axum::routing::get(server_root).head(server_root))
        .route(
            "/api/jf/",
            axum::routing::get(server_root).head(server_root),
        )
        .with_state(state)
}

/// Captured `RootController.ServerRoot`: 200 with `Content-Length: 0`, no
/// auth (GET and HEAD).
async fn server_root() -> StatusCode {
    StatusCode::OK
}

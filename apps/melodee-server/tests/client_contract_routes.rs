//! Client contract guard for the two production listener clients.
//!
//! `melodee-player` (Android / Android Auto) and `meloamp` (desktop) call a
//! concrete subset of native-v1 routes. Losing or renaming one of them is an
//! invisible breaking change in the compatibility suites (they replay frozen
//! goldens rather than the live client call sites), so this test asserts that
//! every route those clients actually use is still registered.
//!
//! The assertion is "the router owns this method+path", expressed as "the
//! response is not a 404/405 routing miss". Authorization outcomes (401), body
//! validation (400/422), or public responses are all acceptable — the route
//! exists and its contract is exercised elsewhere.
//!
//! Source of the list (reviewed 2026-10):
//! - `melodee-player/src/app/src/main/java/com/melodee/autoplayer/data` Retrofit
//!   interfaces (`@GET`/`@POST` annotations).
//! - `meloamp/src/ui/src/api.ts` and page-level `api.get/post/put` call sites.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use melodee_server::native::NativeState;
use melodee_server::native::auth::NativeAuthService;
use tower::ServiceExt;
use uuid::Uuid;

fn native_state(db: &common::TestDb) -> NativeState {
    NativeState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            db.pool.clone(),
        )),
        search: None,
        public_origin: "http://melodee.test".to_string(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(UnconfiguredSigner),
        user_state: Arc::new(melodee_server::native::PostgresNativeUserStateReader::new(
            db.pool.clone(),
        )),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            NativeAuthService::new(
                "client-contract-jwt-key-that-is-at-least-32b".to_string(),
                90,
            )
            .expect("auth config"),
        ),
    }
}

struct UnconfiguredSigner;

impl melodee_server::native::NativeStreamUrlSigner for UnconfiguredSigner {
    fn stream_url(
        &self,
        _track_public_id: Uuid,
        _user_public_id: Uuid,
        _client_binding: &str,
    ) -> melodee_server::native::NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Err(()) })
    }
}

/// (method, path template). `{id}` is replaced with a fresh UUID; other
/// placeholders use concrete values the frozen contracts accept.
const CLIENT_ROUTES: &[(&str, &str)] = &[
    // Authentication/session (both clients).
    ("POST", "/api/v1/auth/authenticate"),
    ("POST", "/api/v1/auth/refresh-token"),
    ("POST", "/api/v1/auth/refresh"),
    ("POST", "/api/v1/auth/logout"),
    ("POST", "/api/v1/auth/revoke"),
    ("POST", "/api/v1/auth/password-reset/request"),
    ("POST", "/api/v1/auth/password-reset/confirm"),
    // Identity + system info (both clients).
    ("GET", "/api/v1/user/me"),
    ("GET", "/api/v1/system/info"),
    ("GET", "/api/v1/system/stats"),
    // Android Auto player browse/play (melodee-player Retrofit interfaces).
    ("GET", "/api/v1/artists"),
    ("GET", "/api/v1/artists/{id}"),
    ("GET", "/api/v1/artists/{id}/albums"),
    ("GET", "/api/v1/artists/{id}/songs"),
    ("GET", "/api/v1/albums"),
    ("GET", "/api/v1/albums/{id}"),
    ("GET", "/api/v1/albums/{id}/songs"),
    ("GET", "/api/v1/songs"),
    ("GET", "/api/v1/songs/{id}"),
    // Stream URLs are issued by the server and use the frozen path-token
    // contract (the clients play whatever `streamUrl` the API returns).
    ("GET", "/song/stream/{id}/{id}/token"),
    ("GET", "/api/v1/search/songs"),
    ("GET", "/api/v1/user/playlists"),
    ("GET", "/api/v1/playlists/{id}/songs"),
    ("POST", "/api/v1/scrobble"),
    ("POST", "/api/v1/songs/starred/{id}/true"),
    ("POST", "/api/v1/songs/starred/{id}/false"),
    // MeloAmp desktop client.
    ("GET", "/api/v1/genres"),
    ("GET", "/api/v1/search"),
    ("GET", "/api/v1/analytics/listening"),
    ("GET", "/api/v1/analytics/top/week"),
    ("GET", "/api/v1/playlists"),
    ("POST", "/api/v1/playlists"),
    ("PUT", "/api/v1/playlists/{id}"),
    ("PUT", "/api/v1/playlists/{id}/songs/reorder"),
    ("POST", "/api/v1/playlists/{id}/image"),
    ("GET", "/api/v1/songs/starred"),
    ("GET", "/api/v1/songs/hated"),
    ("POST", "/api/v1/songs/hated/{id}/false"),
    ("POST", "/api/v1/songs/setrating/{id}/4"),
    ("POST", "/api/v1/albums/starred/{id}/true"),
    ("POST", "/api/v1/albums/hated/{id}/false"),
    ("POST", "/api/v1/albums/setrating/{id}/5"),
    ("POST", "/api/v1/artists/starred/{id}/true"),
    ("POST", "/api/v1/artists/hated/{id}/false"),
    ("GET", "/api/v1/themes"),
    ("GET", "/api/v1/themes/me"),
    ("GET", "/api/v1/user/stats"),
    ("GET", "/api/v1/user/last-played"),
    ("GET", "/api/v1/equalizer/presets"),
    ("GET", "/api/v1/recommendations"),
    ("GET", "/api/v1/charts"),
    ("GET", "/api/v1/charts/{id}"),
    ("GET", "/api/v1/requests"),
];

#[tokio::test]
async fn every_route_used_by_the_shipped_clients_is_still_registered() {
    let db = common::TestDb::create("client_contract_routes").await;
    let app = melodee_server::native::configured_router(native_state(&db));
    let id = Uuid::now_v7().to_string();
    let mut missing = Vec::new();
    for (method, template) in CLIENT_ROUTES {
        let path = template.replace("{id}", &id);
        let has_body = *method == "POST" || *method == "PUT";
        let mut builder = Request::builder().method(*method).uri(&path);
        if has_body {
            builder = builder
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json");
        }
        let body = if has_body {
            Body::from("{}")
        } else {
            Body::empty()
        };
        let response = app
            .clone()
            .oneshot(builder.body(body).expect("request"))
            .await
            .expect("response");
        if matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) {
            missing.push(format!("{method} {path} -> {}", response.status()));
        }
    }
    assert!(
        missing.is_empty(),
        "client-required routes are no longer registered: {missing:#?}"
    );
    db.drop().await;
}

//! Native Melodee API v1 compatibility adapter (ADR-0102).

pub mod auth;
pub mod avatars;
pub mod catalog;
pub mod community;
pub mod discovery;
pub mod dto;
pub mod endpoints;
pub mod error;
pub mod images;
pub mod playlists;
pub mod podcasts;
pub mod preferences;
pub mod stream;

use std::{future::Future, pin::Pin, sync::Arc};

use axum::{
    Json, Router,
    extract::{FromRequestParts, Request},
    http::request::Parts,
    http::{HeaderValue, header::CONTENT_TYPE},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use uuid::Uuid;

use self::{dto::ServerInfoDto, error::unauthorized};

pub type NativeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Authenticated identity installed by the native auth layer. Both IDs are
/// required: internal identity is used for joins; the migrated public ID is
/// used in native URLs and DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativePrincipal {
    pub user_id: Uuid,
    pub public_api_id: Uuid,
    pub is_admin: bool,
    pub is_editor: bool,
}

/// Trusted-proxy-resolved client binding installed by the native request
/// security layer. Raw forwarding headers are never read by catalog handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeClientBinding(pub String);

/// Synchronous URL signer boundary used by catalog DTO assembly. The stream
/// implementation owns the HMAC secret; catalog handlers never receive it.
pub trait NativeStreamUrlSigner: Send + Sync {
    fn stream_url(
        &self,
        track_public_id: Uuid,
        user_public_id: Uuid,
        client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>>;

    fn stream_urls(
        &self,
        track_public_ids: Vec<Uuid>,
        user_public_id: Uuid,
        client_binding: &str,
    ) -> NativeFuture<'_, Result<Vec<String>, ()>> {
        let client_binding = client_binding.to_owned();
        Box::pin(async move {
            let mut urls = Vec::with_capacity(track_public_ids.len());
            for track_public_id in track_public_ids {
                urls.push(
                    self.stream_url(track_public_id, user_public_id, &client_binding)
                        .await?,
                );
            }
            Ok(urls)
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeAnnotation {
    pub starred: bool,
    pub rating: i32,
    pub play_count: i32,
}

/// User annotation boundary. Native user-state persistence supplies the implementation;
/// catalog responses must not manufacture zeros for migrated user state.
pub trait NativeUserStateReader: Send + Sync {
    fn artist_annotation(
        &self,
        user_id: Uuid,
        artist_id: Uuid,
    ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>>;

    fn release_annotation(
        &self,
        user_id: Uuid,
        release_id: Uuid,
    ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>>;

    fn track_annotation(
        &self,
        user_id: Uuid,
        track_id: Uuid,
    ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>>;

    fn artist_annotations(
        &self,
        user_id: Uuid,
        artist_ids: Vec<Uuid>,
    ) -> NativeFuture<'_, Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError>> {
        Box::pin(async move {
            let mut annotations = Vec::with_capacity(artist_ids.len());
            for artist_id in artist_ids {
                annotations.push((artist_id, self.artist_annotation(user_id, artist_id).await?));
            }
            Ok(annotations)
        })
    }

    fn release_annotations(
        &self,
        user_id: Uuid,
        release_ids: Vec<Uuid>,
    ) -> NativeFuture<'_, Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError>> {
        Box::pin(async move {
            let mut annotations = Vec::with_capacity(release_ids.len());
            for release_id in release_ids {
                annotations.push((
                    release_id,
                    self.release_annotation(user_id, release_id).await?,
                ));
            }
            Ok(annotations)
        })
    }

    fn track_annotations(
        &self,
        user_id: Uuid,
        track_ids: Vec<Uuid>,
    ) -> NativeFuture<'_, Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError>> {
        Box::pin(async move {
            let mut annotations = Vec::with_capacity(track_ids.len());
            for track_id in track_ids {
                annotations.push((track_id, self.track_annotation(user_id, track_id).await?));
            }
            Ok(annotations)
        })
    }
}

/// PostgreSQL implementation backed by the native user-state tables.
#[derive(Clone)]
pub struct PostgresNativeUserStateReader {
    pool: sqlx::PgPool,
}

impl PostgresNativeUserStateReader {
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

async fn load_annotation(
    pool: &sqlx::PgPool,
    table: &'static str,
    entity_column: &'static str,
    user_id: Uuid,
    entity_id: Uuid,
) -> Result<NativeAnnotation, error::NativeApiError> {
    let query = sqlx::AssertSqlSafe(format!(
        "SELECT starred, rating::integer, play_count FROM {table} WHERE user_id = $1 AND {entity_column} = $2"
    ));
    let row = sqlx::query_as::<_, (bool, i32, i64)>(query)
        .bind(user_id)
        .bind(entity_id)
        .fetch_optional(pool)
        .await?;
    match row {
        Some((starred, rating, play_count)) => Ok(NativeAnnotation {
            starred,
            rating,
            play_count: i32::try_from(play_count)
                .map_err(|_| error::NativeApiError::Unavailable)?,
        }),
        None => Ok(NativeAnnotation::default()),
    }
}

impl NativeUserStateReader for PostgresNativeUserStateReader {
    fn artist_annotation(
        &self,
        user_id: Uuid,
        artist_id: Uuid,
    ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>> {
        Box::pin(load_annotation(
            &self.pool,
            "native_artist_user_states",
            "artist_id",
            user_id,
            artist_id,
        ))
    }

    fn release_annotation(
        &self,
        user_id: Uuid,
        release_id: Uuid,
    ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>> {
        Box::pin(load_annotation(
            &self.pool,
            "native_release_user_states",
            "release_id",
            user_id,
            release_id,
        ))
    }

    fn track_annotation(
        &self,
        user_id: Uuid,
        track_id: Uuid,
    ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>> {
        Box::pin(load_track_annotation(&self.pool, user_id, track_id))
    }

    fn artist_annotations(
        &self,
        user_id: Uuid,
        artist_ids: Vec<Uuid>,
    ) -> NativeFuture<'_, Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError>> {
        Box::pin(load_annotations(
            &self.pool,
            "native_artist_user_states",
            "artist_id",
            user_id,
            artist_ids,
        ))
    }

    fn release_annotations(
        &self,
        user_id: Uuid,
        release_ids: Vec<Uuid>,
    ) -> NativeFuture<'_, Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError>> {
        Box::pin(load_annotations(
            &self.pool,
            "native_release_user_states",
            "release_id",
            user_id,
            release_ids,
        ))
    }

    fn track_annotations(
        &self,
        user_id: Uuid,
        track_ids: Vec<Uuid>,
    ) -> NativeFuture<'_, Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError>> {
        Box::pin(load_track_annotations(&self.pool, user_id, track_ids))
    }
}

/// Native-v1's `Song.playCount` is the legacy global count of play facts.
/// Star/rating state remains user-scoped, so it must be joined separately from
/// the immutable play ledger rather than read from the denormalized user-state
/// counter. The latter is used for user-scoped list and statistics queries.
async fn load_track_annotation(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    track_id: Uuid,
) -> Result<NativeAnnotation, error::NativeApiError> {
    let row = sqlx::query_as::<_, (bool, i32, i64)>(
        "SELECT COALESCE(state.starred, false),
                COALESCE(state.rating, 0)::integer,
                (SELECT count(*) FROM native_track_plays play
                 WHERE play.track_id = track.id)
         FROM tracks track
         LEFT JOIN native_track_user_states state
           ON state.track_id = track.id AND state.user_id = $1
         WHERE track.id = $2",
    )
    .bind(user_id)
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((starred, rating, play_count)) => Ok(NativeAnnotation {
            starred,
            rating,
            play_count: i32::try_from(play_count)
                .map_err(|_| error::NativeApiError::Unavailable)?,
        }),
        None => Ok(NativeAnnotation::default()),
    }
}

async fn load_track_annotations(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    track_ids: Vec<Uuid>,
) -> Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, bool, i32, i64)>(
        "SELECT track.id,
                COALESCE(state.starred, false),
                COALESCE(state.rating, 0)::integer,
                (SELECT count(*) FROM native_track_plays play
                 WHERE play.track_id = track.id)
         FROM tracks track
         LEFT JOIN native_track_user_states state
           ON state.track_id = track.id AND state.user_id = $1
         WHERE track.id = ANY($2::uuid[])",
    )
    .bind(user_id)
    .bind(track_ids)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|(track_id, starred, rating, play_count)| {
            Ok((
                track_id,
                NativeAnnotation {
                    starred,
                    rating,
                    play_count: i32::try_from(play_count)
                        .map_err(|_| error::NativeApiError::Unavailable)?,
                },
            ))
        })
        .collect()
}

async fn load_annotations(
    pool: &sqlx::PgPool,
    table: &'static str,
    entity_column: &'static str,
    user_id: Uuid,
    entity_ids: Vec<Uuid>,
) -> Result<Vec<(Uuid, NativeAnnotation)>, error::NativeApiError> {
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }
    let query = sqlx::AssertSqlSafe(format!(
        "SELECT {entity_column}, starred, rating::integer, play_count FROM {table} WHERE user_id=$1 AND {entity_column}=ANY($2::uuid[])"
    ));
    let rows = sqlx::query_as::<_, (Uuid, bool, i32, i64)>(query)
        .bind(user_id)
        .bind(entity_ids)
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|(id, starred, rating, play_count)| {
            Ok((
                id,
                NativeAnnotation {
                    starred,
                    rating,
                    play_count: i32::try_from(play_count)
                        .map_err(|_| error::NativeApiError::Unavailable)?,
                },
            ))
        })
        .collect()
}

/// Shared state for the native-v1 adapter.
#[derive(Clone)]
pub struct NativeState {
    pub pool: sqlx::PgPool,
    /// notification render/delivery service. Event wiring dispatches
    /// fire-and-record sends; failures are audited, never propagated
    /// (PRD-NOTIF-002).
    pub notifications: Arc<crate::notifications::NotificationService>,
    pub search: Option<melodee_search::SearchConfig>,
    pub public_origin: String,
    pub podcast_config: Arc<crate::podcast_worker::PodcastConfig>,
    pub podcast_discovery: podcasts::PodcastDiscoveryClient,
    pub stream_url_signer: Arc<dyn NativeStreamUrlSigner>,
    pub user_state: Arc<dyn NativeUserStateReader>,
    pub direct_play: Option<crate::native_stream::DirectPlayService>,
    pub client_addresses: crate::native_stream::ClientAddressResolver,
    pub auth: Arc<auth::NativeAuthService>,
    /// Optional Last.fm API base URL for per-test mock servers.
    /// Production wiring leaves this `None`; it is never an operator setting.
    #[doc(hidden)]
    pub lastfm_api_base_url: Option<String>,
}

/// Extractor that fails with the captured empty 401 if authentication did not
/// install a principal. It never trusts caller-supplied identity headers.
pub struct Authenticated(pub NativePrincipal, pub NativeClientBinding);

impl<S> FromRequestParts<S> for Authenticated
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let principal = parts
            .extensions
            .get::<NativePrincipal>()
            .copied()
            .ok_or_else(unauthorized)?;
        let client_binding = parts
            .extensions
            .get::<NativeClientBinding>()
            .cloned()
            .ok_or_else(unauthorized)?;
        Ok(Self(principal, client_binding))
    }
}

async fn require_native_principal(request: Request, next: Next) -> Response {
    if request.extensions().get::<NativePrincipal>().is_none() {
        return unauthorized();
    }
    next.run(request).await
}

/// The frozen native-v1 responses use ASP.NET's explicit UTF-8 media type.
/// Axum's `Json` extractor intentionally emits the shorter
/// `application/json`; normalize only native JSON responses so the three
/// compatibility products keep independent wire contracts.
fn add_native_json_charset(response: &mut Response) {
    let is_plain_json = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes() == b"application/json");
    if is_plain_json {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
    }
}

async fn normalize_native_json_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    add_native_json_charset(&mut response);
    response
}

pub async fn system_info() -> Json<ServerInfoDto> {
    Json(ServerInfoDto::default())
}

async fn excluded_from_build() -> Response {
    axum::http::StatusCode::NOT_FOUND.into_response()
}

/// Explicit negative routes preserve the frozen `<Compile Remove>` behavior.
/// Wildcards are intentionally limited to the two excluded route families;
/// they do not swallow unrelated native-v1 typos.
fn excluded_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/queue", any(excluded_from_build))
        .route("/api/v1/party-endpoints", any(excluded_from_build))
        .route(
            "/api/v1/party-endpoints/{*remainder}",
            any(excluded_from_build),
        )
        .route("/api/v1/party-sessions", any(excluded_from_build))
        .route(
            "/api/v1/party-sessions/{*remainder}",
            any(excluded_from_build),
        )
}

/// Builds native-v1 routes. The public system-info route stays outside the
/// auth layer because existing clients use it as a pre-login version gate.
pub fn router() -> Router<NativeState> {
    let protected = catalog::router()
        .merge(community::protected_router())
        .merge(discovery::protected_router())
        .merge(endpoints::router())
        .merge(playlists::router())
        .merge(preferences::router())
        .merge(podcasts::router())
        .merge(auth::protected_router())
        .merge(avatars::router())
        .merge(discovery::cookie_protected_router())
        .merge(auth::cookie_protected_router())
        .route_layer(middleware::from_fn(require_native_principal));
    Router::new()
        .route("/api/v1/system/info", get(system_info))
        .merge(discovery::public_router())
        .merge(excluded_router())
        .merge(community::public_router())
        .merge(auth::public_router())
        .merge(images::router())
        .merge(protected)
        .merge(stream::router())
        .layer(middleware::from_fn(normalize_native_json_response))
}

/// Builds the production native router with JWT validation and principal
/// installation. Access JWT validation is cryptographic; the one indexed user
/// lookup maps the captured public sid to Melodee's internal UUID.
pub fn configured_router(state: NativeState) -> Router {
    let protected = catalog::router()
        .merge(community::protected_router())
        .merge(discovery::protected_router())
        .merge(endpoints::router())
        .merge(playlists::router())
        .merge(preferences::router())
        .merge(podcasts::router())
        .merge(auth::protected_router())
        .merge(avatars::router())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate_request,
        ));
    let cookie_protected = discovery::cookie_protected_router()
        .merge(auth::cookie_protected_router())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate_cookie_request,
        ));
    Router::new()
        .route("/api/v1/system/info", get(system_info))
        .merge(discovery::public_router())
        .merge(excluded_router())
        .merge(community::public_router())
        .merge(auth::public_router())
        .merge(images::router())
        .merge(protected)
        .merge(cookie_protected)
        .merge(stream::router())
        .layer(middleware::from_fn(normalize_native_json_response))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use axum::{
        body::Body,
        http::{HeaderMap, Request},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn native_json_charset_is_added_without_touching_other_media_types() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let mut response = Response::new(Body::empty());
        *response.headers_mut() = headers;
        add_native_json_charset(&mut response);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json; charset=utf-8"))
        );

        let mut response = Response::new(Body::empty());
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
        add_native_json_charset(&mut response);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("audio/wav"))
        );
    }

    #[derive(Debug)]
    struct TestSigner;

    impl NativeStreamUrlSigner for TestSigner {
        fn stream_url(
            &self,
            track_public_id: Uuid,
            user_public_id: Uuid,
            _client_binding: &str,
        ) -> NativeFuture<'_, Result<String, ()>> {
            Box::pin(async move {
                Ok(format!(
                    "/song/stream/{track_public_id}/{user_public_id}/test"
                ))
            })
        }
    }

    #[derive(Debug)]
    struct TestUserState;

    impl NativeUserStateReader for TestUserState {
        fn artist_annotation(
            &self,
            _user_id: Uuid,
            _artist_id: Uuid,
        ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>> {
            Box::pin(async { Ok(NativeAnnotation::default()) })
        }

        fn release_annotation(
            &self,
            _user_id: Uuid,
            _release_id: Uuid,
        ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>> {
            Box::pin(async { Ok(NativeAnnotation::default()) })
        }

        fn track_annotation(
            &self,
            _user_id: Uuid,
            _track_id: Uuid,
        ) -> NativeFuture<'_, Result<NativeAnnotation, error::NativeApiError>> {
            Box::pin(async { Ok(NativeAnnotation::default()) })
        }
    }

    fn test_state() -> NativeState {
        NativeState {
            lastfm_api_base_url: None,
            pool: sqlx::PgPool::connect_lazy("postgres://melodee:secret@127.0.0.1:9/test")
                .expect("valid test database URL"),
            notifications: Arc::new(crate::notifications::NotificationService::noop(
                sqlx::PgPool::connect_lazy("postgres://melodee:secret@127.0.0.1:9/test")
                    .expect("valid test database URL"),
            )),
            search: None,
            public_origin: "http://127.0.0.1:8080".to_string(),
            podcast_config: Arc::new(crate::podcast_worker::PodcastConfig::default()),
            podcast_discovery: podcasts::PodcastDiscoveryClient::itunes()
                .expect("podcast discovery config"),
            stream_url_signer: Arc::new(TestSigner),
            user_state: Arc::new(TestUserState),
            direct_play: None,
            client_addresses: crate::native_stream::ClientAddressResolver::default(),
            auth: Arc::new(
                auth::NativeAuthService::new(
                    "test-only-jwt-signing-key-that-is-long".to_string(),
                    90,
                )
                .expect("test auth config"),
            ),
        }
    }

    #[tokio::test]
    async fn system_info_is_public_and_matches_the_frozen_gate() {
        let response = router()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/info")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).expect("json"),
            serde_json::json!({
                "name": "Melodee",
                "description": "Melodee API",
                "majorVersion": 2,
                "minorVersion": 2,
                "patchVersion": 0,
                "version": "2.2.0"
            })
        );
    }

    #[tokio::test]
    async fn protected_catalog_routes_fail_with_the_captured_empty_401() {
        let response = router()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/artists")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert!(
            response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn all_excluded_queue_and_party_paths_preserve_empty_404() {
        let uuid = "00000000-0000-0000-0000-000000000001";
        let item = "00000000-0000-0000-0000-000000000002";
        let cases = [
            ("DELETE", "/api/v1/queue".to_string()),
            ("GET", "/api/v1/queue".to_string()),
            ("PUT", "/api/v1/queue".to_string()),
            ("POST", "/api/v1/party-endpoints/register".to_string()),
            ("GET", format!("/api/v1/party-endpoints/{uuid}")),
            (
                "PUT",
                format!("/api/v1/party-endpoints/{uuid}/capabilities"),
            ),
            ("POST", format!("/api/v1/party-endpoints/{uuid}/detach")),
            ("POST", format!("/api/v1/party-endpoints/{uuid}/heartbeat")),
            ("GET", format!("/api/v1/party-endpoints/{uuid}/state")),
            ("POST", "/api/v1/party-sessions".to_string()),
            ("GET", "/api/v1/party-sessions/active".to_string()),
            ("GET", "/api/v1/party-sessions/my".to_string()),
            ("GET", format!("/api/v1/party-sessions/{uuid}")),
            ("GET", format!("/api/v1/party-sessions/{uuid}/audit")),
            ("POST", format!("/api/v1/party-sessions/{uuid}/end")),
            ("POST", format!("/api/v1/party-sessions/{uuid}/join")),
            ("POST", format!("/api/v1/party-sessions/{uuid}/leave")),
            ("GET", format!("/api/v1/party-sessions/{uuid}/participants")),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/participants/1/ban"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/participants/1/kick"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/participants/1/role"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/participants/1/unban"),
            ),
            ("GET", format!("/api/v1/party-sessions/{uuid}/playback")),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/playback/next"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/playback/pause"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/playback/play"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/playback/seek"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/playback/stop"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/playback/volume"),
            ),
            ("GET", format!("/api/v1/party-sessions/{uuid}/queue")),
            ("POST", format!("/api/v1/party-sessions/{uuid}/queue/clear")),
            ("POST", format!("/api/v1/party-sessions/{uuid}/queue/items")),
            (
                "DELETE",
                format!("/api/v1/party-sessions/{uuid}/queue/items/{item}"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/queue/items/{item}/reorder"),
            ),
            (
                "POST",
                format!("/api/v1/party-sessions/{uuid}/settings/queue-lock"),
            ),
        ];
        assert_eq!(cases.len(), 35);
        for (method, path) in cases {
            let response = router()
                .with_state(test_state())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
            assert!(
                response
                    .into_body()
                    .collect()
                    .await
                    .expect("body")
                    .to_bytes()
                    .is_empty()
            );
        }
    }
}

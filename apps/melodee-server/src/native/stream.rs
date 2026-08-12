//! Captured non-versioned native streaming adapter (ADR-0125/ADR-0128).

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use melodee_persistence::AuthStore;
use serde::Serialize;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{NativeFuture, NativeState, NativeStreamUrlSigner};
use crate::native_stream::{PlayableAsset, StreamError, StreamTokenCodec};

/// PostgreSQL-backed signer for catalog DTO stream URLs. The per-user signing
/// key crosses only this cryptographic boundary and is never retained.
#[derive(Clone)]
pub struct DatabaseStreamUrlSigner {
    pool: sqlx::PgPool,
    public_origin: Arc<str>,
}

impl DatabaseStreamUrlSigner {
    #[must_use]
    pub fn new(pool: sqlx::PgPool, public_origin: impl Into<Arc<str>>) -> Self {
        Self {
            pool,
            public_origin: public_origin.into(),
        }
    }
}

impl NativeStreamUrlSigner for DatabaseStreamUrlSigner {
    fn stream_url(
        &self,
        track_public_id: Uuid,
        user_public_id: Uuid,
        client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        let client_binding = client_binding.to_owned();
        Box::pin(async move {
            let principal = AuthStore::new()
                .stream_principal_by_public_id(&self.pool, user_public_id)
                .await
                .map_err(|_| ())?
                .filter(melodee_persistence::StreamPrincipal::is_authorized)
                .ok_or(())?;
            let release_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT release_id FROM tracks WHERE public_api_id=$1",
            )
            .bind(track_public_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| ())?
            .ok_or(())?;
            if !AuthStore::new()
                .can_access_release(&self.pool, principal.user_id, release_id)
                .await
                .map_err(|_| ())?
            {
                return Err(());
            }
            let token = StreamTokenCodec
                .issue(
                    principal.stream_signing_key.expose_secret(),
                    user_public_id,
                    track_public_id,
                    &client_binding,
                    OffsetDateTime::now_utc().unix_timestamp(),
                )
                .map_err(|_| ())?;
            Ok(format!(
                "{}/song/stream/{track_public_id}/{user_public_id}/{token}",
                self.public_origin.trim_end_matches('/')
            ))
        })
    }

    fn stream_urls(
        &self,
        track_public_ids: Vec<Uuid>,
        user_public_id: Uuid,
        client_binding: &str,
    ) -> NativeFuture<'_, Result<Vec<String>, ()>> {
        let client_binding = client_binding.to_owned();
        Box::pin(async move {
            if track_public_ids.is_empty() {
                return Ok(Vec::new());
            }
            let principal = AuthStore::new()
                .stream_principal_by_public_id(&self.pool, user_public_id)
                .await
                .map_err(|_| ())?
                .filter(melodee_persistence::StreamPrincipal::is_authorized)
                .ok_or(())?;
            let accessible = sqlx::query_scalar::<_, Uuid>(
                "SELECT track.public_api_id FROM tracks track
                 WHERE track.public_api_id=ANY($1::uuid[])
                   AND native_user_can_access_release($2,track.release_id)",
            )
            .bind(&track_public_ids)
            .bind(principal.user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| ())?;
            if accessible.len() != track_public_ids.len() {
                return Err(());
            }
            let now = OffsetDateTime::now_utc().unix_timestamp();
            track_public_ids
                .into_iter()
                .map(|track_public_id| {
                    let token = StreamTokenCodec
                        .issue(
                            principal.stream_signing_key.expose_secret(),
                            user_public_id,
                            track_public_id,
                            &client_binding,
                            now,
                        )
                        .map_err(|_| ())?;
                    Ok(format!(
                        "{}/song/stream/{track_public_id}/{user_public_id}/{token}",
                        self.public_origin.trim_end_matches('/')
                    ))
                })
                .collect()
        })
    }
}

pub fn router() -> Router<NativeState> {
    Router::new().route(
        "/song/stream/{track_public_id}/{user_public_id}/{auth_token}",
        get(stream).head(stream),
    )
}

async fn stream(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((track_public_id, user_public_id, auth_token)): Path<(Uuid, Uuid, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    match stream_inner(
        &state,
        peer,
        track_public_id,
        user_public_id,
        &auth_token,
        &method,
        &headers,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn stream_inner(
    state: &NativeState,
    peer: SocketAddr,
    track_public_id: Uuid,
    user_public_id: Uuid,
    auth_token: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response, StreamRouteError> {
    if user_public_id.is_nil() {
        return Err(StreamRouteError::Unauthorized);
    }
    let client_address = state
        .client_addresses
        .resolve(
            peer,
            header_str(headers, "forwarded"),
            header_str(headers, "x-forwarded-for"),
        )
        .map_err(|_| StreamRouteError::Unauthorized)?;
    let principal = AuthStore::new()
        .stream_principal_by_public_id(&state.pool, user_public_id)
        .await
        .map_err(|_| StreamRouteError::Unavailable)?
        .filter(melodee_persistence::StreamPrincipal::is_authorized)
        .ok_or(StreamRouteError::Unauthorized)?;
    StreamTokenCodec
        .validate(
            principal.stream_signing_key.expose_secret(),
            auth_token,
            user_public_id,
            track_public_id,
            &client_address.to_string(),
            OffsetDateTime::now_utc().unix_timestamp(),
        )
        .map_err(|_| StreamRouteError::Unauthorized)?;

    let (release_id, asset) = resolve_playable_asset(&state.pool, track_public_id)
        .await?
        .ok_or(StreamRouteError::UnableToLoad)?;
    let can_access = AuthStore::new()
        .can_access_release(&state.pool, principal.user_id, release_id)
        .await
        .map_err(|_| StreamRouteError::Unavailable)?;
    if !can_access {
        // A valid HMAC proves who issued the URL, not that this User still has
        // access to the Track's source library. Keep the denial identical to
        // all credential failures so restricted catalog facts are not leaked.
        return Err(StreamRouteError::Unauthorized);
    }
    let range = match headers.get(header::RANGE) {
        Some(value) => Some(value.to_str().map_err(|_| StreamRouteError::UnableToLoad)?),
        None => None,
    };
    let direct_play = state
        .direct_play
        .as_ref()
        .ok_or(StreamRouteError::Unavailable)?;
    direct_play
        .response(&asset, range, method == Method::HEAD)
        .await
        .map_err(StreamRouteError::from)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

async fn resolve_playable_asset(
    pool: &sqlx::PgPool,
    track_public_id: Uuid,
) -> Result<Option<(Uuid, PlayableAsset)>, StreamRouteError> {
    let row = sqlx::query(
        "SELECT r.id AS release_id,m.canonical_relative_path,m.byte_size,m.mime_type
         FROM tracks t
         JOIN releases r ON r.id = t.release_id AND r.status = 'active'
         JOIN media_assets m ON m.id = t.playable_media_asset_id
         JOIN track_media_assets link
           ON link.track_id = t.id
          AND link.media_asset_id = m.id
          AND link.purpose = 'playable'
         WHERE t.public_api_id = $1",
    )
    .bind(track_public_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| StreamRouteError::Unavailable)?;
    row.map(|row| {
        let byte_size = row
            .try_get::<i64, _>("byte_size")
            .map_err(|_| StreamRouteError::Unavailable)
            .and_then(|value| u64::try_from(value).map_err(|_| StreamRouteError::Unavailable))?;
        Ok((
            row.try_get("release_id")
                .map_err(|_| StreamRouteError::Unavailable)?,
            PlayableAsset {
                canonical_relative_path: row
                    .try_get("canonical_relative_path")
                    .map_err(|_| StreamRouteError::Unavailable)?,
                byte_size,
                mime_type: row
                    .try_get("mime_type")
                    .map_err(|_| StreamRouteError::Unavailable)?,
            },
        ))
    })
    .transpose()
}

#[derive(Debug)]
enum StreamRouteError {
    Unauthorized,
    UnableToLoad,
    Unavailable,
}

impl From<StreamError> for StreamRouteError {
    fn from(error: StreamError) -> Self {
        match error {
            StreamError::InvalidRange | StreamError::AssetUnavailable => Self::UnableToLoad,
            StreamError::CapacityClosed | StreamError::InvalidAssetMetadata => Self::Unavailable,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamErrorBody {
    code: &'static str,
    message: &'static str,
    correlation_id: Uuid,
}

impl IntoResponse for StreamRouteError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "Invalid Auth Token",
            ),
            Self::UnableToLoad => (
                StatusCode::BAD_REQUEST,
                "BAD_REQUEST",
                "Unable to load song",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "SERVICE_UNAVAILABLE",
                "Service temporarily unavailable",
            ),
        };
        (
            status,
            Json(StreamErrorBody {
                code,
                message,
                correlation_id: Uuid::now_v7(),
            }),
        )
            .into_response()
    }
}

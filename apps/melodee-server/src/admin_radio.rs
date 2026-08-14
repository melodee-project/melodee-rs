//! Phase 9 WP-06 Admin API: internet radio station administration
//! (PRD-RADIO-001/002, ADR-0134 §1).
//!
//! These routes extend the Rust-native operator API (`/api/admin/v1`,
//! SPEC 19.4) over the single authoritative radio store
//! (`opensubsonic_radio_stations`, migration 0020) — the same table the
//! OpenSubsonic adapter serves, so stations created here appear in
//! `getInternetRadioStations` and vice versa. Station public ids are
//! immutable once issued and are never regenerated (PRD-RADIO-002).
//!
//! Authorization: list/read accept any authenticated, enabled native user;
//! create/update/delete are admin-gated exactly as captured on the
//! OpenSubsonic surface (`is_admin` or the `Administrator` role,
//! case-insensitive). Every mutation is audit-logged via `tracing` with the
//! acting administrator's public id.

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use melodee_persistence::{
    NewRadioStation, RadioStation, RadioStationStore, RadioStationUpdate, RadioStoreError,
    StoredNativeUser,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::AdminAccessState;

/// Maximum rows returned by the station listing; the handler clamps to this
/// even when a client requests more.
const MAX_LIST_LIMIT: u32 = 500;

/// Authenticated native user allowed to list/read radio stations.
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedPrincipal {
    pub public_api_id: Uuid,
}

/// Authenticated administrator allowed to mutate radio stations, matching
/// the captured OpenSubsonic gate (`is_admin` or the `Administrator` role).
#[derive(Debug, Clone, Copy)]
pub struct RadioAdminPrincipal {
    pub public_api_id: Uuid,
}

/// Stable admin API error envelope (matches `admin.rs`/`admin_access.rs`
/// conventions).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminRadioErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(AdminRadioErrorBody {
            code,
            message: code.to_string(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn store_error(error: RadioStoreError) -> Response {
    let (status, code) = match &error {
        RadioStoreError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        RadioStoreError::InvalidName => (StatusCode::BAD_REQUEST, "invalid_name"),
        RadioStoreError::InvalidStreamUrl => (StatusCode::BAD_REQUEST, "invalid_stream_url"),
        RadioStoreError::InvalidHomePageUrl => (StatusCode::BAD_REQUEST, "invalid_home_page_url"),
        RadioStoreError::Database(_) => {
            tracing::error!(%error, "admin radio store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    };
    (
        status,
        Json(AdminRadioErrorBody {
            code,
            // Validation messages are static, operator-safe constraint labels.
            message: error.to_string(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

/// Shared JWT authentication: a valid native access token for an enabled,
/// unlocked, non-blacklisted account.
async fn authenticate(
    parts: &Parts,
    state: &AdminAccessState,
) -> Result<StoredNativeUser, Response> {
    let token = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| admin_error(StatusCode::UNAUTHORIZED, "unauthenticated"))?;
    let claims = state
        .auth
        .validate_access(token, OffsetDateTime::now_utc())
        .ok_or_else(|| admin_error(StatusCode::UNAUTHORIZED, "unauthenticated"))?;
    let user = match melodee_persistence::AuthStore::new()
        .user_by_public_id(&state.pool, claims.sid)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => return Err(admin_error(StatusCode::UNAUTHORIZED, "unauthenticated")),
        Err(_) => return Err(admin_error(StatusCode::SERVICE_UNAVAILABLE, "store_error")),
    };
    if !user.can_authenticate() {
        return Err(admin_error(StatusCode::UNAUTHORIZED, "account_unavailable"));
    }
    Ok(user)
}

impl FromRequestParts<AdminAccessState> for AuthenticatedPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminAccessState,
    ) -> Result<Self, Self::Rejection> {
        let user = authenticate(parts, state).await?;
        Ok(Self {
            public_api_id: user.public_api_id,
        })
    }
}

impl FromRequestParts<AdminAccessState> for RadioAdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminAccessState,
    ) -> Result<Self, Self::Rejection> {
        let user = authenticate(parts, state).await?;
        let is_admin = user.is_admin
            || user
                .roles
                .iter()
                .any(|held| held.eq_ignore_ascii_case("Administrator"));
        if !is_admin {
            return Err(admin_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        Ok(Self {
            public_api_id: user.public_api_id,
        })
    }
}

/// One station as exposed to operators. `id` is the immutable public id (the
/// same UUID the OpenSubsonic surface renders as `radio_{uuid}`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioStationDto {
    pub id: Uuid,
    pub name: String,
    pub stream_url: String,
    pub home_page_url: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl From<RadioStation> for RadioStationDto {
    fn from(station: RadioStation) -> Self {
        Self {
            id: station.public_api_id,
            name: station.name,
            stream_url: station.stream_url,
            home_page_url: station.home_page_url,
            created_at: station.created_at,
            updated_at: station.updated_at,
        }
    }
}

/// Query parameters for the bounded station listing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRadioStationsParams {
    /// Maximum rows to return (clamped to MAX_LIST_LIMIT). Default 50.
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    /// Zero-based offset for paging.
    #[serde(default)]
    pub offset: u32,
}

fn default_list_limit() -> u32 {
    50
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRadioStationRequest {
    pub name: String,
    pub stream_url: String,
    pub home_page_url: Option<String>,
}

/// Field-optional update. `homePageUrl` is a double option: absent keeps the
/// stored value, explicit `null` clears it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRadioStationRequest {
    pub name: Option<String>,
    pub stream_url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub home_page_url: Option<Option<String>>,
}

/// Distinguishes an absent field (`None` via `serde(default)`) from an
/// explicit JSON `null` (`Some(None)`): serde's plain `Option<Option<T>>`
/// collapses both to `None`.
fn deserialize_double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route(
            "/api/admin/v1/radio/stations",
            get(list_stations).post(create_station),
        )
        .route(
            "/api/admin/v1/radio/stations/{id}",
            get(get_station).put(update_station).delete(delete_station),
        )
        .route_layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

async fn list_stations(
    State(state): State<AdminAccessState>,
    _user: AuthenticatedPrincipal,
    Query(params): Query<ListRadioStationsParams>,
) -> Response {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    match RadioStationStore::new()
        .list(&state.pool, limit, params.offset)
        .await
    {
        Ok(stations) => Json(
            stations
                .into_iter()
                .map(RadioStationDto::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => store_error(error),
    }
}

async fn get_station(
    State(state): State<AdminAccessState>,
    _user: AuthenticatedPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match RadioStationStore::new()
        .get_by_public_id(&state.pool, id)
        .await
    {
        Ok(Some(station)) => Json(RadioStationDto::from(station)).into_response(),
        Ok(None) => store_error(RadioStoreError::NotFound),
        Err(error) => store_error(error),
    }
}

async fn create_station(
    State(state): State<AdminAccessState>,
    admin: RadioAdminPrincipal,
    Json(request): Json<CreateRadioStationRequest>,
) -> Response {
    let create = NewRadioStation {
        name: &request.name,
        stream_url: &request.stream_url,
        home_page_url: request.home_page_url.as_deref(),
    };
    match RadioStationStore::new()
        .create(&state.pool, &create, OffsetDateTime::now_utc())
        .await
    {
        Ok(station) => {
            tracing::info!(
                actor = %admin.public_api_id,
                station = %station.public_api_id,
                "admin created internet radio station"
            );
            (StatusCode::CREATED, Json(RadioStationDto::from(station))).into_response()
        }
        Err(error) => store_error(error),
    }
}

async fn update_station(
    State(state): State<AdminAccessState>,
    admin: RadioAdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateRadioStationRequest>,
) -> Response {
    let update = RadioStationUpdate {
        name: request.name,
        stream_url: request.stream_url,
        home_page_url: request.home_page_url,
    };
    match RadioStationStore::new()
        .update(&state.pool, id, &update, OffsetDateTime::now_utc())
        .await
    {
        Ok(station) => {
            tracing::info!(
                actor = %admin.public_api_id,
                station = %id,
                "admin updated internet radio station"
            );
            Json(RadioStationDto::from(station)).into_response()
        }
        Err(error) => store_error(error),
    }
}

async fn delete_station(
    State(state): State<AdminAccessState>,
    admin: RadioAdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match RadioStationStore::new().delete(&state.pool, id).await {
        Ok(true) => {
            tracing::info!(
                actor = %admin.public_api_id,
                station = %id,
                "admin deleted internet radio station"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => store_error(RadioStoreError::NotFound),
        Err(error) => store_error(error),
    }
}

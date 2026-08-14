//! Phase 9 WP-10 Admin API: device/transcoding profile administration
//! (PRD-DEVICE-001/002, ADR-0134 §5).
//!
//! These routes extend the Rust-native operator API (`/api/admin/v1`,
//! SPEC 19.4) over `device_profiles` (migration 0026). Profiles are
//! operator-managed **metadata only** (PRD-DEVICE-002): nothing on this
//! surface feeds stream negotiation, and no response field or documentation
//! may claim profiles control transcoding. Profile-driven stream selection
//! is deliberately unwired — it is post-parity work requiring its own ADR
//! (PRD-DEVICE-003).
//!
//! Authorization: the shared native-JWT administrator gate
//! ([`crate::admin_access::AdminPrincipal`]). Every mutation is audit-logged
//! via `tracing` with the acting administrator's public id.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use melodee_persistence::{
    DeviceProfile, DeviceProfileStore, DeviceProfileStoreError, DeviceProfileUpdate,
    NewDeviceProfile,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};

/// Maximum rows returned by the listing; clamped even when a client
/// requests more.
const MAX_LIST_LIMIT: u32 = 500;

/// Stable admin API error envelope (matches `admin.rs`/`admin_access.rs`
/// conventions).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminDeviceProfileErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(AdminDeviceProfileErrorBody {
            code,
            message: code.to_string(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn store_error(error: DeviceProfileStoreError) -> Response {
    let (status, code) = match &error {
        DeviceProfileStoreError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        DeviceProfileStoreError::InvalidProfile(_) => (StatusCode::BAD_REQUEST, "invalid_profile"),
        DeviceProfileStoreError::Database(_) => {
            tracing::error!(%error, "admin device profile store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    };
    (
        status,
        Json(AdminDeviceProfileErrorBody {
            code,
            // Validation messages are static, operator-safe constraint
            // labels — never echoed values.
            message: error.to_string(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

/// One device profile as exposed to operators. Metadata only: `profile`
/// records transcoding *intent* for a future resolver; it does not control
/// streaming today (PRD-DEVICE-002/003).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProfileDto {
    pub id: Uuid,
    pub name: String,
    pub client_match: serde_json::Value,
    pub profile: serde_json::Value,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
}

fn profile_dto(profile: &DeviceProfile) -> Result<DeviceProfileDto, ()> {
    Ok(DeviceProfileDto {
        id: profile.id,
        name: profile.name.clone(),
        client_match: profile.client_match.clone(),
        profile: profile.profile.clone(),
        is_default: profile.is_default,
        created_at: profile.created_at.format(&Rfc3339).map_err(|_| ())?,
        updated_at: profile.updated_at.format(&Rfc3339).map_err(|_| ())?,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
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

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeviceProfileRequest {
    pub name: String,
    /// Client matcher metadata: a JSON object of bounded string values
    /// (e.g. `{"clientName": "feishin"}`). Defaults to an empty object.
    #[serde(default = "empty_object")]
    pub client_match: serde_json::Value,
    /// Transcoding-intent metadata (e.g. `{"directPlay": false,
    /// "targetCodec": "opus", "maxBitrate": 192}`). Defaults to an empty
    /// object. Unwired to stream selection (PRD-DEVICE-003).
    #[serde(default = "empty_object")]
    pub profile: serde_json::Value,
    /// When true, atomically clears any existing default (legacy parity).
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDeviceProfileRequest {
    pub name: Option<String>,
    pub client_match: Option<serde_json::Value>,
    pub profile: Option<serde_json::Value>,
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route(
            "/api/admin/v1/device-profiles",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/api/admin/v1/device-profiles/{id}",
            get(get_profile).put(update_profile).delete(delete_profile),
        )
        .route(
            "/api/admin/v1/device-profiles/{id}/default",
            axum::routing::post(set_default),
        )
        .route_layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

async fn list_profiles(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<ListParams>,
) -> Response {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    match DeviceProfileStore::new()
        .list(&state.pool, limit, params.offset)
        .await
    {
        Ok(profiles) => match profiles
            .iter()
            .map(profile_dto)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(dtos) => Json(dtos).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

async fn get_profile(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match DeviceProfileStore::new().get(&state.pool, id).await {
        Ok(profile) => match profile_dto(&profile) {
            Ok(dto) => Json(dto).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

async fn create_profile(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<CreateDeviceProfileRequest>,
) -> Response {
    let new = NewDeviceProfile {
        name: request.name,
        client_match: request.client_match,
        profile: request.profile,
        is_default: request.is_default,
    };
    match DeviceProfileStore::new()
        .create(&state.pool, &new, OffsetDateTime::now_utc())
        .await
    {
        Ok(profile) => {
            tracing::info!(
                actor = %admin.public_api_id,
                profile = %profile.id,
                is_default = profile.is_default,
                "admin created device profile"
            );
            match profile_dto(&profile) {
                Ok(dto) => (StatusCode::CREATED, Json(dto)).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

async fn update_profile(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateDeviceProfileRequest>,
) -> Response {
    let update = DeviceProfileUpdate {
        name: request.name,
        client_match: request.client_match,
        profile: request.profile,
    };
    match DeviceProfileStore::new()
        .update(&state.pool, id, &update, OffsetDateTime::now_utc())
        .await
    {
        Ok(profile) => {
            tracing::info!(
                actor = %admin.public_api_id,
                profile = %id,
                "admin updated device profile"
            );
            match profile_dto(&profile) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

async fn delete_profile(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match DeviceProfileStore::new().delete(&state.pool, id).await {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                profile = %id,
                "admin deleted device profile"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => store_error(error),
    }
}

async fn set_default(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match DeviceProfileStore::new()
        .set_default(&state.pool, id, OffsetDateTime::now_utc())
        .await
    {
        Ok(profile) => {
            tracing::info!(
                actor = %admin.public_api_id,
                profile = %id,
                "admin set default device profile"
            );
            match profile_dto(&profile) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

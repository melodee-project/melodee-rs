//! Phase 9 WP-03 Admin API: user and library-access-group administration
//! (PRD-AUTH-003/004, ADR-0134 §2).
//!
//! These routes extend the Rust-native operator API (`/api/admin/v1`,
//! SPEC 19.4); they are not part of any frozen compatibility contract.
//! Authorization: a valid native access JWT whose user is enabled, unlocked,
//! not blacklisted, and `is_admin`. Every mutation is audit-logged via
//! `tracing` with the acting administrator's public id; secrets never appear
//! in logs, DTOs, or error messages (PRD-AUTH-005).

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use melodee_persistence::{
    AccessGroupDetail, GroupAdminError, GroupAdminStore, NativeUserAdminUpdate, NativeUserCreate,
    StoredAccessGroup, StoredNativeUser, UserAdminError, UserAdminStore,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::native::auth::NativeAuthService;

/// Upper bound for a client-supplied password on user creation.
const MAX_PASSWORD_CHARS: usize = 1024;

/// Shared state for the WP-03 admin access surface.
#[derive(Clone)]
pub struct AdminAccessState {
    pub pool: sqlx::PgPool,
    pub auth: Arc<NativeAuthService>,
}

/// Authenticated administrator identity enforced by the extractor below.
#[derive(Debug, Clone, Copy)]
pub struct AdminPrincipal {
    pub user_id: Uuid,
    pub public_api_id: Uuid,
}

/// Extractor enforcing the native-JWT administrator gate on every route.
impl FromRequestParts<AdminAccessState> for AdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminAccessState,
    ) -> Result<Self, Self::Rejection> {
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
            Ok(None) => {
                return Err(admin_error(StatusCode::UNAUTHORIZED, "unauthenticated"));
            }
            Err(_) => return Err(admin_error(StatusCode::SERVICE_UNAVAILABLE, "store_error")),
        };
        if !user.can_authenticate() {
            return Err(admin_error(StatusCode::UNAUTHORIZED, "account_unavailable"));
        }
        if !user.is_admin {
            return Err(admin_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        Ok(Self {
            user_id: user.id,
            public_api_id: user.public_api_id,
        })
    }
}

/// Stable admin API error envelope (matches `admin.rs` conventions).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminAccessErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            message: code.to_string(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn store_error(error: UserAdminError) -> Response {
    let (status, code) = match &error {
        UserAdminError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        UserAdminError::UserConflict => (StatusCode::CONFLICT, "user_conflict"),
        UserAdminError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
        UserAdminError::Database(_) => {
            tracing::error!(%error, "admin access store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    };
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            // InvalidRequest details are operator-safe field labels, never
            // values; other variants are static.
            message: error.to_string(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn group_error(error: GroupAdminError) -> Response {
    let (status, code) = match &error {
        GroupAdminError::InvalidGroupName => (StatusCode::BAD_REQUEST, "invalid_group_name"),
        GroupAdminError::GroupNotFound | GroupAdminError::UserNotFound => {
            (StatusCode::NOT_FOUND, "not_found")
        }
        GroupAdminError::GroupExists => (StatusCode::CONFLICT, "group_exists"),
        GroupAdminError::AmbiguousGroupName => (StatusCode::CONFLICT, "group_name_ambiguous"),
        GroupAdminError::GroupHasMembers | GroupAdminError::GroupHasBindings => {
            (StatusCode::CONFLICT, "group_not_empty")
        }
        GroupAdminError::Database(_) => {
            tracing::error!(%error, "admin access group store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    };
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            message: error.to_string(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserDto {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub is_admin: bool,
    pub is_editor: bool,
    pub enabled: bool,
    pub locked: bool,
    pub roles: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn user_dto(user: &StoredNativeUser) -> Result<AdminUserDto, ()> {
    Ok(AdminUserDto {
        id: user.public_api_id,
        username: user.username.clone(),
        email: user.email.clone(),
        is_admin: user.is_admin,
        is_editor: user.is_editor,
        enabled: user.enabled,
        locked: user.locked,
        roles: user.roles.clone(),
        created_at: format_time(user.created_at)?,
        updated_at: format_time(user.updated_at)?,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessGroupDto {
    pub id: Uuid,
    pub name: String,
    pub member_count: i64,
    pub binding_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

fn group_dto(group: &StoredAccessGroup) -> Result<AccessGroupDto, ()> {
    Ok(AccessGroupDto {
        id: group.public_api_id,
        name: group.name.clone(),
        member_count: group.member_count,
        binding_count: group.binding_count,
        created_at: format_time(group.created_at)?,
        updated_at: format_time(group.updated_at)?,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessGroupDetailDto {
    #[serde(flatten)]
    pub group: AccessGroupDto,
    pub members: Vec<AccessGroupMemberDto>,
    pub library_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessGroupMemberDto {
    pub user_id: Uuid,
    pub username: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupNameRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub is_editor: bool,
    /// Explicit role set; omitted grants the minimal streaming role.
    pub roles: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    pub is_admin: Option<bool>,
    pub is_editor: Option<bool>,
    pub enabled: Option<bool>,
    pub locked: Option<bool>,
    pub roles: Option<Vec<String>>,
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route("/api/admin/v1/groups", get(list_groups).post(create_group))
        .route(
            "/api/admin/v1/groups/{id}",
            get(get_group).put(rename_group).delete(delete_group),
        )
        .route(
            "/api/admin/v1/groups/{id}/members/{userId}",
            put(add_group_member).delete(remove_group_member),
        )
        .route(
            "/api/admin/v1/groups/{id}/libraries/{libraryId}",
            put(bind_group_library).delete(unbind_group_library),
        )
        .route("/api/admin/v1/users", post(create_user))
        .route("/api/admin/v1/users/{id}", put(update_user))
        .route_layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

async fn list_groups(State(state): State<AdminAccessState>, _admin: AdminPrincipal) -> Response {
    match GroupAdminStore::new().list_groups(&state.pool).await {
        Ok(groups) => match groups.iter().map(group_dto).collect::<Result<Vec<_>, _>>() {
            Ok(groups) => Json(groups).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => group_error(error),
    }
}

async fn create_group(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<GroupNameRequest>,
) -> Response {
    match GroupAdminStore::new()
        .create_group(&state.pool, &request.name, OffsetDateTime::now_utc())
        .await
    {
        Ok(group) => {
            tracing::info!(
                actor = %admin.public_api_id,
                group = %group.public_api_id,
                "admin created library access group"
            );
            match group_dto(&group) {
                Ok(dto) => (StatusCode::CREATED, Json(dto)).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => group_error(error),
    }
}

async fn get_group(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    let detail = match GroupAdminStore::new().group_detail(&state.pool, id).await {
        Ok(Some(detail)) => detail,
        Ok(None) => return group_error(GroupAdminError::GroupNotFound),
        Err(error) => return group_error(error),
    };
    match detail_dto(&detail) {
        Ok(dto) => Json(dto).into_response(),
        Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
    }
}

async fn rename_group(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<GroupNameRequest>,
) -> Response {
    match GroupAdminStore::new()
        .rename_group(&state.pool, id, &request.name, OffsetDateTime::now_utc())
        .await
    {
        Ok(group) => {
            tracing::info!(
                actor = %admin.public_api_id,
                group = %id,
                "admin renamed library access group"
            );
            match group_dto(&group) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => group_error(error),
    }
}

async fn delete_group(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match GroupAdminStore::new()
        .delete_group_by_public_id(&state.pool, id)
        .await
    {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                group = %id,
                "admin deleted library access group"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => group_error(error),
    }
}

async fn add_group_member(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Response {
    match GroupAdminStore::new()
        .add_member_by_public_ids(&state.pool, id, user_id, OffsetDateTime::now_utc())
        .await
    {
        Ok(_) => {
            tracing::info!(
                actor = %admin.public_api_id,
                group = %id,
                user = %user_id,
                "admin added user to library access group"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => group_error(error),
    }
}

async fn remove_group_member(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Response {
    match GroupAdminStore::new()
        .remove_member_by_public_ids(&state.pool, id, user_id, OffsetDateTime::now_utc())
        .await
    {
        Ok(true) => {
            tracing::info!(
                actor = %admin.public_api_id,
                group = %id,
                user = %user_id,
                "admin removed user from library access group"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => group_error(GroupAdminError::GroupNotFound),
        Err(error) => group_error(error),
    }
}

async fn bind_group_library(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((id, library_id)): Path<(Uuid, Uuid)>,
) -> Response {
    match GroupAdminStore::new()
        .bind_library_by_public_id(&state.pool, id, library_id, OffsetDateTime::now_utc())
        .await
    {
        Ok(_) => {
            tracing::info!(
                actor = %admin.public_api_id,
                group = %id,
                library = %library_id,
                "admin bound library to access group"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => group_error(error),
    }
}

async fn unbind_group_library(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((id, library_id)): Path<(Uuid, Uuid)>,
) -> Response {
    match GroupAdminStore::new()
        .unbind_library_by_public_id(&state.pool, id, library_id, OffsetDateTime::now_utc())
        .await
    {
        Ok(true) => {
            tracing::info!(
                actor = %admin.public_api_id,
                group = %id,
                library = %library_id,
                "admin unbound library from access group"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => group_error(GroupAdminError::GroupNotFound),
        Err(error) => group_error(error),
    }
}

async fn create_user(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<CreateUserRequest>,
) -> Response {
    if request.password.chars().count() < 8 || request.password.chars().count() > MAX_PASSWORD_CHARS
    {
        return admin_error(StatusCode::BAD_REQUEST, "invalid_password");
    }
    let password_hash = match melodee_auth::hash_password(request.password.as_bytes()) {
        Ok(hash) => hash,
        Err(_) => return admin_error(StatusCode::SERVICE_UNAVAILABLE, "store_error"),
    };
    // Fresh random stream signing key; the clear password is never stored,
    // logged, or returned.
    let stream_signing_key = match melodee_auth::generate_refresh_token() {
        Ok(secret) => secret.expose_secret().to_string(),
        Err(_) => return admin_error(StatusCode::SERVICE_UNAVAILABLE, "store_error"),
    };
    let roles = request
        .roles
        .unwrap_or_else(|| vec!["HasStreamRole".to_string()]);
    let create = NativeUserCreate {
        username: &request.username,
        email: &request.email,
        password_hash: &password_hash,
        stream_signing_key: &stream_signing_key,
        is_admin: request.is_admin,
        is_editor: request.is_editor,
        roles: &roles,
    };
    match UserAdminStore::new()
        .create_user(&state.pool, &create, OffsetDateTime::now_utc())
        .await
    {
        Ok(user) => {
            tracing::info!(
                actor = %admin.public_api_id,
                user = %user.public_api_id,
                is_admin = user.is_admin,
                "admin created user"
            );
            match user_dto(&user) {
                Ok(dto) => (StatusCode::CREATED, Json(dto)).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

async fn update_user(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateUserRequest>,
) -> Response {
    let update = NativeUserAdminUpdate {
        is_admin: request.is_admin,
        is_editor: request.is_editor,
        enabled: request.enabled,
        locked: request.locked,
        roles: request.roles,
    };
    match UserAdminStore::new()
        .update_user(&state.pool, id, &update, OffsetDateTime::now_utc())
        .await
    {
        Ok(user) => {
            tracing::info!(
                actor = %admin.public_api_id,
                user = %id,
                enabled = user.enabled,
                locked = user.locked,
                is_admin = user.is_admin,
                "admin updated user"
            );
            match user_dto(&user) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

fn detail_dto(detail: &AccessGroupDetail) -> Result<AccessGroupDetailDto, ()> {
    Ok(AccessGroupDetailDto {
        group: group_dto(&detail.group)?,
        members: detail
            .members
            .iter()
            .map(|member| AccessGroupMemberDto {
                user_id: member.public_api_id,
                username: member.username.clone(),
            })
            .collect(),
        library_ids: detail.library_ids.clone(),
    })
}

fn format_time(value: OffsetDateTime) -> Result<String, ()> {
    value.format(&Rfc3339).map_err(|_| ())
}

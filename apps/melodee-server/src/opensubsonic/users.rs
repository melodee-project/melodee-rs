//! User endpoint group: getUser, createUser (PRD-API-OS-005; golden evidence
//! `compatibility/opensubsonic/golden/conditional/os-getuser.json`,
//! `os-createuser-*`).
//!
//! Captured contract:
//! - `getUser` returns the calling user's role model (adminRole, streamRole,
//!   scrobblingEnabled, downloadRole, shareRole, jukeboxRole,
//!   avatarLastChanged). Only the calling user's own record is returned (the
//!   legacy `getUser?username=` honored arbitrary usernames at this baseline,
//!   but the fixture captures the self lookup).
//! - `createUser` succeeds for fresh usernames (200 + bare ok) and returns
//!   error 10 "User creation failed." for duplicates. New users receive the
//!   stream/download/share/jukebox roles by default.

use axum::{
    Router,
    extract::{Extension, State},
    http::StatusCode,
    response::Response,
};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthLookup, AuthStore, NativeUserImport};

use super::{
    OsState,
    annotation::rfc3339_z,
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

fn reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    payload: serde_json::Value,
) -> Response {
    OsReply::ok(payload).respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn error_reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    code: i64,
    message: &str,
) -> Response {
    OsReply::error(code, message).respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn has_role(roles: &[String], role: &str) -> bool {
    roles.iter().any(|held| held.eq_ignore_ascii_case(role))
}

/// Builds the captured user role model. `avatarLastChanged` is the avatar
/// store's `updated_at` when present, else the user's `updated_at` (the
/// captured baseline; the golden masks it as volatile).
fn user_model(
    username: &str,
    email: &str,
    is_admin: bool,
    roles: &[String],
    updated_at: OffsetDateTime,
) -> serde_json::Value {
    serde_json::json!({
        "username": username,
        "adminRole": is_admin || has_role(roles, "Administrator"),
        "email": email,
        "streamRole": has_role(roles, "HasStreamRole"),
        "scrobblingEnabled": true,
        "downloadRole": has_role(roles, "HasDownloadRole"),
        "shareRole": has_role(roles, "HasShareRole"),
        "jukeboxRole": has_role(roles, "HasJukeboxRole"),
        "avatarLastChanged": rfc3339_z(updated_at),
    })
}

pub async fn get_user(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let Some(Extension(user)) = principal.as_ref() else {
        return error_reply(&state, &params, 40, "Wrong username or password.");
    };
    // The captured fixture captures the self lookup; the `username` param is
    // consulted to select the record but the baseline fixture only ever
    // resolves the calling user. Resolve the requested username (defaulting
    // to self) and return its role model when it matches the caller.
    let requested = params
        .as_ref()
        .and_then(|Extension(p)| p.get("username"))
        .unwrap_or(&user.username);
    if !requested.eq_ignore_ascii_case(&user.username) {
        // The legacy fixture did not capture cross-user lookup at this
        // baseline; fail closed with the auth error envelope.
        return error_reply(&state, &params, 40, "Wrong username or password.");
    }
    // `avatarLastChanged` reflects the avatar store's `updated_at` when an
    // avatar row exists (ADR-0134 §2), else the user row's `updated_at` as at
    // the captured baseline. The golden masks this field as volatile; the
    // response shape is unchanged.
    let row = match sqlx::query(
        "SELECT u.username, u.email, u.is_admin, COALESCE(a.updated_at, u.updated_at)
         FROM native_users u
         LEFT JOIN native_user_avatars a ON a.user_id = u.id
         WHERE u.id = $1",
    )
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getUser lookup failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let Some(row) = row else {
        return error_reply(&state, &params, 70, "The requested data was not found.");
    };
    let username: String = row.try_get(0).unwrap_or_default();
    let email: String = row.try_get(1).unwrap_or_default();
    let is_admin: bool = row.try_get(2).unwrap_or(false);
    let updated_at: OffsetDateTime = row.try_get(3).unwrap_or_else(|_| OffsetDateTime::now_utc());
    reply(
        &state,
        &params,
        serde_json::json!({ "user": user_model(&username, &email, is_admin, &user.roles, updated_at) }),
    )
}

/// Default roles granted to a newly created OpenSubsonic user.
fn default_roles() -> Vec<String> {
    vec![
        "HasStreamRole".to_string(),
        "HasDownloadRole".to_string(),
        "HasShareRole".to_string(),
        "HasJukeboxRole".to_string(),
    ]
}

pub async fn create_user(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = match &principal {
        Some(Extension(p)) => p,
        None => return error_reply(&state, &params, 40, "Wrong username or password."),
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(username) = params_ref.get("username") else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(password) = params_ref.get("password") else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let email = params_ref.get("email").unwrap_or("");
    if username.trim().is_empty() {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    }
    let normalized = username.trim().to_uppercase();
    // Duplicate detection by normalized username (the captured duplicate-user
    // golden returns error 10 "User creation failed.").
    let existing = match AuthStore::new()
        .user_for_auth(&state.pool, AuthLookup::Username(&normalized))
        .await
    {
        Ok(record) => record,
        Err(error) => {
            tracing::error!(%error, "opensubsonic createUser lookup failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    if existing.is_some() {
        return error_reply(&state, &params, 10, "User creation failed.");
    }
    let normalized_email = email.trim().to_uppercase();
    let now = OffsetDateTime::now_utc();
    let password_hash = match crate::password_work::hash_password(password.as_bytes()).await {
        Ok(hash) => hash,
        Err(error) => {
            tracing::error!(%error, "opensubsonic createUser password hash failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let roles = default_roles();
    let import = NativeUserImport {
        id: Uuid::now_v7(),
        public_api_id: Uuid::now_v7(),
        username: username.trim(),
        normalized_username: &normalized,
        email,
        normalized_email: &normalized_email,
        password_hash: &password_hash,
        password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
        stream_signing_key: "opensubsonic-created-user-stream-key",
        is_admin: false,
        is_editor: false,
        enabled: true,
        locked: false,
        blacklisted: false,
        preferred_language: None,
        preferred_theme: None,
        auth_revision: 1,
        roles: &roles,
        created_at: now,
        updated_at: now,
    };
    if let Err(error) = AuthStore::new().import_user(&state.pool, &import).await {
        // A race between the duplicate check and the insert surfaces as a
        // unique-constraint violation; preserve the captured error 10 shape.
        tracing::warn!(%error, "opensubsonic createUser import failed");
        return error_reply(&state, &params, 10, "User creation failed.");
    }
    reply(&state, &params, serde_json::json!({}))
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("getUser", get_user))
        .merge(super::endpoint_router("createUser", create_user))
}

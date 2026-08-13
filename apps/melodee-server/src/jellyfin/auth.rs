//! Jellyfin authentication and token management (PRD-API-JF-003, ADR-0131).
//!
//! `POST /Users/AuthenticateByName {Username, Pw}` (the DTO reads ONLY `Pw`;
//! a body with `Password` and no `Pw` is a captured 400) returns
//! `{User, SessionInfo, AccessToken, ServerId}`. Tokens are 32 random bytes
//! hex-encoded (64 lowercase hex); stored as HMAC-SHA256(pepper, salt+token)
//! with an 8-char cleartext prefix for lookup; expiry 168h; max 10 active.
//! Five transports: Authorization MediaBrowser, X-Emby-Authorization,
//! X-MediaBrowser-Token, X-Emby-Token, api_key query. Logout revokes
//! immediately (204, idempotent).

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::JfState;
use super::browse::json_response;
use super::error::{bad_request, forbidden, internal_server_error, not_found, unauthorized};
use super::model::{self, user_json};

fn internal_error() -> Response {
    internal_server_error()
}

type HmacSha256 = Hmac<Sha256>;

/// Authenticated Jellyfin identity installed by [`authenticate`].
#[derive(Debug, Clone)]
pub struct JfPrincipal {
    pub user_id: Uuid,
    pub public_api_id: Uuid,
    pub username: String,
    pub is_admin: bool,
    pub is_editor: bool,
    pub roles: Vec<String>,
}

/// Jellyfin login request body. The captured DTO reads ONLY `Pw` (not
/// `Password`); a body with `Password` and no `Pw` is a captured 400.
#[derive(Debug, Deserialize)]
pub struct AuthenticateByNameRequest {
    #[serde(rename = "Username")]
    pub username: Option<String>,
    #[serde(rename = "Pw")]
    pub pw: Option<String>,
}

pub fn router() -> Router<JfState> {
    Router::new()
        .route("/Users/AuthenticateByName", post(authenticate_by_name))
        .route("/Sessions/Logout", post(logout))
        .route("/Users/Me", get(users_me))
        .route("/Users/{id}", get(users_by_id))
        .route("/Users", get(users_list))
        .route("/Users/Public", get(users_public))
}

pub async fn authenticate_by_name(
    State(state): State<JfState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // The captured DTO reads ONLY `Pw`; a body with `Password` and no `Pw`
    // fails JSON deserialization with a captured validation-error 400.
    let request: AuthenticateByNameRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return bad_request("Username and password are required."),
    };

    let Some(username) = request.username.as_ref().filter(|u| !u.is_empty()) else {
        return bad_request("Username and password are required.");
    };
    let Some(pw) = request.pw.as_ref().filter(|p| !p.is_empty()) else {
        return bad_request("Username and password are required.");
    };

    let normalized = username.trim().to_uppercase();
    let record = match melodee_persistence::AuthStore::new()
        .user_for_auth(
            &state.pool,
            melodee_persistence::AuthLookup::Username(&normalized),
        )
        .await
    {
        Ok(Some(record)) => record,
        _ => return unauthorized_401(),
    };
    let password_valid = melodee_auth::verify_password(
        pw.as_bytes(),
        record.password_hash_algorithm,
        record.password_hash.expose_secret(),
    )
    .unwrap_or(false);
    if !password_valid {
        return unauthorized_401();
    }
    // Captured order: credentials first, then the explicit locked check.
    if record.user.locked {
        return forbidden("User account is locked.");
    }
    if !record.user.can_authenticate() {
        return unauthorized_401();
    }

    // Captured client metadata from the MediaBrowser auth header kv pairs
    // (bounded to 255 chars as the legacy insert does).
    let client_info = parse_client_info(&headers);

    // Issue a new token: 32 random bytes hex (64 chars), stored as
    // HMAC-SHA256(pepper, salt+token) with an 8-char cleartext prefix.
    let token = generate_token();
    let token_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    let expires_at = now + time::Duration::hours(state.token_expiry_hours);
    if let Err(error) = issue_token(
        &state,
        token_id,
        record.user.id,
        &token,
        now,
        expires_at,
        &client_info,
    )
    .await
    {
        tracing::error!(%error, "jellyfin token issue failed");
        return unauthorized_401();
    }

    let user = user_json(
        &state.server_id,
        record.user.public_api_id,
        &record.user.username,
        record.user.is_admin,
        false,
        record.user.roles.iter().any(|role| role == "HasStreamRole"),
        record
            .user
            .roles
            .iter()
            .any(|role| role == "HasDownloadRole"),
        now,
    );
    let session_info = model::session_info_json(
        &state.server_id,
        &token_id.simple().to_string(),
        record.user.public_api_id,
        &record.user.username,
        client_info.client.as_deref(),
        client_info.device.as_deref(),
        client_info.device_id.as_deref(),
        client_info.version.as_deref(),
        now,
    );
    let body = json!({
        "User": user,
        "SessionInfo": session_info,
        "AccessToken": token,
        "ServerId": state.server_id
    });
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

/// Client metadata parsed from the MediaBrowser/X-Emby-Authorization header
/// (legacy `JellyfinTokenParser.ParseFromRequest`).
#[derive(Debug, Default)]
struct ClientInfo {
    client: Option<String>,
    device: Option<String>,
    device_id: Option<String>,
    version: Option<String>,
}

fn parse_client_info(headers: &HeaderMap) -> ClientInfo {
    let header = headers
        .get(header::AUTHORIZATION)
        .or_else(|| headers.get("X-Emby-Authorization"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let mut info = ClientInfo::default();
    for part in header.split(',') {
        let part = part.trim();
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        // Column cap is 200 chars (migration 0021).
        let bounded = |value: &str| Some(value.chars().take(200).collect::<String>());
        match key.trim() {
            "Client" => info.client = bounded(value),
            "Device" => info.device = bounded(value),
            "DeviceId" => info.device_id = bounded(value),
            "Version" => info.version = bounded(value),
            _ => {}
        }
    }
    info
}

fn unauthorized_401() -> Response {
    super::error::JellyfinProblemDetails::unauthorized("Invalid username or password.")
        .into_json_response(StatusCode::UNAUTHORIZED)
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    let _ = getrandom::fill(&mut bytes);
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .map(|b| {
            (
                HEX[(b >> 4) as usize] as char,
                HEX[(b & 0xf) as usize] as char,
            )
        })
        .flat_map(|(hi, lo)| [hi, lo])
        .collect()
}

fn token_digest(pepper: &[u8], salt: &[u8], token: &str) -> [u8; 32] {
    let Ok(mut mac) = HmacSha256::new_from_slice(pepper) else {
        return [0u8; 32];
    };
    mac.update(salt);
    mac.update(token.as_bytes());
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

async fn issue_token(
    state: &JfState,
    token_id: Uuid,
    user_id: Uuid,
    token: &str,
    now: OffsetDateTime,
    expires_at: OffsetDateTime,
    client_info: &ClientInfo,
) -> Result<(), sqlx::Error> {
    let mut tx = state.pool.begin().await?;
    // Enforce max-active: soft-revoke the oldest when over the limit.
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM jellyfin_access_tokens WHERE user_id=$1 AND revoked_at IS NULL AND expires_at > now()")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    if active >= state.max_active_tokens as i64 {
        sqlx::query("UPDATE jellyfin_access_tokens SET revoked_at = now(), updated_at = now() WHERE id = (SELECT id FROM jellyfin_access_tokens WHERE user_id=$1 AND revoked_at IS NULL ORDER BY issued_at ASC, id LIMIT 1)")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    let salt = {
        let mut s = [0u8; 16];
        let _ = getrandom::fill(&mut s);
        s.to_vec()
    };
    let prefix = &token[..8];
    let digest = token_digest(state.token_pepper.as_bytes(), &salt, token);
    sqlx::query("INSERT INTO jellyfin_access_tokens (id, user_id, token_prefix, token_digest, token_salt, client, device_name, device_id, issued_at, expires_at, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$9,$9)")
        .bind(token_id)
        .bind(user_id)
        .bind(prefix)
        .bind(digest)
        .bind(&salt)
        .bind(&client_info.client)
        .bind(&client_info.device)
        .bind(&client_info.device_id)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Extracts and validates a token from any of the five captured transports.
/// Returns the resolved principal, or None for any failure (indistinguishable
/// 401 per ADR-0131).
pub async fn resolve_token(state: &JfState, headers: &HeaderMap, uri: &Uri) -> Option<JfPrincipal> {
    let token = extract_token(headers, uri)?;
    validate_token(state, &token).await
}

pub(crate) fn extract_token(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    // Authorization: MediaBrowser ... Token="..."
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        && let Some(token) = extract_token_from_auth_header(auth)
    {
        return Some(token);
    }
    // X-Emby-Authorization: ... Token="..."
    if let Some(auth) = headers
        .get("X-Emby-Authorization")
        .and_then(|v| v.to_str().ok())
        && let Some(token) = extract_token_from_auth_header(auth)
    {
        return Some(token);
    }
    // X-MediaBrowser-Token / X-Emby-Token
    for name in &["X-MediaBrowser-Token", "X-Emby-Token"] {
        if let Some(value) = headers.get(*name).and_then(|v| v.to_str().ok()) {
            return Some(value.to_string());
        }
    }
    // api_key query param
    if let Some(query) = uri.query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("api_key=") {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn extract_token_from_auth_header(header: &str) -> Option<String> {
    // MediaBrowser Token="abc123", Client="...", Device="...", DeviceId="..."
    // or: MediaBrowser token=abc123 (no quotes, no comma after scheme)
    for part in header.split(',') {
        let part = part.trim();
        // The first segment may include the scheme prefix: "MediaBrowser Token=..."
        if let Some(idx) = part.find("Token=") {
            let rest = &part[idx + "Token=".len()..];
            return Some(rest.trim_matches('"').to_string());
        }
        if let Some(idx) = part.find("token=") {
            let rest = &part[idx + "token=".len()..];
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}

async fn validate_token(state: &JfState, token: &str) -> Option<JfPrincipal> {
    if token.len() < 8 {
        return None;
    }
    let prefix = &token[..8];
    let row = sqlx::query("SELECT t.id, t.user_id, t.token_digest, t.token_salt, t.expires_at, t.revoked_at, u.public_api_id, u.username, u.is_admin, u.is_editor, u.enabled, u.locked, u.blacklisted FROM jellyfin_access_tokens t JOIN native_users u ON u.id = t.user_id WHERE t.token_prefix = $1")
        .bind(prefix)
        .fetch_optional(&state.pool)
        .await
        .ok()??;
    let expires_at: OffsetDateTime = row.try_get("expires_at").ok()?;
    let revoked_at: Option<OffsetDateTime> = row.try_get("revoked_at").ok()?;
    if revoked_at.is_some() || expires_at <= OffsetDateTime::now_utc() {
        return None;
    }
    let digest: Vec<u8> = row.try_get("token_digest").ok()?;
    let salt: Vec<u8> = row.try_get("token_salt").ok()?;
    let expected = token_digest(state.token_pepper.as_bytes(), &salt, token);
    if digest != expected {
        return None;
    }
    let user_id: Uuid = row.try_get("user_id").ok()?;
    let public_api_id: Uuid = row.try_get("public_api_id").ok()?;
    let username: String = row.try_get("username").ok()?;
    let is_admin: bool = row.try_get("is_admin").unwrap_or(false);
    let is_editor: bool = row.try_get("is_editor").unwrap_or(false);
    let roles: Vec<String> =
        sqlx::query_scalar("SELECT role FROM native_user_roles WHERE user_id = $1 ORDER BY role")
            .bind(user_id)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();
    Some(JfPrincipal {
        user_id,
        public_api_id,
        username,
        is_admin,
        is_editor,
        roles,
    })
}

pub async fn logout(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if let Some(principal) = resolve_token(&state, &headers, &uri).await {
        let token = extract_token(&headers, &uri).unwrap_or_default();
        if token.len() >= 8 {
            let prefix = &token[..8];
            let _ = sqlx::query("UPDATE jellyfin_access_tokens SET revoked_at = now(), updated_at = now() WHERE token_prefix = $1 AND user_id = $2 AND revoked_at IS NULL")
                .bind(prefix)
                .bind(principal.user_id)
                .execute(&state.pool)
                .await;
        }
    }
    // Idempotent 204 even for garbage tokens (captured).
    StatusCode::NO_CONTENT.into_response()
}

pub async fn users_me(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    json_response(user_json(
        &state.server_id,
        principal.public_api_id,
        &principal.username,
        principal.is_admin,
        false,
        principal.roles.iter().any(|role| role == "HasStreamRole"),
        principal.roles.iter().any(|role| role == "HasDownloadRole"),
        OffsetDateTime::now_utc(),
    ))
}

pub async fn users_by_id(
    State(state): State<JfState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(parsed) = model::parse_id(&id) else {
        return bad_request("Invalid user ID format.");
    };
    if parsed != principal.public_api_id && !principal.is_admin {
        return forbidden("Cannot access other users.");
    }
    // Captured: returns the requested user, not the bearer.
    let row = sqlx::query(
        "SELECT u.public_api_id, u.username, u.is_admin, u.locked, \
                COALESCE((SELECT array_agg(ur.role ORDER BY ur.role) \
                            FROM native_user_roles ur WHERE ur.user_id = u.id), '{}') \
           FROM native_users u \
          WHERE u.public_api_id = $1 AND u.enabled = TRUE AND u.blacklisted = FALSE",
    )
    .bind(parsed)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some(row)) => {
            let public_id: Uuid = row.get(0);
            let username: String = row.get(1);
            let is_admin: bool = row.get(2);
            let locked: bool = row.get(3);
            let roles: Vec<String> = row.get(4);
            json_response(user_json(
                &state.server_id,
                public_id,
                &username,
                is_admin,
                locked,
                roles.iter().any(|role| role == "HasStreamRole"),
                roles.iter().any(|role| role == "HasDownloadRole"),
                OffsetDateTime::now_utc(),
            ))
        }
        Ok(None) => not_found("User not found."),
        Err(error) => {
            tracing::error!(%error, "jellyfin user fetch failed");
            not_found("User not found.")
        }
    }
}

pub async fn users_list(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    // Captured: non-admins see only themselves; admins see up to 100 users
    // ordered by username.
    let rows = sqlx::query(
        "SELECT u.public_api_id, u.username, u.is_admin, u.locked, \
                COALESCE((SELECT array_agg(ur.role ORDER BY ur.role) \
                            FROM native_user_roles ur WHERE ur.user_id = u.id), '{}') \
           FROM native_users u \
          WHERE u.enabled = TRUE AND u.blacklisted = FALSE \
            AND ($1::boolean OR u.id = $2) \
          ORDER BY u.username COLLATE \"C\" \
          LIMIT 100",
    )
    .bind(principal.is_admin)
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(rows) => {
            let users: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|row| {
                    let roles: Vec<String> = row.get(4);
                    user_json(
                        &state.server_id,
                        row.get(0),
                        &row.get::<String, _>(1),
                        row.get(2),
                        row.get(3),
                        roles.iter().any(|role| role == "HasStreamRole"),
                        roles.iter().any(|role| role == "HasDownloadRole"),
                        OffsetDateTime::now_utc(),
                    )
                })
                .collect();
            json_response(serde_json::Value::Array(users))
        }
        Err(error) => {
            tracing::error!(%error, "jellyfin users list failed");
            internal_error()
        }
    }
}

pub async fn users_public(State(state): State<JfState>) -> Response {
    let _ = state;
    // Captured: no local users are public (`Ok(Array.Empty<JellyfinUser>())`);
    // clients fall back to username/password login.
    let mut response = (StatusCode::OK, Json(json!([]))).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

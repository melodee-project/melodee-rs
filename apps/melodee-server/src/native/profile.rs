//! Additive native account self-service routes for MEL-2 (ADR-0178).
//!
//! These routes are intentionally separate from frozen `/api/v1/user/me` and
//! contain no administrator or role mutation surface. MeloAmp owns the
//! listener-facing UI; this module supplies its server contract.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_persistence::{
    MIN_PASSWORD_CHARS, ProfileUpdate, RegistrationRequest, SettingsResolver, UserProfileError,
    UserProfileStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use super::{Authenticated, NativeState};

const MAX_PROFILE_PASSWORD_CHARS: usize = 1024;
const MAX_PROFILE_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RegisterRequest {
    #[serde(alias = "username")]
    user_name: String,
    email: String,
    password: String,
    register_private_code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProfilePatchRequest {
    username: Option<String>,
    email: Option<String>,
    description: Option<Option<String>>,
    time_zone_id: Option<String>,
    preferred_language: Option<Option<String>>,
    preferred_theme: Option<Option<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileResponse {
    public_api_id: Uuid,
    username: String,
    email: String,
    description: Option<String>,
    time_zone_id: String,
    preferred_language: Option<String>,
    preferred_theme: Option<String>,
    is_admin: bool,
    is_editor: bool,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct MessageResponse {
    message: &'static str,
}

pub fn public_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/auth/register", post(register))
        .layer(DefaultBodyLimit::max(MAX_PROFILE_BODY_BYTES))
}

pub fn protected_router() -> Router<NativeState> {
    Router::new()
        .route(
            "/api/v1/user/me/profile",
            get(get_profile).patch(patch_profile),
        )
        .layer(DefaultBodyLimit::max(MAX_PROFILE_BODY_BYTES))
}

async fn register(
    State(state): State<NativeState>,
    Json(request): Json<RegisterRequest>,
) -> Response {
    let resolver = SettingsResolver::new(&state.pool);
    let disabled = match resolver.resolve("register.disabled").await {
        Ok(setting) => setting
            .value
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        Err(error) => {
            tracing::error!(error = ?error, "registration setting resolution failed");
            return unavailable();
        }
    };
    if disabled {
        return profile_error(
            StatusCode::FORBIDDEN,
            "registration_disabled",
            "Account registration is disabled",
        );
    }
    if request.password.chars().count() < MIN_PASSWORD_CHARS
        || request.password.chars().count() > MAX_PROFILE_PASSWORD_CHARS
    {
        return profile_error(
            StatusCode::BAD_REQUEST,
            "invalid_password",
            "Password length is outside the supported range",
        );
    }
    let private_code = match resolver.resolve("register.privateCode").await {
        Ok(setting) => setting
            .value
            .and_then(|value| value.as_str().map(str::to_owned)),
        Err(error) => {
            tracing::error!(error = ?error, "registration private-code resolution failed");
            return unavailable();
        }
    };
    if let Some(expected) = private_code.filter(|value| !value.is_empty()) {
        let supplied = request.register_private_code.as_deref().unwrap_or_default();
        if !secret_equal(&expected, supplied) {
            return profile_error(
                StatusCode::UNAUTHORIZED,
                "invalid_registration_code",
                "Invalid access code",
            );
        }
    }
    let password_hash = match crate::password_work::hash_password(request.password.as_bytes()).await
    {
        Ok(hash) => hash,
        Err(error) => {
            tracing::error!(error = ?error, "registration password hashing failed");
            return unavailable();
        }
    };
    let stream_signing_key = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
    let now = OffsetDateTime::now_utc();
    match UserProfileStore::new()
        .register(
            &state.pool,
            &RegistrationRequest {
                username: &request.user_name,
                email: &request.email,
                password_hash: &password_hash,
                stream_signing_key: &stream_signing_key,
                now,
            },
        )
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(MessageResponse {
                message: "Registration successful",
            }),
        )
            .into_response(),
        Err(UserProfileError::Conflict) => profile_error(
            StatusCode::CONFLICT,
            "identity_conflict",
            "Username or email is already in use",
        ),
        Err(UserProfileError::InvalidValue(_)) => profile_error(
            StatusCode::BAD_REQUEST,
            "invalid_profile",
            "One or more profile fields are invalid",
        ),
        Err(UserProfileError::Database(error)) => {
            tracing::error!(error = ?error, "registration persistence failed");
            unavailable()
        }
        Err(UserProfileError::NotFound) => unavailable(),
    }
}

async fn get_profile(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Response {
    match melodee_persistence::AuthStore::new()
        .user_by_id(&state.pool, principal.user_id)
        .await
    {
        Ok(Some(user)) => match profile_response(&user) {
            Ok(response) => Json(response).into_response(),
            Err(()) => unavailable(),
        },
        Ok(None) => profile_error(StatusCode::NOT_FOUND, "not_found", "User not found"),
        Err(error) => {
            tracing::error!(error = ?error, "profile lookup failed");
            unavailable()
        }
    }
}

async fn patch_profile(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<ProfilePatchRequest>,
) -> Response {
    let update = ProfileUpdate {
        username: request.username,
        email: request.email,
        description: request.description,
        time_zone_id: request.time_zone_id,
        preferred_language: request.preferred_language,
        preferred_theme: request.preferred_theme,
    };
    match UserProfileStore::new()
        .update(
            &state.pool,
            principal.user_id,
            &update,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(user) => match profile_response(&user) {
            Ok(response) => Json(response).into_response(),
            Err(()) => unavailable(),
        },
        Err(UserProfileError::Conflict) => profile_error(
            StatusCode::CONFLICT,
            "identity_conflict",
            "Username or email is already in use",
        ),
        Err(UserProfileError::InvalidValue(_)) => profile_error(
            StatusCode::BAD_REQUEST,
            "invalid_profile",
            "One or more profile fields are invalid",
        ),
        Err(UserProfileError::NotFound) => {
            profile_error(StatusCode::NOT_FOUND, "not_found", "User not found")
        }
        Err(UserProfileError::Database(error)) => {
            tracing::error!(error = ?error, "profile update failed");
            unavailable()
        }
    }
}

fn profile_response(user: &melodee_persistence::StoredNativeUser) -> Result<ProfileResponse, ()> {
    Ok(ProfileResponse {
        public_api_id: user.public_api_id,
        username: user.username.clone(),
        email: user.email.clone(),
        description: user.description.clone(),
        time_zone_id: user.time_zone_id.clone(),
        preferred_language: user.preferred_language.clone(),
        preferred_theme: user.preferred_theme.clone(),
        is_admin: user.is_admin,
        is_editor: user.is_editor,
        updated_at: user.updated_at.format(&Rfc3339).map_err(|_| ())?,
    })
}

fn secret_equal(expected: &str, supplied: &str) -> bool {
    let expected = Sha256::digest(expected.as_bytes());
    let supplied = Sha256::digest(supplied.as_bytes());
    expected == supplied
}

fn profile_error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "code": code,
            "message": message,
            "correlationId": super::super::correlation::envelope_id(),
        })),
    )
        .into_response()
}

fn unavailable() -> Response {
    profile_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "SERVICE_UNAVAILABLE",
        "Service temporarily unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::secret_equal;

    #[test]
    fn private_code_comparison_accepts_only_exact_value() {
        assert!(secret_equal("s3cret", "s3cret"));
        assert!(!secret_equal("s3cret", "S3cret"));
        assert!(!secret_equal("s3cret", ""));
    }
}

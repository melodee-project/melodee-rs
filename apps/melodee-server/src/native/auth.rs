//! Frozen native-v1 authentication and refresh lifecycle (ADR-0124).

use std::{env, fmt, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_auth::{
    NativeAccessClaims, NativeCookieClaims, PasswordHashAlgorithm, RefreshTokenDigest,
    SecretString, generate_password_reset_token, generate_refresh_token, hash_password,
    issue_access_token, issue_cookie_session, validate_access_token, validate_cookie_session,
    verify_password,
};
use melodee_persistence::{
    AuthLookup, AuthStore, AuthStoreError, PasswordResetIssue, PasswordResetOutcome,
    RateLimitOutcome, RateLimitPolicy, RefreshFamilyIssue, RefreshRotationOutcome,
    RefreshRotationRequest, SocialLoginLink, StoredNativeUser, StoredSocialLogin,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{Authenticated, NativeClientBinding, NativePrincipal, NativeState, dto::UserDto};

pub const ENV_NATIVE_JWT_KEY: &str = "MELODEE__NATIVE__AUTH__JWT_KEY";
pub const ENV_NATIVE_MAX_SESSION_DAYS: &str = "MELODEE__NATIVE__AUTH__MAX_SESSION_DAYS";
pub const ENV_NATIVE_DEVELOPMENT_RESET_TOKENS: &str =
    "MELODEE__NATIVE__AUTH__DEVELOPMENT_RESET_TOKENS";
pub const ENV_NATIVE_CORS_ORIGINS: &str = "MELODEE__NATIVE__CORS__ALLOWED_ORIGINS";
pub const ENV_NATIVE_RATE_ATTEMPTS: &str = "MELODEE__NATIVE__AUTH__RATE_ATTEMPTS";
pub const ENV_NATIVE_RATE_WINDOW_SECONDS: &str = "MELODEE__NATIVE__AUTH__RATE_WINDOW_SECONDS";
pub const ENV_NATIVE_RATE_MAX_KEYS: &str = "MELODEE__NATIVE__AUTH__RATE_MAX_KEYS";
pub const ENV_GOOGLE_ENABLED: &str = "MELODEE__NATIVE__AUTH__GOOGLE__ENABLED";
pub const ENV_GOOGLE_CLIENT_ID: &str = "MELODEE__NATIVE__AUTH__GOOGLE__CLIENT_ID";
pub const ENV_GOOGLE_JWKS_URL: &str = "MELODEE__NATIVE__AUTH__GOOGLE__JWKS_URL";
pub const ENV_GOOGLE_HOSTED_DOMAINS: &str = "MELODEE__NATIVE__AUTH__GOOGLE__HOSTED_DOMAINS";
const ACCESS_LIFETIME: Duration = Duration::minutes(15);
const REFRESH_LIFETIME: Duration = Duration::days(30);
const COOKIE_LIFETIME: Duration = Duration::days(14);
const NATIVE_COOKIE_NAME: &str = "melodee_auth";
const DUMMY_BCRYPT: &str = "$2b$12$BP5HSYGph9ZF1C7Jco2fieFhmgb1LzWDjT5V8ZrL4Pk3C9Drn5lDW";
const MAX_AUTH_BODY_BYTES: usize = 64 * 1024;
const MAX_SECRET_BYTES: usize = 4096;
const MAX_CORS_CONFIG_BYTES: usize = 32 * 1024;
const MAX_CONFIG_VALUE_BYTES: usize = 2048;
const INSECURE_EXAMPLE_SIGNING_KEY: &str = "replace-with-at-least-32-random-bytes";

/// Validated native authentication configuration. Debug is secret-safe.
#[derive(Clone)]
pub struct NativeAuthService {
    signing_key: SecretString,
    max_session_days: i64,
    development_reset_tokens: bool,
    rate_attempts: i32,
    rate_window_seconds: i64,
    rate_max_keys: i64,
    google: Option<Arc<GoogleOidcService>>,
}

#[derive(Debug)]
struct GoogleOidcService {
    client: reqwest::Client,
    jwks_url: reqwest::Url,
    audience: String,
    hosted_domains: Vec<String>,
    cache: Mutex<Option<CachedJwks>>,
}

#[derive(Debug)]
struct CachedJwks {
    set: jsonwebtoken::jwk::JwkSet,
    expires_at: OffsetDateTime,
    generation: u64,
}

impl fmt::Debug for NativeAuthService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeAuthService")
            .field("signing_key", &self.signing_key)
            .field("max_session_days", &self.max_session_days)
            .field("development_reset_tokens", &self.development_reset_tokens)
            .field("rate_attempts", &self.rate_attempts)
            .field("rate_window_seconds", &self.rate_window_seconds)
            .field("rate_max_keys", &self.rate_max_keys)
            .field("google_enabled", &self.google.is_some())
            .finish()
    }
}

impl GoogleOidcService {
    fn new(
        url: &str,
        audience: String,
        hosted_domains: Vec<String>,
    ) -> Result<Self, NativeAuthConfigError> {
        Self::new_with_loopback_policy(url, audience, hosted_domains, false)
    }

    fn new_with_loopback_policy(
        url: &str,
        audience: String,
        hosted_domains: Vec<String>,
        allow_test_loopback_http: bool,
    ) -> Result<Self, NativeAuthConfigError> {
        if url.len() > MAX_CONFIG_VALUE_BYTES {
            return Err(NativeAuthConfigError::InvalidGoogleConfig);
        }
        let jwks_url =
            reqwest::Url::parse(url).map_err(|_| NativeAuthConfigError::InvalidGoogleConfig)?;
        let local_test = jwks_url
            .host_str()
            .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1"));
        if audience.trim().is_empty()
            || audience.len() > MAX_CONFIG_VALUE_BYTES
            || hosted_domains.len() > 100
            || hosted_domains.iter().any(|domain| {
                domain.is_empty()
                    || domain.len() > 253
                    || domain != &domain.to_lowercase()
                    || domain.contains(char::is_whitespace)
            })
            || jwks_url.host_str().is_none()
            || !jwks_url.username().is_empty()
            || jwks_url.password().is_some()
            || jwks_url.fragment().is_some()
            || (jwks_url.scheme() != "https" && !(allow_test_loopback_http && local_test))
        {
            return Err(NativeAuthConfigError::InvalidGoogleConfig);
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| NativeAuthConfigError::InvalidGoogleConfig)?;
        Ok(Self {
            client,
            jwks_url,
            audience,
            hosted_domains,
            cache: Mutex::new(None),
        })
    }

    async fn validate(&self, token: &str) -> Option<GoogleClaims> {
        if token.len() > 16_384 {
            return None;
        }
        let header = jsonwebtoken::decode_header(token).ok()?;
        if header.alg != jsonwebtoken::Algorithm::RS256 {
            return None;
        }
        let kid = header.kid?;
        let (mut set, generation) = self.jwks(None).await?;
        let jwk = match set.find(&kid) {
            Some(jwk) => jwk,
            None => {
                set = self.jwks(Some(generation)).await?.0;
                set.find(&kid)?
            }
        };
        if jwk.common.key_algorithm != Some(jsonwebtoken::jwk::KeyAlgorithm::RS256) {
            return None;
        }
        let key = jsonwebtoken::DecodingKey::from_jwk(jwk).ok()?;
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.leeway = 0;
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.set_issuer(&["https://accounts.google.com", "accounts.google.com"]);
        validation.set_audience(&[&self.audience]);
        let claims = jsonwebtoken::decode::<GoogleClaims>(token, &key, &validation)
            .ok()?
            .claims;
        if claims.sub.trim().is_empty()
            || claims.sub.len() > 500
            || !claims.email_verified
            || claims.email.trim().is_empty()
            || claims.email.len() > 320
            || claims.hd.as_ref().is_some_and(|domain| domain.len() > 253)
        {
            return None;
        }
        if !self.hosted_domains.is_empty()
            && !claims
                .hd
                .as_deref()
                .is_some_and(|domain| self.hosted_domains.iter().any(|item| item == domain))
        {
            return None;
        }
        Some(claims)
    }

    /// Returns the cached set or refreshes it exactly once for all callers that
    /// observed the same generation. This keeps an unknown `kid` burst from
    /// serializing one outbound request per waiter behind the mutex.
    async fn jwks(
        &self,
        refresh_after_generation: Option<u64>,
    ) -> Option<(jsonwebtoken::jwk::JwkSet, u64)> {
        let now = OffsetDateTime::now_utc();
        // Hold this lock through fetch to provide process-local singleflight.
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            let another_caller_refreshed =
                refresh_after_generation.is_some_and(|observed| observed != cached.generation);
            if another_caller_refreshed
                || (refresh_after_generation.is_none() && now < cached.expires_at)
            {
                return Some((cached.set.clone(), cached.generation));
            }
        }
        let mut response = self.client.get(self.jwks_url.clone()).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        if response
            .content_length()
            .is_some_and(|length| length > 256 * 1024)
        {
            return None;
        }
        let max_age = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_max_age)
            .unwrap_or(300)
            .clamp(60, 3600);
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len().saturating_add(chunk.len()) > 256 * 1024 {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        let set = serde_json::from_slice::<jsonwebtoken::jwk::JwkSet>(&bytes).ok()?;
        if set.keys.is_empty() || set.keys.len() > 32 {
            return None;
        }
        let generation = cache
            .as_ref()
            .map_or(1, |cached| cached.generation.saturating_add(1));
        *cache = Some(CachedJwks {
            set: set.clone(),
            expires_at: now + Duration::seconds(i64::try_from(max_age).ok()?),
            generation,
        });
        Some((set, generation))
    }
}

fn parse_max_age(value: &str) -> Option<u64> {
    value.split(',').find_map(|directive| {
        directive
            .trim()
            .strip_prefix("max-age=")?
            .parse::<u64>()
            .ok()
    })
}

#[derive(Clone, Debug, Deserialize)]
struct GoogleClaims {
    sub: String,
    email: String,
    email_verified: bool,
    hd: Option<String>,
    #[serde(rename = "exp")]
    _exp: u64,
    #[serde(rename = "iss")]
    _iss: String,
    #[serde(rename = "aud")]
    _aud: String,
}

impl NativeAuthService {
    /// Constructs validated native auth configuration.
    pub fn new(signing_key: String, max_session_days: i64) -> Result<Self, NativeAuthConfigError> {
        if signing_key.len() < 32
            || signing_key.len() > MAX_SECRET_BYTES
            || signing_key == INSECURE_EXAMPLE_SIGNING_KEY
        {
            return Err(NativeAuthConfigError::WeakSigningKey);
        }
        if !(30..=365).contains(&max_session_days) {
            return Err(NativeAuthConfigError::InvalidSessionDays);
        }
        Ok(Self {
            signing_key: SecretString::new(signing_key),
            max_session_days,
            development_reset_tokens: false,
            rate_attempts: 10,
            rate_window_seconds: 60,
            rate_max_keys: 100_000,
            google: None,
        })
    }

    /// Loads configuration without ever including the secret value in errors.
    pub fn from_env() -> Result<Self, NativeAuthConfigError> {
        let key =
            env::var(ENV_NATIVE_JWT_KEY).map_err(|_| NativeAuthConfigError::MissingSigningKey)?;
        let max_session_days = env::var(ENV_NATIVE_MAX_SESSION_DAYS)
            .ok()
            .map(|value| value.parse::<i64>())
            .transpose()
            .map_err(|_| NativeAuthConfigError::InvalidSessionDays)?
            .unwrap_or(90);
        let mut service = Self::new(key, max_session_days)?;
        service.development_reset_tokens = env::var(ENV_NATIVE_DEVELOPMENT_RESET_TOKENS)
            .ok()
            .map(|value| value.parse::<bool>())
            .transpose()
            .map_err(|_| NativeAuthConfigError::InvalidDevelopmentMode)?
            .unwrap_or(false);
        service.rate_attempts = env_i32(ENV_NATIVE_RATE_ATTEMPTS, 10, 1, 10_000)?;
        service.rate_window_seconds = env_i64(ENV_NATIVE_RATE_WINDOW_SECONDS, 60, 1, 86_400)?;
        service.rate_max_keys = env_i64(ENV_NATIVE_RATE_MAX_KEYS, 100_000, 1, 10_000_000)?;
        if env::var(ENV_GOOGLE_ENABLED)
            .ok()
            .map(|value| value.parse::<bool>())
            .transpose()
            .map_err(|_| NativeAuthConfigError::InvalidGoogleConfig)?
            .unwrap_or(false)
        {
            let audience = env::var(ENV_GOOGLE_CLIENT_ID)
                .map_err(|_| NativeAuthConfigError::InvalidGoogleConfig)?;
            let url = env::var(ENV_GOOGLE_JWKS_URL)
                .unwrap_or_else(|_| "https://www.googleapis.com/oauth2/v3/certs".to_string());
            let hosted_domain_config = env::var(ENV_GOOGLE_HOSTED_DOMAINS).unwrap_or_default();
            if hosted_domain_config.len() > MAX_CORS_CONFIG_BYTES {
                return Err(NativeAuthConfigError::InvalidGoogleConfig);
            }
            let hosted_domains = hosted_domain_config
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_lowercase)
                .collect();
            service.google = Some(Arc::new(GoogleOidcService::new(
                &url,
                audience,
                hosted_domains,
            )?));
        }
        Ok(service)
    }

    /// Enables clear reset-token responses only for deterministic local tests.
    #[doc(hidden)]
    #[must_use]
    pub fn with_development_reset_tokens_for_tests(mut self) -> Self {
        self.development_reset_tokens = true;
        self
    }

    /// Enables Google OIDC with the same validation used by environment
    /// configuration. HTTP is accepted only for loopback test fixtures.
    #[doc(hidden)]
    pub fn with_loopback_google_oidc_for_tests(
        mut self,
        jwks_url: &str,
        audience: String,
        hosted_domains: Vec<String>,
    ) -> Result<Self, NativeAuthConfigError> {
        self.google = Some(Arc::new(GoogleOidcService::new_with_loopback_policy(
            jwks_url,
            audience,
            hosted_domains,
            true,
        )?));
        Ok(self)
    }

    /// Exercises the production OIDC verifier without an account lookup.
    #[doc(hidden)]
    pub async fn validate_google_token_for_tests(&self, token: &str) -> bool {
        match self.google.as_ref() {
            Some(google) => google.validate(token).await.is_some(),
            None => false,
        }
    }

    fn validate(&self, token: &str, now: OffsetDateTime) -> Option<NativeAccessClaims> {
        validate_access_token(&self.signing_key, token, now.unix_timestamp()).ok()
    }

    /// Validates one native access JWT. Shared with the Rust-native admin API
    /// gate (`/api/admin/v1` user/group administration), which authorizes
    /// operators through the same native identity authority.
    #[must_use]
    pub fn validate_access(&self, token: &str, now: OffsetDateTime) -> Option<NativeAccessClaims> {
        self.validate(token, now)
    }

    fn validate_cookie(&self, value: &str, now: OffsetDateTime) -> Option<NativeCookieClaims> {
        validate_cookie_session(&self.signing_key, value, now.unix_timestamp()).ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NativeAuthConfigError {
    #[error("{ENV_NATIVE_JWT_KEY} is required")]
    MissingSigningKey,
    #[error(
        "{ENV_NATIVE_JWT_KEY} must be a non-placeholder secret containing between 32 and 4096 bytes"
    )]
    WeakSigningKey,
    #[error("{ENV_NATIVE_MAX_SESSION_DAYS} must be between 30 and 365")]
    InvalidSessionDays,
    #[error("{ENV_NATIVE_DEVELOPMENT_RESET_TOKENS} must be true or false")]
    InvalidDevelopmentMode,
    #[error("{ENV_NATIVE_CORS_ORIGINS} must be a bounded comma-delimited list of HTTP origins")]
    InvalidCorsOrigin,
    #[error("native authentication rate-limit configuration is invalid")]
    InvalidRateLimit,
    #[error("native Google OIDC configuration is invalid")]
    InvalidGoogleConfig,
}

fn env_i64(name: &str, default: i64, min: i64, max: i64) -> Result<i64, NativeAuthConfigError> {
    let value = env::var(name)
        .ok()
        .map(|value| value.parse::<i64>())
        .transpose()
        .map_err(|_| NativeAuthConfigError::InvalidRateLimit)?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(NativeAuthConfigError::InvalidRateLimit);
    }
    Ok(value)
}

fn env_i32(name: &str, default: i32, min: i32, max: i32) -> Result<i32, NativeAuthConfigError> {
    i32::try_from(env_i64(
        name,
        i64::from(default),
        i64::from(min),
        i64::from(max),
    )?)
    .map_err(|_| NativeAuthConfigError::InvalidRateLimit)
}

/// Builds strict native API CORS from a comma-delimited explicit origin list.
/// Empty configuration allows no cross-origin requests.
pub fn cors_from_env() -> Result<tower_http::cors::CorsLayer, NativeAuthConfigError> {
    let raw = env::var(ENV_NATIVE_CORS_ORIGINS).unwrap_or_default();
    cors_from_origins(&raw)
}

pub fn cors_from_origins(raw: &str) -> Result<tower_http::cors::CorsLayer, NativeAuthConfigError> {
    if raw.len() > MAX_CORS_CONFIG_BYTES {
        return Err(NativeAuthConfigError::InvalidCorsOrigin);
    }
    let mut origins = Vec::new();
    for origin in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if origin == "*" || origin.len() > MAX_CONFIG_VALUE_BYTES {
            return Err(NativeAuthConfigError::InvalidCorsOrigin);
        }
        let uri = origin
            .parse::<axum::http::Uri>()
            .map_err(|_| NativeAuthConfigError::InvalidCorsOrigin)?;
        if !matches!(uri.scheme_str(), Some("http" | "https"))
            || uri.authority().is_none()
            || uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
            || uri.path() != "/"
            || uri.query().is_some()
        {
            return Err(NativeAuthConfigError::InvalidCorsOrigin);
        }
        origins.push(
            HeaderValue::from_str(origin).map_err(|_| NativeAuthConfigError::InvalidCorsOrigin)?,
        );
    }
    if origins.len() > 100 {
        return Err(NativeAuthConfigError::InvalidCorsOrigin);
    }
    Ok(tower_http::cors::CorsLayer::new()
        .allow_credentials(true)
        .allow_origin(origins)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::IF_NONE_MATCH,
            header::RANGE,
        ])
        .expose_headers([
            header::ACCEPT_RANGES,
            header::CONTENT_RANGE,
            header::ETAG,
            axum::http::HeaderName::from_static("content-duration"),
            axum::http::HeaderName::from_static("x-total-count"),
        ]))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub user_name: Option<String>,
    pub email: Option<String>,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    pub refresh_token: String,
    pub device_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PasswordResetRequest {
    pub email: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordResetConfirmRequest {
    pub token: String,
    pub new_password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAuthRequest {
    pub id_token: String,
    pub device_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedProviderDto {
    provider: &'static str,
    email: Option<String>,
    linked_at: Option<String>,
    last_login_at: Option<String>,
}

#[derive(Serialize)]
struct LinkedProviderResponse {
    message: &'static str,
    provider: LinkedProviderDto,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationResponse {
    pub user: UserDto,
    pub server_version: &'static str,
    pub token: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token_expires_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthErrorBody {
    code: &'static str,
    message: &'static str,
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct MessageBody {
    message: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DevelopmentResetBody {
    message: &'static str,
    reset_token: String,
    reset_url: String,
}

fn auth_error(code: &'static str, message: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(AuthErrorBody {
            code,
            message,
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn bad_request(code: &'static str, message: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(AuthErrorBody {
            code,
            message,
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn account_unavailable(user: &StoredNativeUser) -> Option<Response> {
    let (code, message) = if !user.enabled {
        ("account_disabled", "Account is disabled")
    } else if user.locked {
        ("USER_LOCKED", "User is locked")
    } else if user.blacklisted {
        ("BLACKLISTED", "User is blacklisted")
    } else {
        return None;
    };
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(AuthErrorBody {
                code,
                message,
                correlation_id: Uuid::now_v7(),
            }),
        )
            .into_response(),
    )
}

async fn enforce_auth_rate(
    state: &NativeState,
    peer: SocketAddr,
    headers: &axum::http::HeaderMap,
    actor: &str,
) -> Option<Response> {
    let client = match state.client_addresses.resolve(
        peer,
        header_value(headers, "forwarded"),
        header_value(headers, "x-forwarded-for"),
    ) {
        Ok(client) => client,
        Err(_) => return Some(StatusCode::BAD_REQUEST.into_response()),
    };
    let digest: [u8; 32] =
        Sha256::digest(format!("{}|{client}", actor.trim().to_uppercase())).into();
    let outcome = AuthStore::new()
        .record_rate_limit_attempt(
            &state.pool,
            &RateLimitPolicy {
                scope: "native-auth",
                max_attempts: state.auth.rate_attempts,
                window: Duration::seconds(state.auth.rate_window_seconds),
                max_retained_keys: state.auth.rate_max_keys,
            },
            &digest,
            OffsetDateTime::now_utc(),
        )
        .await;
    match outcome {
        Ok(RateLimitOutcome::Allowed { .. }) => None,
        Ok(RateLimitOutcome::Limited {
            retry_after_seconds,
        }) => {
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(AuthErrorBody {
                    code: "TOO_MANY_REQUESTS",
                    message: "Rate limit exceeded. Please try again later.",
                    correlation_id: Uuid::now_v7(),
                }),
            )
                .into_response();
            if let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            Some(response)
        }
        Err(_) => Some(unavailable()),
    }
}

pub fn public_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/auth/authenticate", post(authenticate))
        .route("/api/v1/auth/refresh-token", post(refresh_token))
        .route("/api/v1/auth/cookie/sign-in", post(cookie_sign_in))
        .route("/api/v1/auth/google", post(google_authenticate))
        .route(
            "/api/v1/auth/cookie/google",
            post(google_cookie_authenticate),
        )
        .route(
            "/api/v1/auth/password-reset/request",
            post(password_reset_request),
        )
        .route(
            "/api/v1/auth/password-reset/validate/{token}",
            get(password_reset_validate),
        )
        .route(
            "/api/v1/auth/password-reset/confirm",
            post(password_reset_confirm),
        )
        .layer(DefaultBodyLimit::max(MAX_AUTH_BODY_BYTES))
}

pub fn protected_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/auth/refresh", post(refresh_access))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/revoke", post(revoke))
        .route("/api/v1/user/me", get(about_me))
        .route("/api/v1/user/me/linked-providers", get(linked_providers))
        .route(
            "/api/v1/user/me/link/google",
            post(link_google).delete(unlink_google),
        )
        .layer(DefaultBodyLimit::max(MAX_AUTH_BODY_BYTES))
}

pub fn cookie_protected_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/auth/cookie/sign-out", post(cookie_sign_out))
        .layer(DefaultBodyLimit::max(MAX_AUTH_BODY_BYTES))
}

async fn about_me(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Response {
    let user = match AuthStore::new()
        .user_by_id(&state.pool, principal.user_id)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return unavailable(),
    };
    let counts = match user_counts(&state.pool, user.id).await {
        Ok(counts) => counts,
        Err(()) => return unavailable(),
    };
    match user_dto(&state, &user, counts) {
        Ok(user) => Json(user).into_response(),
        Err(()) => unavailable(),
    }
}

fn linked_provider_dto(login: StoredSocialLogin) -> Result<LinkedProviderDto, ()> {
    Ok(LinkedProviderDto {
        provider: "Google",
        email: login.verified_email,
        linked_at: Some(format_time(login.linked_at)?),
        last_login_at: Some(format_time(login.last_login_at)?),
    })
}

async fn linked_providers(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Response {
    match AuthStore::new()
        .social_logins_for_user(&state.pool, principal.user_id)
        .await
    {
        Ok(logins) => {
            let providers = logins
                .into_iter()
                .filter(|login| login.provider.eq_ignore_ascii_case("google"))
                .map(linked_provider_dto)
                .collect::<Result<Vec<_>, _>>();
            match providers {
                Ok(providers) => Json(providers).into_response(),
                Err(()) => unavailable(),
            }
        }
        Err(_) => unavailable(),
    }
}

async fn link_google(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<GoogleAuthRequest>,
) -> Response {
    let actor = format!("google-link:{}", principal.public_api_id);
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, &actor).await {
        return response;
    }
    let Some(google) = state.auth.google.as_ref() else {
        return bad_request("BAD_REQUEST", "Google authentication is not enabled");
    };
    let Some(claims) = google.validate(&request.id_token).await else {
        return bad_request("invalid_google_token", "Google token validation failed");
    };
    let store = AuthStore::new();
    match store
        .social_logins_for_user(&state.pool, principal.user_id)
        .await
    {
        Ok(logins)
            if logins
                .iter()
                .any(|login| login.provider.eq_ignore_ascii_case("google")) =>
        {
            return (
                StatusCode::CONFLICT,
                Json(AuthErrorBody {
                    code: "google_already_linked",
                    message: "Google account is already linked",
                    correlation_id: Uuid::now_v7(),
                }),
            )
                .into_response();
        }
        Ok(_) => {}
        Err(_) => return unavailable(),
    }
    let now = OffsetDateTime::now_utc();
    let link = SocialLoginLink {
        user_id: principal.user_id,
        provider: "google",
        subject: &claims.sub,
        verified_email: Some(&claims.email),
        display_name: None,
        hosted_domain: claims.hd.as_deref(),
        now,
    };
    match store.link_social_login(&state.pool, &link).await {
        Ok(()) => Json(LinkedProviderResponse {
            message: "Google account linked successfully",
            provider: LinkedProviderDto {
                provider: "Google",
                email: Some(claims.email),
                linked_at: format_time(now).ok(),
                last_login_at: format_time(now).ok(),
            },
        })
        .into_response(),
        Err(AuthStoreError::Database(error))
            if error
                .as_database_error()
                .and_then(|database| database.code())
                .is_some_and(|code| code == "23505") =>
        {
            (
                StatusCode::CONFLICT,
                Json(AuthErrorBody {
                    code: "google_already_linked",
                    message: "Google account is already linked",
                    correlation_id: Uuid::now_v7(),
                }),
            )
                .into_response()
        }
        Err(_) => unavailable(),
    }
}

async fn unlink_google(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Response {
    match AuthStore::new()
        .unlink_social_login(&state.pool, principal.user_id, "google")
        .await
    {
        Ok(true) => Json(MessageBody {
            message: "Google account unlinked successfully",
        })
        .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(AuthErrorBody {
                code: "NOT_FOUND",
                // Frozen native-v1 concatenated the resource label with its
                // generic not-found suffix. Preserve the client-visible bug.
                message: "Google account is not linked not found",
                correlation_id: Uuid::now_v7(),
            }),
        )
            .into_response(),
        Err(_) => unavailable(),
    }
}

pub async fn authenticate_request(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(request.headers()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(claims) = state.auth.validate(token, OffsetDateTime::now_utc()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // The frozen sid claim is the public User ApiKey. This indexed mapping is
    // the only DB read: no refresh/session/revocation state participates.
    let user = match AuthStore::new()
        .user_by_public_id(&state.pool, claims.sid)
        .await
    {
        Ok(Some(user)) => user,
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let binding = match state.client_addresses.resolve(
        peer,
        header_value(request.headers(), "forwarded"),
        header_value(request.headers(), "x-forwarded-for"),
    ) {
        Ok(binding) => binding.to_string(),
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    request.extensions_mut().insert(NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: user.is_admin,
        is_editor: user.is_editor,
    });
    request
        .extensions_mut()
        .insert(NativeClientBinding(binding));
    next.run(request).await
}

/// Validates only the native browser cookie. Bearer credentials deliberately
/// do not participate because the legacy default scheme for browser/theme
/// routes was cookie authentication.
pub async fn authenticate_cookie_request(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let claims = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(cookie_value)
        .and_then(|value| state.auth.validate_cookie(value, OffsetDateTime::now_utc()));
    let Some(claims) = claims else {
        return cookie_challenge(&state, request.uri());
    };
    let user = match AuthStore::new()
        .user_by_public_id(&state.pool, claims.public_api_id)
        .await
    {
        Ok(Some(user)) if user.can_authenticate() => user,
        _ => return cookie_challenge(&state, request.uri()),
    };
    let binding = match state.client_addresses.resolve(
        peer,
        header_value(request.headers(), "forwarded"),
        header_value(request.headers(), "x-forwarded-for"),
    ) {
        Ok(binding) => binding.to_string(),
        Err(_) => return cookie_challenge(&state, request.uri()),
    };
    request.extensions_mut().insert(NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: user.is_admin,
        is_editor: user.is_editor,
    });
    request
        .extensions_mut()
        .insert(NativeClientBinding(binding));
    next.run(request).await
}

async fn authenticate(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Response {
    let actor = request
        .user_name
        .as_deref()
        .or(request.email.as_deref())
        .unwrap_or("");
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, actor).await {
        return response;
    }
    let user = match authenticate_user(&state, &request).await {
        Ok(Some(user)) => user,
        Ok(None) => return invalid_credentials(),
        Err(()) => return unavailable(),
    };
    match issue_login(&state, &user, None, OffsetDateTime::now_utc()).await {
        Ok(response) => Json(response).into_response(),
        Err(()) => unavailable(),
    }
}

async fn authenticate_user(
    state: &NativeState,
    request: &LoginRequest,
) -> Result<Option<StoredNativeUser>, ()> {
    let (normalized, by_email) = match request
        .user_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| (value.trim().to_uppercase(), false))
        .or_else(|| {
            request
                .email
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| (value.trim().to_uppercase(), true))
        }) {
        Some(lookup) => lookup,
        None => return Ok(None),
    };
    let lookup = if by_email {
        AuthLookup::Email(&normalized)
    } else {
        AuthLookup::Username(&normalized)
    };
    let record = AuthStore::new().user_for_auth(&state.pool, lookup).await;
    let record = match record {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = verify_password(
                request.password.as_bytes(),
                PasswordHashAlgorithm::Bcrypt,
                DUMMY_BCRYPT,
            );
            return Ok(None);
        }
        Err(_) => return Err(()),
    };
    let valid = verify_password(
        request.password.as_bytes(),
        record.password_hash_algorithm,
        record.password_hash.expose_secret(),
    )
    .unwrap_or(false);
    if !valid || !record.user.can_authenticate() {
        return Ok(None);
    }
    Ok(Some(record.user))
}

async fn cookie_sign_in(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Response {
    let actor = request
        .user_name
        .as_deref()
        .or(request.email.as_deref())
        .unwrap_or("");
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, actor).await {
        return response;
    }
    let user = match authenticate_user(&state, &request).await {
        Ok(Some(user)) => user,
        Ok(None) => return invalid_credentials(),
        Err(()) => return unavailable(),
    };
    issue_cookie_response(&state, &user)
}

fn issue_cookie_response(state: &NativeState, user: &StoredNativeUser) -> Response {
    let now = OffsetDateTime::now_utc();
    let value = match issue_cookie_session(
        &state.auth.signing_key,
        &NativeCookieClaims {
            public_api_id: user.public_api_id,
            iat: now.unix_timestamp(),
            exp: (now + COOKIE_LIFETIME).unix_timestamp(),
        },
    ) {
        Ok(value) => value,
        Err(_) => return unavailable(),
    };
    let cookie = format!("{NATIVE_COOKIE_NAME}={value}; Path=/; Secure; SameSite=Strict; HttpOnly");
    response_with_cookie(
        Json(MessageBody {
            message: "Signed in",
        })
        .into_response(),
        &cookie,
    )
}

async fn cookie_sign_out() -> Response {
    response_with_cookie(
        Json(MessageBody {
            message: "Signed out",
        })
        .into_response(),
        "melodee_auth=; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure; SameSite=Strict; HttpOnly",
    )
}

async fn verified_google_user(
    state: &NativeState,
    request: &GoogleAuthRequest,
) -> Result<StoredNativeUser, Response> {
    let Some(google) = state.auth.google.as_ref() else {
        return Err(bad_request(
            "BAD_REQUEST",
            "Google authentication is not enabled",
        ));
    };
    let Some(claims) = google.validate(&request.id_token).await else {
        return Err(bad_request(
            "invalid_google_token",
            "Google token validation failed",
        ));
    };
    match AuthStore::new()
        .user_for_social_login(&state.pool, "google", &claims.sub, OffsetDateTime::now_utc())
        .await
    {
        Ok(Some(user)) if user.can_authenticate() => Ok(user),
        Ok(_) => Err((
            StatusCode::CONFLICT,
            Json(AuthErrorBody {
                code: "google_account_not_linked",
                message: "Google account is not linked. Please log in with password and link your Google account.",
                correlation_id: Uuid::now_v7(),
            }),
        )
            .into_response()),
        Err(_) => Err(unavailable()),
    }
}

async fn google_authenticate(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<GoogleAuthRequest>,
) -> Response {
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, "google").await {
        return response;
    }
    let user = match verified_google_user(&state, &request).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    match issue_login(
        &state,
        &user,
        request.device_id.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(()) => unavailable(),
    }
}

async fn google_cookie_authenticate(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<GoogleAuthRequest>,
) -> Response {
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, "google-cookie").await {
        return response;
    }
    let user = match verified_google_user(&state, &request).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    issue_cookie_response(&state, &user)
}

async fn password_reset_request(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<PasswordResetRequest>,
) -> Response {
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, &request.email).await {
        return response;
    }
    if request.email.trim().is_empty() {
        return bad_request("VALIDATION_ERROR", "Email is required");
    }
    let clear = match generate_password_reset_token() {
        Ok(token) => token,
        Err(_) => return unavailable(),
    };
    let now = OffsetDateTime::now_utc();
    let digest = RefreshTokenDigest::from_token(clear.expose_secret().as_bytes());
    let normalized = request.email.trim().to_uppercase();
    let exists = match AuthStore::new()
        .issue_password_reset(
            &state.pool,
            &PasswordResetIssue {
                token_id: Uuid::now_v7(),
                normalized_email: &normalized,
                token_digest: &digest,
                issued_at: now,
                expires_at: now + Duration::hours(1),
            },
        )
        .await
    {
        Ok(exists) => exists,
        Err(_) => return unavailable(),
    };
    if state.auth.development_reset_tokens && exists {
        let token = clear.expose_secret().to_string();
        let url = format!(
            "{}/reset-password?token={token}",
            state.public_origin.trim_end_matches('/')
        );
        return Json(DevelopmentResetBody {
            message: "Password reset link generated (development mode).",
            reset_token: token,
            reset_url: url,
        })
        .into_response();
    }
    Json(MessageBody {
        message: "If an account with that email exists, a password reset link has been sent.",
    })
    .into_response()
}

async fn password_reset_validate(
    State(state): State<NativeState>,
    Path(token): Path<String>,
) -> Response {
    let digest = RefreshTokenDigest::from_token(token.as_bytes());
    match AuthStore::new()
        .validate_password_reset(&state.pool, &digest, OffsetDateTime::now_utc())
        .await
    {
        Ok(PasswordResetOutcome::Valid) => Json(serde_json::json!({"valid":true})).into_response(),
        Ok(_) => bad_request("BAD_REQUEST", "Invalid token"),
        Err(_) => unavailable(),
    }
}

async fn password_reset_confirm(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<PasswordResetConfirmRequest>,
) -> Response {
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, "password-reset").await {
        return response;
    }
    if request.token.trim().is_empty() {
        return bad_request("VALIDATION_ERROR", "Token is required");
    }
    if request.new_password.trim().is_empty() {
        return bad_request("VALIDATION_ERROR", "New password is required");
    }
    if request.new_password.chars().count() < 8 {
        return bad_request("VALIDATION_ERROR", "Password must be at least 8 characters");
    }
    let hash = match hash_password(request.new_password.as_bytes()) {
        Ok(hash) => hash,
        Err(_) => return unavailable(),
    };
    let digest = RefreshTokenDigest::from_token(request.token.as_bytes());
    match AuthStore::new()
        .consume_password_reset(&state.pool, &digest, &hash, OffsetDateTime::now_utc())
        .await
    {
        Ok(PasswordResetOutcome::Consumed { .. }) => Json(MessageBody {
            message: "Password has been reset successfully",
        })
        .into_response(),
        Ok(_) => bad_request("BAD_REQUEST", "Invalid token"),
        Err(_) => unavailable(),
    }
}

async fn refresh_token(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<RefreshRequest>,
) -> Response {
    if let Some(response) = enforce_auth_rate(&state, peer, &headers, "refresh-token").await {
        return response;
    }
    let now = OffsetDateTime::now_utc();
    let old_digest = RefreshTokenDigest::from_token(request.refresh_token.as_bytes());
    let next_secret = match generate_refresh_token() {
        Ok(secret) => secret,
        Err(_) => return unavailable(),
    };
    let next_digest = RefreshTokenDigest::from_token(next_secret.expose_secret().as_bytes());
    let outcome = AuthStore::new()
        .rotate_refresh_token(
            &state.pool,
            &RefreshRotationRequest {
                current_digest: &old_digest,
                replacement_token_id: Uuid::now_v7(),
                replacement_digest: &next_digest,
                device_id: request.device_id.as_deref(),
                now,
                replacement_expires_at: now + REFRESH_LIFETIME,
            },
        )
        .await;
    match outcome {
        Ok(RefreshRotationOutcome::Rotated {
            user_id,
            refresh_expires_at,
            ..
        }) => match AuthStore::new().user_by_id(&state.pool, user_id).await {
            Ok(Some(user)) => {
                if let Some(response) = account_unavailable(&user) {
                    response
                } else {
                    match auth_response(
                        &state,
                        &user,
                        now,
                        Some(next_secret),
                        Some(refresh_expires_at),
                    )
                    .await
                    {
                        Ok(response) => Json(response).into_response(),
                        Err(()) => unavailable(),
                    }
                }
            }
            _ => unavailable(),
        },
        Ok(RefreshRotationOutcome::Expired) => {
            auth_error("refresh_token_invalid", "Refresh token has expired.")
        }
        Ok(RefreshRotationOutcome::Replayed) => auth_error(
            "refresh_token_replayed",
            "Refresh token has been revoked. Please re-authenticate.",
        ),
        Ok(RefreshRotationOutcome::Invalid | RefreshRotationOutcome::AccountUnavailable) => {
            auth_error("refresh_token_invalid", "Refresh token is invalid.")
        }
        Err(_) => unavailable(),
    }
}

async fn refresh_access(
    State(state): State<NativeState>,
    super::Authenticated(principal, _): super::Authenticated,
) -> Response {
    match AuthStore::new()
        .user_by_public_id(&state.pool, principal.public_api_id)
        .await
    {
        Ok(Some(user)) => match account_unavailable(&user) {
            Some(response) => response,
            None => match auth_response(&state, &user, OffsetDateTime::now_utc(), None, None).await
            {
                Ok(response) => Json(response).into_response(),
                Err(()) => unavailable(),
            },
        },
        _ => unavailable(),
    }
}

async fn logout(
    State(state): State<NativeState>,
    super::Authenticated(principal, _): super::Authenticated,
) -> Response {
    match AuthStore::new()
        .revoke_all_refresh_tokens(
            &state.pool,
            principal.user_id,
            OffsetDateTime::now_utc(),
            "logout",
        )
        .await
    {
        Ok(_) => Json(MessageBody {
            message: "Logged out successfully",
        })
        .into_response(),
        Err(_) => unavailable(),
    }
}

async fn revoke(
    State(state): State<NativeState>,
    super::Authenticated(principal, _): super::Authenticated,
    Json(request): Json<RefreshRequest>,
) -> Response {
    let digest = RefreshTokenDigest::from_token(request.refresh_token.as_bytes());
    match AuthStore::new()
        .revoke_refresh_token(
            &state.pool,
            principal.user_id,
            &digest,
            OffsetDateTime::now_utc(),
            "revoked",
        )
        .await
    {
        Ok(true) => Json(MessageBody {
            message: "Token revoked successfully",
        })
        .into_response(),
        Ok(false) => auth_error("refresh_token_invalid", "Refresh token is invalid."),
        Err(_) => unavailable(),
    }
}

async fn issue_login(
    state: &NativeState,
    user: &StoredNativeUser,
    device_id: Option<&str>,
    now: OffsetDateTime,
) -> Result<AuthenticationResponse, ()> {
    let refresh = generate_refresh_token().map_err(|_| ())?;
    let refresh_digest = RefreshTokenDigest::from_token(refresh.expose_secret().as_bytes());
    let refresh_expires = now + REFRESH_LIFETIME;
    AuthStore::new()
        .issue_refresh_family(
            &state.pool,
            &RefreshFamilyIssue {
                family_id: Uuid::now_v7(),
                token_id: Uuid::now_v7(),
                user_id: user.id,
                expected_auth_revision: user.auth_revision,
                token_digest: &refresh_digest,
                device_id,
                session_started_at: now,
                family_expires_at: now + Duration::days(state.auth.max_session_days),
                token_expires_at: refresh_expires,
                issued_at: now,
            },
        )
        .await
        .map_err(|_| ())?;
    auth_response(state, user, now, Some(refresh), Some(refresh_expires)).await
}

async fn auth_response(
    state: &NativeState,
    user: &StoredNativeUser,
    now: OffsetDateTime,
    refresh: Option<SecretString>,
    refresh_expires: Option<OffsetDateTime>,
) -> Result<AuthenticationResponse, ()> {
    if !user.can_authenticate() {
        return Err(());
    }
    let expires = now + ACCESS_LIFETIME;
    let claims = NativeAccessClaims {
        aud: "MelodeeClient".to_string(),
        email: user.email.clone(),
        exp: expires.unix_timestamp(),
        sid: user.public_api_id,
        iat: now.unix_timestamp(),
        iss: "MelodeeApi".to_string(),
        nbf: now.unix_timestamp(),
        role: user.roles.clone(),
        unique_name: user.username.clone(),
        theme: user.preferred_theme.clone().unwrap_or_default(),
    };
    let token = issue_access_token(&state.auth.signing_key, &claims).map_err(|_| ())?;
    let counts = user_counts(&state.pool, user.id).await?;
    Ok(AuthenticationResponse {
        user: user_dto(state, user, counts)?,
        server_version: "2.2.0",
        token,
        expires_at: format_time(expires)?,
        refresh_token: refresh.map(|secret| secret.expose_secret().to_string()),
        refresh_token_expires_at: refresh_expires.map(format_time).transpose()?,
    })
}

async fn user_counts(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<(i32, i32, i32, i32, i32, i32, i32), ()> {
    let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
        "SELECT
          COALESCE((SELECT SUM(play_count)::bigint FROM native_track_user_states WHERE user_id=$1),0::bigint),
          (SELECT COUNT(*) FROM native_artist_user_states WHERE user_id=$1 AND starred),
          (SELECT COUNT(*) FROM native_artist_user_states WHERE user_id=$1 AND hated),
          (SELECT COUNT(*) FROM native_release_user_states WHERE user_id=$1 AND starred),
          (SELECT COUNT(*) FROM native_release_user_states WHERE user_id=$1 AND hated),
          (SELECT COUNT(*) FROM native_track_user_states WHERE user_id=$1 AND starred),
          (SELECT COUNT(*) FROM native_track_user_states WHERE user_id=$1 AND hated)",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|_| ())?;
    Ok((
        i32::try_from(row.0).map_err(|_| ())?,
        i32::try_from(row.1).map_err(|_| ())?,
        i32::try_from(row.2).map_err(|_| ())?,
        i32::try_from(row.3).map_err(|_| ())?,
        i32::try_from(row.4).map_err(|_| ())?,
        i32::try_from(row.5).map_err(|_| ())?,
        i32::try_from(row.6).map_err(|_| ())?,
    ))
}

fn user_dto(
    state: &NativeState,
    user: &StoredNativeUser,
    counts: (i32, i32, i32, i32, i32, i32, i32),
) -> Result<UserDto, ()> {
    let origin = state.public_origin.trim_end_matches('/');
    Ok(UserDto {
        id: user.public_api_id,
        thumbnail_url: format!("{origin}/images/user_{}/80", user.public_api_id),
        image_url: format!("{origin}/images/user_{}/1024", user.public_api_id),
        username: user.username.clone(),
        email: user.email.clone(),
        is_admin: user.is_admin,
        is_editor: user.is_editor,
        roles: user.roles.clone(),
        songs_played: counts.0,
        artists_liked: counts.1,
        artists_disliked: counts.2,
        albums_liked: counts.3,
        albums_disliked: counts.4,
        songs_liked: counts.5,
        songs_disliked: counts.6,
        created_at: format_time(user.created_at)?,
        updated_at: format_time(user.updated_at)?,
    })
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn cookie_value(raw: &str) -> Option<&str> {
    raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == NATIVE_COOKIE_NAME && !value.is_empty()).then_some(value)
    })
}

fn response_with_cookie(mut response: Response, cookie: &str) -> Response {
    let Ok(value) = HeaderValue::from_str(cookie) else {
        return unavailable();
    };
    response.headers_mut().insert(header::SET_COOKIE, value);
    response
}

fn cookie_challenge(state: &NativeState, uri: &axum::http::Uri) -> Response {
    let return_url = uri
        .path_and_query()
        .map_or_else(|| uri.path(), axum::http::uri::PathAndQuery::as_str);
    let location = format!(
        "{}/Account/Login?ReturnUrl={}",
        state.public_origin.trim_end_matches('/'),
        percent_encode_query_value(return_url)
    );
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

fn percent_encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn header_value<'a>(headers: &'a axum::http::HeaderMap, name: &'static str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn format_time(value: OffsetDateTime) -> Result<String, ()> {
    value.format(&Rfc3339).map_err(|_| ())
}

fn invalid_credentials() -> Response {
    auth_error("UNAUTHORIZED", "Invalid credentials")
}
fn unavailable() -> Response {
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

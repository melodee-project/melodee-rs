//! PostgreSQL authority for native users and refresh-token families.
//!
//! Token rotation locks both the token and its family so exactly one racing
//! consumer succeeds. A later reuse revokes the entire family (ADR-0124).

use melodee_auth::{PasswordHashAlgorithm, RefreshTokenDigest, SecretString};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// A migrated native user without authentication secrets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredNativeUser {
    pub id: Uuid,
    pub public_api_id: Uuid,
    pub username: String,
    pub email: String,
    pub is_admin: bool,
    pub is_editor: bool,
    pub enabled: bool,
    pub locked: bool,
    pub blacklisted: bool,
    pub preferred_language: Option<String>,
    pub preferred_theme: Option<String>,
    pub auth_revision: i64,
    pub roles: Vec<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl StoredNativeUser {
    /// Whether a new access or refresh credential may be issued.
    #[must_use]
    pub const fn can_authenticate(&self) -> bool {
        self.enabled && !self.locked && !self.blacklisted
    }
}

/// Authentication lookup result. Secret diagnostics are redacted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticationRecord {
    pub user: StoredNativeUser,
    pub password_hash_algorithm: PasswordHashAlgorithm,
    pub password_hash: SecretString,
}

/// Stream authorization facts resolved by preserved public user ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamPrincipal {
    pub user_id: Uuid,
    pub public_api_id: Uuid,
    pub stream_signing_key: SecretString,
    pub enabled: bool,
    pub locked: bool,
    pub blacklisted: bool,
    pub can_stream: bool,
}

impl StreamPrincipal {
    /// Fails closed for account state as well as role authorization.
    #[must_use]
    pub const fn is_authorized(&self) -> bool {
        self.enabled && !self.locked && !self.blacklisted && self.can_stream
    }
}

/// Exact fields imported from the legacy user authority.
pub struct NativeUserImport<'a> {
    pub id: Uuid,
    pub public_api_id: Uuid,
    pub username: &'a str,
    pub normalized_username: &'a str,
    pub email: &'a str,
    pub normalized_email: &'a str,
    pub password_hash: &'a str,
    pub password_hash_algorithm: PasswordHashAlgorithm,
    pub stream_signing_key: &'a str,
    pub is_admin: bool,
    pub is_editor: bool,
    pub enabled: bool,
    pub locked: bool,
    pub blacklisted: bool,
    pub preferred_language: Option<&'a str>,
    pub preferred_theme: Option<&'a str>,
    pub auth_revision: i64,
    pub roles: &'a [String],
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Selects one normalized login-identifier namespace explicitly.
pub enum AuthLookup<'a> {
    Username(&'a str),
    Email(&'a str),
}

/// Login-identifier normalization shared by authentication, password reset,
/// and operator account recovery: trimmed and uppercased, matching the
/// captured `normalized_username`/`normalized_email` columns.
#[must_use]
pub fn normalize_login(value: &str) -> String {
    value.trim().to_uppercase()
}

/// First-admin bootstrap facts. The password is already bcrypt-hashed by the
/// caller (`melodee_auth::hash_password`); clear password material never
/// reaches persistence.
pub struct BootstrapAdminRequest<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub password_hash: &'a str,
    pub stream_signing_key: &'a str,
    pub now: OffsetDateTime,
}

/// Initial refresh family and token facts. The token value is already digested.
pub struct RefreshFamilyIssue<'a> {
    pub family_id: Uuid,
    pub token_id: Uuid,
    pub user_id: Uuid,
    /// Authentication revision observed while verifying the login credential.
    /// Issuance is rejected if reset/account mutation changed it meanwhile.
    pub expected_auth_revision: i64,
    pub token_digest: &'a RefreshTokenDigest,
    pub device_id: Option<&'a str>,
    pub session_started_at: OffsetDateTime,
    pub family_expires_at: OffsetDateTime,
    pub token_expires_at: OffsetDateTime,
    pub issued_at: OffsetDateTime,
}

/// Atomic successor issue request.
pub struct RefreshRotationRequest<'a> {
    pub current_digest: &'a RefreshTokenDigest,
    pub replacement_token_id: Uuid,
    pub replacement_digest: &'a RefreshTokenDigest,
    pub device_id: Option<&'a str>,
    pub now: OffsetDateTime,
    pub replacement_expires_at: OffsetDateTime,
}

/// Security-significant outcome of a refresh attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshRotationOutcome {
    Rotated {
        user_id: Uuid,
        family_id: Uuid,
        refresh_expires_at: OffsetDateTime,
    },
    Invalid,
    Expired,
    Replayed,
    AccountUnavailable,
}

/// Bounded, cluster-wide fixed-window rate-limit policy (ADR-0126).
pub struct RateLimitPolicy<'a> {
    pub scope: &'a str,
    pub max_attempts: i32,
    pub window: Duration,
    pub max_retained_keys: i64,
}

/// Result of atomically recording one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimitOutcome {
    Allowed { remaining: i32 },
    Limited { retry_after_seconds: i64 },
}

pub struct PasswordResetIssue<'a> {
    pub token_id: Uuid,
    pub normalized_email: &'a str,
    pub token_digest: &'a RefreshTokenDigest,
    pub issued_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

pub struct SocialLoginLink<'a> {
    pub user_id: Uuid,
    pub provider: &'a str,
    pub subject: &'a str,
    pub verified_email: Option<&'a str>,
    pub display_name: Option<&'a str>,
    pub hosted_domain: Option<&'a str>,
    pub now: OffsetDateTime,
}

/// Public provider-link facts; provider subjects never leave persistence.
#[derive(Clone, Debug, Eq, FromRow, PartialEq)]
pub struct StoredSocialLogin {
    pub provider: String,
    pub verified_email: Option<String>,
    pub linked_at: OffsetDateTime,
    pub last_login_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordResetOutcome {
    Valid,
    Consumed { user_id: Uuid },
    Invalid,
    Expired,
}

/// Idempotent legacy-to-Rust identity binding.
#[derive(Clone, Debug, Eq, FromRow, PartialEq)]
pub struct MigrationIdentityBinding {
    pub source_kind: String,
    pub source_key: String,
    pub internal_id: Uuid,
    pub public_api_id: Uuid,
    pub imported_at: OffsetDateTime,
}

#[derive(Debug, Error)]
pub enum AuthStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("persisted authentication state is corrupt")]
    CorruptState,
    #[error("imported native user conflicts with existing identity")]
    ImportConflict,
    #[error("migration identity binding conflicts with existing mapping")]
    IdentityBindingConflict,
    #[error("invalid refresh-token lifecycle request")]
    InvalidRefreshRequest,
    #[error("invalid rate-limit policy")]
    InvalidRateLimitPolicy,
    #[error("an administrator already exists; bootstrap refuses to create another")]
    AdminExists,
    #[error("invalid administrator bootstrap request")]
    InvalidBootstrapRequest,
}

/// Stateless entry point for native authentication persistence.
#[derive(Clone, Copy, Debug, Default)]
pub struct AuthStore;

#[derive(FromRow)]
pub(crate) struct UserRow {
    pub(crate) id: Uuid,
    public_api_id: Uuid,
    username: String,
    email: String,
    is_admin: bool,
    is_editor: bool,
    enabled: bool,
    locked: bool,
    blacklisted: bool,
    preferred_language: Option<String>,
    preferred_theme: Option<String>,
    auth_revision: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(FromRow)]
struct AuthenticationRow {
    id: Uuid,
    public_api_id: Uuid,
    username: String,
    email: String,
    is_admin: bool,
    is_editor: bool,
    enabled: bool,
    locked: bool,
    blacklisted: bool,
    preferred_language: Option<String>,
    preferred_theme: Option<String>,
    auth_revision: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    password_hash: String,
    password_hash_algorithm: String,
}

#[derive(FromRow)]
struct RefreshRow {
    token_id: Uuid,
    family_id: Uuid,
    user_id: Uuid,
    family_expires_at: OffsetDateTime,
    token_expires_at: OffsetDateTime,
    family_revoked_at: Option<OffsetDateTime>,
    token_consumed_at: Option<OffsetDateTime>,
    token_revoked_at: Option<OffsetDateTime>,
    family_device_id: Option<String>,
    enabled: bool,
    locked: bool,
    blacklisted: bool,
}

impl AuthStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Imports a legacy user without changing its public ID or password hash.
    /// Exact replay succeeds; any different replay fails closed.
    pub async fn import_user(
        &self,
        pool: &PgPool,
        imported: &NativeUserImport<'_>,
    ) -> Result<StoredNativeUser, AuthStoreError> {
        let mut tx = pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO native_users (
                id, public_api_id, username, normalized_username, email, normalized_email,
                password_hash, password_hash_algorithm, stream_signing_key, is_admin, is_editor,
                enabled, locked, blacklisted, preferred_language, preferred_theme, auth_revision,
                created_at, updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(imported.id)
        .bind(imported.public_api_id)
        .bind(imported.username)
        .bind(imported.normalized_username)
        .bind(imported.email)
        .bind(imported.normalized_email)
        .bind(imported.password_hash)
        .bind(password_algorithm_tag(imported.password_hash_algorithm))
        .bind(imported.stream_signing_key)
        .bind(imported.is_admin)
        .bind(imported.is_editor)
        .bind(imported.enabled)
        .bind(imported.locked)
        .bind(imported.blacklisted)
        .bind(imported.preferred_language)
        .bind(imported.preferred_theme)
        .bind(imported.auth_revision)
        .bind(imported.created_at)
        .bind(imported.updated_at)
        .execute(&mut *tx)
        .await?;

        if inserted.rows_affected() == 0 && !user_import_matches(&mut tx, imported).await? {
            return Err(AuthStoreError::ImportConflict);
        }

        let mut requested_roles = imported.roles.to_vec();
        requested_roles.sort_unstable();
        requested_roles.dedup();
        if requested_roles.iter().any(|role| role.trim().is_empty()) {
            return Err(AuthStoreError::ImportConflict);
        }
        for role in &requested_roles {
            sqlx::query(
                "INSERT INTO native_user_roles (user_id, role, created_at)
                 VALUES ($1,$2,$3) ON CONFLICT (user_id, role) DO NOTHING",
            )
            .bind(imported.id)
            .bind(role)
            .bind(imported.created_at)
            .execute(&mut *tx)
            .await?;
        }
        let persisted_roles = roles_for_user(&mut tx, imported.id).await?;
        if persisted_roles != requested_roles {
            return Err(AuthStoreError::ImportConflict);
        }
        tx.commit().await?;
        self.user_by_public_id(pool, imported.public_api_id)
            .await?
            .ok_or(AuthStoreError::CorruptState)
    }

    /// Loads credentials by an explicitly normalized username or email.
    pub async fn user_for_auth(
        &self,
        pool: &PgPool,
        lookup: AuthLookup<'_>,
    ) -> Result<Option<AuthenticationRecord>, AuthStoreError> {
        let row = match lookup {
            AuthLookup::Username(value) => {
                sqlx::query_as::<_, AuthenticationRow>(
                    "SELECT id, public_api_id, username, email, is_admin, is_editor, enabled, locked, blacklisted,
                        preferred_language, preferred_theme, auth_revision, created_at, updated_at,
                        password_hash, password_hash_algorithm
                 FROM native_users WHERE normalized_username = $1",
                )
                .bind(value)
                .fetch_optional(pool)
                .await?
            }
            AuthLookup::Email(value) => {
                sqlx::query_as::<_, AuthenticationRow>(
                    "SELECT id, public_api_id, username, email, is_admin, is_editor, enabled, locked, blacklisted,
                        preferred_language, preferred_theme, auth_revision, created_at, updated_at,
                        password_hash, password_hash_algorithm
                 FROM native_users WHERE normalized_email = $1",
                )
                .bind(value)
                .fetch_optional(pool)
                .await?
            }
        };
        match row {
            Some(row) => {
                let roles = roles_for_pool(pool, row.id).await?;
                Ok(Some(authentication_from_row(row, roles)?))
            }
            None => Ok(None),
        }
    }

    /// Resolves one immutable provider subject to its Melodee User.
    pub async fn user_for_social_login(
        &self,
        pool: &PgPool,
        provider: &str,
        subject: &str,
        now: OffsetDateTime,
    ) -> Result<Option<StoredNativeUser>, AuthStoreError> {
        let mut tx = pool.begin().await?;
        let user_id = sqlx::query_scalar::<_, Uuid>(
            "UPDATE native_user_social_logins SET last_login_at=$3
             WHERE provider=$1 AND provider_subject=$2 RETURNING user_id",
        )
        .bind(provider)
        .bind(subject)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        match user_id {
            Some(user_id) => self.user_by_id(pool, user_id).await,
            None => Ok(None),
        }
    }

    /// Links one verified provider subject. Existing ownership is immutable.
    pub async fn link_social_login(
        &self,
        pool: &PgPool,
        link: &SocialLoginLink<'_>,
    ) -> Result<(), AuthStoreError> {
        sqlx::query(
            "INSERT INTO native_user_social_logins(
                user_id,provider,provider_subject,verified_email,display_name,hosted_domain,
                linked_at,last_login_at
             ) VALUES($1,$2,$3,$4,$5,$6,$7,$7)",
        )
        .bind(link.user_id)
        .bind(link.provider)
        .bind(link.subject)
        .bind(link.verified_email)
        .bind(link.display_name)
        .bind(link.hosted_domain)
        .bind(link.now)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Lists a User's provider links without exposing immutable provider
    /// subjects or other provider claims.
    pub async fn social_logins_for_user(
        &self,
        pool: &PgPool,
        user_id: Uuid,
    ) -> Result<Vec<StoredSocialLogin>, AuthStoreError> {
        Ok(sqlx::query_as::<_, StoredSocialLogin>(
            "SELECT provider,verified_email,linked_at,last_login_at
             FROM native_user_social_logins WHERE user_id=$1 ORDER BY provider",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?)
    }

    /// Removes only the named provider link owned by this User.
    pub async fn unlink_social_login(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        provider: &str,
    ) -> Result<bool, AuthStoreError> {
        if provider.trim().is_empty() || provider.len() > 50 {
            return Ok(false);
        }
        Ok(
            sqlx::query("DELETE FROM native_user_social_logins WHERE user_id=$1 AND provider=$2")
                .bind(user_id)
                .bind(provider)
                .execute(pool)
                .await?
                .rows_affected()
                == 1,
        )
    }

    /// Resolves the stable public user ID used by native URLs.
    pub async fn user_by_public_id(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<StoredNativeUser>, AuthStoreError> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, public_api_id, username, email, is_admin, is_editor, enabled, locked, blacklisted,
                    preferred_language, preferred_theme, auth_revision, created_at, updated_at
             FROM native_users WHERE public_api_id = $1",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?;
        match row {
            Some(row) => {
                let roles = roles_for_pool(pool, row.id).await?;
                Ok(Some(stored_user_from_row(row, roles)))
            }
            None => Ok(None),
        }
    }

    /// Resolves an internal identity after a refresh-family transaction.
    pub async fn user_by_id(
        &self,
        pool: &PgPool,
        user_id: Uuid,
    ) -> Result<Option<StoredNativeUser>, AuthStoreError> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, public_api_id, username, email, is_admin, is_editor, enabled, locked,
                    blacklisted, preferred_language, preferred_theme, auth_revision,
                    created_at, updated_at FROM native_users WHERE id=$1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
        match row {
            Some(row) => Ok(Some(stored_user_from_row(
                row,
                roles_for_pool(pool, user_id).await?,
            ))),
            None => Ok(None),
        }
    }

    /// Resolves stream authorization without ever placing its signing key in Debug output.
    pub async fn stream_principal_by_public_id(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<StreamPrincipal>, AuthStoreError> {
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, bool, bool, bool, bool)>(
            "SELECT u.id, u.public_api_id, u.stream_signing_key, u.enabled, u.locked,
                    u.blacklisted, EXISTS (
                        SELECT 1 FROM native_user_roles r
                        WHERE r.user_id = u.id AND r.role = 'HasStreamRole'
                    )
             FROM native_users u WHERE u.public_api_id = $1",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(
            |(user_id, public_api_id, signing_key, enabled, locked, blacklisted, can_stream)| {
                StreamPrincipal {
                    user_id,
                    public_api_id,
                    stream_signing_key: SecretString::new(signing_key),
                    enabled,
                    locked,
                    blacklisted,
                    can_stream,
                }
            },
        ))
    }

    /// Applies the authoritative migrated library-group predicate.
    pub async fn can_access_library(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        library_id: Option<Uuid>,
    ) -> Result<bool, AuthStoreError> {
        Ok(
            sqlx::query_scalar::<_, bool>("SELECT native_user_can_access_library($1,$2)")
                .bind(user_id)
                .bind(library_id)
                .fetch_one(pool)
                .await?,
        )
    }

    /// Applies the release→catalog import→source library authorization chain.
    pub async fn can_access_release(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        release_id: Uuid,
    ) -> Result<bool, AuthStoreError> {
        Ok(
            sqlx::query_scalar::<_, bool>("SELECT native_user_can_access_release($1,$2)")
                .bind(user_id)
                .bind(release_id)
                .fetch_one(pool)
                .await?,
        )
    }

    /// Returns true only when an Artist participates in at least one visible
    /// Release/Track for this User.
    pub async fn can_access_artist(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        artist_id: Uuid,
    ) -> Result<bool, AuthStoreError> {
        Ok(
            sqlx::query_scalar::<_, bool>("SELECT native_user_can_access_artist($1,$2)")
                .bind(user_id)
                .bind(artist_id)
                .fetch_one(pool)
                .await?,
        )
    }

    /// Loads the migrated user avatar without changing the stable import DTO.
    pub async fn user_image_by_public_id(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<(String, Vec<u8>)>, AuthStoreError> {
        Ok(sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT image_mime_type,image_bytes FROM native_users
             WHERE public_api_id=$1 AND image_mime_type IS NOT NULL AND image_bytes IS NOT NULL",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?)
    }

    /// Atomically records one attempt using a bounded PostgreSQL key set.
    ///
    /// The singleton admission lock makes the retained-key ceiling exact
    /// across processes. Expired keys are removed before every admission.
    pub async fn record_rate_limit_attempt(
        &self,
        pool: &PgPool,
        policy: &RateLimitPolicy<'_>,
        actor_key_digest: &[u8; 32],
        now: OffsetDateTime,
    ) -> Result<RateLimitOutcome, AuthStoreError> {
        if policy.scope.trim().is_empty()
            || policy.scope.len() > 100
            || policy.max_attempts <= 0
            || policy.max_attempts > 1_000_000
            || policy.window <= Duration::ZERO
            || policy.window > Duration::days(1)
            || policy.max_retained_keys <= 0
            || policy.max_retained_keys > 10_000_000
        {
            return Err(AuthStoreError::InvalidRateLimitPolicy);
        }
        let expires_at = now + policy.window;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT singleton FROM native_rate_limit_state WHERE singleton FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM native_rate_limit_counters WHERE expires_at <= $1")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query_as::<_, (i32, OffsetDateTime)>(
            "SELECT attempt_count, expires_at FROM native_rate_limit_counters
             WHERE scope=$1 AND actor_key_digest=$2 FOR UPDATE",
        )
        .bind(policy.scope)
        .bind(actor_key_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let (attempts, active_expiry) = if let Some((attempts, active_expiry)) = existing {
            let next = attempts
                .checked_add(1)
                .ok_or(AuthStoreError::InvalidRateLimitPolicy)?;
            sqlx::query(
                "UPDATE native_rate_limit_counters SET attempt_count=$3
                 WHERE scope=$1 AND actor_key_digest=$2",
            )
            .bind(policy.scope)
            .bind(actor_key_digest.as_slice())
            .bind(next)
            .execute(&mut *tx)
            .await?;
            (next, active_expiry)
        } else {
            let retained =
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM native_rate_limit_counters")
                    .fetch_one(&mut *tx)
                    .await?;
            if retained >= policy.max_retained_keys {
                let earliest = sqlx::query_scalar::<_, Option<OffsetDateTime>>(
                    "SELECT min(expires_at) FROM native_rate_limit_counters",
                )
                .fetch_one(&mut *tx)
                .await?
                .unwrap_or(expires_at);
                tx.commit().await?;
                return Ok(RateLimitOutcome::Limited {
                    retry_after_seconds: retry_after_seconds(now, earliest),
                });
            }
            sqlx::query(
                "INSERT INTO native_rate_limit_counters (
                    scope,actor_key_digest,attempt_count,window_started_at,expires_at
                 ) VALUES ($1,$2,1,$3,$4)",
            )
            .bind(policy.scope)
            .bind(actor_key_digest.as_slice())
            .bind(now)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
            (1, expires_at)
        };
        tx.commit().await?;
        if attempts > policy.max_attempts {
            Ok(RateLimitOutcome::Limited {
                retry_after_seconds: retry_after_seconds(now, active_expiry),
            })
        } else {
            Ok(RateLimitOutcome::Allowed {
                remaining: policy.max_attempts - attempts,
            })
        }
    }

    /// Stores one password-reset digest and invalidates any older active token.
    /// A missing email is deliberately indistinguishable from success.
    pub async fn issue_password_reset(
        &self,
        pool: &PgPool,
        issue: &PasswordResetIssue<'_>,
    ) -> Result<bool, AuthStoreError> {
        if issue.normalized_email.trim().is_empty() || issue.issued_at >= issue.expires_at {
            return Err(AuthStoreError::InvalidRefreshRequest);
        }
        let mut tx = pool.begin().await?;
        let user_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM native_users
             WHERE normalized_email=$1 AND enabled AND NOT locked AND NOT blacklisted
             FOR UPDATE",
        )
        .bind(issue.normalized_email)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(user_id) = user_id else {
            tx.rollback().await?;
            return Ok(false);
        };
        sqlx::query(
            "UPDATE native_password_reset_tokens SET invalidated_at=$2
             WHERE user_id=$1 AND consumed_at IS NULL AND invalidated_at IS NULL",
        )
        .bind(user_id)
        .bind(issue.issued_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO native_password_reset_tokens (
                id,user_id,token_digest,issued_at,expires_at,created_at
             ) VALUES($1,$2,$3,$4,$5,$4)",
        )
        .bind(issue.token_id)
        .bind(user_id)
        .bind(issue.token_digest.as_bytes().as_slice())
        .bind(issue.issued_at)
        .bind(issue.expires_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn validate_password_reset(
        &self,
        pool: &PgPool,
        digest: &RefreshTokenDigest,
        now: OffsetDateTime,
    ) -> Result<PasswordResetOutcome, AuthStoreError> {
        let row = sqlx::query_as::<
            _,
            (
                OffsetDateTime,
                Option<OffsetDateTime>,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT expires_at,consumed_at,invalidated_at FROM native_password_reset_tokens
             WHERE token_digest=$1",
        )
        .bind(digest.as_bytes().as_slice())
        .fetch_optional(pool)
        .await?;
        Ok(match row {
            Some((expires_at, None, None)) if now < expires_at => PasswordResetOutcome::Valid,
            Some((expires_at, None, None)) if now >= expires_at => PasswordResetOutcome::Expired,
            _ => PasswordResetOutcome::Invalid,
        })
    }

    /// Atomically consumes a reset token, replaces the bcrypt hash, and
    /// revokes every refresh family for the account.
    pub async fn consume_password_reset(
        &self,
        pool: &PgPool,
        digest: &RefreshTokenDigest,
        new_password_hash: &str,
        now: OffsetDateTime,
    ) -> Result<PasswordResetOutcome, AuthStoreError> {
        if new_password_hash.trim().is_empty() {
            return Err(AuthStoreError::InvalidRefreshRequest);
        }
        let mut tx = pool.begin().await?;
        let row = sqlx::query_as::<
            _,
            (
                Uuid,
                OffsetDateTime,
                Option<OffsetDateTime>,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT user_id,expires_at,consumed_at,invalidated_at
             FROM native_password_reset_tokens WHERE token_digest=$1 FOR UPDATE",
        )
        .bind(digest.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, expires_at, consumed_at, invalidated_at)) = row else {
            tx.rollback().await?;
            return Ok(PasswordResetOutcome::Invalid);
        };
        if consumed_at.is_some() || invalidated_at.is_some() {
            tx.rollback().await?;
            return Ok(PasswordResetOutcome::Invalid);
        }
        if now >= expires_at {
            tx.rollback().await?;
            return Ok(PasswordResetOutcome::Expired);
        }
        sqlx::query("UPDATE native_password_reset_tokens SET consumed_at=$2 WHERE token_digest=$1")
            .bind(digest.as_bytes().as_slice())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE native_users SET password_hash=$2,password_hash_algorithm='bcrypt',
             auth_revision=auth_revision+1,updated_at=$3 WHERE id=$1",
        )
        .bind(user_id)
        .bind(new_password_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        revoke_all_families_in_tx(&mut tx, user_id, now, "password_reset").await?;
        tx.commit().await?;
        Ok(PasswordResetOutcome::Consumed { user_id })
    }

    /// Issues the first token of one bounded refresh family.
    pub async fn issue_refresh_family(
        &self,
        pool: &PgPool,
        issue: &RefreshFamilyIssue<'_>,
    ) -> Result<(), AuthStoreError> {
        if issue.expected_auth_revision <= 0
            || issue.issued_at < issue.session_started_at
            || issue.issued_at >= issue.token_expires_at
            || issue.token_expires_at > issue.family_expires_at
            || issue
                .device_id
                .is_some_and(|value| value.trim().is_empty() || value.len() > 256)
        {
            return Err(AuthStoreError::InvalidRefreshRequest);
        }
        let mut tx = pool.begin().await?;
        // Serialize credential issuance with password reset/account mutation.
        // The revision fence also rejects a login that verified the old
        // password before a concurrent reset acquired this row lock.
        let account = sqlx::query_as::<_, (bool, bool, bool, i64)>(
            "SELECT enabled, locked, blacklisted, auth_revision
             FROM native_users WHERE id = $1 FOR UPDATE",
        )
        .bind(issue.user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AuthStoreError::InvalidRefreshRequest)?;
        if !account.0 || account.1 || account.2 || account.3 != issue.expected_auth_revision {
            tx.rollback().await?;
            return Err(AuthStoreError::InvalidRefreshRequest);
        }
        sqlx::query(
            "INSERT INTO native_refresh_families (
                id,user_id,device_id,session_started_at,expires_at,created_at,updated_at
             ) VALUES ($1,$2,$3,$4,$5,$4,$4)",
        )
        .bind(issue.family_id)
        .bind(issue.user_id)
        .bind(issue.device_id)
        .bind(issue.session_started_at)
        .bind(issue.family_expires_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO native_refresh_tokens (
                id,family_id,token_digest,issued_at,expires_at,created_at
             ) VALUES ($1,$2,$3,$4,$5,$4)",
        )
        .bind(issue.token_id)
        .bind(issue.family_id)
        .bind(issue.token_digest.as_bytes().as_slice())
        .bind(issue.issued_at)
        .bind(issue.token_expires_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically consumes one refresh token and inserts its successor.
    pub async fn rotate_refresh_token(
        &self,
        pool: &PgPool,
        request: &RefreshRotationRequest<'_>,
    ) -> Result<RefreshRotationOutcome, AuthStoreError> {
        if request
            .device_id
            .is_some_and(|value| value.trim().is_empty() || value.len() > 256)
        {
            return Ok(RefreshRotationOutcome::Invalid);
        }
        let mut tx = pool.begin().await?;
        let row = sqlx::query_as::<_, RefreshRow>(
            "SELECT t.id AS token_id, f.id AS family_id, f.user_id,
                    f.expires_at AS family_expires_at, t.expires_at AS token_expires_at,
                    f.revoked_at AS family_revoked_at, t.consumed_at AS token_consumed_at,
                    t.revoked_at AS token_revoked_at, f.device_id AS family_device_id,
                    u.enabled, u.locked, u.blacklisted
             FROM native_refresh_tokens t
             JOIN native_refresh_families f ON f.id = t.family_id
             JOIN native_users u ON u.id = f.user_id
             WHERE t.token_digest = $1
             FOR UPDATE OF f, t, u",
        )
        .bind(request.current_digest.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(RefreshRotationOutcome::Invalid);
        };
        if row.family_revoked_at.is_some()
            || row.token_consumed_at.is_some()
            || row.token_revoked_at.is_some()
        {
            revoke_family(
                &mut tx,
                row.family_id,
                request.now,
                "refresh_token_replayed",
            )
            .await?;
            tx.commit().await?;
            return Ok(RefreshRotationOutcome::Replayed);
        }
        if request.now >= row.family_expires_at || request.now >= row.token_expires_at {
            tx.rollback().await?;
            return Ok(RefreshRotationOutcome::Expired);
        }
        if !row.enabled || row.locked || row.blacklisted {
            revoke_family(&mut tx, row.family_id, request.now, "account_unavailable").await?;
            tx.commit().await?;
            return Ok(RefreshRotationOutcome::AccountUnavailable);
        }
        if row.family_device_id.is_some() && row.family_device_id.as_deref() != request.device_id
            || request.replacement_expires_at <= request.now
        {
            tx.rollback().await?;
            return Ok(RefreshRotationOutcome::Invalid);
        }
        let replacement_expires_at = request.replacement_expires_at.min(row.family_expires_at);
        sqlx::query(
            "INSERT INTO native_refresh_tokens (
                id,family_id,token_digest,issued_at,expires_at,created_at
             ) VALUES ($1,$2,$3,$4,$5,$4)",
        )
        .bind(request.replacement_token_id)
        .bind(row.family_id)
        .bind(request.replacement_digest.as_bytes().as_slice())
        .bind(request.now)
        .bind(replacement_expires_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE native_refresh_tokens
             SET consumed_at=$2, replacement_token_id=$3
             WHERE id=$1",
        )
        .bind(row.token_id)
        .bind(request.now)
        .bind(request.replacement_token_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE native_refresh_families
             SET updated_at=$2, device_id=COALESCE(device_id,$3) WHERE id=$1",
        )
        .bind(row.family_id)
        .bind(request.now)
        .bind(request.device_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(RefreshRotationOutcome::Rotated {
            user_id: row.user_id,
            family_id: row.family_id,
            refresh_expires_at: replacement_expires_at,
        })
    }

    /// Revokes one token. Reuse later is classified as replay and revokes its family.
    pub async fn revoke_refresh_token(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        digest: &RefreshTokenDigest,
        now: OffsetDateTime,
        reason: &str,
    ) -> Result<bool, AuthStoreError> {
        if reason.trim().is_empty() {
            return Err(AuthStoreError::InvalidRefreshRequest);
        }
        let result = sqlx::query(
            "UPDATE native_refresh_tokens t SET revoked_at=$3, revocation_reason=$4
             FROM native_refresh_families f
             WHERE t.family_id=f.id AND f.user_id=$1 AND t.token_digest=$2
               AND t.revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(digest.as_bytes().as_slice())
        .bind(now)
        .bind(reason)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Revokes every refresh family owned by a user, matching captured logout behavior.
    pub async fn revoke_all_refresh_tokens(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        now: OffsetDateTime,
        reason: &str,
    ) -> Result<u64, AuthStoreError> {
        if reason.trim().is_empty() {
            return Err(AuthStoreError::InvalidRefreshRequest);
        }
        let result = sqlx::query(
            "UPDATE native_refresh_families
             SET revoked_at=$2, revocation_reason=$3, updated_at=$2
             WHERE user_id=$1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(now)
        .bind(reason)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Creates the first administrator, failing closed when any admin already
    /// exists (operator bootstrap/recovery, PRD-CLI-001). The advisory lock
    /// serializes the existence check with a concurrent bootstrap in another
    /// process. The account receives the captured `Administrator` and
    /// `HasStreamRole` roles and fresh v7 identities.
    pub async fn bootstrap_admin(
        &self,
        pool: &PgPool,
        request: &BootstrapAdminRequest<'_>,
    ) -> Result<StoredNativeUser, AuthStoreError> {
        let username = request.username.trim();
        let email = request.email.trim();
        if username.is_empty()
            || email.is_empty()
            || request.password_hash.trim().is_empty()
            || request.stream_signing_key.trim().is_empty()
        {
            return Err(AuthStoreError::InvalidBootstrapRequest);
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('native_users_bootstrap_admin'))")
            .execute(&mut *tx)
            .await?;
        let admin_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM native_users WHERE is_admin)",
        )
        .fetch_one(&mut *tx)
        .await?;
        if admin_exists {
            tx.rollback().await?;
            return Err(AuthStoreError::AdminExists);
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO native_users (
                id, public_api_id, username, normalized_username, email, normalized_email,
                password_hash, password_hash_algorithm, stream_signing_key, is_admin, is_editor,
                enabled, locked, blacklisted, auth_revision, created_at, updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,'bcrypt',$8,true,false,true,false,false,1,$9,$9)",
        )
        .bind(id)
        .bind(Uuid::now_v7())
        .bind(username)
        .bind(normalize_login(username))
        .bind(email)
        .bind(normalize_login(email))
        .bind(request.password_hash)
        .bind(request.stream_signing_key)
        .bind(request.now)
        .execute(&mut *tx)
        .await?;
        for role in ["Administrator", "HasStreamRole"] {
            sqlx::query(
                "INSERT INTO native_user_roles (user_id, role, created_at)
                 VALUES ($1,$2,$3) ON CONFLICT (user_id, role) DO NOTHING",
            )
            .bind(id)
            .bind(role)
            .bind(request.now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.user_by_id(pool, id)
            .await?
            .ok_or(AuthStoreError::CorruptState)
    }

    /// Records an idempotent public-ID import binding.
    pub async fn bind_migration_identity(
        &self,
        pool: &PgPool,
        binding: &MigrationIdentityBinding,
    ) -> Result<(), AuthStoreError> {
        let inserted = sqlx::query(
            "INSERT INTO native_migration_identity_bindings (
                source_kind,source_key,internal_id,public_api_id,imported_at
             ) VALUES ($1,$2,$3,$4,$5)
             ON CONFLICT (source_kind,source_key) DO NOTHING",
        )
        .bind(&binding.source_kind)
        .bind(&binding.source_key)
        .bind(binding.internal_id)
        .bind(binding.public_api_id)
        .bind(binding.imported_at)
        .execute(pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(());
        }
        let existing = sqlx::query_as::<_, MigrationIdentityBinding>(
            "SELECT source_kind,source_key,internal_id,public_api_id,imported_at
             FROM native_migration_identity_bindings
             WHERE source_kind=$1 AND source_key=$2",
        )
        .bind(&binding.source_kind)
        .bind(&binding.source_key)
        .fetch_one(pool)
        .await?;
        if existing.internal_id == binding.internal_id
            && existing.public_api_id == binding.public_api_id
        {
            Ok(())
        } else {
            Err(AuthStoreError::IdentityBindingConflict)
        }
    }
}

fn password_algorithm_tag(algorithm: PasswordHashAlgorithm) -> &'static str {
    match algorithm {
        PasswordHashAlgorithm::Bcrypt => "bcrypt",
    }
}

async fn roles_for_pool(pool: &PgPool, user_id: Uuid) -> Result<Vec<String>, AuthStoreError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT role FROM native_user_roles WHERE user_id=$1 ORDER BY role",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

async fn roles_for_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Vec<String>, AuthStoreError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT role FROM native_user_roles WHERE user_id=$1 ORDER BY role",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?)
}

async fn user_import_matches(
    tx: &mut Transaction<'_, Postgres>,
    imported: &NativeUserImport<'_>,
) -> Result<bool, AuthStoreError> {
    #[derive(FromRow)]
    struct ImportRow {
        public_api_id: Uuid,
        username: String,
        normalized_username: String,
        email: String,
        normalized_email: String,
        password_hash: String,
        password_hash_algorithm: String,
        stream_signing_key: String,
        is_admin: bool,
        is_editor: bool,
        enabled: bool,
        locked: bool,
        blacklisted: bool,
        preferred_language: Option<String>,
        preferred_theme: Option<String>,
        auth_revision: i64,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
    }
    let row = sqlx::query_as::<_, ImportRow>(
        "SELECT public_api_id,username,normalized_username,email,normalized_email,password_hash,
                password_hash_algorithm,stream_signing_key,is_admin,is_editor,enabled,locked,
                blacklisted,preferred_language,
                preferred_theme,auth_revision,created_at,updated_at
         FROM native_users WHERE id=$1 FOR UPDATE",
    )
    .bind(imported.id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.public_api_id == imported.public_api_id
        && row.username == imported.username
        && row.normalized_username == imported.normalized_username
        && row.email == imported.email
        && row.normalized_email == imported.normalized_email
        && row.password_hash == imported.password_hash
        && row.password_hash_algorithm == password_algorithm_tag(imported.password_hash_algorithm)
        && row.stream_signing_key == imported.stream_signing_key
        && row.is_admin == imported.is_admin
        && row.is_editor == imported.is_editor
        && row.enabled == imported.enabled
        && row.locked == imported.locked
        && row.blacklisted == imported.blacklisted
        && row.preferred_language.as_deref() == imported.preferred_language
        && row.preferred_theme.as_deref() == imported.preferred_theme
        && row.auth_revision == imported.auth_revision
        && row.created_at == imported.created_at
        && row.updated_at == imported.updated_at)
}

pub(crate) fn stored_user_from_row(row: UserRow, roles: Vec<String>) -> StoredNativeUser {
    StoredNativeUser {
        id: row.id,
        public_api_id: row.public_api_id,
        username: row.username,
        email: row.email,
        is_admin: row.is_admin,
        is_editor: row.is_editor,
        enabled: row.enabled,
        locked: row.locked,
        blacklisted: row.blacklisted,
        preferred_language: row.preferred_language,
        preferred_theme: row.preferred_theme,
        auth_revision: row.auth_revision,
        roles,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn authentication_from_row(
    row: AuthenticationRow,
    roles: Vec<String>,
) -> Result<AuthenticationRecord, AuthStoreError> {
    let algorithm = PasswordHashAlgorithm::from_storage(&row.password_hash_algorithm)
        .ok_or(AuthStoreError::CorruptState)?;
    let user = stored_user_from_row(
        UserRow {
            id: row.id,
            public_api_id: row.public_api_id,
            username: row.username,
            email: row.email,
            is_admin: row.is_admin,
            is_editor: row.is_editor,
            enabled: row.enabled,
            locked: row.locked,
            blacklisted: row.blacklisted,
            preferred_language: row.preferred_language,
            preferred_theme: row.preferred_theme,
            auth_revision: row.auth_revision,
            created_at: row.created_at,
            updated_at: row.updated_at,
        },
        roles,
    );
    Ok(AuthenticationRecord {
        user,
        password_hash_algorithm: algorithm,
        password_hash: SecretString::new(row.password_hash),
    })
}

async fn revoke_family(
    tx: &mut Transaction<'_, Postgres>,
    family_id: Uuid,
    now: OffsetDateTime,
    reason: &str,
) -> Result<(), AuthStoreError> {
    sqlx::query(
        "UPDATE native_refresh_families
         SET revoked_at=COALESCE(revoked_at,$2),
             revocation_reason=COALESCE(revocation_reason,$3), updated_at=$2
         WHERE id=$1",
    )
    .bind(family_id)
    .bind(now)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE native_refresh_tokens
         SET revoked_at=COALESCE(revoked_at,$2),
             revocation_reason=COALESCE(revocation_reason,$3)
         WHERE family_id=$1",
    )
    .bind(family_id)
    .bind(now)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn revoke_all_families_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    now: OffsetDateTime,
    reason: &str,
) -> Result<(), AuthStoreError> {
    sqlx::query("SELECT id FROM native_refresh_families WHERE user_id=$1 FOR UPDATE")
        .bind(user_id)
        .fetch_all(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE native_refresh_tokens token
         SET revoked_at=COALESCE(token.revoked_at,$2),
             revocation_reason=COALESCE(token.revocation_reason,$3)
         FROM native_refresh_families family
         WHERE token.family_id=family.id AND family.user_id=$1",
    )
    .bind(user_id)
    .bind(now)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE native_refresh_families
         SET revoked_at=COALESCE(revoked_at,$2),
             revocation_reason=COALESCE(revocation_reason,$3),updated_at=$2
         WHERE user_id=$1",
    )
    .bind(user_id)
    .bind(now)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn retry_after_seconds(now: OffsetDateTime, expires_at: OffsetDateTime) -> i64 {
    (expires_at - now).whole_seconds().max(1)
}

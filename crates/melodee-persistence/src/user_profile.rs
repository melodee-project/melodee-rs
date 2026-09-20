//! Self-service account/profile persistence for MEL-2 (ADR-0178).
//!
//! The request types deliberately contain no role or account-state fields. All
//! mutations are scoped by the authenticated internal user id and the update
//! plus its value-free audit event commit together.

use sqlx::{FromRow, PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::{AuthStore, StoredNativeUser, normalize_login};

pub const MAX_USERNAME_CHARS: usize = 255;
pub const MAX_EMAIL_CHARS: usize = 255;
pub const MAX_DESCRIPTION_CHARS: usize = 62_000;
pub const MAX_TIME_ZONE_CHARS: usize = 64;
pub const MAX_LANGUAGE_CHARS: usize = 10;
pub const MAX_THEME_CHARS: usize = 20;
pub const MIN_PASSWORD_CHARS: usize = 6;

const DEFAULT_TIME_ZONE: &str = "UTC";
const DEFAULT_ROLES: [&str; 10] = [
    "HasSettingsRole",
    "HasDownloadRole",
    "HasUploadRole",
    "HasPlaylistRole",
    "HasShareRole",
    "HasCoverArtRole",
    "HasCommentRole",
    "HasPodcastRole",
    "HasStreamRole",
    "HasJukeboxRole",
];

/// Clear-text values have already been validated by the HTTP layer's bounded
/// request body. Persistence repeats the checks because this is the authority
/// boundary and is also used by migration/tests.
pub struct RegistrationRequest<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub password_hash: &'a str,
    pub stream_signing_key: &'a str,
    pub now: OffsetDateTime,
}

/// Optional profile replacement fields. `Some(None)` clears nullable fields;
/// `None` leaves them unchanged.
#[derive(Clone, Debug, Default)]
pub struct ProfileUpdate {
    pub username: Option<String>,
    pub email: Option<String>,
    pub description: Option<Option<String>>,
    pub time_zone_id: Option<String>,
    pub preferred_language: Option<Option<String>>,
    pub preferred_theme: Option<Option<String>>,
}

#[derive(Debug, Error)]
pub enum UserProfileError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("invalid profile value: {0}")]
    InvalidValue(&'static str),
    #[error("username or email conflicts with an existing user")]
    Conflict,
    #[error("user was not found")]
    NotFound,
}

/// Validates a bounded timezone identifier without making the target server's
/// installed tzdata part of account identity. The character grammar matches
/// IANA-style IDs and accepts `UTC`, offsets, and region IDs.
#[must_use]
pub fn valid_time_zone_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TIME_ZONE_CHARS
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-' | b'.' | b'/')
        })
}

fn validate_registration(request: &RegistrationRequest<'_>) -> Result<(), UserProfileError> {
    validate_username(request.username)?;
    validate_email(request.email)?;
    if request.password_hash.trim().is_empty() {
        return Err(UserProfileError::InvalidValue("password hash"));
    }
    if request.stream_signing_key.trim().is_empty() {
        return Err(UserProfileError::InvalidValue("stream signing key"));
    }
    Ok(())
}

fn validate_username(value: &str) -> Result<(), UserProfileError> {
    let chars = value.chars().count();
    if !(3..=MAX_USERNAME_CHARS).contains(&chars) || value.trim().is_empty() {
        return Err(UserProfileError::InvalidValue("username"));
    }
    Ok(())
}

fn validate_email(value: &str) -> Result<(), UserProfileError> {
    let chars = value.chars().count();
    let Some((local, domain)) = value.split_once('@') else {
        return Err(UserProfileError::InvalidValue("email"));
    };
    if !(1..=MAX_EMAIL_CHARS).contains(&chars)
        || local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || value.chars().any(char::is_whitespace)
    {
        return Err(UserProfileError::InvalidValue("email"));
    }
    Ok(())
}

fn bounded_optional(
    value: Option<String>,
    max: usize,
    field: &'static str,
) -> Result<Option<String>, UserProfileError> {
    match value {
        None => Ok(None),
        Some(value) if value.chars().count() > max => Err(UserProfileError::InvalidValue(field)),
        Some(value) if value.is_empty() => Ok(None),
        Some(value) => Ok(Some(value)),
    }
}

#[derive(FromRow)]
struct ProfileRow {
    username: String,
    email: String,
    description: Option<String>,
    time_zone_id: String,
    preferred_language: Option<String>,
    preferred_theme: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UserProfileStore;

impl UserProfileStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Registers an account and serializes first-admin bootstrap with a table
    /// lock so concurrent registrations cannot both become administrators.
    pub async fn register(
        &self,
        pool: &PgPool,
        request: &RegistrationRequest<'_>,
    ) -> Result<StoredNativeUser, UserProfileError> {
        validate_registration(request)?;
        let username = request.username.trim();
        let email = request.email.trim();
        let normalized_username = normalize_login(username);
        let normalized_email = normalize_login(email);
        let mut tx = pool.begin().await?;
        sqlx::query("LOCK TABLE native_users IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let is_first =
            sqlx::query_scalar::<_, bool>("SELECT NOT EXISTS (SELECT 1 FROM native_users)")
                .fetch_one(&mut *tx)
                .await?;
        let user_id = Uuid::now_v7();
        let public_api_id = Uuid::now_v7();
        let roles = DEFAULT_ROLES
            .iter()
            .copied()
            .chain(is_first.then_some("Administrator"))
            .collect::<Vec<_>>();
        let is_admin = is_first;
        sqlx::query(
            "INSERT INTO native_users (
                id, public_api_id, username, normalized_username, email, normalized_email,
                password_hash, password_hash_algorithm, stream_signing_key, is_admin, is_editor,
                enabled, locked, blacklisted, description, time_zone_id, auth_revision,
                created_at, updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,'bcrypt',$8,$9,$9,true,false,false,NULL,'UTC',1,$10,$10)",
        )
        .bind(user_id)
        .bind(public_api_id)
        .bind(username)
        .bind(&normalized_username)
        .bind(email)
        .bind(&normalized_email)
        .bind(request.password_hash)
        .bind(request.stream_signing_key)
        .bind(is_admin)
        .bind(request.now)
        .execute(&mut *tx)
        .await
        .map_err(map_database_error)?;
        for role in roles {
            sqlx::query(
                "INSERT INTO native_user_roles (user_id, role, created_at) VALUES ($1,$2,$3)",
            )
            .bind(user_id)
            .bind(role)
            .bind(request.now)
            .execute(&mut *tx)
            .await?;
        }
        insert_audit(
            &mut tx,
            user_id,
            user_id,
            if is_first {
                "account_registered_first_admin"
            } else {
                "account_registered"
            },
            &["username", "email"],
            request.now,
        )
        .await?;
        tx.commit().await?;
        AuthStore::new()
            .user_by_id(pool, user_id)
            .await
            .map_err(|error| match error {
                crate::auth::AuthStoreError::Database(error) => UserProfileError::Database(error),
                _ => UserProfileError::NotFound,
            })?
            .ok_or(UserProfileError::NotFound)
    }

    /// Applies only profile fields for the authenticated subject.
    pub async fn update(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        update: &ProfileUpdate,
        now: OffsetDateTime,
    ) -> Result<StoredNativeUser, UserProfileError> {
        let mut tx = pool.begin().await?;
        let current = sqlx::query_as::<_, ProfileRow>(
            "SELECT username,email,description,time_zone_id,preferred_language,preferred_theme
             FROM native_users WHERE id=$1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(UserProfileError::NotFound)?;
        let username = update
            .username
            .as_deref()
            .unwrap_or(&current.username)
            .trim()
            .to_owned();
        let email = update
            .email
            .as_deref()
            .unwrap_or(&current.email)
            .trim()
            .to_owned();
        validate_username(&username)?;
        validate_email(&email)?;
        let description = match &update.description {
            Some(value) => {
                if value
                    .as_ref()
                    .is_some_and(|v| v.chars().count() > MAX_DESCRIPTION_CHARS)
                {
                    return Err(UserProfileError::InvalidValue("description"));
                }
                value.clone()
            }
            None => current.description.clone(),
        };
        let time_zone_id = update
            .time_zone_id
            .as_deref()
            .unwrap_or(&current.time_zone_id)
            .trim();
        let time_zone_id = if time_zone_id.is_empty() {
            DEFAULT_TIME_ZONE
        } else if valid_time_zone_id(time_zone_id) {
            time_zone_id
        } else {
            return Err(UserProfileError::InvalidValue("time zone id"));
        };
        let preferred_language = match &update.preferred_language {
            Some(value) => bounded_optional(value.clone(), MAX_LANGUAGE_CHARS, "language")?,
            None => current.preferred_language.clone(),
        };
        let preferred_theme = match &update.preferred_theme {
            Some(value) => bounded_optional(value.clone(), MAX_THEME_CHARS, "theme")?,
            None => current.preferred_theme.clone(),
        };
        let normalized_username = normalize_login(&username);
        let normalized_email = normalize_login(&email);
        let conflict = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM native_users
                 WHERE id <> $1 AND (normalized_username=$2 OR normalized_email=$3)
             )",
        )
        .bind(user_id)
        .bind(&normalized_username)
        .bind(&normalized_email)
        .fetch_one(&mut *tx)
        .await?;
        if conflict {
            return Err(UserProfileError::Conflict);
        }
        let mut changed = Vec::new();
        if current.username != username {
            changed.push("username");
        }
        if current.email != email {
            changed.push("email");
        }
        if current.description != description {
            changed.push("description");
        }
        if current.time_zone_id != time_zone_id {
            changed.push("timeZoneId");
        }
        if current.preferred_language != preferred_language {
            changed.push("language");
        }
        if current.preferred_theme != preferred_theme {
            changed.push("theme");
        }
        if !changed.is_empty() {
            sqlx::query(
                "UPDATE native_users
                    SET username=$2, normalized_username=$3, email=$4, normalized_email=$5,
                        description=$6, time_zone_id=$7, preferred_language=$8,
                        preferred_theme=$9, updated_at=$10
                  WHERE id=$1",
            )
            .bind(user_id)
            .bind(&username)
            .bind(&normalized_username)
            .bind(&email)
            .bind(&normalized_email)
            .bind(&description)
            .bind(time_zone_id)
            .bind(&preferred_language)
            .bind(&preferred_theme)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            insert_audit(&mut tx, user_id, user_id, "profile_updated", &changed, now).await?;
        }
        tx.commit().await?;
        AuthStore::new()
            .user_by_id(pool, user_id)
            .await
            .map_err(|error| match error {
                crate::auth::AuthStoreError::Database(error) => UserProfileError::Database(error),
                _ => UserProfileError::NotFound,
            })?
            .ok_or(UserProfileError::NotFound)
    }
}

async fn insert_audit(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    actor_user_id: Uuid,
    event: &str,
    changed_fields: &[&str],
    now: OffsetDateTime,
) -> Result<(), UserProfileError> {
    let fields = changed_fields
        .iter()
        .map(|field| (*field).to_owned())
        .collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO native_user_profile_audits
            (id,user_id,actor_user_id,event,changed_fields,created_at)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(actor_user_id)
    .bind(event)
    .bind(fields)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn map_database_error(error: sqlx::Error) -> UserProfileError {
    if error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23505")
    {
        UserProfileError::Conflict
    } else {
        UserProfileError::Database(error)
    }
}

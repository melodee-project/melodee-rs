//! Phase 9 WP-03 user administration persistence (ADR-0134 §2): the avatar
//! store (`native_user_avatars`, migration 0022) and admin user management
//! beyond the frozen list route. Library-access-group administration lives in
//! `group_admin` (shared with the WP-05 CLI surface).
//!
//! Every mutation is bounded and validated before touching the database;
//! secrets (password hashes, stream signing keys) never appear in returned
//! values or error messages (PRD-AUTH-005). Groups and users are addressed by
//! their immutable public ids; names are display values, never identity.

use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::auth::{AuthStoreError, StoredNativeUser, UserRow, stored_user_from_row};

/// Maximum accepted length for a username or email on admin user creation.
pub const MAX_USER_IDENTITY_CHARS: usize = 320;
/// Maximum roles assignable to one user through the admin surface.
pub const MAX_USER_ROLES: usize = 64;
/// Maximum bytes of one stored avatar image (mirrors the 0022 CHECK).
pub const MAX_AVATAR_BYTES: usize = 1024 * 1024;

/// One stored avatar row. `updated_at` drives avatarLastChanged semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAvatar {
    pub user_id: Uuid,
    pub content_type: String,
    pub image: Vec<u8>,
    pub updated_at: OffsetDateTime,
}

/// Admin user-creation request. The password is already bcrypt-hashed by the
/// caller (`melodee_auth::hash_password`); this store never sees clear text.
pub struct NativeUserCreate<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub password_hash: &'a str,
    pub stream_signing_key: &'a str,
    pub is_admin: bool,
    pub is_editor: bool,
    pub roles: &'a [String],
}

/// Field-optional admin user update. Omitted fields keep their stored value.
#[derive(Clone, Debug, Default)]
pub struct NativeUserAdminUpdate {
    pub is_admin: Option<bool>,
    pub is_editor: Option<bool>,
    pub enabled: Option<bool>,
    pub locked: Option<bool>,
    pub roles: Option<Vec<String>>,
}

#[derive(Debug, Error)]
pub enum UserAdminError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("resource not found")]
    NotFound,
    #[error("username or email conflicts with an existing user")]
    UserConflict,
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
}

impl From<AuthStoreError> for UserAdminError {
    fn from(error: AuthStoreError) -> Self {
        match error {
            AuthStoreError::Database(error) => Self::Database(error),
            _ => Self::InvalidRequest("authentication store rejected the operation"),
        }
    }
}

/// Stateless entry point for Phase 9 user/group/avatar administration.
#[derive(Clone, Copy, Debug, Default)]
pub struct UserAdminStore;

impl UserAdminStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    // ------------------------------------------------------------------
    // Avatars (ADR-0134 §2, migration 0022)
    // ------------------------------------------------------------------

    /// Loads the avatar row for an internal user id.
    pub async fn avatar_by_user_id(
        &self,
        pool: &PgPool,
        user_id: Uuid,
    ) -> Result<Option<StoredAvatar>, UserAdminError> {
        Ok(
            sqlx::query_as::<_, (Uuid, String, Vec<u8>, OffsetDateTime)>(
                "SELECT user_id, content_type, image, updated_at
             FROM native_user_avatars WHERE user_id=$1",
            )
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .map(|(user_id, content_type, image, updated_at)| StoredAvatar {
                user_id,
                content_type,
                image,
                updated_at,
            }),
        )
    }

    /// Replaces (or installs) one user's avatar. The caller has already
    /// validated content type, magic bytes, and the 1 MiB bound; the table
    /// CHECK constraints are the fail-closed backstop.
    pub async fn set_avatar(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        content_type: &str,
        image: &[u8],
        now: OffsetDateTime,
    ) -> Result<StoredAvatar, UserAdminError> {
        if !matches!(content_type, "image/jpeg" | "image/png") {
            return Err(UserAdminError::InvalidRequest("avatar content type"));
        }
        if image.is_empty() || image.len() > MAX_AVATAR_BYTES {
            return Err(UserAdminError::InvalidRequest("avatar size"));
        }
        let updated = sqlx::query(
            "INSERT INTO native_user_avatars (user_id, content_type, image, updated_at)
             VALUES ($1,$2,$3,$4)
             ON CONFLICT (user_id) DO UPDATE
             SET content_type=$2, image=$3, updated_at=$4",
        )
        .bind(user_id)
        .bind(content_type)
        .bind(image)
        .bind(now)
        .execute(pool)
        .await?;
        debug_assert_eq!(updated.rows_affected(), 1);
        self.avatar_by_user_id(pool, user_id)
            .await?
            .ok_or(UserAdminError::NotFound)
    }

    /// Removes one user's avatar. Returns false when none was stored.
    pub async fn delete_avatar(
        &self,
        pool: &PgPool,
        user_id: Uuid,
    ) -> Result<bool, UserAdminError> {
        Ok(
            sqlx::query("DELETE FROM native_user_avatars WHERE user_id=$1")
                .bind(user_id)
                .execute(pool)
                .await?
                .rows_affected()
                == 1,
        )
    }

    // ------------------------------------------------------------------
    // Admin user management (migration 0010 columns)
    // ------------------------------------------------------------------

    /// Creates one enabled, unlocked user with a caller-supplied bcrypt hash.
    /// Duplicate normalized username/email fails closed with `UserConflict`.
    pub async fn create_user(
        &self,
        pool: &PgPool,
        create: &NativeUserCreate<'_>,
        now: OffsetDateTime,
    ) -> Result<StoredNativeUser, UserAdminError> {
        let username = create.username.trim();
        let email = create.email.trim();
        if username.is_empty() || username.chars().count() > MAX_USER_IDENTITY_CHARS {
            return Err(UserAdminError::InvalidRequest("username"));
        }
        if email.is_empty() || email.chars().count() > MAX_USER_IDENTITY_CHARS {
            return Err(UserAdminError::InvalidRequest("email"));
        }
        if create.password_hash.trim().is_empty()
            || !create.password_hash.starts_with("$2")
            || create.stream_signing_key.trim().is_empty()
            || create.stream_signing_key.len() > 256
        {
            return Err(UserAdminError::InvalidRequest("credentials"));
        }
        validate_roles(create.roles)?;
        let id = Uuid::now_v7();
        let public_api_id = Uuid::now_v7();
        let mut tx = pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO native_users (
                id, public_api_id, username, normalized_username, email, normalized_email,
                password_hash, password_hash_algorithm, stream_signing_key, is_admin, is_editor,
                enabled, locked, blacklisted, auth_revision, created_at, updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,'bcrypt',$8,$9,$10,true,false,false,1,$11,$11)
             ON CONFLICT (normalized_username) DO NOTHING",
        )
        .bind(id)
        .bind(public_api_id)
        .bind(username)
        .bind(username.to_uppercase())
        .bind(email)
        .bind(email.to_uppercase())
        .bind(create.password_hash)
        .bind(create.stream_signing_key)
        .bind(create.is_admin)
        .bind(create.is_editor)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            tx.rollback().await?;
            return Err(UserAdminError::UserConflict);
        }
        for role in create.roles {
            sqlx::query(
                "INSERT INTO native_user_roles (user_id, role, created_at)
                 VALUES ($1,$2,$3) ON CONFLICT (user_id, role) DO NOTHING",
            )
            .bind(id)
            .bind(role)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        load_user_by_public_id(pool, public_api_id)
            .await?
            .ok_or(UserAdminError::NotFound)
    }

    /// Applies a field-optional admin update. Any flag/role change bumps
    /// `auth_revision` (invalidating in-flight issuance) and revokes every
    /// refresh family when the account becomes disabled or locked.
    pub async fn update_user(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
        update: &NativeUserAdminUpdate,
        now: OffsetDateTime,
    ) -> Result<StoredNativeUser, UserAdminError> {
        if let Some(roles) = &update.roles {
            validate_roles(roles)?;
        }
        let mut tx = pool.begin().await?;
        let row = sqlx::query_as::<_, (Uuid, bool, bool)>(
            "SELECT id, enabled, locked FROM native_users WHERE public_api_id=$1 FOR UPDATE",
        )
        .bind(public_api_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, was_enabled, was_locked)) = row else {
            tx.rollback().await?;
            return Err(UserAdminError::NotFound);
        };
        sqlx::query(
            "UPDATE native_users SET
                is_admin=COALESCE($2, is_admin),
                is_editor=COALESCE($3, is_editor),
                enabled=COALESCE($4, enabled),
                locked=COALESCE($5, locked),
                auth_revision=auth_revision+1,
                updated_at=$6
             WHERE id=$1",
        )
        .bind(user_id)
        .bind(update.is_admin)
        .bind(update.is_editor)
        .bind(update.enabled)
        .bind(update.locked)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if let Some(roles) = &update.roles {
            sqlx::query("DELETE FROM native_user_roles WHERE user_id=$1")
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            for role in roles {
                sqlx::query(
                    "INSERT INTO native_user_roles (user_id, role, created_at)
                     VALUES ($1,$2,$3) ON CONFLICT (user_id, role) DO NOTHING",
                )
                .bind(user_id)
                .bind(role)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
        }
        let now_enabled = update.enabled.unwrap_or(was_enabled);
        let now_locked = update.locked.unwrap_or(was_locked);
        if !now_enabled || now_locked {
            revoke_all_families(&mut tx, user_id, now, "admin_account_state").await?;
        }
        tx.commit().await?;
        load_user_by_public_id(pool, public_api_id)
            .await?
            .ok_or(UserAdminError::NotFound)
    }
}

fn validate_roles(roles: &[String]) -> Result<(), UserAdminError> {
    if roles.len() > MAX_USER_ROLES
        || roles
            .iter()
            .any(|role| role.trim().is_empty() || role.chars().count() > 100)
    {
        return Err(UserAdminError::InvalidRequest("roles"));
    }
    Ok(())
}

async fn load_user_by_public_id(
    pool: &PgPool,
    public_api_id: Uuid,
) -> Result<Option<StoredNativeUser>, UserAdminError> {
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, public_api_id, username, email, is_admin, is_editor, enabled, locked,
                blacklisted, preferred_language, preferred_theme, auth_revision,
                created_at, updated_at FROM native_users WHERE public_api_id=$1",
    )
    .bind(public_api_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => {
            let roles = sqlx::query_scalar::<_, String>(
                "SELECT role FROM native_user_roles WHERE user_id=$1 ORDER BY role",
            )
            .bind(row.id)
            .fetch_all(pool)
            .await?;
            Ok(Some(stored_user_from_row(row, roles)))
        }
        None => Ok(None),
    }
}

async fn revoke_all_families(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    now: OffsetDateTime,
    reason: &str,
) -> Result<(), UserAdminError> {
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

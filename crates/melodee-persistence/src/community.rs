//! Replay-safe import for native public-share bearer capabilities (ADR-0127/ADR-0129).

use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

const SHARE_TOKEN_BYTES: usize = 16;
const MAX_TOKEN_ATTEMPTS: usize = 4;

/// One legacy share record presented by an explicit migration adapter.
#[derive(Clone, Copy)]
pub struct NativeShareImport<'a> {
    pub source_key: &'a str,
    pub internal_id: Uuid,
    pub public_api_id: Uuid,
    pub legacy_token: &'a str,
    /// Set only when the source migration proves at least 128 unpredictable bits.
    pub legacy_token_is_secure: bool,
    pub owner_user_id: Uuid,
    pub share_type: i16,
    pub resource_public_api_id: Uuid,
    pub description: Option<&'a str>,
    pub is_downloadable: bool,
    pub expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Owner-visible cutover result; a weak former bearer is never persisted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeShareImportResult {
    pub public_api_id: Uuid,
    pub active_token: String,
    pub rotated_weak_token: bool,
    pub rotated_legacy_source_key: Option<String>,
}

#[derive(Debug, Error)]
pub enum NativeShareStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("invalid share import")]
    InvalidImport,
    #[error("share import target is missing or belongs to another user")]
    InvalidTarget,
    #[error("share import conflicts with an existing identity or bearer")]
    ImportConflict,
    #[error("operating system randomness unavailable")]
    RandomnessUnavailable,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeShareStore;

impl NativeShareStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Imports one share and its identity binding atomically. Exact replay
    /// adopts the prior result; any changed identity or target fails closed.
    pub async fn import_share(
        &self,
        pool: &PgPool,
        request: &NativeShareImport<'_>,
    ) -> Result<NativeShareImportResult, NativeShareStoreError> {
        validate_import(request)?;
        let mut tx = pool.begin().await?;
        if let Some(result) = replay_result(&mut tx, request).await? {
            tx.commit().await?;
            return Ok(result);
        }
        validate_target(&mut tx, request).await?;
        let preserve = request.legacy_token_is_secure && secure_token(request.legacy_token);
        for _ in 0..MAX_TOKEN_ATTEMPTS {
            let active_token = if preserve {
                request.legacy_token.to_owned()
            } else {
                random_token()?
            };
            let inserted = sqlx::query(
                "INSERT INTO native_shares (
                    id,public_api_id,share_unique_id,owner_user_id,share_type,
                    resource_public_api_id,description,is_downloadable,expires_at,created_at,updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
                 ON CONFLICT DO NOTHING",
            )
            .bind(request.internal_id)
            .bind(request.public_api_id)
            .bind(&active_token)
            .bind(request.owner_user_id)
            .bind(request.share_type)
            .bind(request.resource_public_api_id)
            .bind(request.description)
            .bind(request.is_downloadable)
            .bind(request.expires_at)
            .bind(request.created_at)
            .bind(request.updated_at)
            .execute(&mut *tx)
            .await?;
            if inserted.rows_affected() == 0 {
                if preserve {
                    tx.rollback().await?;
                    return Err(NativeShareStoreError::ImportConflict);
                }
                continue;
            }
            sqlx::query(
                "INSERT INTO native_migration_identity_bindings (
                    source_kind,source_key,internal_id,public_api_id,imported_at
                 ) VALUES ('share',$1,$2,$3,$4)",
            )
            .bind(request.source_key)
            .bind(request.internal_id)
            .bind(request.public_api_id)
            .bind(request.updated_at)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(NativeShareImportResult {
                public_api_id: request.public_api_id,
                active_token,
                rotated_weak_token: !preserve,
                rotated_legacy_source_key: (!preserve).then(|| request.source_key.to_owned()),
            });
        }
        tx.rollback().await?;
        Err(NativeShareStoreError::ImportConflict)
    }
}

fn validate_import(request: &NativeShareImport<'_>) -> Result<(), NativeShareStoreError> {
    if request.source_key.trim().is_empty()
        || request.source_key.len() > 500
        || !(1..=4).contains(&request.share_type)
        || request.created_at > request.updated_at
        || request
            .description
            .is_some_and(|value| value.chars().count() > 10_000)
    {
        return Err(NativeShareStoreError::InvalidImport);
    }
    Ok(())
}

async fn replay_result(
    tx: &mut Transaction<'_, Postgres>,
    request: &NativeShareImport<'_>,
) -> Result<Option<NativeShareImportResult>, NativeShareStoreError> {
    let binding = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT internal_id,public_api_id FROM native_migration_identity_bindings
         WHERE source_kind='share' AND source_key=$1 FOR UPDATE",
    )
    .bind(request.source_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((internal_id, public_api_id)) = binding else {
        return Ok(None);
    };
    if internal_id != request.internal_id || public_api_id != request.public_api_id {
        return Err(NativeShareStoreError::ImportConflict);
    }
    let stored = sqlx::query_as::<_, (String, Uuid, i16, Uuid, bool)>(
        "SELECT share_unique_id,owner_user_id,share_type,resource_public_api_id,
                description IS NOT DISTINCT FROM $3
                AND is_downloadable=$4
                AND expires_at IS NOT DISTINCT FROM $5
                AND created_at=$6 AND updated_at=$7 AS mutable_facts_match
         FROM native_shares WHERE id=$1 AND public_api_id=$2",
    )
    .bind(internal_id)
    .bind(public_api_id)
    .bind(request.description)
    .bind(request.is_downloadable)
    .bind(request.expires_at)
    .bind(request.created_at)
    .bind(request.updated_at)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(NativeShareStoreError::ImportConflict)?;
    if stored.1 != request.owner_user_id
        || stored.2 != request.share_type
        || stored.3 != request.resource_public_api_id
        || !stored.4
        || ((stored.0 == request.legacy_token)
            != (request.legacy_token_is_secure && secure_token(request.legacy_token)))
    {
        return Err(NativeShareStoreError::ImportConflict);
    }
    Ok(Some(NativeShareImportResult {
        public_api_id,
        rotated_weak_token: stored.0 != request.legacy_token,
        rotated_legacy_source_key: (stored.0 != request.legacy_token)
            .then(|| request.source_key.to_owned()),
        active_token: stored.0,
    }))
}

async fn validate_target(
    tx: &mut Transaction<'_, Postgres>,
    request: &NativeShareImport<'_>,
) -> Result<(), NativeShareStoreError> {
    let exists = match request.share_type {
        1 => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM tracks
                 WHERE public_api_id=$1 AND native_user_can_access_release($2,release_id))",
            )
            .bind(request.resource_public_api_id)
            .bind(request.owner_user_id)
            .fetch_one(&mut **tx)
            .await?
        }
        2 => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM releases
                 WHERE public_api_id=$1 AND native_user_can_access_release($2,id))",
            )
            .bind(request.resource_public_api_id)
            .bind(request.owner_user_id)
            .fetch_one(&mut **tx)
            .await?
        }
        3 => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM native_playlists
             WHERE public_api_id=$1 AND owner_user_id=$2)",
            )
            .bind(request.resource_public_api_id)
            .bind(request.owner_user_id)
            .fetch_one(&mut **tx)
            .await?
        }
        4 => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM artists
                 WHERE public_api_id=$1 AND native_user_can_access_artist($2,id))",
            )
            .bind(request.resource_public_api_id)
            .bind(request.owner_user_id)
            .fetch_one(&mut **tx)
            .await?
        }
        _ => false,
    };
    if exists {
        Ok(())
    } else {
        Err(NativeShareStoreError::InvalidTarget)
    }
}

fn secure_token(token: &str) -> bool {
    token.len() == SHARE_TOKEN_BYTES * 2 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn random_token() -> Result<String, NativeShareStoreError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; SHARE_TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| NativeShareStoreError::RandomnessUnavailable)?;
    let mut token = String::with_capacity(SHARE_TOKEN_BYTES * 2);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(token)
}

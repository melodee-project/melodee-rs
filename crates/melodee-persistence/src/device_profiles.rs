//! device/transcoding profile persistence (ADR-0134 §5,
//! PRD-DEVICE-001..003) over `device_profiles` (migration 0026).
//!
//! Rows are operator-managed **metadata only**: nothing in this crate feeds
//! stream negotiation, and callers must not claim profiles control
//! transcoding (PRD-DEVICE-002). Profile-driven stream selection is
//! post-parity work requiring its own ADR (PRD-DEVICE-003).
//!
//! Preserved legacy preview behavior (evidence:
//! `design/evidence/phase-9/device-profiles-legacy-inventory.md`):
//! bounded names, the direct-play/codec/bitrate consistency rules, and the
//! single-default invariant. Unlike the legacy service's load-modify-save,
//! default flips here are transactional and the at-most-one-default
//! invariant is additionally fail-closed via a partial unique index.

use serde_json::Value;
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Bounded name length, mirroring the legacy `MaxGeneralInputLength` (255)
/// and the migration CHECK.
pub const MAX_PROFILE_NAME_CHARS: usize = 255;
/// Serialized-size bound for each jsonb document, mirroring the migration
/// CHECKs. Metadata documents are small; this stops abuse payloads.
pub const MAX_PROFILE_JSON_BYTES: usize = 4096;
/// Bound for one `client_match` matcher value.
pub const MAX_CLIENT_MATCH_VALUE_CHARS: usize = 255;
/// Upper bound for list responses; the API layer clamps to this.
pub const MAX_DEVICE_PROFILE_LIST: u32 = 500;

type ProfileRow = (
    Uuid,
    String,
    Value,
    Value,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

/// One stored device profile row (metadata only; see module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceProfile {
    pub id: Uuid,
    pub name: String,
    /// Client matcher metadata. JSON object; string leaf values only.
    pub client_match: Value,
    /// Transcoding-intent metadata. JSON object; known legacy keys
    /// (`directPlay`, `targetCodec`, `maxBitrate`, `resampleRate`,
    /// `priority`) are validated when present.
    pub profile: Value,
    pub is_default: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl From<ProfileRow> for DeviceProfile {
    fn from(row: ProfileRow) -> Self {
        let (id, name, client_match, profile, is_default, created_at, updated_at) = row;
        Self {
            id,
            name,
            client_match,
            profile,
            is_default,
            created_at,
            updated_at,
        }
    }
}

/// Creation request. `is_default` atomically clears any existing default.
pub struct NewDeviceProfile {
    pub name: String,
    pub client_match: Value,
    pub profile: Value,
    pub is_default: bool,
}

/// Field-optional update. Omitted fields keep their stored value. Default
/// management goes through [`DeviceProfileStore::set_default`] so the
/// single-default flip is always an explicit, audited operation.
#[derive(Clone, Debug, Default)]
pub struct DeviceProfileUpdate {
    pub name: Option<String>,
    pub client_match: Option<Value>,
    pub profile: Option<Value>,
}

#[derive(Debug, Error)]
pub enum DeviceProfileStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("device profile not found")]
    NotFound,
    #[error("invalid device profile: {0}")]
    InvalidProfile(&'static str),
}

fn validate_name(name: &str) -> Result<(), DeviceProfileStoreError> {
    if name.trim().is_empty() {
        return Err(DeviceProfileStoreError::InvalidProfile(
            "name must be non-blank",
        ));
    }
    if name.chars().count() > MAX_PROFILE_NAME_CHARS {
        return Err(DeviceProfileStoreError::InvalidProfile(
            "name exceeds 255 characters",
        ));
    }
    Ok(())
}

/// `client_match` is a JSON object of matcher metadata. Leaf values must be
/// non-blank bounded strings; nesting is rejected so a future resolver can
/// interpret the document without a recursive grammar.
fn validate_client_match(value: &Value) -> Result<(), DeviceProfileStoreError> {
    let object = value
        .as_object()
        .ok_or(DeviceProfileStoreError::InvalidProfile(
            "client_match must be a JSON object",
        ))?;
    for entry in object.values() {
        let Some(text) = entry.as_str() else {
            return Err(DeviceProfileStoreError::InvalidProfile(
                "client_match values must be strings",
            ));
        };
        if text.trim().is_empty() || text.chars().count() > MAX_CLIENT_MATCH_VALUE_CHARS {
            return Err(DeviceProfileStoreError::InvalidProfile(
                "client_match values must be non-blank and at most 255 characters",
            ));
        }
    }
    if value.to_string().len() > MAX_PROFILE_JSON_BYTES {
        return Err(DeviceProfileStoreError::InvalidProfile(
            "client_match exceeds 4096 serialized bytes",
        ));
    }
    Ok(())
}

fn optional_bounded_int(
    object: &serde_json::Map<String, Value>,
    key: &str,
    min: i64,
    max: i64,
) -> Result<Option<i64>, DeviceProfileStoreError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let number = value
                .as_i64()
                .ok_or(DeviceProfileStoreError::InvalidProfile(
                    "profile integer fields must be integers",
                ))?;
            if !(min..=max).contains(&number) {
                return Err(DeviceProfileStoreError::InvalidProfile(
                    "profile integer field out of range",
                ));
            }
            Ok(Some(number))
        }
    }
}

/// `profile` is a JSON object of transcoding-intent metadata. Known legacy
/// keys are type/range-checked when present, and the two legacy consistency
/// rules are preserved: a direct-play profile must not set codec/bitrate,
/// and a non-direct-play profile must name a target codec. Unknown keys are
/// tolerated as forward-compatible metadata.
fn validate_profile(value: &Value) -> Result<(), DeviceProfileStoreError> {
    let object = value
        .as_object()
        .ok_or(DeviceProfileStoreError::InvalidProfile(
            "profile must be a JSON object",
        ))?;
    let direct_play = match object.get("directPlay") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(flag)) => Some(*flag),
        Some(_) => {
            return Err(DeviceProfileStoreError::InvalidProfile(
                "profile.directPlay must be a boolean",
            ));
        }
    };
    let target_codec = match object.get("targetCodec") {
        None | Some(Value::Null) => None,
        Some(Value::String(codec))
            if !codec.trim().is_empty() && codec.chars().count() <= MAX_PROFILE_NAME_CHARS =>
        {
            Some(codec)
        }
        Some(_) => {
            return Err(DeviceProfileStoreError::InvalidProfile(
                "profile.targetCodec must be a non-blank string of at most 255 characters",
            ));
        }
    };
    let max_bitrate = optional_bounded_int(object, "maxBitrate", 1, 9999)?;
    optional_bounded_int(object, "resampleRate", 1000, 192_000)?;
    optional_bounded_int(object, "priority", 0, 100)?;

    match direct_play {
        Some(true) if target_codec.is_some() || max_bitrate.is_some() => {
            return Err(DeviceProfileStoreError::InvalidProfile(
                "direct-play profiles must not set targetCodec or maxBitrate",
            ));
        }
        Some(false) if target_codec.is_none() => {
            return Err(DeviceProfileStoreError::InvalidProfile(
                "non-direct-play profiles must set targetCodec",
            ));
        }
        _ => {}
    }
    if value.to_string().len() > MAX_PROFILE_JSON_BYTES {
        return Err(DeviceProfileStoreError::InvalidProfile(
            "profile exceeds 4096 serialized bytes",
        ));
    }
    Ok(())
}

/// Stateless entry point for device profile administration.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceProfileStore;

impl DeviceProfileStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Bounded, deterministic listing: default first, then name.
    pub async fn list(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DeviceProfile>, DeviceProfileStoreError> {
        let rows = sqlx::query_as::<_, ProfileRow>("SELECT id,name,client_match,profile,is_default,created_at,updated_at FROM device_profiles
             ORDER BY is_default DESC, name, id LIMIT $1 OFFSET $2")
        .bind(i64::from(limit.min(MAX_DEVICE_PROFILE_LIST)))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(DeviceProfile::from).collect())
    }

    pub async fn get(
        &self,
        pool: &PgPool,
        id: Uuid,
    ) -> Result<DeviceProfile, DeviceProfileStoreError> {
        let row = sqlx::query_as::<_, ProfileRow>("SELECT id,name,client_match,profile,is_default,created_at,updated_at FROM device_profiles WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
        row.map(DeviceProfile::from)
            .ok_or(DeviceProfileStoreError::NotFound)
    }

    /// Creates a profile. When `is_default` is set, any existing default is
    /// cleared in the same transaction; the partial unique index is the
    /// fail-closed backstop against a concurrent default flip.
    pub async fn create(
        &self,
        pool: &PgPool,
        new: &NewDeviceProfile,
        now: OffsetDateTime,
    ) -> Result<DeviceProfile, DeviceProfileStoreError> {
        validate_name(&new.name)?;
        validate_client_match(&new.client_match)?;
        validate_profile(&new.profile)?;
        let id = Uuid::now_v7();
        let mut tx = pool.begin().await?;
        if new.is_default {
            sqlx::query(
                "UPDATE device_profiles SET is_default=false, updated_at=$1 WHERE is_default",
            )
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        let row = sqlx::query_as::<_, ProfileRow>("INSERT INTO device_profiles
               (id,name,client_match,profile,is_default,created_at,updated_at)
             VALUES ($1,$2,$3,$4,$5,$6,$6) RETURNING id,name,client_match,profile,is_default,created_at,updated_at")
        .bind(id)
        .bind(&new.name)
        .bind(&new.client_match)
        .bind(&new.profile)
        .bind(new.is_default)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(DeviceProfile::from(row))
    }

    /// Applies a field-optional update. Not-found rows report
    /// [`DeviceProfileStoreError::NotFound`].
    pub async fn update(
        &self,
        pool: &PgPool,
        id: Uuid,
        update: &DeviceProfileUpdate,
        now: OffsetDateTime,
    ) -> Result<DeviceProfile, DeviceProfileStoreError> {
        if let Some(name) = &update.name {
            validate_name(name)?;
        }
        if let Some(client_match) = &update.client_match {
            validate_client_match(client_match)?;
        }
        if let Some(profile) = &update.profile {
            validate_profile(profile)?;
        }
        let row = sqlx::query_as::<_, ProfileRow>(
            "UPDATE device_profiles SET
               name = COALESCE($2, name),
               client_match = COALESCE($3, client_match),
               profile = COALESCE($4, profile),
               updated_at = $5
             WHERE id=$1 RETURNING id,name,client_match,profile,is_default,created_at,updated_at",
        )
        .bind(id)
        .bind(&update.name)
        .bind(&update.client_match)
        .bind(&update.profile)
        .bind(now)
        .fetch_optional(pool)
        .await?;
        row.map(DeviceProfile::from)
            .ok_or(DeviceProfileStoreError::NotFound)
    }

    pub async fn delete(&self, pool: &PgPool, id: Uuid) -> Result<(), DeviceProfileStoreError> {
        let result = sqlx::query("DELETE FROM device_profiles WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DeviceProfileStoreError::NotFound);
        }
        Ok(())
    }

    /// Atomically makes `id` the single default: every other row is cleared
    /// in the same transaction. Clearing the previous default is the
    /// preserved legacy behavior; atomicity replaces the legacy
    /// load-modify-save race.
    pub async fn set_default(
        &self,
        pool: &PgPool,
        id: Uuid,
        now: OffsetDateTime,
    ) -> Result<DeviceProfile, DeviceProfileStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE device_profiles SET is_default=false, updated_at=$1 WHERE is_default AND id<>$2")
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query_as::<_, ProfileRow>(
            "UPDATE device_profiles SET is_default=true, updated_at=$2
             WHERE id=$1 RETURNING id,name,client_match,profile,is_default,created_at,updated_at",
        )
        .bind(id)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Err(DeviceProfileStoreError::NotFound);
        };
        tx.commit().await?;
        Ok(DeviceProfile::from(row))
    }
}

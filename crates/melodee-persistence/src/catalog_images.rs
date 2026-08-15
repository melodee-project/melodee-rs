//! Phase 10 catalog artwork override store (accepted ADR-0143; parity
//! matrix artwork row, checklist 07).
//!
//! `catalog_entity_images` (migration 0030) holds operator-uploaded custom
//! Artist/Release artwork: curation data, deliberately separate from the
//! canonical publication evidence (`media_assets`, publication plans). Every
//! mutation writes its `catalog_audit_events` row in the same transaction so
//! the override and its evidence commit atomically. Deletes remove only the
//! override row — canonical media files and publication plans are never
//! touched, and image serving falls back to the plan/placeholder branch.

use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Storage byte bound per override row (mirrors the upload pipeline's
/// `MAX_IMAGE_INPUT_BYTES` and the migration-0030 CHECK constraint).
pub const MAX_ENTITY_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// The override-owning catalog entity kinds (ADR-0143 §1 CHECK domain).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogImageEntity {
    Artist,
    Release,
}

impl CatalogImageEntity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Release => "release",
        }
    }
}

/// One stored artwork override row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCatalogEntityImage {
    pub entity_type: CatalogImageEntity,
    /// Internal entity id (`artists.id` / `releases.id`).
    pub entity_id: Uuid,
    pub image_mime_type: String,
    pub image_bytes: Vec<u8>,
    pub uploaded_by: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Typed override-store failures with the SQL root cause retained.
#[derive(Debug, Error)]
pub enum CatalogImageError {
    #[error("catalog image database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("catalog image request violates the storage bounds: {0}")]
    InvalidRequest(&'static str),
}

/// Audit attribution for one artwork mutation (acting administrator,
/// operator reason, and the mutation timestamp written to both the row and
/// its `catalog_audit_events` evidence).
#[derive(Debug, Clone, Copy)]
pub struct CatalogImageAudit<'a> {
    pub actor: &'a str,
    pub reason: &'a str,
    pub now: OffsetDateTime,
}

/// Unit store, matching the `CatalogStore` associated-function pattern.
pub struct CatalogImageStore;

type ImageRow = (String, Vec<u8>, String, OffsetDateTime, OffsetDateTime);

impl CatalogImageStore {
    fn row(entity: CatalogImageEntity, entity_id: Uuid, row: ImageRow) -> StoredCatalogEntityImage {
        StoredCatalogEntityImage {
            entity_type: entity,
            entity_id,
            image_mime_type: row.0,
            image_bytes: row.1,
            uploaded_by: row.2,
            created_at: row.3,
            updated_at: row.4,
        }
    }

    /// Reads the override for one entity (internal id), when present.
    ///
    /// # Errors
    /// Returns the database error describing the failed read.
    pub async fn image(
        pool: &PgPool,
        entity: CatalogImageEntity,
        entity_id: Uuid,
    ) -> Result<Option<StoredCatalogEntityImage>, CatalogImageError> {
        let row = match entity {
            CatalogImageEntity::Artist => {
                sqlx::query_as::<_, ImageRow>(
                    "SELECT image_mime_type, image_bytes, uploaded_by, created_at, updated_at
                     FROM catalog_entity_images
                     WHERE entity_type = 'artist' AND artist_id = $1",
                )
                .bind(entity_id)
                .fetch_optional(pool)
                .await?
            }
            CatalogImageEntity::Release => {
                sqlx::query_as::<_, ImageRow>(
                    "SELECT image_mime_type, image_bytes, uploaded_by, created_at, updated_at
                     FROM catalog_entity_images
                     WHERE entity_type = 'release' AND release_id = $1",
                )
                .bind(entity_id)
                .fetch_optional(pool)
                .await?
            }
        };
        Ok(row.map(|row| Self::row(entity, entity_id, row)))
    }

    /// Installs or replaces the override for one entity and audit-logs the
    /// mutation (`image_override_set`) atomically. The caller has already
    /// magic-byte/dimension validated the bytes; the mime and 1–10 MiB bounds
    /// are re-enforced here so the table CHECK constraints stay the backstop.
    ///
    /// # Errors
    /// `InvalidRequest` when the mime type or byte bounds are violated;
    /// `Database` for SQL failures (including an unknown entity id FK).
    pub async fn set_image(
        pool: &PgPool,
        entity: CatalogImageEntity,
        entity_id: Uuid,
        image_mime_type: &str,
        image_bytes: &[u8],
        audit: CatalogImageAudit<'_>,
    ) -> Result<StoredCatalogEntityImage, CatalogImageError> {
        if !matches!(image_mime_type, "image/jpeg" | "image/png") {
            return Err(CatalogImageError::InvalidRequest("image mime type"));
        }
        if image_bytes.is_empty() || image_bytes.len() > MAX_ENTITY_IMAGE_BYTES {
            return Err(CatalogImageError::InvalidRequest("image byte bound"));
        }
        let mut tx = pool.begin().await?;
        match entity {
            CatalogImageEntity::Artist => sqlx::query(
                "INSERT INTO catalog_entity_images (
                        entity_type, artist_id, image_mime_type, image_bytes,
                        uploaded_by, created_at, updated_at
                     ) VALUES ('artist', $1, $2, $3, $4, $5, $5)
                     ON CONFLICT (artist_id) WHERE artist_id IS NOT NULL DO UPDATE
                     SET image_mime_type = $2, image_bytes = $3, uploaded_by = $4,
                         updated_at = $5",
            ),
            CatalogImageEntity::Release => sqlx::query(
                "INSERT INTO catalog_entity_images (
                        entity_type, release_id, image_mime_type, image_bytes,
                        uploaded_by, created_at, updated_at
                     ) VALUES ('release', $1, $2, $3, $4, $5, $5)
                     ON CONFLICT (release_id) WHERE release_id IS NOT NULL DO UPDATE
                     SET image_mime_type = $2, image_bytes = $3, uploaded_by = $4,
                         updated_at = $5",
            ),
        }
        .bind(entity_id)
        .bind(image_mime_type)
        .bind(image_bytes)
        .bind(audit.actor)
        .bind(audit.now)
        .execute(&mut *tx)
        .await?;
        audit_image_event(&mut tx, entity, entity_id, "image_override_set", audit).await?;
        tx.commit().await?;
        Self::image(pool, entity, entity_id)
            .await?
            .ok_or(CatalogImageError::InvalidRequest(
                "image row missing after upsert",
            ))
    }

    /// Removes the override for one entity, returning whether a row existed.
    /// Removal is audit-logged (`image_override_removed`) only when a row was
    /// actually deleted — deleting an absent override is a no-op so the
    /// endpoint stays idempotent (ADR-0143 §4). Canonical media files and
    /// publication plans are never touched.
    ///
    /// # Errors
    /// Returns the database error describing the failed delete.
    pub async fn delete_image(
        pool: &PgPool,
        entity: CatalogImageEntity,
        entity_id: Uuid,
        audit: CatalogImageAudit<'_>,
    ) -> Result<bool, CatalogImageError> {
        let mut tx = pool.begin().await?;
        let deleted = match entity {
            CatalogImageEntity::Artist => sqlx::query(
                "DELETE FROM catalog_entity_images
                 WHERE entity_type = 'artist' AND artist_id = $1",
            ),
            CatalogImageEntity::Release => sqlx::query(
                "DELETE FROM catalog_entity_images
                 WHERE entity_type = 'release' AND release_id = $1",
            ),
        }
        .bind(entity_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if deleted == 1 {
            audit_image_event(&mut tx, entity, entity_id, "image_override_removed", audit).await?;
        }
        tx.commit().await?;
        Ok(deleted == 1)
    }
}

async fn audit_image_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    entity: CatalogImageEntity,
    entity_id: Uuid,
    event_type: &str,
    audit: CatalogImageAudit<'_>,
) -> Result<(), CatalogImageError> {
    sqlx::query(
        "INSERT INTO catalog_audit_events (
            event_id, entity_type, entity_id, event_type, actor,
            reason, payload, created_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(entity.as_str())
    .bind(entity_id)
    .bind(event_type)
    .bind(audit.actor)
    .bind(audit.reason)
    .bind(serde_json::json!({ "surface": "admin_catalog_artwork" }))
    .bind(audit.now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

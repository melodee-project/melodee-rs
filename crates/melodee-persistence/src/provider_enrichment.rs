//! Durable provider-enrichment scheduling and lease transitions (ADR-0162).
//!
//! The store deliberately contains no provider HTTP code. It records work and
//! its operator-visible outcome so the server worker can be restarted without
//! losing provenance or turning a provider outage into an import failure.

use std::fmt;

use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::KNOWN_PROVIDERS;

pub const MAX_ENRICHMENT_ATTEMPTS: i32 = 20;
pub const DEFAULT_ENRICHMENT_LEASE_SECONDS: i64 = 120;
pub const MAX_ENRICHMENT_BATCH: i64 = 32;
pub const MAX_PROVIDER_IMAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrichmentEntityType {
    Artist,
    Release,
}

impl EnrichmentEntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrichmentPurpose {
    ArtistMetadata,
    ReleaseMetadata,
    ArtistArtwork,
    ReleaseArtwork,
}

impl EnrichmentPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArtistMetadata => "artist_metadata",
            Self::ReleaseMetadata => "release_metadata",
            Self::ArtistArtwork => "artist_artwork",
            Self::ReleaseArtwork => "release_artwork",
        }
    }

    pub const fn entity_type(self) -> EnrichmentEntityType {
        match self {
            Self::ArtistMetadata | Self::ArtistArtwork => EnrichmentEntityType::Artist,
            Self::ReleaseMetadata | Self::ReleaseArtwork => EnrichmentEntityType::Release,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedEnrichmentTask {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub provider: String,
    pub purpose: String,
    pub request_key: String,
    pub attempt_count: i32,
    pub lease_expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrichmentOutcome {
    CacheHit,
    Success,
    Disabled,
    Retry,
    Failed,
}

impl EnrichmentOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CacheHit => "cache_hit",
            Self::Success => "success",
            Self::Disabled => "disabled",
            Self::Retry => "retry",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Error)]
pub enum ProviderEnrichmentStoreError {
    #[error("provider enrichment database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("provider enrichment request is invalid")]
    InvalidRequest,
    #[error("provider enrichment lease is no longer owned")]
    LeaseLost,
}

impl fmt::Display for EnrichmentEntityType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str((*self).as_str())
    }
}

/// Persistence boundary for provider task lifecycle operations.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderEnrichmentStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProviderImage {
    pub entity_type: EnrichmentEntityType,
    pub entity_id: Uuid,
    pub provider: String,
    pub source_url: String,
    pub image_mime_type: String,
    pub image_bytes: Vec<u8>,
    pub confidence: i32,
    pub observed_at: OffsetDateTime,
}

/// One provider search result retained as candidate evidence. It is not an
/// `artist_external_identities` row and therefore cannot authorize a merge.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderIdentityCandidateObservation {
    pub external_id: String,
    pub candidate_rank: i32,
    pub confidence: i32,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderIdentityCandidateStore;

impl ProviderIdentityCandidateStore {
    pub async fn upsert_artist_candidates(
        &self,
        pool: &PgPool,
        artist_id: Uuid,
        provider: &str,
        query: &str,
        candidates: &[ProviderIdentityCandidateObservation],
        observed_at: OffsetDateTime,
    ) -> Result<(), ProviderEnrichmentStoreError> {
        if !KNOWN_PROVIDERS.contains(&provider) || query.trim().is_empty() || candidates.len() > 32
        {
            return Err(ProviderEnrichmentStoreError::InvalidRequest);
        }
        let mut tx = pool.begin().await?;
        for candidate in candidates {
            if candidate.external_id.trim().is_empty()
                || candidate.candidate_rank <= 0
                || !(0..=10_000).contains(&candidate.confidence)
                || candidate.payload.to_string().len() > 65_536
            {
                return Err(ProviderEnrichmentStoreError::InvalidRequest);
            }
            sqlx::query(
                "INSERT INTO provider_identity_candidates (
                    id, artist_id, provider, external_id, query,
                    candidate_rank, confidence, payload, observed_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
                 ON CONFLICT (artist_id, provider, external_id, query) DO UPDATE
                 SET candidate_rank=EXCLUDED.candidate_rank,
                     confidence=EXCLUDED.confidence,
                     payload=EXCLUDED.payload,
                     observed_at=EXCLUDED.observed_at",
            )
            .bind(Uuid::now_v7())
            .bind(artist_id)
            .bind(provider)
            .bind(&candidate.external_id)
            .bind(query)
            .bind(candidate.candidate_rank)
            .bind(candidate.confidence)
            .bind(&candidate.payload)
            .bind(observed_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderArtworkStore;

impl ProviderArtworkStore {
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        &self,
        pool: &PgPool,
        entity_type: EnrichmentEntityType,
        entity_id: Uuid,
        provider: &str,
        source_url: &str,
        image_mime_type: &str,
        image_bytes: &[u8],
        confidence: i32,
        observed_at: OffsetDateTime,
    ) -> Result<(), ProviderEnrichmentStoreError> {
        if !matches!(image_mime_type, "image/jpeg" | "image/png")
            || image_bytes.is_empty()
            || image_bytes.len() > MAX_PROVIDER_IMAGE_BYTES
            || source_url.trim().is_empty()
        {
            return Err(ProviderEnrichmentStoreError::InvalidRequest);
        }
        sqlx::query(
            "INSERT INTO catalog_provider_images (
                id, entity_type, entity_id, provider, source_url,
                image_mime_type, image_bytes, confidence, observed_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
             ON CONFLICT (entity_type, entity_id, provider) DO UPDATE
             SET source_url=EXCLUDED.source_url,
                 image_mime_type=EXCLUDED.image_mime_type,
                 image_bytes=EXCLUDED.image_bytes,
                 confidence=EXCLUDED.confidence,
                 observed_at=EXCLUDED.observed_at",
        )
        .bind(Uuid::now_v7())
        .bind(entity_type.as_str())
        .bind(entity_id)
        .bind(provider)
        .bind(source_url)
        .bind(image_mime_type)
        .bind(image_bytes)
        .bind(confidence.clamp(0, 10_000))
        .bind(observed_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn image(
        &self,
        pool: &PgPool,
        entity_type: EnrichmentEntityType,
        entity_id: Uuid,
    ) -> Result<Option<StoredProviderImage>, ProviderEnrichmentStoreError> {
        let row = sqlx::query_as::<_, (String, String, String, Vec<u8>, i32, OffsetDateTime)>(
            "SELECT provider, source_url, image_mime_type, image_bytes, confidence, observed_at
             FROM catalog_provider_images
             WHERE entity_type=$1 AND entity_id=$2
             ORDER BY confidence DESC, observed_at DESC, id DESC
             LIMIT 1",
        )
        .bind(entity_type.as_str())
        .bind(entity_id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(
            |(provider, source_url, image_mime_type, image_bytes, confidence, observed_at)| {
                StoredProviderImage {
                    entity_type,
                    entity_id,
                    provider,
                    source_url,
                    image_mime_type,
                    image_bytes,
                    confidence,
                    observed_at,
                }
            },
        ))
    }
}

impl ProviderEnrichmentStore {
    #[allow(clippy::too_many_arguments)]
    pub async fn schedule(
        &self,
        pool: &PgPool,
        entity_type: EnrichmentEntityType,
        entity_id: Uuid,
        provider: &str,
        purpose: EnrichmentPurpose,
        request_key: &str,
        now: OffsetDateTime,
    ) -> Result<(), ProviderEnrichmentStoreError> {
        validate_schedule(entity_type, provider, purpose, request_key)?;
        let mut tx = pool.begin().await?;
        schedule_in_transaction(
            &mut tx,
            entity_type,
            entity_id,
            provider,
            purpose,
            request_key,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn claim(
        &self,
        pool: &PgPool,
        worker: &str,
        limit: i64,
        lease_seconds: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<ClaimedEnrichmentTask>, ProviderEnrichmentStoreError> {
        if worker.trim().is_empty() || !(1..=MAX_ENRICHMENT_BATCH).contains(&limit) {
            return Err(ProviderEnrichmentStoreError::InvalidRequest);
        }
        let lease_seconds = lease_seconds.clamp(5, 900);
        let mut tx = pool.begin().await?;
        let lease_expires_at = now + Duration::seconds(lease_seconds);
        let rows = sqlx::query_as::<_, (Uuid, String, Uuid, String, String, String, i32)>(
            "SELECT id, entity_type, entity_id, provider, purpose, request_key,
                    attempt_count
             FROM provider_enrichment_tasks
             WHERE (status IN ('queued','retry') AND next_attempt_at <= $1)
                OR (status = 'running' AND lease_expires_at < $1)
             ORDER BY next_attempt_at, updated_at, id
             FOR UPDATE SKIP LOCKED
             LIMIT $2",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        let mut claimed = Vec::with_capacity(rows.len());
        for (id, entity_type, entity_id, provider, purpose, request_key, attempt_count) in rows {
            let next_attempt = attempt_count
                .checked_add(1)
                .ok_or(ProviderEnrichmentStoreError::InvalidRequest)?;
            if next_attempt > MAX_ENRICHMENT_ATTEMPTS {
                sqlx::query(
                    "UPDATE provider_enrichment_tasks
                     SET status='failed', last_error_category='attempt_limit',
                         last_error_message='maximum provider attempts exhausted',
                         lease_owner=NULL, lease_acquired_at=NULL,
                         lease_expires_at=NULL, updated_at=$2
                     WHERE id=$1",
                )
                .bind(id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                continue;
            }
            sqlx::query(
                "UPDATE provider_enrichment_tasks
                 SET status='running', attempt_count=$2, lease_owner=$3,
                     lease_acquired_at=$4, lease_expires_at=$5, updated_at=$4
                 WHERE id=$1",
            )
            .bind(id)
            .bind(next_attempt)
            .bind(worker)
            .bind(now)
            .bind(lease_expires_at)
            .execute(&mut *tx)
            .await?;
            claimed.push(ClaimedEnrichmentTask {
                id,
                entity_type,
                entity_id,
                provider,
                purpose,
                request_key,
                attempt_count: next_attempt,
                lease_expires_at,
            });
        }
        tx.commit().await?;
        Ok(claimed)
    }

    pub async fn complete(
        &self,
        pool: &PgPool,
        task: &ClaimedEnrichmentTask,
        worker: &str,
        outcome: EnrichmentOutcome,
        cache_key: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), ProviderEnrichmentStoreError> {
        let mut tx = pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE provider_enrichment_tasks
             SET status='succeeded', lease_owner=NULL, lease_acquired_at=NULL,
                 lease_expires_at=NULL, completed_at=$2, updated_at=$2,
                 last_error_category=NULL, last_error_message=NULL
             WHERE id=$1 AND status='running' AND lease_owner=$3
               AND attempt_count=$4",
        )
        .bind(task.id)
        .bind(now)
        .bind(worker)
        .bind(task.attempt_count)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(ProviderEnrichmentStoreError::LeaseLost);
        }
        insert_attempt(&mut tx, task, outcome, None, None, cache_key, now).await?;
        tx.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fail_or_retry(
        &self,
        pool: &PgPool,
        task: &ClaimedEnrichmentTask,
        worker: &str,
        category: &str,
        message: &str,
        retry_at: Option<OffsetDateTime>,
        cache_key: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), ProviderEnrichmentStoreError> {
        if category.trim().is_empty() || message.trim().is_empty() {
            return Err(ProviderEnrichmentStoreError::InvalidRequest);
        }
        let mut tx = pool.begin().await?;
        let retry = retry_at.is_some() && task.attempt_count < MAX_ENRICHMENT_ATTEMPTS;
        let status = if retry { "retry" } else { "failed" };
        let updated = sqlx::query(
            "UPDATE provider_enrichment_tasks
             SET status=$2, next_attempt_at=COALESCE($3,$4),
                 lease_owner=NULL, lease_acquired_at=NULL, lease_expires_at=NULL,
                 last_error_category=$5, last_error_message=$6,
                 completed_at=CASE WHEN $2='failed' THEN $4 ELSE NULL END,
                 updated_at=$4
             WHERE id=$1 AND status='running' AND lease_owner=$7
               AND attempt_count=$8",
        )
        .bind(task.id)
        .bind(status)
        .bind(retry_at)
        .bind(now)
        .bind(category)
        .bind(message.chars().take(1_000).collect::<String>())
        .bind(worker)
        .bind(task.attempt_count)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(ProviderEnrichmentStoreError::LeaseLost);
        }
        insert_attempt(
            &mut tx,
            task,
            if retry {
                EnrichmentOutcome::Retry
            } else {
                EnrichmentOutcome::Failed
            },
            Some(category),
            Some(message),
            cache_key,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

pub async fn schedule_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: EnrichmentEntityType,
    entity_id: Uuid,
    provider: &str,
    purpose: EnrichmentPurpose,
    request_key: &str,
    now: OffsetDateTime,
) -> Result<(), ProviderEnrichmentStoreError> {
    validate_schedule(entity_type, provider, purpose, request_key)?;
    sqlx::query(
        "INSERT INTO provider_enrichment_tasks (
            id, entity_type, entity_id, provider, purpose, request_key,
            status, attempt_count, next_attempt_at, created_at, updated_at
         ) VALUES ($1,$2,$3,$4,$5,$6,'queued',0,$7,$7,$7)
         ON CONFLICT (entity_type, entity_id, provider, purpose, request_key)
         DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(entity_type.as_str())
    .bind(entity_id)
    .bind(provider)
    .bind(purpose.as_str())
    .bind(request_key)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_attempt(
    tx: &mut Transaction<'_, Postgres>,
    task: &ClaimedEnrichmentTask,
    outcome: EnrichmentOutcome,
    category: Option<&str>,
    message: Option<&str>,
    cache_key: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), ProviderEnrichmentStoreError> {
    sqlx::query(
        "INSERT INTO provider_enrichment_attempts (
            id, task_id, attempt_number, provider, purpose, outcome,
            error_category, error_message, cache_key, started_at, completed_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10)",
    )
    .bind(Uuid::now_v7())
    .bind(task.id)
    .bind(task.attempt_count)
    .bind(&task.provider)
    .bind(&task.purpose)
    .bind(outcome.as_str())
    .bind(category)
    .bind(message.map(|value| value.chars().take(1_000).collect::<String>()))
    .bind(cache_key)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_schedule(
    entity_type: EnrichmentEntityType,
    provider: &str,
    purpose: EnrichmentPurpose,
    request_key: &str,
) -> Result<(), ProviderEnrichmentStoreError> {
    if purpose.entity_type() != entity_type
        || !KNOWN_PROVIDERS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(provider))
        || request_key.trim().is_empty()
        || request_key.chars().count() > 128
    {
        return Err(ProviderEnrichmentStoreError::InvalidRequest);
    }
    Ok(())
}

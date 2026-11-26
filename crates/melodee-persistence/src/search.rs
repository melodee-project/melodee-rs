//! Durable PostgreSQL boundary for the rebuildable Tantivy projection.
//!
//! PostgreSQL owns outbox leases, generation/rebuild state, and bounded
//! catalog snapshots. Tantivy and MQL types deliberately do not cross this
//! crate boundary (ADR-0105, ADR-0121).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::{AssertSqlSafe, PgPool, Postgres, Row};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::work_items::HourlyCompletionCount;

pub const SEARCH_PROJECTION_NAME: &str = "tantivy.catalog";
pub const MAX_SEARCH_OUTBOX_BATCH: usize = 512;
pub const MAX_SEARCH_SNAPSHOT_PAGE: usize = 1_000;
pub const MAX_SEARCH_OUTBOX_ATTEMPTS: u32 = 100;

#[derive(Debug, Error)]
pub enum SearchStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("invalid search request: {0}")]
    InvalidRequest(String),
    #[error("search projection lease is owned by another worker")]
    LeaseConflict,
    #[error("search projection lease was lost")]
    LeaseLost,
    #[error("search outbox row {0} was not found")]
    OutboxNotFound(i64),
    #[error("search generation {0} was not found")]
    GenerationNotFound(Uuid),
    #[error("search rebuild job {0} was not found")]
    RebuildNotFound(Uuid),
    #[error("search rebuild already exists for root {0}")]
    RebuildConflict(String),
    #[error("search generation must catch up through outbox row {0} before activation")]
    NeedsCatchup(i64),
    #[error("search catalog row has an unsupported entity type: {0}")]
    UnsupportedEntity(String),
    #[error("search catalog row is corrupt: {0}")]
    CorruptCatalog(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchOperation {
    Upsert,
    Delete,
}

impl SearchOperation {
    fn parse(value: &str) -> Result<Self, SearchStoreError> {
        match value {
            "upsert" => Ok(Self::Upsert),
            "delete" => Ok(Self::Delete),
            other => Err(SearchStoreError::CorruptCatalog(format!(
                "unknown outbox operation '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchEntityType {
    Artist,
    Release,
    Track,
    AudioIdentity,
}

impl SearchEntityType {
    fn parse(value: &str) -> Result<Self, SearchStoreError> {
        match value {
            "artist" => Ok(Self::Artist),
            "release" => Ok(Self::Release),
            "track" => Ok(Self::Track),
            // emits AudioIdentity rows together with affected Track
            // rows. The schema has no AudioIdentity document; the
            // consumer records this row as an intentional no-op.
            "audio_identity" => Ok(Self::AudioIdentity),
            other => Err(SearchStoreError::UnsupportedEntity(other.to_string())),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Release => "release",
            Self::Track => "track",
            Self::AudioIdentity => "audio_identity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchOutboxItem {
    pub id: i64,
    pub entity_type: SearchEntityType,
    pub entity_id: Uuid,
    pub operation: SearchOperation,
    pub entity_revision: i64,
    pub attempt_count: i32,
    pub lease_expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchProjectionStatus {
    pub pending: i64,
    pub failed: i64,
    pub oldest_available_at: Option<OffsetDateTime>,
    pub leased: i64,
    pub last_processed_outbox_id: i64,
    pub active_generation_id: Option<Uuid>,
    pub latest_rebuild_status: Option<String>,
    pub latest_rebuild_error: Option<String>,
    pub latest_failure_kind: Option<String>,
    pub latest_failure_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchDocumentCounts {
    pub artists: u64,
    pub releases: u64,
    pub tracks: u64,
}

impl SearchDocumentCounts {
    #[must_use]
    pub const fn checked_total(&self) -> Option<u64> {
        match self.artists.checked_add(self.releases) {
            Some(value) => value.checked_add(self.tracks),
            None => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredSearchGeneration {
    pub generation_id: Uuid,
    pub search_root: String,
    pub schema_version: i32,
    pub status: String,
    pub source_high_water_mark: i64,
    pub caught_up_outbox_id: i64,
    pub document_counts: SearchDocumentCounts,
    /// Current live projection counts. `document_counts` remains the sealed
    /// snapshot manifest fact; this value advances with catch-up and outbox
    /// acknowledgements so diagnostics do not compare a live index with its
    /// historical snapshot baseline.
    pub current_document_counts: SearchDocumentCounts,
    pub manifest_hash: Option<String>,
    pub owner_lease: Option<String>,
    pub owner_lease_expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredSearchRebuildJob {
    pub job_id: Uuid,
    pub search_root: String,
    pub requested_by: String,
    pub status: String,
    pub generation_id: Option<Uuid>,
    pub requested_high_water_mark: Option<i64>,
    pub attempt_count: i32,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchArtistRecord {
    pub internal_id: Uuid,
    pub public_id: Uuid,
    pub display_name: String,
    pub sort_name: String,
    pub aliases: Vec<String>,
    pub redirect_ids: Vec<Uuid>,
    pub disambiguation: Option<String>,
    pub artist_type: String,
    pub status: String,
    pub begin_year: Option<i64>,
    pub end_year: Option<i64>,
    pub revision: i64,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchReleaseRecord {
    pub internal_id: Uuid,
    pub public_id: Uuid,
    pub title: String,
    pub sort_title: String,
    pub artist_ids: Vec<Uuid>,
    pub primary_artist_ids: Vec<Uuid>,
    pub featured_artist_ids: Vec<Uuid>,
    pub performer_artist_ids: Vec<Uuid>,
    pub other_artist_ids: Vec<Uuid>,
    pub artist_display: String,
    pub genre_tags: Vec<String>,
    pub release_year: Option<i64>,
    pub release_date_days: Option<i64>,
    pub release_type: String,
    pub status: String,
    pub revision: i64,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTrackRecord {
    pub internal_id: Uuid,
    pub public_id: Uuid,
    pub release_id: Uuid,
    pub title: String,
    pub sort_title: String,
    pub primary_artist_ids: Vec<Uuid>,
    pub featured_artist_ids: Vec<Uuid>,
    pub performer_artist_ids: Vec<Uuid>,
    pub other_artist_ids: Vec<Uuid>,
    pub participant_artist_ids: Vec<Uuid>,
    pub display_credits: String,
    pub genre_tags: Vec<String>,
    pub track_number: Option<i64>,
    pub duration_ms: Option<i64>,
    pub release_year: Option<i64>,
    pub release_date_days: Option<i64>,
    pub revision: i64,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchCatalogRecord {
    Artist(SearchArtistRecord),
    Release(SearchReleaseRecord),
    Track(SearchTrackRecord),
    NoDocument,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchProjectionStore;

impl SearchProjectionStore {
    pub async fn request_rebuild(
        pool: &PgPool,
        search_root: &str,
        requested_by: &str,
        now: OffsetDateTime,
    ) -> Result<StoredSearchRebuildJob, SearchStoreError> {
        validate_nonblank("search root", search_root)?;
        validate_nonblank("requested by", requested_by)?;
        let job_id = Uuid::now_v7();
        // ADR-0214: pruning pauses while a rebuild is requested or resumable.
        // Registration takes the same exclusive advisory key as a prune batch,
        // so a registration that commits concurrently with a prune either
        // blocks the delete or observes the new job.
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind((i64::from(19_761_125_i32) << 32) | i64::from(301_i32))
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query(
            "INSERT INTO search_rebuild_jobs (
                job_id, search_root, requested_at, requested_by, status, available_at
             ) VALUES ($1, $2, $3, $4, 'requested', $3)",
        )
        .bind(job_id)
        .bind(search_root)
        .bind(now)
        .bind(requested_by)
        .execute(&mut *tx)
        .await;
        tx.commit().await?;
        match result {
            Ok(_) => Self::get_rebuild_job(pool, job_id).await,
            Err(error) if is_unique_violation(&error) => {
                Err(SearchStoreError::RebuildConflict(search_root.to_string()))
            }
            Err(error) => {
                // Surface the database root cause for operator diagnostics;
                // the durable error boundary already maps this to a generic
                // message for API consumers.
                tracing::error!(%error, "search rebuild job request failed");
                Err(error.into())
            }
        }
    }

    pub async fn get_rebuild_job(
        pool: &PgPool,
        job_id: Uuid,
    ) -> Result<StoredSearchRebuildJob, SearchStoreError> {
        let row = sqlx::query(
            "SELECT job_id, search_root, requested_by, status, generation_id,
                    requested_high_water_mark, attempt_count, lease_owner,
                    lease_expires_at, error
             FROM search_rebuild_jobs WHERE job_id = $1",
        )
        .bind(job_id)
        .fetch_optional(pool)
        .await?
        .ok_or(SearchStoreError::RebuildNotFound(job_id))?;
        parse_rebuild_row(row)
    }

    pub async fn get_generation(
        pool: &PgPool,
        generation_id: Uuid,
    ) -> Result<StoredSearchGeneration, SearchStoreError> {
        let row = sqlx::query(
            "SELECT generation_id, search_root, schema_version, status,
                    source_high_water_mark, caught_up_outbox_id, document_counts,
                    current_document_counts, manifest_hash, owner_lease,
                    owner_lease_expires_at
             FROM search_generations WHERE generation_id = $1",
        )
        .bind(generation_id)
        .fetch_optional(pool)
        .await?
        .ok_or(SearchStoreError::GenerationNotFound(generation_id))?;
        parse_generation_row(row)
    }

    pub async fn active_generation(
        pool: &PgPool,
        search_root: &str,
    ) -> Result<Option<StoredSearchGeneration>, SearchStoreError> {
        validate_nonblank("search root", search_root)?;
        let row = sqlx::query(
            "SELECT generation.generation_id, generation.search_root,
                    generation.schema_version, generation.status,
                    generation.source_high_water_mark, generation.caught_up_outbox_id,
                    generation.document_counts, generation.current_document_counts,
                    generation.manifest_hash,
                    generation.owner_lease, generation.owner_lease_expires_at
             FROM search_active_generation active
             JOIN search_generations generation
               ON generation.generation_id = active.generation_id
              AND generation.search_root = active.search_root
             WHERE active.search_root = $1",
        )
        .bind(search_root)
        .fetch_optional(pool)
        .await?;
        row.map(parse_generation_row).transpose()
    }

    pub async fn retired_generation_ids(
        pool: &PgPool,
        search_root: &str,
        limit: usize,
    ) -> Result<Vec<Uuid>, SearchStoreError> {
        validate_nonblank("search root", search_root)?;
        if !(1..=100).contains(&limit) {
            return Err(SearchStoreError::InvalidRequest(
                "retired-generation limit must be between 1 and 100".to_string(),
            ));
        }
        sqlx::query_scalar(
            "SELECT generation_id FROM search_generations
             WHERE search_root = $1 AND status = 'retired' AND removed_at IS NULL
             ORDER BY retired_at, generation_id LIMIT $2",
        )
        .bind(search_root)
        .bind(i64::try_from(limit).map_err(|_| {
            SearchStoreError::InvalidRequest("retired-generation limit is too large".to_string())
        })?)
        .fetch_all(pool)
        .await
        .map_err(SearchStoreError::from)
    }

    pub async fn record_generation_removed(
        pool: &PgPool,
        generation_id: Uuid,
        search_root: &str,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        let updated = sqlx::query(
            "UPDATE search_generations SET removed_at = $3
             WHERE generation_id = $1 AND search_root = $2
               AND status = 'retired' AND removed_at IS NULL",
        )
        .bind(generation_id)
        .bind(search_root)
        .bind(now)
        .execute(pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(SearchStoreError::GenerationNotFound(generation_id));
        }
        Ok(())
    }

    pub async fn claim_rebuild(
        pool: &PgPool,
        search_root: &str,
        worker: &str,
        schema_version: i32,
        lease_duration: Duration,
        now: OffsetDateTime,
    ) -> Result<Option<StoredSearchRebuildJob>, SearchStoreError> {
        validate_nonblank("search root", search_root)?;
        validate_nonblank("worker", worker)?;
        if schema_version <= 0 || lease_duration.is_zero() {
            return Err(SearchStoreError::InvalidRequest(
                "schema version and lease duration must be positive".to_string(),
            ));
        }
        let expires_at = checked_expiry(now, lease_duration)?;
        let mut tx = pool.begin().await?;
        let row = sqlx::query(
            "SELECT job_id, status, generation_id
             FROM search_rebuild_jobs
             WHERE search_root = $1
               AND status IN ('requested', 'running', 'validated')
               AND available_at <= $2
               AND (lease_owner IS NULL OR lease_expires_at <= $2 OR lease_owner = $3)
             ORDER BY requested_at, job_id
             LIMIT 1 FOR UPDATE SKIP LOCKED",
        )
        .bind(search_root)
        .bind(now)
        .bind(worker)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let job_id: Uuid = row.try_get(0)?;
        let status: String = row.try_get(1)?;
        let generation_id: Option<Uuid> = row.try_get(2)?;
        if status == "requested" {
            // `max(id)` cannot prove completeness once processed detail has
            // been pruned; the durable per-projection watermark participates.
            // A database migrated from a historical schema may not have the
            // maintenance table yet, so the read falls back to the raw maximum.
            let maintenance_available: bool = sqlx::query_scalar(
                "SELECT to_regclass('public.search_outbox_maintenance') IS NOT NULL",
            )
            .fetch_one(&mut *tx)
            .await?;
            let high_water_mark = if maintenance_available {
                sqlx::query_scalar::<_, i64>(
                    "SELECT GREATEST(
                                COALESCE((SELECT max(id) FROM catalog_projection_outbox
                                           WHERE projection_name = $1), 0),
                                COALESCE((SELECT highest_pruned_id
                                            FROM search_outbox_maintenance
                                           WHERE projection_name = $1), 0)
                            )",
                )
                .bind(SEARCH_PROJECTION_NAME)
                .fetch_one(&mut *tx)
                .await?
            } else {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COALESCE((SELECT max(id) FROM catalog_projection_outbox
                                       WHERE projection_name = $1), 0)",
                )
                .bind(SEARCH_PROJECTION_NAME)
                .fetch_one(&mut *tx)
                .await?
            };
            let new_generation_id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO search_generations (
                    generation_id, search_root, schema_version, status, created_at,
                    source_high_water_mark, caught_up_outbox_id,
                    owner_lease, owner_lease_expires_at
                 ) VALUES ($1, $2, $3, 'building', $4, $5, $5, $6, $7)",
            )
            .bind(new_generation_id)
            .bind(search_root)
            .bind(schema_version)
            .bind(now)
            .bind(high_water_mark)
            .bind(worker)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE search_rebuild_jobs
                 SET status = 'running', generation_id = $2,
                     requested_high_water_mark = $3,
                     lease_owner = $4, lease_expires_at = $5,
                     attempt_count = attempt_count + 1
                 WHERE job_id = $1",
            )
            .bind(job_id)
            .bind(new_generation_id)
            .bind(high_water_mark)
            .bind(worker)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
        } else {
            let generation_id = generation_id.ok_or_else(|| {
                SearchStoreError::CorruptCatalog("running rebuild has no generation".to_string())
            })?;
            let generation = sqlx::query(
                "UPDATE search_generations
                 SET owner_lease = $2, owner_lease_expires_at = $3
                 WHERE generation_id = $1 AND search_root = $4
                   AND status IN ('building', 'validated')
                   AND (owner_lease IS NULL OR owner_lease_expires_at <= $5 OR owner_lease = $2)",
            )
            .bind(generation_id)
            .bind(worker)
            .bind(expires_at)
            .bind(search_root)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if generation.rows_affected() != 1 {
                return Err(SearchStoreError::LeaseConflict);
            }
            sqlx::query(
                "UPDATE search_rebuild_jobs
                 SET lease_owner = $2, lease_expires_at = $3,
                     attempt_count = attempt_count + 1
                 WHERE job_id = $1",
            )
            .bind(job_id)
            .bind(worker)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Self::get_rebuild_job(pool, job_id).await.map(Some)
    }

    pub async fn renew_rebuild_lease(
        pool: &PgPool,
        job_id: Uuid,
        worker: &str,
        lease_duration: Duration,
        now: OffsetDateTime,
    ) -> Result<OffsetDateTime, SearchStoreError> {
        validate_nonblank("worker", worker)?;
        let expires_at = checked_expiry(now, lease_duration)?;
        let mut tx = pool.begin().await?;
        let generation_id = sqlx::query_scalar::<_, Uuid>(
            "UPDATE search_rebuild_jobs
             SET lease_expires_at = $3
             WHERE job_id = $1 AND lease_owner = $2 AND lease_expires_at > $4
               AND status IN ('running', 'validated')
             RETURNING generation_id",
        )
        .bind(job_id)
        .bind(worker)
        .bind(expires_at)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(SearchStoreError::LeaseLost)?;
        let generation = sqlx::query(
            "UPDATE search_generations
             SET owner_lease_expires_at = $3
             WHERE generation_id = $1 AND owner_lease = $2
               AND owner_lease_expires_at > $4
               AND status IN ('building', 'validated')",
        )
        .bind(generation_id)
        .bind(worker)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if generation.rows_affected() != 1 {
            return Err(SearchStoreError::LeaseLost);
        }
        tx.commit().await?;
        Ok(expires_at)
    }

    pub async fn release_rebuild_lease(
        pool: &PgPool,
        job_id: Uuid,
        worker: &str,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        let mut tx = pool.begin().await?;
        let generation_id = sqlx::query_scalar::<_, Uuid>(
            "UPDATE search_rebuild_jobs
             SET lease_owner = NULL, lease_expires_at = NULL, available_at = $3
             WHERE job_id = $1 AND lease_owner = $2
               AND status IN ('running', 'validated')
             RETURNING generation_id",
        )
        .bind(job_id)
        .bind(worker)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(SearchStoreError::LeaseLost)?;
        sqlx::query(
            "UPDATE search_generations
             SET owner_lease = NULL, owner_lease_expires_at = NULL
             WHERE generation_id = $1 AND owner_lease = $2
               AND status IN ('building', 'validated')",
        )
        .bind(generation_id)
        .bind(worker)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn record_rebuild_retry(
        pool: &PgPool,
        job_id: Uuid,
        worker: &str,
        message: &str,
        retry_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        if message.trim().is_empty() || message.len() > 4_096 || retry_at < now {
            return Err(SearchStoreError::InvalidRequest(
                "retry evidence or schedule is invalid".to_string(),
            ));
        }
        let mut tx = pool.begin().await?;
        let generation_id = sqlx::query_scalar::<_, Uuid>(
            "UPDATE search_rebuild_jobs
             SET last_error = $4, available_at = $3,
                 lease_owner = NULL, lease_expires_at = NULL
             WHERE job_id = $1 AND lease_owner = $2 AND lease_expires_at > $5
               AND status IN ('running', 'validated')
             RETURNING generation_id",
        )
        .bind(job_id)
        .bind(worker)
        .bind(retry_at)
        .bind(message)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(SearchStoreError::LeaseLost)?;
        sqlx::query(
            "UPDATE search_generations
             SET owner_lease = NULL, owner_lease_expires_at = NULL
             WHERE generation_id = $1 AND owner_lease = $2
               AND status IN ('building', 'validated')",
        )
        .bind(generation_id)
        .bind(worker)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn mark_generation_validated(
        pool: &PgPool,
        job_id: Uuid,
        worker: &str,
        counts: &SearchDocumentCounts,
        manifest_hash: &str,
        caught_up_outbox_id: i64,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        validate_nonblank("worker", worker)?;
        if !is_lower_hex_hash(manifest_hash) || caught_up_outbox_id < 0 {
            return Err(SearchStoreError::InvalidRequest(
                "manifest hash or outbox high-water mark is invalid".to_string(),
            ));
        }
        let counts_json = serde_json::to_value(counts).map_err(|error| {
            SearchStoreError::InvalidRequest(format!("document counts are invalid: {error}"))
        })?;
        let mut tx = pool.begin().await?;
        let generation_id = lock_rebuild_job(&mut tx, job_id, worker, now, "running").await?;
        let generation = sqlx::query(
            "UPDATE search_generations
             SET status = 'validated', validated_at = $5,
                 document_counts = $3, current_document_counts = $3,
                 manifest_hash = $4,
                 caught_up_outbox_id = $6
             WHERE generation_id = $1 AND owner_lease = $2
               AND owner_lease_expires_at > $5 AND status = 'building'
               AND source_high_water_mark <= $6",
        )
        .bind(generation_id)
        .bind(worker)
        .bind(counts_json)
        .bind(manifest_hash)
        .bind(now)
        .bind(caught_up_outbox_id)
        .execute(&mut *tx)
        .await?;
        if generation.rows_affected() != 1 {
            return Err(SearchStoreError::LeaseLost);
        }
        sqlx::query("UPDATE search_rebuild_jobs SET status = 'validated' WHERE job_id = $1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_generation_catchup(
        pool: &PgPool,
        job_id: Uuid,
        worker: &str,
        caught_up_outbox_id: i64,
        current_counts: &SearchDocumentCounts,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        if caught_up_outbox_id < 0 {
            return Err(SearchStoreError::InvalidRequest(
                "outbox high-water mark must be non-negative".to_string(),
            ));
        }
        let counts_json = serde_json::to_value(current_counts).map_err(|error| {
            SearchStoreError::InvalidRequest(format!(
                "current document counts are invalid: {error}"
            ))
        })?;
        let mut tx = pool.begin().await?;
        let generation_id = lock_rebuild_job(&mut tx, job_id, worker, now, "validated").await?;
        let updated = sqlx::query(
            "UPDATE search_generations
             SET caught_up_outbox_id = GREATEST(caught_up_outbox_id, $3),
                 current_document_counts = $5
             WHERE generation_id = $1 AND owner_lease = $2
               AND owner_lease_expires_at > $4 AND status = 'validated'",
        )
        .bind(generation_id)
        .bind(worker)
        .bind(caught_up_outbox_id)
        .bind(now)
        .bind(counts_json)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(SearchStoreError::LeaseLost);
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn activate_generation(
        pool: &PgPool,
        job_id: Uuid,
        worker: &str,
        now: OffsetDateTime,
    ) -> Result<StoredSearchGeneration, SearchStoreError> {
        let mut tx = pool.begin().await?;
        let generation_id = lock_rebuild_job(&mut tx, job_id, worker, now, "validated").await?;
        let row = sqlx::query(
            "SELECT generation.search_root, generation.caught_up_outbox_id
             FROM search_generations generation
             WHERE generation.generation_id = $1 AND generation.owner_lease = $2
               AND generation.owner_lease_expires_at > $3
               AND generation.status = 'validated'
             FOR UPDATE",
        )
        .bind(generation_id)
        .bind(worker)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(SearchStoreError::LeaseLost)?;
        // Locks catalog outbox only after rebuild/generation rows are fenced, so row-lock
        // contention on unrelated rebuild work does not block INSERT writers.
        sqlx::query("LOCK TABLE catalog_projection_outbox IN SHARE MODE")
            .execute(&mut *tx)
            .await?;
        let search_root: String = row.try_get(0)?;
        let caught_up: i64 = row.try_get(1)?;
        let latest = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(id) FROM catalog_projection_outbox WHERE projection_name = $1",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .fetch_one(&mut *tx)
        .await?
        .unwrap_or(0);
        if caught_up < latest {
            return Err(SearchStoreError::NeedsCatchup(latest));
        }
        sqlx::query(
            "UPDATE search_generations retired
             SET status = 'retired', retired_at = $2
             FROM search_active_generation active
             WHERE active.search_root = $1
               AND retired.generation_id = active.generation_id
               AND retired.status = 'active'",
        )
        .bind(&search_root)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE search_generations
             SET status = 'active', activated_at = $2,
                 owner_lease = NULL, owner_lease_expires_at = NULL
             WHERE generation_id = $1",
        )
        .bind(generation_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO search_active_generation (
                search_root, generation_id, activated_at, updated_at
             ) VALUES ($1, $2, $3, $3)
             ON CONFLICT (search_root) DO UPDATE
             SET generation_id = EXCLUDED.generation_id,
                 activated_at = EXCLUDED.activated_at,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&search_root)
        .bind(generation_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE search_rebuild_jobs
             SET status = 'activated', caught_up_at = $2, finished_at = $2,
                 lease_owner = NULL, lease_expires_at = NULL
             WHERE job_id = $1",
        )
        .bind(job_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Self::get_generation(pool, generation_id).await
    }

    pub async fn record_rebuild_failure(
        pool: &PgPool,
        job_id: Uuid,
        worker: &str,
        failure_kind: &str,
        message: &str,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        validate_failure(failure_kind, message)?;
        let mut tx = pool.begin().await?;
        let generation_id = sqlx::query_scalar::<_, Uuid>(
            "UPDATE search_rebuild_jobs
             SET status = 'failed', failure_kind = $4, error = $5,
                 finished_at = $3, lease_owner = NULL, lease_expires_at = NULL
             WHERE job_id = $1 AND lease_owner = $2 AND lease_expires_at > $3
               AND status IN ('running', 'validated')
             RETURNING generation_id",
        )
        .bind(job_id)
        .bind(worker)
        .bind(now)
        .bind(failure_kind)
        .bind(message)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(SearchStoreError::LeaseLost)?;
        sqlx::query(
            "UPDATE search_generations
             SET status = 'failed', failure_kind = $4, error = $5,
                 owner_lease = NULL, owner_lease_expires_at = NULL
             WHERE generation_id = $1 AND owner_lease = $2
               AND owner_lease_expires_at > $3
               AND status IN ('building', 'validated')",
        )
        .bind(generation_id)
        .bind(worker)
        .bind(now)
        .bind(failure_kind)
        .bind(message)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn load_outbox_after(
        pool: &PgPool,
        after_id: i64,
        through_id: i64,
        limit: usize,
    ) -> Result<Vec<SearchOutboxItem>, SearchStoreError> {
        if after_id < 0 || through_id < after_id || !(1..=MAX_SEARCH_OUTBOX_BATCH).contains(&limit)
        {
            return Err(SearchStoreError::InvalidRequest(
                "outbox replay bounds are invalid".to_string(),
            ));
        }
        let rows = sqlx::query(
            "SELECT id, entity_type, entity_id, operation, entity_revision,
                    attempt_count, COALESCE(lease_expires_at, available_at)
             FROM catalog_projection_outbox
             WHERE projection_name = $1 AND id > $2 AND id <= $3
             ORDER BY id LIMIT $4",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(after_id)
        .bind(through_id)
        .bind(i64::try_from(limit).map_err(|_| {
            SearchStoreError::InvalidRequest("outbox replay limit is too large".to_string())
        })?)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(parse_outbox_row).collect()
    }

    pub async fn claim_outbox(
        pool: &PgPool,
        projection_name: &str,
        worker: &str,
        batch_size: usize,
        lease_duration: Duration,
        now: OffsetDateTime,
    ) -> Result<Vec<SearchOutboxItem>, SearchStoreError> {
        validate_lease_request(projection_name, worker, batch_size, lease_duration)?;
        let lease_expires_at = now
            + time::Duration::try_from(lease_duration).map_err(|_| {
                SearchStoreError::InvalidRequest("lease duration is too large".to_string())
            })?;
        let batch_size = i64::try_from(batch_size)
            .map_err(|_| SearchStoreError::InvalidRequest("batch size is too large".to_string()))?;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO search_projection_checkpoint (
                projection_name, last_processed_outbox_id, updated_at
             ) VALUES ($1, 0, $2)
             ON CONFLICT (projection_name) DO NOTHING",
        )
        .bind(projection_name)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let owns_projection = sqlx::query_scalar::<_, String>(
            "UPDATE search_projection_checkpoint
             SET lease_owner = $2, lease_expires_at = $3, updated_at = $4
             WHERE projection_name = $1
               AND (lease_owner IS NULL OR lease_expires_at <= $4 OR lease_owner = $2)
             RETURNING lease_owner",
        )
        .bind(projection_name)
        .bind(worker)
        .bind(lease_expires_at)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        if !owns_projection {
            tx.commit().await?;
            return Ok(Vec::new());
        }

        let rows = sqlx::query(
            "WITH candidates AS (
                SELECT id
                FROM catalog_projection_outbox
                WHERE projection_name = $1
                  AND processed_at IS NULL
                  AND failed_at IS NULL
                  AND available_at <= $2
                  AND (lease_owner IS NULL OR lease_expires_at <= $2 OR lease_owner = $3)
                ORDER BY id
                LIMIT $4
                FOR UPDATE SKIP LOCKED
             )
             UPDATE catalog_projection_outbox outbox
             SET lease_owner = $3, lease_expires_at = $5
             FROM candidates
             WHERE outbox.id = candidates.id
             RETURNING outbox.id, outbox.entity_type, outbox.entity_id,
                       outbox.operation, outbox.entity_revision,
                       outbox.attempt_count, outbox.lease_expires_at",
        )
        .bind(projection_name)
        .bind(now)
        .bind(worker)
        .bind(batch_size)
        .bind(lease_expires_at)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            // An idle poll must not retain the global writer lease. The
            // scheduler uses a fresh process-unique worker token per pass;
            // retaining this lease would delay newly committed outbox work
            // until the full lease duration elapsed.
            sqlx::query(
                "UPDATE search_projection_checkpoint
                 SET lease_owner = NULL, lease_expires_at = NULL, updated_at = $3
                 WHERE projection_name = $1 AND lease_owner = $2",
            )
            .bind(projection_name)
            .bind(worker)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        rows.into_iter().map(parse_outbox_row).collect()
    }

    pub async fn renew_outbox_lease(
        pool: &PgPool,
        projection_name: &str,
        worker: &str,
        outbox_ids: &[i64],
        lease_duration: Duration,
        now: OffsetDateTime,
    ) -> Result<OffsetDateTime, SearchStoreError> {
        if outbox_ids.is_empty() {
            return Err(SearchStoreError::InvalidRequest(
                "at least one outbox id is required".to_string(),
            ));
        }
        validate_lease_request(projection_name, worker, 1, lease_duration)?;
        let lease_expires_at = now
            + time::Duration::try_from(lease_duration).map_err(|_| {
                SearchStoreError::InvalidRequest("lease duration is too large".to_string())
            })?;
        let mut tx = pool.begin().await?;
        let checkpoint = sqlx::query(
            "UPDATE search_projection_checkpoint
             SET lease_expires_at = $3, updated_at = $4
             WHERE projection_name = $1 AND lease_owner = $2
               AND lease_expires_at > $4",
        )
        .bind(projection_name)
        .bind(worker)
        .bind(lease_expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if checkpoint.rows_affected() != 1 {
            return Err(SearchStoreError::LeaseLost);
        }
        let renewed = sqlx::query(
            "UPDATE catalog_projection_outbox
             SET lease_expires_at = $4
             WHERE id = ANY($1) AND projection_name = $2
               AND lease_owner = $3 AND lease_expires_at > $5
               AND processed_at IS NULL AND failed_at IS NULL",
        )
        .bind(outbox_ids)
        .bind(projection_name)
        .bind(worker)
        .bind(lease_expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if renewed.rows_affected() != outbox_ids.len() as u64 {
            return Err(SearchStoreError::LeaseLost);
        }
        tx.commit().await?;
        Ok(lease_expires_at)
    }

    pub async fn acknowledge_outbox(
        pool: &PgPool,
        projection_name: &str,
        worker: &str,
        outbox_ids: &[i64],
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        Self::acknowledge_outbox_inner(pool, projection_name, worker, outbox_ids, None, now).await
    }

    /// Acknowledge rows and advance the active generation's live document
    /// counts in the same PostgreSQL transaction. The sealed snapshot counts
    /// remain immutable; these live counts are the diagnostic comparison for
    /// an incrementally changing Tantivy generation.
    pub async fn acknowledge_outbox_with_generation(
        pool: &PgPool,
        projection_name: &str,
        worker: &str,
        outbox_ids: &[i64],
        generation_id: Uuid,
        current_counts: &SearchDocumentCounts,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        Self::acknowledge_outbox_inner(
            pool,
            projection_name,
            worker,
            outbox_ids,
            Some((generation_id, current_counts)),
            now,
        )
        .await
    }

    async fn acknowledge_outbox_inner(
        pool: &PgPool,
        projection_name: &str,
        worker: &str,
        outbox_ids: &[i64],
        generation: Option<(Uuid, &SearchDocumentCounts)>,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        if outbox_ids.is_empty() {
            return Err(SearchStoreError::InvalidRequest(
                "at least one outbox id is required".to_string(),
            ));
        }
        let mut tx = pool.begin().await?;
        // ADR-0214: hour finalization takes the exclusive form of this key, so
        // a completion that is in flight when an hour closes is visible to the
        // aggregate and a late commit cannot lose a count.
        sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
            .bind((i64::from(19_761_125_i32) << 32) | i64::from(302_i32))
            .execute(&mut *tx)
            .await?;
        let updated = sqlx::query(
            "UPDATE catalog_projection_outbox
             SET processed_at = $4, lease_owner = NULL, lease_expires_at = NULL,
                 last_error = NULL
             WHERE id = ANY($1) AND projection_name = $2
               AND lease_owner = $3 AND lease_expires_at > $4
               AND processed_at IS NULL AND failed_at IS NULL",
        )
        .bind(outbox_ids)
        .bind(projection_name)
        .bind(worker)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != outbox_ids.len() as u64 {
            return Err(SearchStoreError::LeaseLost);
        }
        if let Some((generation_id, current_counts)) = generation {
            let counts_json = serde_json::to_value(current_counts).map_err(|error| {
                SearchStoreError::InvalidRequest(format!(
                    "current document counts are invalid: {error}"
                ))
            })?;
            let generation = sqlx::query(
                "UPDATE search_generations generation
                 SET current_document_counts = $2
                 WHERE generation.generation_id = $1
                   AND generation.status = 'active'
                   AND EXISTS (
                       SELECT 1 FROM search_active_generation active
                       WHERE active.generation_id = generation.generation_id
                         AND active.search_root = generation.search_root
                   )",
            )
            .bind(generation_id)
            .bind(counts_json)
            .execute(&mut *tx)
            .await?;
            if generation.rows_affected() != 1 {
                return Err(SearchStoreError::LeaseLost);
            }
        }
        let max_id = outbox_ids.iter().copied().max().ok_or_else(|| {
            SearchStoreError::InvalidRequest("at least one outbox id is required".to_string())
        })?;
        let checkpoint = sqlx::query(
            "UPDATE search_projection_checkpoint
             SET last_processed_outbox_id = GREATEST(last_processed_outbox_id, $3),
                 lease_owner = NULL, lease_expires_at = NULL, updated_at = $4
             WHERE projection_name = $1 AND lease_owner = $2
               AND lease_expires_at > $4",
        )
        .bind(projection_name)
        .bind(worker)
        .bind(max_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if checkpoint.rows_affected() != 1 {
            return Err(SearchStoreError::LeaseLost);
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn release_outbox_lease(
        pool: &PgPool,
        projection_name: &str,
        worker: &str,
        now: OffsetDateTime,
    ) -> Result<u64, SearchStoreError> {
        let mut tx = pool.begin().await?;
        let released = sqlx::query(
            "UPDATE catalog_projection_outbox
             SET lease_owner = NULL, lease_expires_at = NULL
             WHERE projection_name = $1 AND lease_owner = $2
               AND processed_at IS NULL AND failed_at IS NULL",
        )
        .bind(projection_name)
        .bind(worker)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE search_projection_checkpoint
             SET lease_owner = NULL, lease_expires_at = NULL, updated_at = $3
             WHERE projection_name = $1 AND lease_owner = $2",
        )
        .bind(projection_name)
        .bind(worker)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(released.rows_affected())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_outbox_failure(
        pool: &PgPool,
        projection_name: &str,
        worker: &str,
        outbox_ids: &[i64],
        failure_kind: &str,
        message: &str,
        retry_at: Option<OffsetDateTime>,
        max_attempts: u32,
        release_checkpoint: bool,
        now: OffsetDateTime,
    ) -> Result<(), SearchStoreError> {
        if outbox_ids.is_empty()
            || failure_kind.trim().is_empty()
            || message.trim().is_empty()
            || message.len() > 4_096
            || !(1..=MAX_SEARCH_OUTBOX_ATTEMPTS).contains(&max_attempts)
        {
            return Err(SearchStoreError::InvalidRequest(
                "failure evidence is missing or oversized".to_string(),
            ));
        }
        let max_attempts = i32::try_from(max_attempts).map_err(|_| {
            SearchStoreError::InvalidRequest("maximum attempts is too large".to_string())
        })?;
        let mut tx = pool.begin().await?;
        let updated = if let Some(available_at) = retry_at {
            sqlx::query(
                "UPDATE catalog_projection_outbox
                 SET attempt_count = attempt_count + 1,
                     available_at = CASE
                         WHEN attempt_count + 1 >= $7 THEN available_at ELSE $5 END,
                     last_error = $6,
                     failure_kind = CASE
                         WHEN attempt_count + 1 >= $7 THEN $8 ELSE NULL END,
                     failed_at = CASE
                         WHEN attempt_count + 1 >= $7 THEN $4 ELSE NULL END,
                     lease_owner = NULL, lease_expires_at = NULL
                 WHERE id = ANY($1) AND projection_name = $2
                   AND lease_owner = $3 AND lease_expires_at > $4
                   AND processed_at IS NULL AND failed_at IS NULL",
            )
            .bind(outbox_ids)
            .bind(projection_name)
            .bind(worker)
            .bind(now)
            .bind(available_at)
            .bind(message)
            .bind(max_attempts)
            .bind(failure_kind)
            .execute(&mut *tx)
            .await?
        } else {
            sqlx::query(
                "UPDATE catalog_projection_outbox
                 SET attempt_count = attempt_count + 1, last_error = $6,
                     failure_kind = $5, failed_at = $4,
                     lease_owner = NULL, lease_expires_at = NULL
                 WHERE id = ANY($1) AND projection_name = $2
                   AND lease_owner = $3 AND lease_expires_at > $4
                   AND processed_at IS NULL AND failed_at IS NULL",
            )
            .bind(outbox_ids)
            .bind(projection_name)
            .bind(worker)
            .bind(now)
            .bind(failure_kind)
            .bind(message)
            .execute(&mut *tx)
            .await?
        };
        if updated.rows_affected() != outbox_ids.len() as u64 {
            return Err(SearchStoreError::LeaseLost);
        }
        if release_checkpoint {
            sqlx::query(
                "UPDATE search_projection_checkpoint
                 SET lease_owner = NULL, lease_expires_at = NULL, updated_at = $3
                 WHERE projection_name = $1 AND lease_owner = $2",
            )
            .bind(projection_name)
            .bind(worker)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn status(
        pool: &PgPool,
        projection_name: &str,
        search_root: &str,
        now: OffsetDateTime,
    ) -> Result<SearchProjectionStatus, SearchStoreError> {
        validate_nonblank("projection name", projection_name)?;
        validate_nonblank("search root", search_root)?;
        let row = sqlx::query(
            "SELECT
                (SELECT count(*) FROM catalog_projection_outbox outbox
                 WHERE outbox.projection_name = $1
                   AND outbox.processed_at IS NULL AND outbox.failed_at IS NULL
                   AND (outbox.lease_owner IS NULL OR
                        (outbox.lease_owner IS NOT NULL AND outbox.lease_expires_at <= $3))),
                (SELECT count(*) FROM catalog_projection_outbox outbox
                 WHERE outbox.projection_name = $1 AND outbox.failed_at IS NOT NULL),
                (SELECT min(outbox.available_at) FROM catalog_projection_outbox outbox
                 WHERE outbox.projection_name = $1
                   AND outbox.processed_at IS NULL AND outbox.failed_at IS NULL
                   AND (outbox.lease_owner IS NULL OR
                        (outbox.lease_owner IS NOT NULL AND outbox.lease_expires_at <= $3))),
                (SELECT count(*) FROM catalog_projection_outbox outbox
                 WHERE outbox.projection_name = $1
                   AND outbox.processed_at IS NULL AND outbox.failed_at IS NULL
                   AND outbox.lease_owner IS NOT NULL
                   AND outbox.lease_expires_at > $3),
                COALESCE((SELECT checkpoint.last_processed_outbox_id
                          FROM search_projection_checkpoint checkpoint
                          WHERE checkpoint.projection_name = $1), 0),
                (SELECT active.generation_id FROM search_active_generation active
                 WHERE active.search_root = $2),
                (SELECT job.status FROM search_rebuild_jobs job
                 WHERE job.search_root = $2
                 ORDER BY job.requested_at DESC, job.job_id DESC LIMIT 1),
                (SELECT COALESCE(job.error, job.last_error)
                 FROM search_rebuild_jobs job WHERE job.search_root = $2
                 ORDER BY job.requested_at DESC, job.job_id DESC LIMIT 1),
                (SELECT outbox.failure_kind FROM catalog_projection_outbox outbox
                 WHERE outbox.projection_name = $1 AND outbox.failed_at IS NOT NULL
                 ORDER BY outbox.failed_at DESC, outbox.id DESC LIMIT 1),
                (SELECT outbox.last_error FROM catalog_projection_outbox outbox
                 WHERE outbox.projection_name = $1 AND outbox.failed_at IS NOT NULL
                 ORDER BY outbox.failed_at DESC, outbox.id DESC LIMIT 1)",
        )
        .bind(projection_name)
        .bind(search_root)
        .bind(now)
        .fetch_one(pool)
        .await?;
        Ok(SearchProjectionStatus {
            pending: row.try_get(0)?,
            failed: row.try_get(1)?,
            oldest_available_at: row.try_get(2)?,
            leased: row.try_get(3)?,
            last_processed_outbox_id: row.try_get(4)?,
            active_generation_id: row.try_get(5)?,
            latest_rebuild_status: row.try_get(6)?,
            latest_rebuild_error: row.try_get(7)?,
            latest_failure_kind: row.try_get(8)?,
            latest_failure_message: row.try_get(9)?,
        })
    }

    pub async fn load_catalog_record(
        pool: &PgPool,
        entity_type: SearchEntityType,
        entity_id: Uuid,
    ) -> Result<Option<SearchCatalogRecord>, SearchStoreError> {
        match entity_type {
            SearchEntityType::Artist => load_artist(pool, entity_id)
                .await
                .map(|record| record.map(SearchCatalogRecord::Artist)),
            SearchEntityType::Release => load_release(pool, entity_id)
                .await
                .map(|record| record.map(SearchCatalogRecord::Release)),
            SearchEntityType::Track => load_track(pool, entity_id)
                .await
                .map(|record| record.map(SearchCatalogRecord::Track)),
            SearchEntityType::AudioIdentity => Ok(Some(SearchCatalogRecord::NoDocument)),
        }
    }

    /// Distinct Tracks successfully indexed per UTC hour, oldest first.
    ///
    /// This is the durable evidence behind "Tracks indexed per hour": the
    /// outbox row is marked `processed_at` only after the Tantivy commit that
    /// made the document queryable, so the series measures real index work
    /// rather than a work-item state. Counts are distinct Tracks per hour, so a
    /// Track re-projected several times in one hour is not counted repeatedly.
    ///
    /// ADR-0214: a closed hour comes from its immutable finalized total, while
    /// hours at or after `finalized_through` (including the live partial hour)
    /// still come from raw detail. The result is identical before and after
    /// pruning, and the zero-filled spine keeps the Admin chart axis stable.
    pub async fn processed_tracks_hourly(
        pool: &PgPool,
        since: OffsetDateTime,
    ) -> Result<Vec<HourlyCompletionCount>, SearchStoreError> {
        if !search_outbox_maintenance_available(pool).await? {
            let raw: Vec<(OffsetDateTime, i64)> = sqlx::query_as(
                "WITH spine AS (
                   SELECT generate_series(
                            date_trunc('hour', $2::timestamptz),
                            date_trunc('hour', now()),
                            interval '1 hour'
                          ) AS hour
                 ),
                 counted AS (
                   SELECT date_trunc('hour', processed_at) AS hour,
                          COUNT(DISTINCT entity_id)::bigint AS count
                     FROM catalog_projection_outbox
                    WHERE projection_name = $1
                      AND entity_type = 'track'
                      AND processed_at IS NOT NULL
                      AND processed_at >= $2
                    GROUP BY 1
                 )
                 SELECT spine.hour, COALESCE(counted.count, 0)
                   FROM spine LEFT JOIN counted USING (hour)
                  ORDER BY 1",
            )
            .bind(SEARCH_PROJECTION_NAME)
            .bind(since)
            .fetch_all(pool)
            .await?;
            return Ok(raw
                .into_iter()
                .map(|(hour, count)| HourlyCompletionCount { hour, count })
                .collect());
        }
        let rows: Vec<(OffsetDateTime, i64)> = sqlx::query_as(
            "WITH spine AS (
               SELECT generate_series(
                        date_trunc('hour', $2::timestamptz),
                        date_trunc('hour', now()),
                        interval '1 hour'
                      ) AS hour
             ),
             maintenance AS (
               SELECT finalized_through FROM search_outbox_maintenance
                WHERE projection_name = $1
             ),
             counted AS (
               SELECT hour, sum(indexed_tracks)::bigint AS count
                 FROM (
                   SELECT totals.hour, totals.indexed_tracks
                     FROM search_outbox_hourly_totals totals
                    WHERE totals.projection_name = $1
                      AND totals.hour >= date_trunc('hour', $2::timestamptz)
                   UNION ALL
                   SELECT date_trunc('hour', outbox.processed_at) AS hour,
                          COUNT(DISTINCT outbox.entity_id)::bigint AS indexed_tracks
                     FROM catalog_projection_outbox outbox
                    WHERE outbox.projection_name = $1
                      AND outbox.entity_type = 'track'
                      AND outbox.processed_at IS NOT NULL
                      AND outbox.processed_at >= $2
                      AND date_trunc('hour', outbox.processed_at) > COALESCE(
                            (SELECT finalized_through FROM maintenance),
                            '-infinity'::timestamptz)
                    GROUP BY 1
                 ) merged
                GROUP BY hour
             )
             SELECT spine.hour, COALESCE(counted.count, 0)
               FROM spine LEFT JOIN counted USING (hour)
              ORDER BY 1",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(since)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(hour, count)| HourlyCompletionCount { hour, count })
            .collect())
    }

    pub async fn max_outbox_id(pool: &PgPool) -> Result<i64, SearchStoreError> {
        // ADR-0214: after pruning, the highest existing id is no longer the
        // highest id that ever existed. The pruned watermark supplies the rest
        // where the maintenance table exists (historical schemas may not).
        if search_outbox_maintenance_available(pool).await? {
            return Ok(sqlx::query_scalar::<_, i64>(
                "SELECT GREATEST(
                            COALESCE((SELECT max(id) FROM catalog_projection_outbox
                                       WHERE projection_name = $1), 0),
                            COALESCE((SELECT highest_pruned_id
                                        FROM search_outbox_maintenance
                                       WHERE projection_name = $1), 0)
                        )",
            )
            .bind(SEARCH_PROJECTION_NAME)
            .fetch_one(pool)
            .await?);
        }
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(max(id), 0) FROM catalog_projection_outbox
              WHERE projection_name = $1",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .fetch_one(pool)
        .await?)
    }
}

/// True when the ADR-0214 maintenance table exists. Historical-schema
/// compatibility tests migrate a database to an earlier version and still call
/// the rebuild/status paths, so those reads must not assume the table.
async fn search_outbox_maintenance_available(pool: &PgPool) -> Result<bool, SearchStoreError> {
    Ok(
        sqlx::query_scalar("SELECT to_regclass('public.search_outbox_maintenance') IS NOT NULL")
            .fetch_one(pool)
            .await?,
    )
}

async fn lock_rebuild_job(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    job_id: Uuid,
    worker: &str,
    now: OffsetDateTime,
    expected_status: &str,
) -> Result<Uuid, SearchStoreError> {
    let row = sqlx::query(
        "SELECT generation_id, status
         FROM search_rebuild_jobs
         WHERE job_id = $1 AND lease_owner = $2 AND lease_expires_at > $3
         FOR UPDATE",
    )
    .bind(job_id)
    .bind(worker)
    .bind(now)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(SearchStoreError::LeaseLost)?;
    let status: String = row.try_get(1)?;
    if status != expected_status {
        return Err(SearchStoreError::InvalidRequest(format!(
            "rebuild must be {expected_status}, not {status}"
        )));
    }
    row.try_get::<Option<Uuid>, _>(0)?
        .ok_or_else(|| SearchStoreError::CorruptCatalog("rebuild has no generation".to_string()))
}

fn checked_expiry(
    now: OffsetDateTime,
    lease_duration: Duration,
) -> Result<OffsetDateTime, SearchStoreError> {
    if lease_duration.is_zero() {
        return Err(SearchStoreError::InvalidRequest(
            "lease duration must be positive".to_string(),
        ));
    }
    Ok(now
        + time::Duration::try_from(lease_duration).map_err(|_| {
            SearchStoreError::InvalidRequest("lease duration is too large".to_string())
        })?)
}

fn is_lower_hex_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_failure(kind: &str, message: &str) -> Result<(), SearchStoreError> {
    if kind.trim().is_empty() || message.trim().is_empty() || message.len() > 4_096 {
        return Err(SearchStoreError::InvalidRequest(
            "failure evidence is missing or oversized".to_string(),
        ));
    }
    Ok(())
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23505")
}

fn parse_rebuild_row(
    row: sqlx::postgres::PgRow,
) -> Result<StoredSearchRebuildJob, SearchStoreError> {
    Ok(StoredSearchRebuildJob {
        job_id: row.try_get(0)?,
        search_root: row.try_get(1)?,
        requested_by: row.try_get(2)?,
        status: row.try_get(3)?,
        generation_id: row.try_get(4)?,
        requested_high_water_mark: row.try_get(5)?,
        attempt_count: row.try_get(6)?,
        lease_owner: row.try_get(7)?,
        lease_expires_at: row.try_get(8)?,
        error: row.try_get(9)?,
    })
}

fn parse_generation_row(
    row: sqlx::postgres::PgRow,
) -> Result<StoredSearchGeneration, SearchStoreError> {
    let snapshot_counts: serde_json::Value = row.try_get(6)?;
    let document_counts = serde_json::from_value(snapshot_counts).map_err(|error| {
        SearchStoreError::CorruptCatalog(format!("invalid search document counts: {error}"))
    })?;
    let live_counts: serde_json::Value = row.try_get(7)?;
    let current_document_counts = serde_json::from_value(live_counts).map_err(|error| {
        SearchStoreError::CorruptCatalog(format!("invalid current search document counts: {error}"))
    })?;
    Ok(StoredSearchGeneration {
        generation_id: row.try_get(0)?,
        search_root: row.try_get(1)?,
        schema_version: row.try_get(2)?,
        status: row.try_get(3)?,
        source_high_water_mark: row.try_get(4)?,
        caught_up_outbox_id: row.try_get(5)?,
        document_counts,
        current_document_counts,
        manifest_hash: row.try_get(8)?,
        owner_lease: row.try_get(9)?,
        owner_lease_expires_at: row.try_get(10)?,
    })
}

fn validate_nonblank(label: &str, value: &str) -> Result<(), SearchStoreError> {
    if value.trim().is_empty() {
        return Err(SearchStoreError::InvalidRequest(format!(
            "{label} must not be blank"
        )));
    }
    Ok(())
}

fn validate_lease_request(
    projection_name: &str,
    worker: &str,
    batch_size: usize,
    lease_duration: Duration,
) -> Result<(), SearchStoreError> {
    validate_nonblank("projection name", projection_name)?;
    validate_nonblank("worker", worker)?;
    if !(1..=MAX_SEARCH_OUTBOX_BATCH).contains(&batch_size) {
        return Err(SearchStoreError::InvalidRequest(format!(
            "batch size must be between 1 and {MAX_SEARCH_OUTBOX_BATCH}"
        )));
    }
    if lease_duration.is_zero() {
        return Err(SearchStoreError::InvalidRequest(
            "lease duration must be positive".to_string(),
        ));
    }
    Ok(())
}

fn parse_outbox_row(row: sqlx::postgres::PgRow) -> Result<SearchOutboxItem, SearchStoreError> {
    let entity_type = SearchEntityType::parse(row.try_get::<String, _>(1)?.as_str())?;
    let operation = SearchOperation::parse(row.try_get::<String, _>(3)?.as_str())?;
    Ok(SearchOutboxItem {
        id: row.try_get(0)?,
        entity_type,
        entity_id: row.try_get(2)?,
        operation,
        entity_revision: row.try_get(4)?,
        attempt_count: row.try_get(5)?,
        lease_expires_at: row.try_get(6)?,
    })
}

// The remaining repository methods and bounded snapshot reader are defined
// below. Keeping the SQL here prevents catalog schema details from leaking
// into the Tantivy application crate.

async fn load_artist(
    pool: &PgPool,
    artist_id: Uuid,
) -> Result<Option<SearchArtistRecord>, SearchStoreError> {
    let row = sqlx::query(ARTIST_SELECT_BY_ID)
        .bind(artist_id)
        .fetch_optional(pool)
        .await?;
    row.map(parse_artist_row).transpose()
}

async fn load_release(
    pool: &PgPool,
    release_id: Uuid,
) -> Result<Option<SearchReleaseRecord>, SearchStoreError> {
    let row = sqlx::query(RELEASE_SELECT_BY_ID)
        .bind(release_id)
        .fetch_optional(pool)
        .await?;
    row.map(parse_release_row).transpose()
}

async fn load_track(
    pool: &PgPool,
    track_id: Uuid,
) -> Result<Option<SearchTrackRecord>, SearchStoreError> {
    let row = sqlx::query(TRACK_SELECT_BY_ID)
        .bind(track_id)
        .fetch_optional(pool)
        .await?;
    row.map(parse_track_row).transpose()
}

const ARTIST_COLUMNS: &str = "
    artist.id, artist.public_api_id, artist.canonical_name, artist.sort_name,
    ARRAY(
        SELECT alias.alias FROM artist_aliases alias
        WHERE alias.artist_id = artist.id
        ORDER BY alias.normalized_alias, alias.id
    ),
    ARRAY(
        SELECT DISTINCT redirect_id
        FROM (
            WITH RECURSIVE redirects(id, public_api_id) AS (
                SELECT merged.id, merged.public_api_id
                FROM artists merged
                WHERE merged.merged_into_artist_id = artist.id
                  AND merged.status = 'merged'
                UNION ALL
                SELECT merged.id, merged.public_api_id
                FROM artists merged
                JOIN redirects source ON merged.merged_into_artist_id = source.id
                WHERE merged.status = 'merged'
            )
            SELECT id AS redirect_id FROM redirects
            UNION
            SELECT public_api_id AS redirect_id FROM redirects
        ) redirects
        ORDER BY redirect_id
    ),
    artist.disambiguation, artist.artist_type, artist.status,
    extract(year FROM artist.begin_date)::bigint,
    extract(year FROM artist.end_date)::bigint,
    artist.revision, artist.updated_at";

const ARTIST_SELECT_BY_ID: &str = "
    SELECT artist.id, artist.public_api_id, artist.canonical_name, artist.sort_name,
           ARRAY(
               SELECT alias.alias FROM artist_aliases alias
               WHERE alias.artist_id = artist.id
               ORDER BY alias.normalized_alias, alias.id
           ),
           ARRAY(
               SELECT DISTINCT redirect_id
               FROM (
            WITH RECURSIVE redirects(id, public_api_id) AS (
                SELECT merged.id, merged.public_api_id
                FROM artists merged
                WHERE merged.merged_into_artist_id = artist.id
                  AND merged.status = 'merged'
                UNION ALL
                SELECT merged.id, merged.public_api_id
                FROM artists merged
                JOIN redirects source ON merged.merged_into_artist_id = source.id
                WHERE merged.status = 'merged'
                   )
                   SELECT id AS redirect_id FROM redirects
                   UNION
                   SELECT public_api_id AS redirect_id FROM redirects
               ) redirects
               ORDER BY redirect_id
           ),
           artist.disambiguation, artist.artist_type, artist.status,
           extract(year FROM artist.begin_date)::bigint,
           extract(year FROM artist.end_date)::bigint,
           artist.revision, artist.updated_at
    FROM artists artist
    WHERE artist.id = $1
      AND artist.status IN ('active', 'verified')";

const RELEASE_SELECT_BY_ID: &str = "
    SELECT release.id, release.public_api_id, release.title, release.sort_title,
           ARRAY(
               SELECT billing.artist_id FROM release_artists billing
               WHERE billing.release_id = release.id
               ORDER BY billing.presentation_order, billing.id
           ),
           ARRAY(
               SELECT billing.artist_id FROM release_artists billing
               WHERE billing.release_id = release.id AND billing.role = 'primary'
               ORDER BY billing.presentation_order, billing.id
           ),
           ARRAY(
               SELECT billing.artist_id FROM release_artists billing
               WHERE billing.release_id = release.id
                 AND billing.role IN ('featured', 'guest')
               ORDER BY billing.presentation_order, billing.id
           ),
           ARRAY(
               SELECT DISTINCT contribution.artist_id
               FROM contributions contribution
               WHERE contribution.release_id = release.id
                 AND contribution.track_id IS NULL
                 AND contribution.is_performance
               ORDER BY contribution.artist_id
           ),
           ARRAY(
               SELECT billing.artist_id FROM release_artists billing
               WHERE billing.release_id = release.id AND billing.role = 'other'
               ORDER BY billing.presentation_order, billing.id
           ),
           COALESCE(release.display_artist, ''),
           ARRAY(
               SELECT DISTINCT term FROM (
                   SELECT trait.trait AS term
                   FROM release_classifications classification
                   JOIN release_traits trait ON trait.classification_id = classification.id
                   WHERE classification.release_id = release.id
                   UNION ALL
                   SELECT genre.display_value AS term
                   FROM catalog_genre_terms genre
                   WHERE genre.release_id = release.id
                     AND genre.surface = 'native'
               ) terms WHERE btrim(term) <> '' ORDER BY term
           ),
           extract(year FROM release.release_date)::bigint,
           (release.release_date - DATE '1970-01-01')::bigint,
           COALESCE((
               SELECT trait.trait
               FROM release_classifications classification
               JOIN release_traits trait ON trait.classification_id = classification.id
               WHERE classification.release_id = release.id
               ORDER BY trait.trait LIMIT 1
           ), 'unknown'),
           release.status, release.revision, release.updated_at
    FROM releases release WHERE release.id = $1";

const TRACK_SELECT_BY_ID: &str = "
    SELECT track.id, track.public_api_id, track.release_id, track.title, track.sort_title,
           ARRAY(
               SELECT DISTINCT artist_id FROM (
                   SELECT billing.artist_id
                   FROM track_artists billing
                   WHERE billing.track_id = track.id AND billing.role = 'primary'
                   UNION ALL
                   SELECT billing.artist_id
                   FROM release_artists billing
                   WHERE billing.release_id = track.release_id AND billing.role = 'primary'
               ) artist_values ORDER BY artist_id
           ),
           ARRAY(
               SELECT DISTINCT artist_id FROM (
                   SELECT billing.artist_id
                   FROM track_artists billing
                   WHERE billing.track_id = track.id AND billing.role IN ('featured', 'guest')
                   UNION ALL
                   SELECT billing.artist_id
                   FROM release_artists billing
                   WHERE billing.release_id = track.release_id
                     AND billing.role IN ('featured', 'guest')
               ) artist_values ORDER BY artist_id
           ),
           ARRAY(
               SELECT DISTINCT contribution.artist_id
               FROM contributions contribution
               WHERE contribution.release_id = track.release_id
                 AND (contribution.track_id IS NULL OR contribution.track_id = track.id)
                 AND contribution.is_performance
                ORDER BY contribution.artist_id
           ),
           ARRAY(
               SELECT DISTINCT other_artist_id
               FROM (
                   SELECT association.artist_id AS other_artist_id
                   FROM artist_track_associations association
                   WHERE association.track_id = track.id
                     AND association.artist_id NOT IN (
                         SELECT billing.artist_id FROM track_artists billing
                         WHERE billing.track_id = track.id
                         UNION
                         SELECT contribution.artist_id FROM contributions contribution
                         WHERE contribution.release_id = track.release_id
                           AND (contribution.track_id IS NULL OR contribution.track_id = track.id)
                           AND contribution.is_performance
                     )
                   UNION
                   SELECT billing.artist_id AS other_artist_id
                   FROM track_artists billing
                   WHERE billing.track_id = track.id
                     AND billing.role = 'other'
                   UNION
                   SELECT billing.artist_id AS other_artist_id
                   FROM release_artists billing
                   WHERE billing.release_id = track.release_id
                     AND billing.role = 'other'
               ) other_values
               ORDER BY other_artist_id
           ),
           ARRAY(
               SELECT DISTINCT association.artist_id
               FROM artist_track_associations association
               WHERE association.track_id = track.id
               ORDER BY association.artist_id
           ),
           COALESCE(track.display_artist, (
               SELECT string_agg(billing.display_value, '' ORDER BY billing.presentation_order)
               FROM track_artists billing WHERE billing.track_id = track.id
           ), ''),
           ARRAY(
               SELECT DISTINCT term FROM (
                   SELECT trait.trait AS term
                   FROM release_classifications classification
                   JOIN release_traits trait ON trait.classification_id = classification.id
                   WHERE classification.release_id = track.release_id
                   UNION ALL
                   SELECT genre.display_value AS term
                   FROM catalog_genre_terms genre
                   WHERE genre.track_id = track.id
                     AND genre.surface = 'native'
               ) terms WHERE btrim(term) <> '' ORDER BY term
           ),
           track.track_number::bigint, track.duration_ms,
           extract(year FROM release.release_date)::bigint,
           (release.release_date - DATE '1970-01-01')::bigint,
           track.revision, track.updated_at
    FROM tracks track
    JOIN releases release ON release.id = track.release_id
    WHERE track.id = $1";

fn parse_artist_row(row: sqlx::postgres::PgRow) -> Result<SearchArtistRecord, SearchStoreError> {
    Ok(SearchArtistRecord {
        internal_id: row.try_get(0)?,
        public_id: row.try_get(1)?,
        display_name: row.try_get(2)?,
        sort_name: row.try_get(3)?,
        aliases: row.try_get(4)?,
        redirect_ids: row.try_get(5)?,
        disambiguation: row.try_get(6)?,
        artist_type: row.try_get(7)?,
        status: row.try_get(8)?,
        begin_year: row.try_get(9)?,
        end_year: row.try_get(10)?,
        revision: row.try_get(11)?,
        updated_at: row.try_get(12)?,
    })
}

fn parse_release_row(row: sqlx::postgres::PgRow) -> Result<SearchReleaseRecord, SearchStoreError> {
    Ok(SearchReleaseRecord {
        internal_id: row.try_get(0)?,
        public_id: row.try_get(1)?,
        title: row.try_get(2)?,
        sort_title: row.try_get(3)?,
        artist_ids: row.try_get(4)?,
        primary_artist_ids: row.try_get(5)?,
        featured_artist_ids: row.try_get(6)?,
        performer_artist_ids: row.try_get(7)?,
        other_artist_ids: row.try_get(8)?,
        artist_display: row.try_get(9)?,
        genre_tags: row.try_get(10)?,
        release_year: row.try_get(11)?,
        release_date_days: row.try_get(12)?,
        release_type: row.try_get(13)?,
        status: row.try_get(14)?,
        revision: row.try_get(15)?,
        updated_at: row.try_get(16)?,
    })
}

fn parse_track_row(row: sqlx::postgres::PgRow) -> Result<SearchTrackRecord, SearchStoreError> {
    Ok(SearchTrackRecord {
        internal_id: row.try_get(0)?,
        public_id: row.try_get(1)?,
        release_id: row.try_get(2)?,
        title: row.try_get(3)?,
        sort_title: row.try_get(4)?,
        primary_artist_ids: row.try_get(5)?,
        featured_artist_ids: row.try_get(6)?,
        performer_artist_ids: row.try_get(7)?,
        other_artist_ids: row.try_get(8)?,
        participant_artist_ids: row.try_get(9)?,
        display_credits: row.try_get(10)?,
        genre_tags: row.try_get(11)?,
        track_number: row.try_get(12)?,
        duration_ms: row.try_get(13)?,
        release_year: row.try_get(14)?,
        release_date_days: row.try_get(15)?,
        revision: row.try_get(16)?,
        updated_at: row.try_get(17)?,
    })
}

/// Repeatable-read snapshot held while a generation streams bounded pages.
pub struct SearchSnapshotReader {
    connection: PoolConnection<Postgres>,
    source_high_water_mark: i64,
    finished: bool,
}

impl SearchSnapshotReader {
    pub async fn begin(pool: &PgPool) -> Result<Self, SearchStoreError> {
        let mut connection = pool.acquire().await?;
        sqlx::query("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *connection)
            .await?;
        let source_high_water_mark = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(id) FROM catalog_projection_outbox WHERE projection_name = $1",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .fetch_one(&mut *connection)
        .await?
        .unwrap_or(0);
        Ok(Self {
            connection,
            source_high_water_mark,
            finished: false,
        })
    }

    #[must_use]
    pub const fn source_high_water_mark(&self) -> i64 {
        self.source_high_water_mark
    }

    pub async fn artists_page(
        &mut self,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<SearchArtistRecord>, SearchStoreError> {
        validate_snapshot_limit(limit)?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "SELECT {ARTIST_COLUMNS} FROM artists artist
             WHERE ($1::uuid IS NULL OR artist.id > $1)
               AND artist.status IN ('active', 'verified')
             ORDER BY artist.id LIMIT $2"
        )))
        .bind(after)
        .bind(i64::try_from(limit).map_err(|_| {
            SearchStoreError::InvalidRequest("snapshot limit is too large".to_string())
        })?)
        .fetch_all(&mut *self.connection)
        .await?;
        rows.into_iter().map(parse_artist_row).collect()
    }

    pub async fn releases_page(
        &mut self,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<SearchReleaseRecord>, SearchStoreError> {
        validate_snapshot_limit(limit)?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM releases
             WHERE ($1::uuid IS NULL OR id > $1)
             ORDER BY id LIMIT $2",
        )
        .bind(after)
        .bind(i64::try_from(limit).map_err(|_| {
            SearchStoreError::InvalidRequest("snapshot limit is too large".to_string())
        })?)
        .fetch_all(&mut *self.connection)
        .await?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Both fragments are compile-time constants owned by this module;
        // no caller input enters the SQL text.
        let rows = sqlx::query(AssertSqlSafe(RELEASE_SELECT_BY_ID.replace(
            "release.id = $1",
            "release.id = ANY($1) ORDER BY release.id",
        )))
        .bind(&ids)
        .fetch_all(&mut *self.connection)
        .await?;
        rows.into_iter().map(parse_release_row).collect()
    }

    pub async fn tracks_page(
        &mut self,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<SearchTrackRecord>, SearchStoreError> {
        validate_snapshot_limit(limit)?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tracks
             WHERE ($1::uuid IS NULL OR id > $1)
             ORDER BY id LIMIT $2",
        )
        .bind(after)
        .bind(i64::try_from(limit).map_err(|_| {
            SearchStoreError::InvalidRequest("snapshot limit is too large".to_string())
        })?)
        .fetch_all(&mut *self.connection)
        .await?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Both fragments are compile-time constants owned by this module;
        // no caller input enters the SQL text.
        let rows = sqlx::query(AssertSqlSafe(
            TRACK_SELECT_BY_ID.replace("track.id = $1", "track.id = ANY($1) ORDER BY track.id"),
        ))
        .bind(&ids)
        .fetch_all(&mut *self.connection)
        .await?;
        rows.into_iter().map(parse_track_row).collect()
    }

    pub async fn finish(mut self) -> Result<(), SearchStoreError> {
        sqlx::query("COMMIT").execute(&mut *self.connection).await?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for SearchSnapshotReader {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                source_high_water_mark = self.source_high_water_mark,
                "search snapshot dropped before explicit completion; SQLx will roll it back"
            );
        }
    }
}

fn validate_snapshot_limit(limit: usize) -> Result<(), SearchStoreError> {
    if !(1..=MAX_SEARCH_SNAPSHOT_PAGE).contains(&limit) {
        return Err(SearchStoreError::InvalidRequest(format!(
            "snapshot page must be between 1 and {MAX_SEARCH_SNAPSHOT_PAGE}"
        )));
    }
    Ok(())
}

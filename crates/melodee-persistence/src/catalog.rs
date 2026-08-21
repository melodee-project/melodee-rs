//! Phase 4 PostgreSQL catalog boundary (ADR-0117 and ADR-0118).
//!
//! Cataloging consumes only the durable manifest-v3/plan/receipt contract. It
//! never opens source or canonical media. One fenced PostgreSQL transaction
//! inserts the normalized catalog, semantic overlay, audit/outbox evidence,
//! and `Cataloged` work-item transition.

use std::collections::BTreeMap;

use melodee_domain::{
    ArtistId, AudioIdentityId, CatalogImportId, CatalogSemanticOverlay, CatalogSemanticOverlayId,
    ContributionRole, DisplayArtist, DisplayArtistOwner, FailureClass, IngestionState,
    ManifestPublication, NewCatalogSemanticOverlay, ObservedArtistId, OverlayArtistResolution,
    OverlayAudioIdentityLink, OverlayDisplayArtist, OverlayReferences, ParticipationKind,
    PublicationPlan, ReleaseArtistId, ReleaseId, ReleaseManifest, ReleaseWorkItem, RetryPolicy,
    TrackArtistId, TrackId, WorkErrorCode, WorkEventType,
};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use thiserror::Error;
use time::{Date, Month, OffsetDateTime};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::config::WorkEngineConfig;
use crate::error::WorkStoreError;
use crate::work_items::{
    WorkItemRow, WorkItemStore, ensure_active_lease, insert_event, lock_for_update,
};
use crate::worker::WorkerId;

/// Maximum Releases whose immutable semantic overlays one correction may refresh.
///
/// Larger identity changes must be decomposed by an explicit maintenance workflow so
/// ordinary catalog mutations retain a fixed memory and transaction-size boundary.
pub const MAX_CATALOG_CORRECTION_RELEASES: usize = 1_000;

/// Maximum Tracks whose projections one identity correction may rewrite atomically.
pub const MAX_CATALOG_CORRECTION_TRACKS: usize = 10_000;

/// Maximum explicitly selected rows accepted by one Artist split.
pub const MAX_CATALOG_SPLIT_ASSIGNMENTS: usize = 10_000;

/// Maximum source-owned identity rows one merge may retarget atomically.
pub const MAX_CATALOG_CORRECTION_IDENTITY_ROWS: usize = 50_000;

/// Maximum normalized reference rows loaded to rebuild bounded overlay snapshots.
pub const MAX_CATALOG_CORRECTION_OVERLAY_REFERENCES: usize = 100_000;

/// Bounded correction dimension reported to operators when maintenance is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogCorrectionDimension {
    Releases,
    Tracks,
    SplitAssignments,
    IdentityRows,
    OverlayReferences,
}

impl std::fmt::Display for CatalogCorrectionDimension {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Releases => "affected Releases",
            Self::Tracks => "affected Tracks",
            Self::SplitAssignments => "selected split assignments",
            Self::IdentityRows => "source-owned identity rows",
            Self::OverlayReferences => "semantic overlay reference rows",
        })
    }
}

/// Typed catalog persistence failures with SQL/manifest root causes retained.
#[derive(Debug, Error)]
pub enum CatalogStoreError {
    #[error("catalog database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("catalog manifest or overlay serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("catalog domain invariant failed")]
    Domain(#[from] melodee_domain::DomainInvariantError),
    #[error("catalog work-item operation failed")]
    Work(#[from] WorkStoreError),
    #[error("catalog work-item transition is invalid")]
    InvalidTransition(#[from] melodee_domain::InvalidStateTransition),
    #[error("catalog request is invalid or exceeds a bounded operation limit")]
    InvalidRequest,
    #[error(
        "catalog correction exceeds the {dimension} limit of {limit}; narrow the operation or use a bounded maintenance workflow"
    )]
    CorrectionScopeExceeded {
        dimension: CatalogCorrectionDimension,
        limit: usize,
    },
    #[error("work item {0} has an invalid or conflicting manifest-v3 catalog binding")]
    ImportConflict(Uuid),
    #[error("manifest-v3 Track mapping is incomplete for {0}")]
    IncompleteTrackMapping(String),
    #[error("catalog value exceeds the PostgreSQL representation")]
    NumericOverflow,
    #[error("persisted catalog role is unknown: {0}")]
    CorruptRole(String),
    #[error("a locked manual identity decision prevents this automated update")]
    LockedDecision,
    #[error("a durable negative assertion suppresses this identity candidate")]
    NegativeAssertion,
    #[error("the requested merge, split, or legacy mapping conflicts with persisted catalog data")]
    MutationConflict,
}

/// Legacy catalog entity namespace for immutable public-ID mapping evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyCatalogEntityType {
    Artist,
    Release,
    Track,
}

/// One reconciled legacy numeric/internal/public identity tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyCatalogIdMapping {
    pub migration_run_id: Uuid,
    pub entity_type: LegacyCatalogEntityType,
    pub legacy_numeric_id: i64,
    pub internal_id: Uuid,
    pub public_api_id: Uuid,
    pub source_snapshot: String,
    pub imported_at: OffsetDateTime,
}

/// Negative candidate keyed by a provider namespace when no local Artist exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderNegativeAssertion {
    pub id: Uuid,
    pub observed_artist_id: ObservedArtistId,
    pub provider: String,
    pub external_id: String,
    pub authority: melodee_domain::DecisionAuthority,
    pub locked: bool,
    pub reason: String,
    pub decided_at: OffsetDateTime,
}

/// Versioned local resolution inputs for one manifest-v3 import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogImportRequest {
    pub resolver_version: String,
    pub actor: String,
}

impl CatalogImportRequest {
    #[must_use]
    pub fn new(resolver_version: impl Into<String>, actor: impl Into<String>) -> Self {
        Self {
            resolver_version: resolver_version.into(),
            actor: actor.into(),
        }
    }

    fn validate(&self) -> Result<(), CatalogStoreError> {
        if self.resolver_version.trim().is_empty() || self.actor.trim().is_empty() {
            Err(CatalogStoreError::InvalidRequest)
        } else {
            Ok(())
        }
    }
}

/// Durable result of an atomic catalog import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogImportResult {
    pub catalog_import_id: CatalogImportId,
    pub release_id: ReleaseId,
    pub overlay_id: CatalogSemanticOverlayId,
    pub track_count: u32,
    pub artist_count: u32,
    pub media_asset_count: u32,
    /// Constant set-based semantic mutation statements, excluding reads,
    /// transaction control, audit/outbox, and work-item completion statements.
    pub set_based_mutation_statement_count: u32,
    /// False when an identical uncertain-commit retry found the durable result.
    pub inserted: bool,
}

/// Stored exact-once import receipt for diagnostics and retry comparison.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StoredCatalogImport {
    pub id: Uuid,
    pub work_item_id: Uuid,
    pub release_id: Uuid,
    pub manifest_row_id: i64,
    pub manifest_id: Uuid,
    pub manifest_schema_version: i32,
    pub semantic_manifest_hash: String,
    pub publication_plan_hash: String,
    pub sealed_manifest_hash: String,
    pub resolver_version: String,
    pub imported_at: OffsetDateTime,
}

/// Small catalog diagnostics summary; no whole-catalog rows are loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CatalogCounts {
    pub artists: i64,
    pub releases: i64,
    pub tracks: i64,
    pub media_assets: i64,
    pub imports: i64,
    pub pending_outbox: i64,
}

/// Query result from the rebuildable Artist/Track participation projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtistTrackAssociation {
    pub artist_id: ArtistId,
    pub track_id: TrackId,
    pub role: ContributionRole,
    pub participation: ParticipationKind,
}

/// Bounded identity candidate with the evidence attribute that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtistCandidate {
    pub artist_id: ArtistId,
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub evidence_kind: String,
    pub evidence_value: String,
    pub confidence_basis_points: u16,
}

/// Stateless PostgreSQL catalog repository.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogStore;

/// A Cataloged work item eligible for policy evaluation, with its catalog
/// release ID resolved from the import receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEligibleWorkItem {
    pub item: ReleaseWorkItem,
    pub release_id: Uuid,
}

impl CatalogStore {
    /// Claims Published work and directly reclaims abandoned Cataloging work.
    #[instrument(skip_all, fields(worker_id = %worker, batch = config.claim_batch_size))]
    pub async fn claim_cataloging(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<ReleaseWorkItem>, CatalogStoreError> {
        let lease_expiry = now + config.lease_duration;
        let mut tx = pool.begin().await?;
        let candidates = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT w.id, w.state
             FROM release_work_items w
             WHERE w.state IN ('published', 'cataloging')
               AND (w.lease_expires_at IS NULL OR w.lease_expires_at <= $1)
               AND (w.next_attempt_at IS NULL OR w.next_attempt_at <= $1)
               AND EXISTS (
                   SELECT 1 FROM release_publication_receipts r
                   WHERE r.work_item_id = w.id
               )
             ORDER BY w.updated_at, w.id
             LIMIT $2
             FOR UPDATE OF w SKIP LOCKED",
        )
        .bind(now)
        .bind(i64::from(config.claim_batch_size))
        .fetch_all(&mut *tx)
        .await?;

        let mut claimed = Vec::with_capacity(candidates.len());
        for (id, persisted_state) in candidates {
            let from = IngestionState::parse(&persisted_state)
                .ok_or_else(|| WorkStoreError::CorruptState(persisted_state.clone()))?;
            if from == IngestionState::Published {
                from.ensure_transition(
                    IngestionState::Cataloging,
                    melodee_domain::TransitionKind::Worker,
                )?;
            }
            let row = sqlx::query_as::<_, WorkItemRow>(
                "UPDATE release_work_items
                 SET state = 'cataloging', state_reason = $2,
                     lease_owner = $3, lease_acquired_at = $4,
                     lease_expires_at = $5, next_attempt_at = NULL, updated_at = $4
                 WHERE id = $1
                 RETURNING *",
            )
            .bind(id)
            .bind(if from == IngestionState::Published {
                "catalog.import_started"
            } else {
                "catalog.import_reclaimed"
            })
            .bind(worker.as_str())
            .bind(now)
            .bind(lease_expiry)
            .fetch_one(&mut *tx)
            .await?;
            let item = ReleaseWorkItem::try_from(row)?;
            if from == IngestionState::Published {
                insert_event(
                    &mut tx,
                    id,
                    WorkEventType::Transitioned,
                    Some(IngestionState::Published),
                    Some(IngestionState::Cataloging),
                    Some("catalog.import_started"),
                    Some(worker.as_str()),
                    item.correlation_id,
                    now,
                    None,
                )
                .await?;
            }
            insert_event(
                &mut tx,
                id,
                WorkEventType::Claimed,
                None,
                None,
                Some(if from == IngestionState::Published {
                    "catalog.claimed"
                } else {
                    "catalog.reclaimed"
                }),
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({ "lease_expires_at": lease_expiry.to_string() })),
            )
            .await?;
            claimed.push(item);
        }
        tx.commit().await?;
        Ok(claimed)
    }

    /// Reads the immutable import receipt by work-item identity.
    pub async fn get_import(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Option<StoredCatalogImport>, CatalogStoreError> {
        Ok(sqlx::query_as::<_, StoredCatalogImport>(
            "SELECT id, work_item_id, release_id, manifest_row_id, manifest_id,
                    manifest_schema_version, semantic_manifest_hash,
                    publication_plan_hash, sealed_manifest_hash, resolver_version,
                    imported_at
             FROM catalog_imports WHERE work_item_id = $1",
        )
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?)
    }

    /// Resolves catalog `release_id` bindings for a bounded set of work
    /// items in one statement (Phase 10 review-queue display). Items without
    /// an import receipt are simply absent from the result.
    pub async fn list_import_release_ids(
        &self,
        pool: &PgPool,
        work_item_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Uuid)>, CatalogStoreError> {
        if work_item_ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT work_item_id, release_id
             FROM catalog_imports WHERE work_item_id = ANY($1)",
        )
        .bind(work_item_ids)
        .fetch_all(pool)
        .await?)
    }

    /// Claims up to `batch_size` work items in the `Cataloged` state that are
    /// eligible for policy evaluation, establishing leases and returning each
    /// item alongside its catalog `release_id`.
    ///
    /// Returns only work actually owned by this worker.
    #[instrument(
        skip_all,
        fields(worker_id = %worker, batch = config.claim_batch_size)
    )]
    pub async fn claim_policy_eligible(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<PolicyEligibleWorkItem>, CatalogStoreError> {
        let lease_expiry = now + config.lease_duration;
        let mut tx = pool.begin().await?;
        let candidates: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT w.id, ci.release_id
             FROM release_work_items w
             JOIN catalog_imports ci ON ci.work_item_id = w.id
             WHERE w.state = 'cataloged'
               AND (w.lease_expires_at IS NULL OR w.lease_expires_at <= $1)
               AND (w.next_attempt_at IS NULL OR w.next_attempt_at <= $1)
             ORDER BY w.updated_at, w.id
             LIMIT $2
             FOR UPDATE OF w SKIP LOCKED",
        )
        .bind(now)
        .bind(i64::from(config.claim_batch_size))
        .fetch_all(&mut *tx)
        .await?;

        let mut claimed = Vec::with_capacity(candidates.len());
        for (work_item_id, release_id) in candidates {
            let release_id = match release_id {
                Some(id) => id,
                None => {
                    warn!(%work_item_id, "cataloged work item has no release_id, skipping");
                    continue;
                }
            };
            let row = sqlx::query_as::<_, WorkItemRow>(
                "UPDATE release_work_items
                 SET lease_owner = $1, lease_acquired_at = $2,
                     lease_expires_at = $3, updated_at = $2
                 WHERE id = $4
                 RETURNING *",
            )
            .bind(worker.as_str())
            .bind(now)
            .bind(lease_expiry)
            .bind(work_item_id)
            .fetch_one(&mut *tx)
            .await?;
            let item = ReleaseWorkItem::try_from(row)?;
            insert_event(
                &mut tx,
                item.id,
                WorkEventType::Claimed,
                None,
                None,
                Some("policy.eligible_claimed"),
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({ "lease_expires_at": lease_expiry.to_string() })),
            )
            .await?;
            claimed.push(PolicyEligibleWorkItem { item, release_id });
        }
        tx.commit().await?;
        Ok(claimed)
    }

    /// Returns bounded aggregate counts for operational diagnostics.
    pub async fn counts(&self, pool: &PgPool) -> Result<CatalogCounts, CatalogStoreError> {
        let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64)>(
            "SELECT
                (SELECT count(*) FROM artists),
                (SELECT count(*) FROM releases),
                (SELECT count(*) FROM tracks),
                (SELECT count(*) FROM media_assets),
                (SELECT count(*) FROM catalog_imports),
                (SELECT count(*) FROM catalog_projection_outbox WHERE processed_at IS NULL)",
        )
        .fetch_one(pool)
        .await?;
        Ok(CatalogCounts {
            artists: row.0,
            releases: row.1,
            tracks: row.2,
            media_assets: row.3,
            imports: row.4,
            pending_outbox: row.5,
        })
    }

    /// Records a fenced catalog failure and its retry/permanent disposition.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_failure(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        worker: &WorkerId,
        code: WorkErrorCode,
        message: &str,
        class: FailureClass,
        policy: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, CatalogStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, work_item_id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Cataloging {
            return Err(WorkStoreError::InvalidState(work_item_id, item.state).into());
        }
        let attempt_count = item.attempt_count.saturating_add(1);
        let retry_at = if class == FailureClass::Retryable && policy.can_retry(attempt_count) {
            policy.next_attempt_at(attempt_count, now)
        } else {
            None
        };
        let new_state = if retry_at.is_some() {
            IngestionState::Cataloging
        } else {
            IngestionState::Failed
        };
        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = $2, state_reason = $3, attempt_count = $4,
                 last_attempt_at = $5, last_error_code = $6,
                 last_error_message = $3, next_attempt_at = $7,
                 lease_owner = NULL, lease_acquired_at = NULL,
                 lease_expires_at = NULL, updated_at = $5
             WHERE id = $1 AND state = 'cataloging' AND lease_owner = $8
             RETURNING *",
        )
        .bind(work_item_id)
        .bind(new_state.as_str())
        .bind(message)
        .bind(i32::try_from(attempt_count).unwrap_or(i32::MAX))
        .bind(now)
        .bind(code.as_str())
        .bind(retry_at)
        .bind(worker.as_str())
        .fetch_one(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            work_item_id,
            WorkEventType::Failed,
            Some(IngestionState::Cataloging),
            Some(new_state),
            Some(code.as_str()),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "attempt_count": attempt_count,
                "failure_class": class,
                "catalog_retry": retry_at.is_some(),
            })),
        )
        .await?;
        if let Some(next) = retry_at {
            insert_event(
                &mut tx,
                work_item_id,
                WorkEventType::RetryScheduled,
                None,
                None,
                Some(code.as_str()),
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({
                    "attempt_count": attempt_count,
                    "next_attempt_at": next.to_string(),
                    "resume_state": "cataloging",
                })),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(ReleaseWorkItem::try_from(row)?)
    }

    /// Renews Cataloging ownership for a bounded long-running import task.
    pub async fn renew_lease(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        worker: &WorkerId,
        lease_duration: time::Duration,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, CatalogStoreError> {
        WorkItemStore
            .renew_lease(pool, work_item_id, worker, lease_duration, now)
            .await?;
        Ok(WorkItemStore.get(pool, work_item_id).await?)
    }

    /// Releases Cataloging ownership when application cancellation wins.
    pub async fn release_lease(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        Ok(WorkItemStore
            .release_lease(pool, work_item_id, worker, now)
            .await?)
    }

    /// Imports one manifest-v3 Release atomically using set-based statements.
    #[instrument(skip_all, fields(work_item_id = %work_item_id, worker_id = %worker))]
    pub async fn import_manifest_v3(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        worker: &WorkerId,
        request: &CatalogImportRequest,
        now: OffsetDateTime,
    ) -> Result<CatalogImportResult, CatalogStoreError> {
        request.validate()?;
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, work_item_id).await?;

        if item.state == IngestionState::Cataloged {
            let existing = load_existing_result(&mut tx, work_item_id, request).await?;
            tx.commit().await?;
            return existing.ok_or(CatalogStoreError::ImportConflict(work_item_id));
        }
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Cataloging {
            return Err(WorkStoreError::InvalidState(work_item_id, item.state).into());
        }

        let binding = load_manifest_binding(&mut tx, work_item_id).await?;
        let prepared = PreparedImport::build(work_item_id, binding, request, now)?;
        let result = persist_prepared_import(&mut tx, &prepared, &item, worker, now).await?;
        tx.commit().await?;
        info!(work_item_id = %work_item_id, release_id = %result.release_id, "manifest v3 cataloged");
        Ok(result)
    }

    /// Default performance participation or the explicit all-credit view.
    pub async fn list_artist_participation(
        &self,
        pool: &PgPool,
        artist_id: ArtistId,
        include_all_credits: bool,
        limit: u32,
    ) -> Result<Vec<StoredArtistTrackAssociation>, CatalogStoreError> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, Option<Uuid>, Option<Uuid>)>(
            "SELECT artist_id, track_id, association_type, participation_kind,
                    source_artist_id,
                    CASE WHEN participation_kind = 'inferred_group' THEN source_key ELSE NULL END
             FROM artist_track_associations
             WHERE artist_id = $1 AND ($2 OR is_performance)
             ORDER BY track_id, association_type, source_key
             LIMIT $3",
        )
        .bind(artist_id.into_uuid())
        .bind(include_all_credits)
        .bind(i64::from(limit.clamp(1, 10_000)))
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(stored_association).collect()
    }
}

// Import preparation and persistence helpers follow below. Keeping them
// private prevents protocol/application DTOs from becoming the SQL contract.

struct ManifestBinding {
    manifest_row_id: i64,
    schema_version: i32,
    semantic_hash: String,
    payload: serde_json::Value,
    receipt_release_id: Uuid,
    plan_hash: String,
    plan_payload: serde_json::Value,
    sealed_manifest_hash: String,
}

#[derive(Clone)]
struct PreparedTrack {
    id: Uuid,
    public_api_id: Uuid,
    audio_identity_id: Uuid,
    ordinal: i32,
    title: String,
    normalized_title: String,
    display_artist: Option<String>,
    disc_number: Option<i32>,
    track_number: Option<i32>,
    duration_ms: i64,
    source_path: String,
    media_asset_id: Uuid,
}

#[derive(Clone)]
struct PreparedAsset {
    id: Uuid,
    canonical_relative_path: String,
    byte_hash: String,
    byte_size: i64,
    container: String,
    codec: String,
    sample_rate_hz: i32,
    channels: i32,
    bitrate_bps: Option<i64>,
    duration_ms: i64,
}

#[derive(Clone)]
struct PreparedResolution {
    artist_id: Uuid,
    observation_id: Uuid,
    evidence_id: Uuid,
    decision_id: Uuid,
    presentation_id: Uuid,
    track_id: Option<Uuid>,
    source_ordinal: i32,
    raw_value: String,
    normalized_value: String,
    source_pointer: String,
    source_payload: serde_json::Value,
}

struct PreparedImport {
    work_item_id: Uuid,
    manifest_row_id: i64,
    manifest_id: Uuid,
    semantic_hash: String,
    plan_hash: String,
    sealed_manifest_hash: String,
    release_id: ReleaseId,
    release_title: String,
    normalized_release_title: String,
    release_display_artist: Option<String>,
    release_date: Option<Date>,
    release_date_text: Option<String>,
    import_id: CatalogImportId,
    overlay: CatalogSemanticOverlay,
    resolver_version: String,
    actor: String,
    tracks: Vec<PreparedTrack>,
    assets: Vec<PreparedAsset>,
    resolutions: Vec<PreparedResolution>,
}

impl PreparedImport {
    fn build(
        work_item_id: Uuid,
        binding: ManifestBinding,
        request: &CatalogImportRequest,
        now: OffsetDateTime,
    ) -> Result<Self, CatalogStoreError> {
        if binding.schema_version != i32::from(melodee_domain::PUBLICATION_MANIFEST_VERSION) {
            return Err(CatalogStoreError::ImportConflict(work_item_id));
        }
        let manifest: ReleaseManifest = serde_json::from_value(binding.payload)?;
        let plan: PublicationPlan = serde_json::from_value(binding.plan_payload)?;
        let publication: &ManifestPublication = manifest
            .publication
            .as_ref()
            .ok_or(CatalogStoreError::ImportConflict(work_item_id))?;
        plan.validate()
            .map_err(|_| CatalogStoreError::ImportConflict(work_item_id))?;
        publication
            .validate_against(&plan)
            .map_err(|_| CatalogStoreError::ImportConflict(work_item_id))?;
        if manifest.work_item_id != work_item_id
            || publication.release_id.into_uuid() != binding.receipt_release_id
            || publication.release_id != plan.release_id
            || publication.plan_hash != binding.plan_hash
            || plan.input_manifest_hash != binding.semantic_hash
            || plan
                .plan_hash()
                .map_err(|_| CatalogStoreError::ImportConflict(work_item_id))?
                != binding.plan_hash
        {
            return Err(CatalogStoreError::ImportConflict(work_item_id));
        }
        let normalization = manifest
            .normalization
            .as_ref()
            .ok_or(CatalogStoreError::ImportConflict(work_item_id))?;
        let release_title = manifest
            .release_observation
            .title
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or(CatalogStoreError::ImportConflict(work_item_id))?;
        let release_artists = manifest
            .release_observation
            .artists
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>();
        if release_artists.is_empty() {
            return Err(CatalogStoreError::ImportConflict(work_item_id));
        }
        // The manifest field is a sorted union of observations, not an exact
        // billing string. Only a singleton is safe presentation evidence.
        let release_display_artist =
            (release_artists.len() == 1).then(|| release_artists[0].clone());

        // ADR-0146: join on the logical track key so CUE-governed segment
        // outputs (`<source>#cue-track-NN`) match their Tracks and canonical
        // asset `source_path` values; whole-file outputs keep the plain
        // source path as their key.
        let outputs = normalization
            .outputs
            .iter()
            .map(|output| (output.track_key(), output))
            .collect::<BTreeMap<_, _>>();
        let mut source_assets = BTreeMap::new();
        let mut prepared_assets = BTreeMap::<String, PreparedAsset>::new();
        for asset in plan
            .assets
            .iter()
            .filter(|asset| asset.kind == melodee_domain::CanonicalAssetKind::Media)
        {
            let output = outputs.get(asset.source_path.as_str()).ok_or_else(|| {
                CatalogStoreError::IncompleteTrackMapping(asset.source_path.clone())
            })?;
            let id = prepared_assets
                .get(&asset.canonical_path)
                .map_or_else(Uuid::now_v7, |existing| existing.id);
            let prepared = PreparedAsset {
                id,
                canonical_relative_path: format!(
                    "{}/{}",
                    plan.target_relative_path, asset.canonical_path
                ),
                byte_hash: asset.byte_hash.clone(),
                byte_size: i64::try_from(asset.size_bytes)
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                container: output.container.clone(),
                codec: output.codec.clone(),
                sample_rate_hz: i32::try_from(output.sample_rate_hz)
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                channels: i32::try_from(output.channels)
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                bitrate_bps: output
                    .bitrate_bps
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                duration_ms: i64::try_from(output.duration_ms)
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
            };
            if let Some(existing) = prepared_assets.get(&asset.canonical_path)
                && (existing.byte_hash != prepared.byte_hash
                    || existing.byte_size != prepared.byte_size)
            {
                return Err(CatalogStoreError::ImportConflict(work_item_id));
            }
            prepared_assets.insert(asset.canonical_path.clone(), prepared);
            if source_assets
                .insert(asset.source_path.as_str(), id)
                .is_some()
            {
                return Err(CatalogStoreError::IncompleteTrackMapping(
                    asset.source_path.clone(),
                ));
            }
        }

        let mut tracks = Vec::with_capacity(manifest.tracks.len());
        for (index, track) in manifest.tracks.iter().enumerate() {
            let output = outputs.get(track.file_path.as_str()).ok_or_else(|| {
                CatalogStoreError::IncompleteTrackMapping(track.file_path.clone())
            })?;
            let media_asset_id = source_assets
                .get(track.file_path.as_str())
                .copied()
                .ok_or_else(|| {
                    CatalogStoreError::IncompleteTrackMapping(track.file_path.clone())
                })?;
            let title = track
                .track_title
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    CatalogStoreError::IncompleteTrackMapping(track.file_path.clone())
                })?;
            let id = Uuid::now_v7();
            tracks.push(PreparedTrack {
                id,
                public_api_id: id,
                audio_identity_id: Uuid::now_v7(),
                ordinal: i32::try_from(index + 1)
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                title: title.clone(),
                normalized_title: normalize_lookup(&title),
                display_artist: track
                    .observed_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.artist.clone())
                    .filter(|value| !value.trim().is_empty()),
                disc_number: track
                    .media_number
                    .map(i32::try_from)
                    .transpose()
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                track_number: track
                    .track_number
                    .map(i32::try_from)
                    .transpose()
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                duration_ms: i64::try_from(output.duration_ms)
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                source_path: track.file_path.clone(),
                media_asset_id,
            });
        }
        if tracks.is_empty() {
            return Err(CatalogStoreError::ImportConflict(work_item_id));
        }

        let mut resolutions = Vec::new();
        for (index, raw) in release_artists.iter().enumerate() {
            resolutions.push(PreparedResolution::new(
                None,
                i32::try_from(index + 1).map_err(|_| CatalogStoreError::NumericOverflow)?,
                raw,
                format!("/release_observation/artists/{index}"),
                serde_json::json!({ "value": raw }),
            ));
        }
        for (index, (manifest_track, prepared_track)) in
            manifest.tracks.iter().zip(tracks.iter()).enumerate()
        {
            if let Some(raw) = manifest_track
                .observed_metadata
                .as_ref()
                .and_then(|metadata| metadata.artist.as_ref())
                .filter(|value| !value.trim().is_empty())
            {
                resolutions.push(PreparedResolution::new(
                    Some(prepared_track.id),
                    1,
                    raw,
                    format!("/tracks/{index}/observed_metadata/artist"),
                    serde_json::json!({ "value": raw, "source_file": manifest_track.file_path }),
                ));
            }
        }

        let import_id = CatalogImportId::new();
        let overlay_id = CatalogSemanticOverlayId::new();
        let release_id = publication.release_id;
        let artist_resolutions = resolutions
            .iter()
            .map(|resolution| OverlayArtistResolution {
                observed_artist_id: ObservedArtistId::from_uuid(resolution.observation_id),
                artist_id: ArtistId::from_uuid(resolution.artist_id),
                decision_id: Some(melodee_domain::ArtistMatchDecisionId::from_uuid(
                    resolution.decision_id,
                )),
            })
            .collect();
        let release_artist_ids = resolutions
            .iter()
            .filter(|resolution| resolution.track_id.is_none())
            .map(|resolution| ReleaseArtistId::from_uuid(resolution.presentation_id))
            .collect();
        let track_artist_ids = resolutions
            .iter()
            .filter(|resolution| resolution.track_id.is_some())
            .map(|resolution| TrackArtistId::from_uuid(resolution.presentation_id))
            .collect();
        let audio_identities = tracks
            .iter()
            .map(|track| OverlayAudioIdentityLink {
                track_id: TrackId::from_uuid(track.id),
                audio_identity_id: AudioIdentityId::from_uuid(track.audio_identity_id),
            })
            .collect();
        let mut display_artists = Vec::new();
        if let Some(display) = &release_display_artist {
            display_artists.push(OverlayDisplayArtist {
                owner: DisplayArtistOwner::Release(release_id),
                value: DisplayArtist::new(display.clone())
                    .map_err(|_| CatalogStoreError::ImportConflict(work_item_id))?,
            });
        }
        for track in &tracks {
            if let Some(display) = &track.display_artist {
                display_artists.push(OverlayDisplayArtist {
                    owner: DisplayArtistOwner::Track(TrackId::from_uuid(track.id)),
                    value: DisplayArtist::new(display.clone())
                        .map_err(|_| CatalogStoreError::ImportConflict(work_item_id))?,
                });
            }
        }
        let overlay = CatalogSemanticOverlay::new(NewCatalogSemanticOverlay {
            id: overlay_id,
            release_id,
            source_manifest_id: manifest.manifest_id,
            source_semantic_manifest_hash: binding.semantic_hash.clone(),
            publication_plan_hash: binding.plan_hash.clone(),
            catalog_import_id: import_id,
            resolver_version: request.resolver_version.clone(),
            predecessor_id: None,
            references: OverlayReferences {
                artist_resolutions,
                release_artists: release_artist_ids,
                track_artists: track_artist_ids,
                contributions: Vec::new(),
                audio_identities,
                display_artists,
            },
            created_at: now,
            created_by: request.actor.clone(),
        })
        .map_err(|_| CatalogStoreError::ImportConflict(work_item_id))?;

        let (release_date, release_date_text) =
            parse_observed_date(manifest.release_observation.date.as_deref());
        Ok(Self {
            work_item_id,
            manifest_row_id: binding.manifest_row_id,
            manifest_id: manifest.manifest_id,
            semantic_hash: binding.semantic_hash,
            plan_hash: binding.plan_hash,
            sealed_manifest_hash: binding.sealed_manifest_hash,
            release_id,
            release_title: release_title.clone(),
            normalized_release_title: normalize_lookup(&release_title),
            release_display_artist,
            release_date,
            release_date_text,
            import_id,
            overlay,
            resolver_version: request.resolver_version.clone(),
            actor: request.actor.clone(),
            tracks,
            assets: prepared_assets.into_values().collect(),
            resolutions,
        })
    }
}

impl PreparedResolution {
    fn new(
        track_id: Option<Uuid>,
        source_ordinal: i32,
        raw: &str,
        source_pointer: String,
        source_payload: serde_json::Value,
    ) -> Self {
        Self {
            artist_id: Uuid::now_v7(),
            observation_id: Uuid::now_v7(),
            evidence_id: Uuid::now_v7(),
            decision_id: Uuid::now_v7(),
            presentation_id: Uuid::now_v7(),
            track_id,
            source_ordinal,
            raw_value: raw.to_owned(),
            normalized_value: normalize_lookup(raw),
            source_pointer,
            source_payload,
        }
    }
}

async fn load_manifest_binding(
    tx: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
) -> Result<ManifestBinding, CatalogStoreError> {
    let row = sqlx::query_as::<
        _,
        (
            i64,
            i32,
            String,
            serde_json::Value,
            Uuid,
            String,
            serde_json::Value,
            String,
        ),
    >(
        "SELECT m.id, m.schema_version, m.manifest_hash, m.manifest_payload,
                r.release_id, p.plan_hash, p.plan_payload, r.sealed_manifest_hash
         FROM release_work_item_manifests m
         JOIN release_publication_plans p ON p.work_item_id = m.work_item_id
         JOIN release_publication_receipts r ON r.work_item_id = p.work_item_id
         WHERE m.work_item_id = $1
         FOR SHARE OF m, p, r",
    )
    .bind(work_item_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(CatalogStoreError::ImportConflict(work_item_id))?;
    Ok(ManifestBinding {
        manifest_row_id: row.0,
        schema_version: row.1,
        semantic_hash: row.2,
        payload: row.3,
        receipt_release_id: row.4,
        plan_hash: row.5,
        plan_payload: row.6,
        sealed_manifest_hash: row.7,
    })
}

fn normalize_lookup(value: &str) -> String {
    value.trim().to_lowercase()
}

fn parse_observed_date(value: Option<&str>) -> (Option<Date>, Option<String>) {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return (None, None);
    };
    let text = value.to_owned();
    let mut parts = value.split('-');
    let date = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(year), Some(month), Some(day), None) => year
            .parse::<i32>()
            .ok()
            .zip(month.parse::<u8>().ok())
            .zip(day.parse::<u8>().ok())
            .and_then(|((year, month), day)| {
                Month::try_from(month)
                    .ok()
                    .and_then(|month| Date::from_calendar_date(year, month, day).ok())
            }),
        _ => None,
    };
    (date, Some(text))
}

const SET_BASED_MUTATION_STATEMENT_COUNT: u32 = 18;

async fn persist_prepared_import(
    tx: &mut Transaction<'_, Postgres>,
    prepared: &PreparedImport,
    item: &ReleaseWorkItem,
    worker: &WorkerId,
    now: OffsetDateTime,
) -> Result<CatalogImportResult, CatalogStoreError> {
    item.state.ensure_transition(
        IngestionState::Cataloged,
        melodee_domain::TransitionKind::Worker,
    )?;

    sqlx::query(
        "INSERT INTO releases (
            id, public_api_id, title, normalized_title, sort_title,
            display_artist, release_date, release_date_text, status,
            revision, created_at, updated_at
         ) VALUES ($1,$1,$2,$3,$2,$4,$5,$6,'active',1,$7,$7)",
    )
    .bind(prepared.release_id.into_uuid())
    .bind(&prepared.release_title)
    .bind(&prepared.normalized_release_title)
    .bind(&prepared.release_display_artist)
    .bind(prepared.release_date)
    .bind(&prepared.release_date_text)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let mut audio_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO audio_identities (id, status, revision, created_at, updated_at) ",
    );
    audio_query.push_values(&prepared.tracks, |mut row, track| {
        row.push_bind(track.audio_identity_id)
            .push_bind("provisional")
            .push_bind(1_i64)
            .push_bind(now)
            .push_bind(now);
    });
    audio_query.build().execute(&mut **tx).await?;

    let release_id = prepared.release_id.into_uuid();
    let mut track_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO tracks (
            id, public_api_id, release_id, audio_identity_id,
            playable_media_asset_id, ordinal,
            title, normalized_title, sort_title, display_artist, disc_number,
            track_number, duration_ms, source_path, revision, created_at, updated_at
         ) ",
    );
    track_query.push_values(&prepared.tracks, |mut row, track| {
        row.push_bind(track.id)
            .push_bind(track.public_api_id)
            .push_bind(release_id)
            .push_bind(track.audio_identity_id)
            .push_bind(track.media_asset_id)
            .push_bind(track.ordinal)
            .push_bind(&track.title)
            .push_bind(&track.normalized_title)
            .push_bind(&track.title)
            .push_bind(&track.display_artist)
            .push_bind(track.disc_number)
            .push_bind(track.track_number)
            .push_bind(track.duration_ms)
            .push_bind(&track.source_path)
            .push_bind(1_i64)
            .push_bind(now)
            .push_bind(now);
    });
    track_query.build().execute(&mut **tx).await?;

    let mut asset_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO media_assets (
            id, canonical_relative_path, byte_hash, byte_size, mime_type,
            container, codec, sample_rate_hz, channels, bitrate_bps,
            duration_ms, created_at
         ) ",
    );
    asset_query.push_values(&prepared.assets, |mut row, asset| {
        row.push_bind(asset.id)
            .push_bind(&asset.canonical_relative_path)
            .push_bind(&asset.byte_hash)
            .push_bind(asset.byte_size)
            .push_bind("audio/mpeg")
            .push_bind(&asset.container)
            .push_bind(&asset.codec)
            .push_bind(asset.sample_rate_hz)
            .push_bind(asset.channels)
            .push_bind(asset.bitrate_bps)
            .push_bind(asset.duration_ms)
            .push_bind(now);
    });
    asset_query.build().execute(&mut **tx).await?;

    let mut link_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO track_media_assets (
            track_id, media_asset_id, purpose, ordinal, created_at
         ) ",
    );
    link_query.push_values(&prepared.tracks, |mut row, track| {
        row.push_bind(track.id)
            .push_bind(track.media_asset_id)
            .push_bind("playable")
            .push_bind(1_i32)
            .push_bind(now);
    });
    link_query.build().execute(&mut **tx).await?;

    let mut artist_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO artists (
            id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, status, revision, created_at, updated_at
         ) ",
    );
    artist_query.push_values(&prepared.resolutions, |mut row, resolution| {
        row.push_bind(resolution.artist_id)
            .push_bind(resolution.artist_id)
            .push_bind(&resolution.raw_value)
            .push_bind(&resolution.normalized_value)
            .push_bind(&resolution.raw_value)
            .push_bind("unknown")
            .push_bind("provisional")
            .push_bind(1_i64)
            .push_bind(now)
            .push_bind(now);
    });
    artist_query.build().execute(&mut **tx).await?;

    let mut observation_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO observed_artists (
            id, release_id, track_id, scope, source_ordinal, raw_value,
            normalized_value, display_value, source_pointer, source_payload,
            created_at
         ) ",
    );
    observation_query.push_values(&prepared.resolutions, |mut row, resolution| {
        row.push_bind(resolution.observation_id)
            .push_bind(release_id)
            .push_bind(resolution.track_id)
            .push_bind(if resolution.track_id.is_some() {
                "track"
            } else {
                "release"
            })
            .push_bind(resolution.source_ordinal)
            .push_bind(&resolution.raw_value)
            .push_bind(&resolution.normalized_value)
            .push_bind(&resolution.raw_value)
            .push_bind(&resolution.source_pointer)
            .push_bind(&resolution.source_payload)
            .push_bind(now);
    });
    observation_query.build().execute(&mut **tx).await?;

    let mut evidence_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO artist_match_evidence (
            id, observed_artist_id, candidate_artist_id, evidence_kind,
            polarity, confidence, source, payload, created_at
         ) ",
    );
    evidence_query.push_values(&prepared.resolutions, |mut row, resolution| {
        row.push_bind(resolution.evidence_id)
            .push_bind(resolution.observation_id)
            .push_bind(resolution.artist_id)
            .push_bind("release_context")
            .push_bind("supports")
            .push_bind(6000_i32)
            .push_bind("manifest-v3")
            .push_bind(serde_json::json!({
                "source_pointer": resolution.source_pointer,
                "provisional_bias": true
            }))
            .push_bind(now);
    });
    evidence_query.build().execute(&mut **tx).await?;

    let mut decision_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO artist_match_decisions (
            id, observed_artist_id, artist_id, outcome, authority,
            confidence, reason, is_locked, payload, created_at
         ) ",
    );
    decision_query.push_values(&prepared.resolutions, |mut row, resolution| {
        row.push_bind(resolution.decision_id)
            .push_bind(resolution.observation_id)
            .push_bind(resolution.artist_id)
            .push_bind("confirmed")
            .push_bind("automated")
            .push_bind(6000_i32)
            .push_bind("created a distinct provisional identity from manifest context")
            .push_bind(false)
            .push_bind(serde_json::json!({
                "resolver_version": prepared.resolver_version,
                "evidence_ids": [resolution.evidence_id]
            }))
            .push_bind(now);
    });
    decision_query.build().execute(&mut **tx).await?;

    let mut current_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO artist_match_current (observed_artist_id, decision_id, updated_at) ",
    );
    current_query.push_values(&prepared.resolutions, |mut row, resolution| {
        row.push_bind(resolution.observation_id)
            .push_bind(resolution.decision_id)
            .push_bind(now);
    });
    current_query.build().execute(&mut **tx).await?;

    let release_resolutions = prepared
        .resolutions
        .iter()
        .filter(|resolution| resolution.track_id.is_none())
        .collect::<Vec<_>>();
    let mut release_artist_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO release_artists (
            id, release_id, artist_id, observed_artist_id, role,
            presentation_order, display_value, join_phrase, created_at
         ) ",
    );
    release_artist_query.push_values(release_resolutions, |mut row, resolution| {
        row.push_bind(resolution.presentation_id)
            .push_bind(release_id)
            .push_bind(resolution.artist_id)
            .push_bind(resolution.observation_id)
            .push_bind("primary")
            .push_bind(resolution.source_ordinal)
            .push_bind(&resolution.raw_value)
            .push_bind("")
            .push_bind(now);
    });
    release_artist_query.build().execute(&mut **tx).await?;

    let track_resolutions = prepared
        .resolutions
        .iter()
        .filter(|resolution| resolution.track_id.is_some())
        .collect::<Vec<_>>();
    if !track_resolutions.is_empty() {
        let mut track_artist_query: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO track_artists (
                id, track_id, artist_id, observed_artist_id, role,
                presentation_order, display_value, join_phrase, created_at
             ) ",
        );
        track_artist_query.push_values(track_resolutions, |mut row, resolution| {
            row.push_bind(resolution.presentation_id)
                .push_bind(resolution.track_id)
                .push_bind(resolution.artist_id)
                .push_bind(resolution.observation_id)
                .push_bind("primary")
                .push_bind(resolution.source_ordinal)
                .push_bind(&resolution.raw_value)
                .push_bind("")
                .push_bind(now);
        });
        track_artist_query.build().execute(&mut **tx).await?;
    }

    // Direct Release billing applies to each Track; direct Track billing is
    // additional structured evidence. Rows remain source-distinct.
    let mut association_rows = Vec::new();
    for resolution in prepared
        .resolutions
        .iter()
        .filter(|resolution| resolution.track_id.is_none())
    {
        for track in &prepared.tracks {
            association_rows.push((resolution, track.id));
        }
    }
    for resolution in prepared
        .resolutions
        .iter()
        .filter(|resolution| resolution.track_id.is_some())
    {
        if let Some(track_id) = resolution.track_id {
            association_rows.push((resolution, track_id));
        }
    }
    let mut association_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO artist_track_associations (
            artist_id, track_id, association_type, participation_kind,
            source_key, confidence, is_performance, created_at
         ) ",
    );
    association_query.push_values(association_rows, |mut row, (resolution, track_id)| {
        row.push_bind(resolution.artist_id)
            .push_bind(track_id)
            .push_bind("primary_artist")
            .push_bind("direct")
            .push_bind(resolution.presentation_id)
            .push_bind(6000_i32)
            .push_bind(true)
            .push_bind(now);
    });
    association_query.build().execute(&mut **tx).await?;

    auto_complete_native_requests(tx, release_id, now).await?;

    sqlx::query(
        "INSERT INTO catalog_imports (
            id, work_item_id, release_id, manifest_row_id, manifest_id,
            manifest_schema_version, semantic_manifest_hash,
            publication_plan_hash, sealed_manifest_hash, resolver_version,
            imported_at
         ) VALUES ($1,$2,$3,$4,$5,3,$6,$7,$8,$9,$10)",
    )
    .bind(prepared.import_id.into_uuid())
    .bind(prepared.work_item_id)
    .bind(release_id)
    .bind(prepared.manifest_row_id)
    .bind(prepared.manifest_id)
    .bind(&prepared.semantic_hash)
    .bind(&prepared.plan_hash)
    .bind(&prepared.sealed_manifest_hash)
    .bind(&prepared.resolver_version)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let overlay_hash = prepared.overlay.overlay_hash()?;
    let overlay_payload = serde_json::to_value(&prepared.overlay)?;
    sqlx::query(
        "INSERT INTO catalog_semantic_overlays (
            id, release_id, catalog_import_id, predecessor_overlay_id,
            schema_name, schema_version, source_manifest_id,
            source_semantic_manifest_hash, publication_plan_hash,
            resolver_version, payload, overlay_hash, created_at
         ) VALUES ($1,$2,$3,NULL,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
    )
    .bind(prepared.overlay.id().into_uuid())
    .bind(release_id)
    .bind(prepared.import_id.into_uuid())
    .bind(CatalogSemanticOverlay::SCHEMA_NAME)
    .bind(i32::from(CatalogSemanticOverlay::SCHEMA_VERSION))
    .bind(prepared.manifest_id)
    .bind(&prepared.semantic_hash)
    .bind(&prepared.plan_hash)
    .bind(&prepared.resolver_version)
    .bind(overlay_payload)
    .bind(&overlay_hash)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    sqlx::query("UPDATE releases SET current_overlay_id = $2 WHERE id = $1")
        .bind(release_id)
        .bind(prepared.overlay.id().into_uuid())
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT INTO release_catalog_aggregates (
            release_id, track_count, duration_ms, media_bytes, revision, updated_at
         )
         SELECT $1, count(DISTINCT t.id), COALESCE(sum(t.duration_ms), 0),
                COALESCE((
                    SELECT sum(distinct_assets.byte_size)
                    FROM (
                        SELECT DISTINCT ma.id, ma.byte_size
                        FROM tracks it
                        JOIN track_media_assets tma ON tma.track_id = it.id
                        JOIN media_assets ma ON ma.id = tma.media_asset_id
                        WHERE it.release_id = $1
                    ) distinct_assets
                ), 0), 1, $2
         FROM tracks t WHERE t.release_id = $1",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "INSERT INTO artist_catalog_aggregates (
            artist_id, release_count, billed_track_count,
            performance_track_count, all_credit_track_count, revision, updated_at
         )
         SELECT a.artist_id, count(DISTINCT t.release_id),
                count(DISTINCT a.track_id),
                count(DISTINCT a.track_id) FILTER (WHERE a.is_performance),
                count(DISTINCT a.track_id), 1, $2
         FROM artist_track_associations a
         JOIN tracks t ON t.id = a.track_id
         WHERE t.release_id = $1
         GROUP BY a.artist_id",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let mut outbox_rows = vec![("release", release_id)];
    outbox_rows.extend(prepared.tracks.iter().map(|track| ("track", track.id)));
    outbox_rows.extend(
        prepared
            .resolutions
            .iter()
            .map(|resolution| ("artist", resolution.artist_id)),
    );
    let mut outbox_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, created_at
         ) ",
    );
    outbox_query.push_values(outbox_rows, |mut row, (entity_type, entity_id)| {
        row.push_bind("tantivy.catalog")
            .push_bind(entity_type)
            .push_bind(entity_id)
            .push_bind("upsert")
            .push_bind(1_i64)
            .push_bind(now)
            .push_bind(0_i32)
            .push_bind(now);
    });
    outbox_query.build().execute(&mut **tx).await?;

    sqlx::query(
        "INSERT INTO catalog_audit_events (
            event_id, entity_type, entity_id, event_type, actor, reason,
            payload, created_at
         ) VALUES ($1,'release',$2,'manifest_v3_imported',$3,
                   'atomic set-based catalog import',$4,$5)",
    )
    .bind(Uuid::now_v7())
    .bind(release_id)
    .bind(&prepared.actor)
    .bind(serde_json::json!({
        "work_item_id": prepared.work_item_id,
        "catalog_import_id": prepared.import_id,
        "overlay_id": prepared.overlay.id(),
        "manifest_id": prepared.manifest_id,
        "plan_hash": prepared.plan_hash,
    }))
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let completion = sqlx::query(
        "UPDATE release_work_items
         SET state = 'cataloged', state_reason = 'catalog.import_complete',
             lease_owner = NULL, lease_acquired_at = NULL,
             lease_expires_at = NULL, updated_at = $2
         WHERE id = $1
           AND state = 'cataloging'
           AND lease_owner = $3
           AND lease_expires_at > clock_timestamp()",
    )
    .bind(prepared.work_item_id)
    .bind(now)
    .bind(worker.as_str())
    .execute(&mut **tx)
    .await?;
    if completion.rows_affected() != 1 {
        return Err(WorkStoreError::LeaseExpired(prepared.work_item_id).into());
    }
    insert_event(
        tx,
        prepared.work_item_id,
        WorkEventType::Transitioned,
        Some(IngestionState::Cataloging),
        Some(IngestionState::Cataloged),
        Some("catalog.import_complete"),
        Some(worker.as_str()),
        item.correlation_id,
        now,
        Some(serde_json::json!({
            "release_id": release_id,
            "catalog_import_id": prepared.import_id,
            "overlay_id": prepared.overlay.id(),
            "track_count": prepared.tracks.len(),
            "artist_count": prepared.resolutions.len(),
            "media_asset_count": prepared.assets.len(),
        })),
    )
    .await?;

    Ok(CatalogImportResult {
        catalog_import_id: prepared.import_id,
        release_id: prepared.release_id,
        overlay_id: prepared.overlay.id(),
        track_count: u32::try_from(prepared.tracks.len())
            .map_err(|_| CatalogStoreError::NumericOverflow)?,
        artist_count: u32::try_from(prepared.resolutions.len())
            .map_err(|_| CatalogStoreError::NumericOverflow)?,
        media_asset_count: u32::try_from(prepared.assets.len())
            .map_err(|_| CatalogStoreError::NumericOverflow)?,
        set_based_mutation_statement_count: SET_BASED_MUTATION_STATEMENT_COUNT,
        inserted: true,
    })
}

/// Preserve native-v1 request auto-completion at the catalog commit boundary.
///
/// The frozen server runs this behavior when a Release or Track is added. Keeping
/// it inside the same authoritative transaction prevents a crash window between
/// catalog publication and request completion. Exact public IDs win; the legacy
/// normalized-name fallback is scoped to the one newly cataloged Release and its
/// ID-backed Artist associations.
async fn auto_complete_native_requests(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "WITH primary_artist AS (
             SELECT artist.id, artist.public_api_id, artist.canonical_name
             FROM release_artists billing
             JOIN artists artist ON artist.id = billing.artist_id
             WHERE billing.release_id = $1 AND billing.role = 'primary'
             ORDER BY billing.presentation_order, artist.id
             LIMIT 1
         ), candidate AS (
             SELECT request.id, release.public_api_id AS target_id,
                    release.title AS target_title, artist.canonical_name AS artist_name
             FROM native_requests request
             JOIN releases release ON release.id = $1
             CROSS JOIN primary_artist artist
             WHERE request.category = 1 AND request.status IN (1, 2)
               AND (request.release_year IS NULL
                    OR request.release_year = EXTRACT(year FROM release.release_date)::integer)
               AND (
                    request.target_release_public_api_id = release.public_api_id
                    OR request.target_artist_public_api_id = artist.public_api_id
                    OR (
                        request.target_release_public_api_id IS NULL
                        AND request.target_artist_public_api_id IS NULL
                        AND request.release_title IS NOT NULL
                        AND request.artist_name IS NOT NULL
                        AND lower(btrim(request.release_title)) = lower(btrim(release.title))
                        AND lower(btrim(request.artist_name)) = lower(btrim(artist.canonical_name))
                    )
               )
         ), completed AS (
             UPDATE native_requests request
             SET status = 3,
                 target_release_public_api_id = candidate.target_id,
                 last_activity_at = $2,
                 last_activity_type = 'SystemComment',
                 last_activity_user_id = NULL,
                 updated_at = $2
             FROM candidate
             WHERE request.id = candidate.id AND request.status IN (1, 2)
             RETURNING request.id, candidate.target_id, candidate.target_title,
                       candidate.artist_name
         )
         INSERT INTO native_request_comments (
             id, public_api_id, request_id, parent_comment_id,
             created_by_user_id, body, is_system, created_at
         )
         SELECT completed.id, completed.id, completed.id, NULL, NULL,
                format(
                    E'✅ **Request automatically completed!**\\n\\nThe release **%s** by **%s** has been added to the library.\\n\\n[View Release](/data/release/%s)',
                    completed.target_title, completed.artist_name, completed.target_id
                ),
                true, $2
         FROM completed",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "WITH candidate AS (
             SELECT DISTINCT ON (request.id)
                    request.id, track.public_api_id AS target_id,
                    track.title AS target_title, artist.canonical_name AS artist_name
             FROM native_requests request
             JOIN tracks track ON track.release_id = $1
             JOIN releases release ON release.id = track.release_id
             JOIN artist_track_associations participation
               ON participation.track_id = track.id AND participation.is_performance
             JOIN artists artist ON artist.id = participation.artist_id
             WHERE request.category = 2 AND request.status IN (1, 2)
               AND (request.release_year IS NULL
                    OR request.release_year = EXTRACT(year FROM release.release_date)::integer)
               AND (
                    request.target_track_public_api_id = track.public_api_id
                    OR request.target_artist_public_api_id = artist.public_api_id
                    OR (
                        request.target_track_public_api_id IS NULL
                        AND request.target_artist_public_api_id IS NULL
                        AND request.track_title IS NOT NULL
                        AND request.artist_name IS NOT NULL
                        AND lower(btrim(request.track_title)) = lower(btrim(track.title))
                        AND lower(btrim(request.artist_name)) = lower(btrim(artist.canonical_name))
                    )
               )
             ORDER BY request.id, track.ordinal, artist.id
         ), completed AS (
             UPDATE native_requests request
             SET status = 3,
                 target_track_public_api_id = candidate.target_id,
                 last_activity_at = $2,
                 last_activity_type = 'SystemComment',
                 last_activity_user_id = NULL,
                 updated_at = $2
             FROM candidate
             WHERE request.id = candidate.id AND request.status IN (1, 2)
             RETURNING request.id, candidate.target_id, candidate.target_title,
                       candidate.artist_name
         )
         INSERT INTO native_request_comments (
             id, public_api_id, request_id, parent_comment_id,
             created_by_user_id, body, is_system, created_at
         )
         SELECT completed.id, completed.id, completed.id, NULL, NULL,
                format(
                    E'✅ **Request automatically completed!**\\n\\nThe track **%s** by **%s** has been added to the library.\\n\\n[View Track](/data/track/%s)',
                    completed.target_title, completed.artist_name, completed.target_id
                ),
                true, $2
         FROM completed",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn load_existing_result(
    tx: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
    request: &CatalogImportRequest,
) -> Result<Option<CatalogImportResult>, CatalogStoreError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>, String, i64, i64, i64)>(
        "SELECT ci.id, ci.release_id, r.current_overlay_id, ci.resolver_version,
                (SELECT count(*) FROM tracks t WHERE t.release_id = ci.release_id),
                (SELECT count(*) FROM observed_artists o WHERE o.release_id = ci.release_id),
                (SELECT count(DISTINCT tma.media_asset_id)
                   FROM tracks t
                   JOIN track_media_assets tma ON tma.track_id = t.id
                  WHERE t.release_id = ci.release_id)
         FROM catalog_imports ci
         JOIN releases r ON r.id = ci.release_id
         WHERE ci.work_item_id = $1",
    )
    .bind(work_item_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.3 != request.resolver_version {
        return Err(CatalogStoreError::ImportConflict(work_item_id));
    }
    let overlay_id = row
        .2
        .ok_or(CatalogStoreError::ImportConflict(work_item_id))?;
    Ok(Some(CatalogImportResult {
        catalog_import_id: CatalogImportId::from_uuid(row.0),
        release_id: ReleaseId::from_uuid(row.1),
        overlay_id: CatalogSemanticOverlayId::from_uuid(overlay_id),
        track_count: u32::try_from(row.4).map_err(|_| CatalogStoreError::NumericOverflow)?,
        artist_count: u32::try_from(row.5).map_err(|_| CatalogStoreError::NumericOverflow)?,
        media_asset_count: u32::try_from(row.6).map_err(|_| CatalogStoreError::NumericOverflow)?,
        set_based_mutation_statement_count: SET_BASED_MUTATION_STATEMENT_COUNT,
        inserted: false,
    }))
}

fn stored_association(
    row: (Uuid, Uuid, String, String, Option<Uuid>, Option<Uuid>),
) -> Result<StoredArtistTrackAssociation, CatalogStoreError> {
    let role = parse_contribution_role(&row.2)?;
    let participation = match row.3.as_str() {
        "direct" => ParticipationKind::Direct,
        "inferred_group" => ParticipationKind::InferredGroupParticipation {
            source_group_id: ArtistId::from_uuid(
                row.4
                    .ok_or_else(|| CatalogStoreError::CorruptRole(row.3.clone()))?,
            ),
            membership_id: melodee_domain::GroupMembershipId::from_uuid(
                row.5
                    .ok_or_else(|| CatalogStoreError::CorruptRole(row.3.clone()))?,
            ),
        },
        value => return Err(CatalogStoreError::CorruptRole(value.to_owned())),
    };
    Ok(StoredArtistTrackAssociation {
        artist_id: ArtistId::from_uuid(row.0),
        track_id: TrackId::from_uuid(row.1),
        role,
        participation,
    })
}

fn parse_contribution_role(value: &str) -> Result<ContributionRole, CatalogStoreError> {
    match value {
        "primary_artist" => Ok(ContributionRole::PrimaryArtist),
        "featured_artist" => Ok(ContributionRole::FeaturedArtist),
        "guest_artist" => Ok(ContributionRole::GuestArtist),
        "performer" => Ok(ContributionRole::Performer),
        "band_member" => Ok(ContributionRole::BandMember),
        "vocalist" => Ok(ContributionRole::Vocalist),
        "instrumentalist" => Ok(ContributionRole::Instrumentalist),
        "composer" => Ok(ContributionRole::Composer),
        "lyricist" => Ok(ContributionRole::Lyricist),
        "producer" => Ok(ContributionRole::Producer),
        "remixer" => Ok(ContributionRole::Remixer),
        "engineer" => Ok(ContributionRole::Engineer),
        "other" => Ok(ContributionRole::Other),
        value => Err(CatalogStoreError::CorruptRole(value.to_owned())),
    }
}

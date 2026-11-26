//! PostgreSQL catalog boundary (ADR-0117 and ADR-0118).
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
    OverlayAudioIdentityLink, OverlayDisplayArtist, OverlayReferences, ParsedArtistCandidate,
    ParticipationKind, PresentationRole, PublicationPlan, ReleaseArtistId, ReleaseId,
    ReleaseManifest, ReleaseWorkItem, RetryPolicy, TrackArtistId, TrackId, WorkErrorCode,
    WorkEventType, acoustic_fingerprint_digest, parse_explicit_artist_billing,
    parse_slash_artist_billing,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use thiserror::Error;
use time::{Date, Month, OffsetDateTime};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::catalog_mutations::ensure_correction_scope;
use crate::config::WorkEngineConfig;
use crate::error::WorkStoreError;
use crate::provider_eligibility::schedule_catalog_enrichment;
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

/// Maximum source Releases accepted by one administrative Release merge.
pub const MAX_RELEASE_MERGE_SOURCES: usize = 100;

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
    #[error("provider enrichment scheduling failed")]
    ProviderEnrichment(#[from] crate::provider_enrichment::ProviderEnrichmentStoreError),
    #[error("provider eligibility scheduling failed")]
    ProviderEligibility(#[from] crate::provider_eligibility::ProviderEligibilityError),
    #[error("manifest evidence could not be read: {0}")]
    ManifestEvidence(String),
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

/// One deterministic, evidence-backed Artist convergence candidate. The
/// caller still runs the local resolver before invoking the audited merge
/// mutation; this row only describes durable facts that made a candidate safe
/// to consider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrongArtistMergeCandidate {
    pub source_artist_id: Uuid,
    pub source_public_api_id: Uuid,
    pub target_artist_id: Uuid,
    pub target_public_api_id: Uuid,
    pub provider: String,
    pub external_id: String,
}

/// One exact-name Artist candidate that requires operator review.
///
/// The current active identity is deliberately retained. This row only
/// provides enough information to append immutable name evidence and a
/// non-authoritative proposal for Identity Review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameOnlyArtistMatchProposalCandidate {
    pub observed_artist_id: Uuid,
    pub current_artist_id: Uuid,
    pub candidate_artist_id: Uuid,
    pub normalized_name: String,
}

/// One active Artist that can be consolidated with the oldest
/// indistinguishable primary Release Artist carrying the same exact name.
///
/// This is not a global normalized-name key. The candidate query requires
/// repeated primary billing on different Releases and rejects the complete
/// name group when any identity carries differentiating evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatedPrimaryReleaseArtistMergeCandidate {
    pub source_artist_id: Uuid,
    pub source_public_api_id: Uuid,
    pub target_artist_id: Uuid,
    pub target_public_api_id: Uuid,
    pub normalized_name: String,
}

/// One repeated structured Track credit that can safely reuse another
/// active, track-only Artist identity inside the same immutable Release.
///
/// Release Artist identities and mixed-scope observations never enter this
/// candidate set. That keeps ADR-0173's bounded co-resolution rule separate
/// from global name identity and from ambiguous same-Release billing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameReleaseTrackArtistMergeCandidate {
    pub source_artist_id: Uuid,
    pub source_public_api_id: Uuid,
    pub target_artist_id: Uuid,
    pub target_public_api_id: Uuid,
    pub release_id: Uuid,
    pub normalized_name: String,
}

/// Structured evidence retained with an automated Artist reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtistReconciliationEvidence {
    /// Both active identities carried the same trusted provider key.
    EmbeddedExternalId {
        provider: String,
        external_id: String,
    },
    /// ADR-0173's exact Track credit recurred inside one immutable Release.
    SameReleaseTrackCredit {
        release_id: Uuid,
        normalized_name: String,
    },
    /// The same exact primary Artist credit recurred on different Releases,
    /// and the full active name group had no differentiating identity facts.
    RepeatedPrimaryReleaseCredit { normalized_name: String },
    /// The same exact Artist credit (primary Release credit *or* Track credit)
    /// recurred on different Releases, and the full active name group had no
    /// differentiating identity facts. ADR-0189: C# reuses an Artist by
    /// normalized name at import, so featured-only credits must converge too.
    RepeatedCredit { normalized_name: String },
}

impl ArtistReconciliationEvidence {
    pub(crate) fn validate(&self) -> Result<(), CatalogStoreError> {
        let valid = match self {
            Self::EmbeddedExternalId {
                provider,
                external_id,
            } => !provider.trim().is_empty() && !external_id.trim().is_empty(),
            Self::SameReleaseTrackCredit {
                normalized_name, ..
            } => !normalized_name.trim().is_empty(),
            Self::RepeatedPrimaryReleaseCredit { normalized_name }
            | Self::RepeatedCredit { normalized_name } => !normalized_name.trim().is_empty(),
        };
        if valid {
            Ok(())
        } else {
            Err(CatalogStoreError::InvalidRequest)
        }
    }

    pub(crate) const fn predicate(&self) -> &'static str {
        match self {
            Self::EmbeddedExternalId { .. } => "embedded_external_id",
            Self::SameReleaseTrackCredit { .. } => "same_release_track_credit",
            Self::RepeatedPrimaryReleaseCredit { .. } => "repeated_primary_release_credit",
            Self::RepeatedCredit { .. } => "repeated_credit",
        }
    }

    pub(crate) fn payload(&self) -> serde_json::Value {
        match self {
            Self::EmbeddedExternalId {
                provider,
                external_id,
            } => serde_json::json!({
                "predicate": self.predicate(),
                "provider": provider,
                "external_id": external_id,
            }),
            Self::SameReleaseTrackCredit {
                release_id,
                normalized_name,
            } => serde_json::json!({
                "predicate": self.predicate(),
                "release_id": release_id,
                "normalized_name": normalized_name,
            }),
            Self::RepeatedPrimaryReleaseCredit { normalized_name } => serde_json::json!({
                "predicate": self.predicate(),
                "normalized_name": normalized_name,
                "scope": "different_releases",
                "role": "primary",
                "conflict_gated": true,
            }),
            Self::RepeatedCredit { normalized_name } => serde_json::json!({
                "predicate": self.predicate(),
                "normalized_name": normalized_name,
                "scope": "different_releases",
                "role": "any",
                "conflict_gated": true,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrongReleaseMergeCandidate {
    pub source_release_id: Uuid,
    pub target_release_id: Uuid,
    pub provider: String,
    pub external_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReleaseMovedTrack {
    track_id: Uuid,
    original_ordinal: i32,
    merged_ordinal: i32,
    observations: Vec<ReleaseMovedObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReleaseMovedObservation {
    observation_id: Uuid,
    original_ordinal: i32,
    merged_ordinal: i32,
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

// Re-export the metadata-edit patch contracts so callers outside this crate can
// name them without depending on the private `catalog_mutations` module path.
pub use crate::catalog_mutations::{ArtistMetadataPatch, ReleaseMetadataPatch, TrackMetadataPatch};

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
        let candidates =
            crate::processing::claim_candidates(&mut tx, "catalog", config, now).await?;
        let mut claimed = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let (id, persisted_state) = (candidate.id, candidate.state);
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
    /// items in one statement (review-queue display). Items without
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
        let candidates =
            crate::processing::claim_candidates(&mut tx, "policy", config, now).await?;
        let mut claimed = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let work_item_id = candidate.id;
            let release_id = match candidate.release_id {
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
        if let Some(existing) = load_existing_release_result(
            &mut tx,
            binding.receipt_release_id,
            &binding.semantic_hash,
            request,
        )
        .await?
        {
            let completion = sqlx::query(
                "UPDATE release_work_items
                 SET state = 'cataloged', state_reason = 'catalog.import_existing_release',
                     lease_owner = NULL, lease_acquired_at = NULL,
                     lease_expires_at = NULL, updated_at = $2
                 WHERE id = $1 AND state = 'cataloging'
                   AND lease_owner = $3 AND lease_expires_at > clock_timestamp()",
            )
            .bind(work_item_id)
            .bind(now)
            .bind(worker.as_str())
            .execute(&mut *tx)
            .await?;
            if completion.rows_affected() != 1 {
                return Err(WorkStoreError::LeaseExpired(work_item_id).into());
            }
            insert_event(
                &mut tx,
                work_item_id,
                WorkEventType::Transitioned,
                Some(IngestionState::Cataloging),
                Some(IngestionState::Cataloged),
                Some("catalog.import_existing_release"),
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({
                    "release_id": binding.receipt_release_id,
                    "existing_catalog_import_id": existing.catalog_import_id,
                })),
            )
            .await?;
            tx.commit().await?;
            return Ok(existing);
        }
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

    /// Returns a bounded, deterministic set of duplicate Artist identities
    /// sharing strong embedded evidence. Name similarity is intentionally not
    /// part of this query, and locked/negative decisions suppress candidates.
    pub async fn strong_artist_merge_candidates(
        &self,
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<StrongArtistMergeCandidate>, CatalogStoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(CatalogStoreError::InvalidRequest);
        }
        Ok(sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, String, String)>(
            "WITH distinct_evidence AS (
                 SELECT DISTINCT evidence.provider, evidence.external_id, evidence.artist_id
                 FROM artist_external_identities evidence
                 JOIN artists artist ON artist.id=evidence.artist_id
                 WHERE evidence.provider='musicbrainz'
                   AND evidence.confidence >= 9000
                   AND evidence.external_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                   AND artist.status NOT IN ('merged','deprecated')
             ), groups AS (
                 SELECT provider, external_id
                 FROM distinct_evidence
                 GROUP BY provider, external_id
                 HAVING count(*) > 1
             )
             SELECT source_artist.id, source_artist.public_api_id,
                    target_artist.id, target_artist.public_api_id,
                    groups.provider, groups.external_id
             FROM groups
             JOIN distinct_evidence source
               ON source.provider=groups.provider AND source.external_id=groups.external_id
             JOIN distinct_evidence target
               ON target.provider=groups.provider AND target.external_id=groups.external_id
              AND source.artist_id < target.artist_id
             JOIN artists source_artist ON source_artist.id=source.artist_id
             JOIN artists target_artist ON target_artist.id=target.artist_id
             WHERE NOT EXISTS (
                 SELECT 1 FROM artist_match_decisions locked
                 WHERE locked.artist_id IN (source.artist_id, target.artist_id)
                   AND locked.is_locked
             )
             AND NOT (
                 source_artist.artist_type <> target_artist.artist_type
                 AND source_artist.artist_type <> 'unknown'
                 AND target_artist.artist_type <> 'unknown'
             )
             AND NOT (
                 source_artist.end_date IS NOT NULL
                 AND target_artist.begin_date IS NOT NULL
                 AND source_artist.end_date < target_artist.begin_date
                 OR target_artist.end_date IS NOT NULL
                 AND source_artist.begin_date IS NOT NULL
                 AND target_artist.end_date < source_artist.begin_date
             )
             AND NOT EXISTS (
                 SELECT 1
                 FROM artist_external_identities source_conflict
                 JOIN artist_external_identities target_conflict
                   ON target_conflict.provider=source_conflict.provider
                  AND target_conflict.external_id <> source_conflict.external_id
                 WHERE source_conflict.artist_id=source.artist_id
                   AND target_conflict.artist_id=target.artist_id
             )
             AND NOT (
                 EXISTS (
                     SELECT 1 FROM group_memberships source_membership
                     WHERE source_membership.group_artist_id=source.artist_id
                 )
                 AND EXISTS (
                     SELECT 1 FROM group_memberships target_membership
                     WHERE target_membership.group_artist_id=target.artist_id
                 )
                 AND NOT EXISTS (
                     SELECT 1
                     FROM group_memberships source_membership
                     JOIN group_memberships target_membership
                       ON resolve_artist_redirect(target_membership.member_artist_id)
                          = resolve_artist_redirect(source_membership.member_artist_id)
                     WHERE source_membership.group_artist_id=source.artist_id
                       AND target_membership.group_artist_id=target.artist_id
                 )
             )
             AND NOT EXISTS (
                 SELECT 1 FROM artist_negative_assertions negative
                 WHERE negative.candidate_provider=groups.provider
                   AND negative.candidate_external_id=groups.external_id
                   AND negative.assertion_kind='do_not_match'
             )
             ORDER BY groups.provider, groups.external_id, source.artist_id, target.artist_id
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                provider,
                external_id,
            )| StrongArtistMergeCandidate {
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                provider,
                external_id,
            },
        )
        .collect())
    }

    /// Returns bounded ADR-0173 candidates for repeated structured Track
    /// credits inside one immutable Release.
    ///
    /// Every identity in a candidate group is active (unverified), resolves only
    /// Track-scoped observations from one Release, and has no Release Artist
    /// link. Mixed-scope identities, locked decisions, active do-not-match
    /// assertions, and same-provider/different-ID evidence block the group.
    pub async fn same_release_track_artist_merge_candidates(
        &self,
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<SameReleaseTrackArtistMergeCandidate>, CatalogStoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(CatalogStoreError::InvalidRequest);
        }
        Ok(sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, Uuid, String)>(
            "WITH eligible_releases AS (
                 SELECT release.id
                 FROM releases release
                 JOIN release_artists release_credit
                   ON release_credit.release_id=release.id
                 JOIN artists release_artist
                   ON release_artist.id=release_credit.artist_id
                 WHERE release.status='active'
                 GROUP BY release.id
                 HAVING count(DISTINCT release_credit.artist_id)
                            FILTER (WHERE release_credit.role='primary')=1
                    AND count(*) FILTER (
                            WHERE release_credit.role='primary'
                              AND release_artist.normalized_name IN (
                                  'various artists', 'various', 'va'
                              ))=0
                    AND NOT EXISTS (
                        SELECT 1
                        FROM release_classifications classification
                        JOIN release_traits trait
                          ON trait.classification_id=classification.id
                        WHERE classification.release_id=release.id
                          AND trait.trait='compilation'
                    )
                    AND NOT EXISTS (
                        SELECT 1
                        FROM release_catalog_trait_assertions assertion
                        WHERE assertion.release_id=release.id
                          AND assertion.trait='compilation'
                    )
             ), duplicate_names AS (
                 SELECT normalized_name
                 FROM artists
                 WHERE status='active'
                 GROUP BY normalized_name
                 HAVING count(*) > 1
             ), track_only_identities AS (
                 SELECT artist.id, artist.public_api_id, artist.normalized_name,
                        min(observed.release_id::text)::uuid AS release_id
                 FROM duplicate_names duplicate
                 JOIN artists artist ON artist.normalized_name=duplicate.normalized_name
                 JOIN artist_match_decisions decision ON decision.artist_id=artist.id
                 JOIN artist_match_current current
                   ON current.decision_id=decision.id
                  AND current.observed_artist_id=decision.observed_artist_id
                 JOIN observed_artists observed ON observed.id=current.observed_artist_id
                 JOIN eligible_releases eligible ON eligible.id=observed.release_id
                 LEFT JOIN track_artists track_credit
                   ON track_credit.artist_id=artist.id
                  AND track_credit.observed_artist_id=observed.id
                 LEFT JOIN release_artists release_credit
                   ON release_credit.artist_id=artist.id
                 WHERE artist.status='active'
                   AND btrim(observed.normalized_value) <> ''
                 GROUP BY artist.id, artist.public_api_id, artist.normalized_name
                 HAVING count(DISTINCT observed.normalized_value)=1
                    AND min(observed.normalized_value)=artist.normalized_name
                    AND count(DISTINCT observed.release_id)=1
                    AND bool_and(observed.scope='track')
                    AND count(DISTINCT observed.id)
                        = count(DISTINCT track_credit.observed_artist_id)
                    AND count(DISTINCT release_credit.release_id)=0
             ), candidate_groups AS (
                 SELECT normalized_name, release_id
                 FROM track_only_identities
                 GROUP BY normalized_name, release_id
                 HAVING count(*) > 1
             ), blocked_groups AS (
                 SELECT DISTINCT candidate_group.normalized_name,
                                 candidate_group.release_id
                 FROM candidate_groups candidate_group
                 JOIN artists same_name
                   ON same_name.normalized_name=candidate_group.normalized_name
                  AND same_name.status NOT IN ('merged','deprecated')
                 JOIN artist_match_decisions same_decision
                   ON same_decision.artist_id=same_name.id
                 JOIN artist_match_current same_current
                   ON same_current.decision_id=same_decision.id
                  AND same_current.observed_artist_id=same_decision.observed_artist_id
                 JOIN observed_artists same_observed
                   ON same_observed.id=same_current.observed_artist_id
                  AND same_observed.release_id=candidate_group.release_id
                 LEFT JOIN track_only_identities qualified
                   ON qualified.id=same_name.id
                  AND qualified.normalized_name=candidate_group.normalized_name
                  AND qualified.release_id=candidate_group.release_id
                 WHERE qualified.id IS NULL
                    OR same_name.status <> 'active'
                    OR same_decision.is_locked
                    OR EXISTS (
                        SELECT 1
                        FROM artist_negative_assertions negative
                        JOIN artist_match_current owner_current
                          ON owner_current.observed_artist_id=negative.observed_artist_id
                        JOIN artist_match_decisions owner
                          ON owner.id=owner_current.decision_id
                        JOIN track_only_identities other
                          ON other.id=negative.candidate_artist_id
                         AND other.normalized_name=candidate_group.normalized_name
                         AND other.release_id=candidate_group.release_id
                        WHERE owner.artist_id=same_name.id
                          AND negative.assertion_kind='do_not_match'
                          AND NOT EXISTS (
                              SELECT 1 FROM artist_negative_assertions revoked
                              WHERE revoked.predecessor_assertion_id=negative.id
                                AND revoked.assertion_kind='revoke'
                          )
                    )
             ), conflicting_external_groups AS (
                 SELECT DISTINCT left_identity.normalized_name,
                                 left_identity.release_id
                 FROM track_only_identities left_identity
                 JOIN track_only_identities right_identity
                   ON right_identity.normalized_name=left_identity.normalized_name
                  AND right_identity.release_id=left_identity.release_id
                  AND right_identity.id > left_identity.id
                 JOIN artist_external_identities left_external
                   ON left_external.artist_id=left_identity.id
                 JOIN artist_external_identities right_external
                  ON right_external.artist_id=right_identity.id
                  AND right_external.provider=left_external.provider
                  AND right_external.external_id <> left_external.external_id
             ), conflicting_attribute_groups AS (
                 SELECT DISTINCT left_identity.normalized_name,
                                 left_identity.release_id
                 FROM track_only_identities left_identity
                 JOIN track_only_identities right_identity
                   ON right_identity.normalized_name=left_identity.normalized_name
                  AND right_identity.release_id=left_identity.release_id
                  AND right_identity.id > left_identity.id
                 JOIN artists left_artist ON left_artist.id=left_identity.id
                 JOIN artists right_artist ON right_artist.id=right_identity.id
                 WHERE (
                     left_artist.artist_type <> right_artist.artist_type
                     AND left_artist.artist_type <> 'unknown'
                     AND right_artist.artist_type <> 'unknown'
                 )
                    OR (
                        left_artist.end_date IS NOT NULL
                        AND right_artist.begin_date IS NOT NULL
                        AND left_artist.end_date < right_artist.begin_date
                    )
                    OR (
                        right_artist.end_date IS NOT NULL
                        AND left_artist.begin_date IS NOT NULL
                        AND right_artist.end_date < left_artist.begin_date
                    )
                    OR (
                        EXISTS (
                            SELECT 1 FROM group_memberships left_membership
                            WHERE left_membership.group_artist_id=left_identity.id
                        )
                        AND EXISTS (
                            SELECT 1 FROM group_memberships right_membership
                            WHERE right_membership.group_artist_id=right_identity.id
                        )
                        AND NOT EXISTS (
                            SELECT 1
                            FROM group_memberships left_membership
                            JOIN group_memberships right_membership
                              ON resolve_artist_redirect(right_membership.member_artist_id)
                                 = resolve_artist_redirect(left_membership.member_artist_id)
                            WHERE left_membership.group_artist_id=left_identity.id
                              AND right_membership.group_artist_id=right_identity.id
                        )
                    )
             ), ranked AS (
                 SELECT candidate.*,
                        first_value(candidate.id) OVER (
                            PARTITION BY candidate.normalized_name, candidate.release_id
                            ORDER BY candidate.id
                        ) AS target_artist_id
                 FROM track_only_identities candidate
                 JOIN candidate_groups candidate_group
                   ON candidate_group.normalized_name=candidate.normalized_name
                  AND candidate_group.release_id=candidate.release_id
                 WHERE NOT EXISTS (
                     SELECT 1 FROM blocked_groups blocked
                     WHERE blocked.normalized_name=candidate.normalized_name
                       AND blocked.release_id=candidate.release_id
                 )
                   AND NOT EXISTS (
                     SELECT 1 FROM conflicting_external_groups conflicting
                     WHERE conflicting.normalized_name=candidate.normalized_name
                       AND conflicting.release_id=candidate.release_id
                 )
                   AND NOT EXISTS (
                     SELECT 1 FROM conflicting_attribute_groups conflicting
                     WHERE conflicting.normalized_name=candidate.normalized_name
                       AND conflicting.release_id=candidate.release_id
                 )
             )
             SELECT source.id, source.public_api_id,
                    target.id, target.public_api_id,
                    source.release_id, source.normalized_name
             FROM ranked source
             JOIN track_only_identities target ON target.id=source.target_artist_id
             WHERE source.id <> target.id
             ORDER BY source.normalized_name, source.release_id, source.id
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                release_id,
                normalized_name,
            )| SameReleaseTrackArtistMergeCandidate {
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                release_id,
                normalized_name,
            },
        )
        .collect())
    }

    /// Returns bounded candidates for repeated exact primary Artist billing
    /// across different cataloged Releases.
    ///
    /// The whole active normalized-name group must be active and unverified, and
    /// indistinguishable. External identities, disambiguation, differing
    /// types or dates, memberships, relationships, manual/locked decisions,
    /// negative assertions, or same-Release ambiguity make the group
    /// ineligible. This makes the predicate contextual and conflict-gated
    /// rather than treating the normalized name as a global identity key.
    pub async fn repeated_primary_release_artist_merge_candidates(
        &self,
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<RepeatedPrimaryReleaseArtistMergeCandidate>, CatalogStoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(CatalogStoreError::InvalidRequest);
        }
        Ok(sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, String)>(
            "WITH active_artists AS (
                 SELECT artist.id, artist.public_api_id, artist.normalized_name,
                        artist.artist_type, artist.begin_date, artist.end_date
                 FROM artists artist
                 WHERE artist.status IN ('active','verified')
                   AND btrim(artist.normalized_name) <> ''
             ), eligible_artists AS (
                 SELECT active.*
                 FROM active_artists active
                 JOIN artists artist ON artist.id=active.id
                 WHERE artist.status='active'
                   AND COALESCE(btrim(artist.disambiguation), '')=''
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_external_identities external
                       WHERE external.artist_id=artist.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM group_memberships membership
                       WHERE membership.member_artist_id=artist.id
                          OR membership.group_artist_id=artist.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_relationships relationship
                       WHERE relationship.source_artist_id=artist.id
                          OR relationship.target_artist_id=artist.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_aliases alias
                       WHERE alias.artist_id=artist.id
                         AND alias.normalized_alias <> artist.normalized_name
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_match_decisions decision
                       WHERE decision.artist_id=artist.id
                         AND (decision.is_locked OR decision.authority <> 'automated')
                   )
                   AND EXISTS (
                       SELECT 1
                       FROM artist_match_current current
                       JOIN artist_match_decisions decision
                         ON decision.id=current.decision_id
                       WHERE decision.artist_id=artist.id
                         AND decision.authority='automated'
                         AND NOT decision.is_locked
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM artist_match_current current
                       JOIN artist_match_decisions decision
                         ON decision.id=current.decision_id
                       JOIN observed_artists observed
                         ON observed.id=current.observed_artist_id
                       WHERE decision.artist_id=artist.id
                         AND observed.normalized_value <> artist.normalized_name
                   )
             ), primary_context AS (
                 SELECT DISTINCT eligible.id, eligible.public_api_id,
                        eligible.normalized_name, eligible.artist_type,
                        eligible.begin_date, eligible.end_date,
                        credit.release_id
                 FROM eligible_artists eligible
                 JOIN release_artists credit ON credit.artist_id=eligible.id
                 JOIN releases release ON release.id=credit.release_id
                 WHERE credit.role='primary' AND release.status <> 'deleted'
             ), candidate_names AS (
                 SELECT normalized_name
                 FROM primary_context
                 GROUP BY normalized_name
                 HAVING count(DISTINCT id)>1
                    AND count(DISTINCT release_id)>1
                    AND count(DISTINCT artist_type)=1
                    AND count(DISTINCT COALESCE(begin_date, DATE '0001-01-01'))=1
                    AND count(DISTINCT COALESCE(end_date, DATE '9999-12-31'))=1
             ), blocked_names AS (
                 SELECT DISTINCT active.normalized_name
                 FROM active_artists active
                 JOIN candidate_names candidate
                   ON candidate.normalized_name=active.normalized_name
                 LEFT JOIN eligible_artists eligible ON eligible.id=active.id
                 WHERE eligible.id IS NULL
             ), same_release_ambiguity AS (
                 SELECT normalized_name
                 FROM primary_context
                 GROUP BY normalized_name, release_id
                 HAVING count(DISTINCT id)>1
             ), negative_names AS (
                 SELECT DISTINCT owner.normalized_name
                 FROM active_artists owner
                 JOIN artist_match_decisions owner_decision
                   ON owner_decision.artist_id=owner.id
                 JOIN artist_match_current current
                   ON current.decision_id=owner_decision.id
                  AND current.observed_artist_id=owner_decision.observed_artist_id
                 JOIN artist_negative_assertions negative
                   ON negative.observed_artist_id=current.observed_artist_id
                  AND negative.candidate_artist_id IS NOT NULL
                 JOIN active_artists candidate
                   ON candidate.id=negative.candidate_artist_id
                  AND candidate.normalized_name=owner.normalized_name
                 WHERE negative.assertion_kind='do_not_match'
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_negative_assertions revoked
                       WHERE revoked.predecessor_assertion_id=negative.id
                         AND revoked.assertion_kind='revoke'
                   )
             ), distinct_candidates AS (
                 SELECT DISTINCT context.id, context.public_api_id,
                        context.normalized_name
                 FROM primary_context context
                 JOIN candidate_names candidate
                   ON candidate.normalized_name=context.normalized_name
                 WHERE NOT EXISTS (
                     SELECT 1 FROM blocked_names blocked
                     WHERE blocked.normalized_name=context.normalized_name
                 )
                   AND NOT EXISTS (
                       SELECT 1 FROM same_release_ambiguity ambiguous
                       WHERE ambiguous.normalized_name=context.normalized_name
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM negative_names negative
                       WHERE negative.normalized_name=context.normalized_name
                   )
             ), ranked AS (
                 SELECT candidate.*,
                        first_value(candidate.id) OVER (
                            PARTITION BY candidate.normalized_name
                            ORDER BY candidate.id
                        ) AS target_artist_id
                 FROM distinct_candidates candidate
             )
             SELECT source.id, source.public_api_id,
                    target.id, target.public_api_id,
                    source.normalized_name
             FROM ranked source
             JOIN distinct_candidates target ON target.id=source.target_artist_id
             WHERE source.id <> target.id
             ORDER BY source.normalized_name, source.id
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                normalized_name,
            )| RepeatedPrimaryReleaseArtistMergeCandidate {
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                normalized_name,
            },
        )
        .collect())
    }

    /// Cross-Release exact-name merge candidates over **every** credit form:
    /// primary Release credits and Track credits (featured/guest included).
    ///
    /// ADR-0189: C# reuses an Artist row by normalized name at import, so a
    /// featured artist appearing on two different Releases is one Artist there.
    /// The guards are unchanged from the primary-Release variant — the whole
    /// active name group must be active, disambiguation-free, free of
    /// external identifiers, relationships, group memberships and aliases, carry
    /// only automated decisions, have every observation matching the name, no
    /// same-Release ambiguity, and no negative assertion — so genuinely distinct
    /// same-name artists are still preserved.
    pub async fn repeated_artist_credit_merge_candidates(
        &self,
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<RepeatedPrimaryReleaseArtistMergeCandidate>, CatalogStoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(CatalogStoreError::InvalidRequest);
        }
        Ok(sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, String)>(
            "WITH active_artists AS (
                 SELECT artist.id, artist.public_api_id, artist.normalized_name,
                        artist.artist_type, artist.begin_date, artist.end_date
                 FROM artists artist
                 WHERE artist.status IN ('active','verified')
                   AND btrim(artist.normalized_name) <> ''
             ), eligible_artists AS (
                 SELECT active.*
                 FROM active_artists active
                 JOIN artists artist ON artist.id=active.id
                 WHERE artist.status='active'
                   AND COALESCE(btrim(artist.disambiguation), '')=''
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_external_identities external
                       WHERE external.artist_id=artist.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM group_memberships membership
                       WHERE membership.member_artist_id=artist.id
                          OR membership.group_artist_id=artist.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_relationships relationship
                       WHERE relationship.source_artist_id=artist.id
                          OR relationship.target_artist_id=artist.id
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_aliases alias
                       WHERE alias.artist_id=artist.id
                         AND alias.normalized_alias <> artist.normalized_name
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_match_decisions decision
                       WHERE decision.artist_id=artist.id
                         AND (decision.is_locked OR decision.authority <> 'automated')
                   )
                   AND EXISTS (
                       SELECT 1
                       FROM artist_match_current current
                       JOIN artist_match_decisions decision
                         ON decision.id=current.decision_id
                       WHERE decision.artist_id=artist.id
                         AND decision.authority='automated'
                         AND NOT decision.is_locked
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM artist_match_current current
                       JOIN artist_match_decisions decision
                         ON decision.id=current.decision_id
                       JOIN observed_artists observed
                         ON observed.id=current.observed_artist_id
                       WHERE decision.artist_id=artist.id
                         AND observed.normalized_value <> artist.normalized_name
                   )
             ), credit_context AS (
                 SELECT DISTINCT eligible.id, eligible.public_api_id,
                        eligible.normalized_name, eligible.artist_type,
                        eligible.begin_date, eligible.end_date,
                        credit.release_id
                 FROM eligible_artists eligible
                 JOIN release_artists credit ON credit.artist_id=eligible.id
                 JOIN releases release ON release.id=credit.release_id
                 WHERE credit.role='primary' AND release.status <> 'deleted'
                 UNION
                 SELECT DISTINCT eligible.id, eligible.public_api_id,
                        eligible.normalized_name, eligible.artist_type,
                        eligible.begin_date, eligible.end_date,
                        release.id AS release_id
                 FROM eligible_artists eligible
                 JOIN track_artists credit ON credit.artist_id=eligible.id
                 JOIN tracks track ON track.id=credit.track_id
                 JOIN releases release ON release.id=track.release_id
                 WHERE release.status <> 'deleted'
             ), candidate_names AS (
                 SELECT normalized_name
                 FROM credit_context
                 GROUP BY normalized_name
                 HAVING count(DISTINCT id)>1
                    AND count(DISTINCT release_id)>1
                    AND count(DISTINCT artist_type)=1
                    AND count(DISTINCT COALESCE(begin_date, DATE '0001-01-01'))=1
                    AND count(DISTINCT COALESCE(end_date, DATE '9999-12-31'))=1
             ), blocked_names AS (
                 SELECT DISTINCT active.normalized_name
                 FROM active_artists active
                 JOIN candidate_names candidate
                   ON candidate.normalized_name=active.normalized_name
                 LEFT JOIN eligible_artists eligible ON eligible.id=active.id
                 WHERE eligible.id IS NULL
             ), same_release_ambiguity AS (
                 SELECT normalized_name
                 FROM credit_context
                 GROUP BY normalized_name, release_id
                 HAVING count(DISTINCT id)>1
             ), negative_names AS (
                 SELECT DISTINCT owner.normalized_name
                 FROM active_artists owner
                 JOIN artist_match_decisions owner_decision
                   ON owner_decision.artist_id=owner.id
                 JOIN artist_match_current current
                   ON current.decision_id=owner_decision.id
                  AND current.observed_artist_id=owner_decision.observed_artist_id
                 JOIN artist_negative_assertions negative
                   ON negative.observed_artist_id=current.observed_artist_id
                  AND negative.candidate_artist_id IS NOT NULL
                 JOIN active_artists candidate
                   ON candidate.id=negative.candidate_artist_id
                  AND candidate.normalized_name=owner.normalized_name
                 WHERE negative.assertion_kind='do_not_match'
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_negative_assertions revoked
                       WHERE revoked.predecessor_assertion_id=negative.id
                         AND revoked.assertion_kind='revoke'
                   )
             ), distinct_candidates AS (
                 SELECT DISTINCT context.id, context.public_api_id,
                        context.normalized_name
                 FROM credit_context context
                 JOIN candidate_names candidate
                   ON candidate.normalized_name=context.normalized_name
                 WHERE NOT EXISTS (
                     SELECT 1 FROM blocked_names blocked
                     WHERE blocked.normalized_name=context.normalized_name
                 )
                   AND NOT EXISTS (
                       SELECT 1 FROM same_release_ambiguity ambiguous
                       WHERE ambiguous.normalized_name=context.normalized_name
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM negative_names negative
                       WHERE negative.normalized_name=context.normalized_name
                   )
             ), ranked AS (
                 SELECT candidate.*,
                        first_value(candidate.id) OVER (
                            PARTITION BY candidate.normalized_name
                            ORDER BY candidate.id
                        ) AS target_artist_id
                 FROM distinct_candidates candidate
             )
             SELECT source.id, source.public_api_id,
                    target.id, target.public_api_id,
                    source.normalized_name
             FROM ranked source
             JOIN distinct_candidates target ON target.id=source.target_artist_id
             WHERE source.id <> target.id
             ORDER BY source.normalized_name, source.id
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                normalized_name,
            )| RepeatedPrimaryReleaseArtistMergeCandidate {
                source_artist_id,
                source_public_api_id,
                target_artist_id,
                target_public_api_id,
                normalized_name,
            },
        )
        .collect())
    }

    /// Returns bounded exact-name candidates for non-authoritative review
    /// proposals. Name equality is discovery evidence, never an identity key.
    ///
    /// Only observations still owned by an automated, unverified decision are
    /// eligible. Existing proposals and active local do-not-match assertions
    /// make the scan idempotent without suppressing other same-name Artists.
    pub async fn name_only_artist_match_proposal_candidates(
        &self,
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<NameOnlyArtistMatchProposalCandidate>, CatalogStoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(CatalogStoreError::InvalidRequest);
        }
        Ok(sqlx::query_as::<_, (Uuid, Uuid, Uuid, String)>(
            "WITH current_observations AS (
                 SELECT observed.id AS observed_artist_id,
                        owner.id AS current_artist_id,
                        owner.normalized_name,
                        current_decision.created_at AS current_decided_at
                 FROM observed_artists observed
                 JOIN artist_match_current current
                   ON current.observed_artist_id=observed.id
                 JOIN artist_match_decisions current_decision
                   ON current_decision.id=current.decision_id
                 JOIN artists owner ON owner.id=current_decision.artist_id
                 WHERE owner.status='active'
                   AND current_decision.authority='automated'
                   AND NOT current_decision.is_locked
                   AND btrim(owner.normalized_name) <> ''
             )
             SELECT source.observed_artist_id, source.current_artist_id,
                    candidate.id, source.normalized_name
             FROM current_observations source
             JOIN artists candidate
               ON candidate.normalized_name=source.normalized_name
              AND candidate.status IN ('active','verified')
              AND candidate.id <> source.current_artist_id
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM artist_negative_assertions negative
                 WHERE negative.observed_artist_id=source.observed_artist_id
                   AND negative.candidate_artist_id IS NOT NULL
                   AND resolve_artist_redirect(negative.candidate_artist_id)
                       = resolve_artist_redirect(candidate.id)
                   AND negative.assertion_kind='do_not_match'
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_negative_assertions revoked
                       WHERE revoked.predecessor_assertion_id=negative.id
                         AND revoked.assertion_kind='revoke'
                   )
             )
               AND NOT EXISTS (
                   SELECT 1
                   FROM artist_match_decisions proposal
                   WHERE proposal.observed_artist_id=source.observed_artist_id
                     AND proposal.artist_id=candidate.id
                     AND proposal.outcome='proposed'
                     AND proposal.created_at >= source.current_decided_at
               )
             ORDER BY source.normalized_name, source.observed_artist_id, candidate.id
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(observed_artist_id, current_artist_id, candidate_artist_id, normalized_name)| {
                NameOnlyArtistMatchProposalCandidate {
                    observed_artist_id,
                    current_artist_id,
                    candidate_artist_id,
                    normalized_name,
                }
            },
        )
        .collect())
    }

    /// Returns duplicate active Releases with the same trusted provider ID.
    /// Release names and normalized titles are deliberately absent from this
    /// identity predicate.
    pub async fn strong_release_merge_candidates(
        &self,
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<StrongReleaseMergeCandidate>, CatalogStoreError> {
        if !(1..=1_000).contains(&limit) {
            return Err(CatalogStoreError::InvalidRequest);
        }
        Ok(sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            "WITH distinct_evidence AS (
                 SELECT DISTINCT release_id, provider, external_id
                 FROM release_external_identities
                 WHERE trusted
             ), groups AS (
                 SELECT provider, external_id
                 FROM distinct_evidence
                 GROUP BY provider, external_id
                 HAVING count(*) > 1
             )
             SELECT source.release_id, target.release_id, groups.provider, groups.external_id
             FROM groups
             JOIN distinct_evidence source
               ON source.provider=groups.provider AND source.external_id=groups.external_id
             JOIN distinct_evidence target
               ON target.provider=groups.provider AND target.external_id=groups.external_id
              AND source.release_id < target.release_id
             JOIN releases source_release ON source_release.id=source.release_id
             JOIN releases target_release ON target_release.id=target.release_id
             WHERE source_release.status='active' AND target_release.status='active'
             ORDER BY groups.provider, groups.external_id, source.release_id, target.release_id
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(source_release_id, target_release_id, provider, external_id)| {
                StrongReleaseMergeCandidate {
                    source_release_id,
                    target_release_id,
                    provider,
                    external_id,
                }
            },
        )
        .collect())
    }

    /// Merges one Release into a redirect tombstone without deleting source
    /// observations or media assets. Track ordinals are appended to the target
    /// and recorded for the reversible split operation.
    pub async fn merge_release(
        &self,
        pool: &PgPool,
        source_release_id: Uuid,
        target_release_id: Uuid,
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<Uuid, CatalogStoreError> {
        let merge = ReleaseMerge {
            source_release_id,
            target_release_id,
            actor: actor.to_owned(),
            reason: reason.to_owned(),
            occurred_at: now,
        };
        self.merge_releases(pool, std::slice::from_ref(&merge))
            .await?
            .first()
            .map(|event| event.merge_event_id)
            .ok_or(CatalogStoreError::InvalidRequest)
    }

    /// Atomically folds one or more source Releases into one administrator
    /// selected target. Every source event commits together or the whole batch
    /// rolls back.
    ///
    /// The target keeps its own metadata, Release-level billing, and artwork.
    /// Source Tracks (with their observations, contributions, and media), and
    /// any artwork the target lacks, move to the target; the sources become
    /// redirect tombstones. See ADR-0198 for the exact merge scope and why the
    /// surviving Release keeps the target's Release artist.
    pub async fn merge_releases(
        &self,
        pool: &PgPool,
        merges: &[ReleaseMerge],
    ) -> Result<Vec<ReleaseMergeEvent>, CatalogStoreError> {
        validate_release_merge_batch(merges)?;
        let mut ordered = merges.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|merge| merge.source_release_id);
        let mut tx = pool.begin().await?;
        let mut events = Vec::with_capacity(ordered.len());
        // Only the first source that owns one contributes its operator image;
        // later sources keep theirs on their recoverable tombstones so the
        // adoption stays deterministic and reversible.
        let mut operator_image_adopted = false;
        for merge in ordered {
            let (event_id, _assets) =
                merge_release_in_transaction(&mut tx, merge, &mut operator_image_adopted).await?;
            events.push(ReleaseMergeEvent {
                source_release_id: merge.source_release_id,
                target_release_id: merge.target_release_id,
                merge_event_id: event_id,
            });
        }
        tx.commit().await?;
        Ok(events)
    }

    /// Returns the exact bounded impact of folding the selected source
    /// Releases into the target without mutating catalog state.
    pub async fn preview_release_merge(
        &self,
        pool: &PgPool,
        source_release_ids: &[Uuid],
        target_release_id: Uuid,
    ) -> Result<ReleaseMergePreview, CatalogStoreError> {
        let sources = validate_release_merge_selection(source_release_ids, target_release_id)?;
        let mut requested = sources.clone();
        requested.push(target_release_id);
        let rows = sqlx::query_as::<_, ReleaseMergePreviewRow>(
            "SELECT r.id, r.public_api_id, r.title, r.display_artist,
                    r.release_date::text, r.release_date_text, r.status,
                    COALESCE(agg.track_count, 0),
                    (SELECT count(*) FROM release_artists billing
                      WHERE billing.release_id = r.id),
                    EXISTS (SELECT 1 FROM catalog_entity_images image
                             WHERE image.entity_type='release' AND image.release_id = r.id),
                    (SELECT count(*) FROM catalog_provider_images image
                      WHERE image.entity_type='release' AND image.entity_id = r.id)
               FROM releases r
               LEFT JOIN release_catalog_aggregates agg ON agg.release_id = r.id
              WHERE r.id = ANY($1)
              ORDER BY r.public_api_id",
        )
        .bind(&requested)
        .fetch_all(pool)
        .await?;
        if rows.len() != requested.len() || rows.iter().any(|row| row.6 != "active") {
            return Err(CatalogStoreError::MutationConflict);
        }
        let to_preview = |row: &ReleaseMergePreviewRow| ReleaseMergePreviewRelease {
            public_api_id: row.1,
            title: row.2.clone(),
            display_artist: row.3.clone(),
            release_date: row.4.clone(),
            release_date_text: row.5.clone(),
            status: row.6.clone(),
            track_count: row.7,
            release_artist_count: row.8,
            has_operator_image: row.9,
            provider_image_count: row.10,
        };
        let target = rows
            .iter()
            .find(|row| row.0 == target_release_id)
            .map(to_preview)
            .ok_or(CatalogStoreError::MutationConflict)?;
        let source_rows = rows
            .iter()
            .filter(|row| row.0 != target_release_id)
            .collect::<Vec<_>>();
        let preview_sources = source_rows
            .iter()
            .map(|row| to_preview(row))
            .collect::<Vec<_>>();

        let counts = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
            "SELECT
                 (SELECT count(*) FROM tracks WHERE release_id = ANY($1)),
                 (SELECT count(*) FROM observed_artists WHERE release_id = ANY($1)),
                 (SELECT count(*) FROM contributions WHERE release_id = ANY($1)),
                 (SELECT count(DISTINCT source.audio_identity_id)
                    FROM tracks source
                    JOIN tracks target
                      ON target.audio_identity_id = source.audio_identity_id
                     AND target.release_id = $2
                   WHERE source.release_id = ANY($1)),
                 (SELECT count(*) FROM release_artists
                   WHERE release_id = ANY($1) AND observed_artist_id IS NOT NULL),
                 (SELECT count(*) FROM native_release_user_states
                   WHERE release_id = ANY($1)),
                 (SELECT count(*) FROM native_release_user_states source
                   WHERE source.release_id = ANY($1)
                     AND EXISTS (SELECT 1 FROM native_release_user_states target
                                  WHERE target.release_id = $2
                                    AND target.user_id = source.user_id))",
        )
        .bind(&sources)
        .bind(target_release_id)
        .fetch_one(pool)
        .await?;
        let (
            moved_tracks,
            moved_observations,
            moved_contributions,
            shared_audio_identities,
            detach,
            source_user_states,
            retained_user_states,
        ) = counts;

        // Artwork adoption mirrors the merge: one operator image, only when
        // the target has none, taken from the lowest public id source; one
        // provider image per provider the target has no image for.
        let source_operator_images = source_rows.iter().filter(|row| row.9).count();
        let operator_images_adopted =
            i64::from(!target.has_operator_image && source_operator_images > 0);
        let operator_images_preserved = i64::try_from(source_operator_images)
            .map_err(|_| CatalogStoreError::NumericOverflow)?
            - operator_images_adopted;
        let provider_rows = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT entity_id, provider FROM catalog_provider_images
              WHERE entity_type='release' AND entity_id = ANY($1)
              ORDER BY provider, entity_id",
        )
        .bind(&sources)
        .fetch_all(pool)
        .await?;
        let target_providers = sqlx::query_scalar::<_, String>(
            "SELECT provider FROM catalog_provider_images
              WHERE entity_type='release' AND entity_id = $1",
        )
        .bind(target_release_id)
        .fetch_all(pool)
        .await?;
        let mut adopted_providers = std::collections::BTreeSet::new();
        for (_, provider) in &provider_rows {
            if !target_providers.contains(provider) {
                adopted_providers.insert(provider.clone());
            }
        }
        let provider_images_adopted = i64::try_from(adopted_providers.len())
            .map_err(|_| CatalogStoreError::NumericOverflow)?;
        let provider_image_conflicts = i64::try_from(provider_rows.len())
            .map_err(|_| CatalogStoreError::NumericOverflow)?
            - provider_images_adopted;

        let billing_rows = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT release_id, artist_id FROM release_artists
              WHERE release_id = ANY($1) ORDER BY release_id, artist_id",
        )
        .bind(&requested)
        .fetch_all(pool)
        .await?;
        let billing = |release_id: Uuid| {
            billing_rows
                .iter()
                .filter(|(row_release, _)| *row_release == release_id)
                .map(|(_, artist_id)| *artist_id)
                .collect::<Vec<_>>()
        };
        let target_billing = billing(target_release_id);

        let mut metadata_conflicts = Vec::new();
        if preview_sources
            .iter()
            .any(|source| source.title != target.title)
        {
            metadata_conflicts.push("title".to_owned());
        }
        if preview_sources
            .iter()
            .any(|source| source.display_artist != target.display_artist)
        {
            metadata_conflicts.push("displayArtist".to_owned());
        }
        if preview_sources
            .iter()
            .any(|source| source.release_date != target.release_date)
        {
            metadata_conflicts.push("releaseDate".to_owned());
        }
        if preview_sources
            .iter()
            .any(|source| source.release_date_text != target.release_date_text)
        {
            metadata_conflicts.push("releaseDateText".to_owned());
        }
        if preview_sources
            .iter()
            .any(|source| billing(source.public_api_id) != target_billing)
        {
            metadata_conflicts.push("releaseArtist".to_owned());
        }
        let mut warnings = Vec::new();
        if moved_tracks == 0 {
            warnings.push("source has no tracks".to_owned());
        }
        if shared_audio_identities > 0 {
            warnings.push(
                "source and target share recordings; review duplicate Tracks after merge"
                    .to_owned(),
            );
        }
        if operator_images_preserved > 0 {
            warnings.push(
                "source artwork is kept on the merge tombstones because the target already has artwork"
                    .to_owned(),
            );
        }
        Ok(ReleaseMergePreview {
            target_track_count_after: target.track_count + moved_tracks,
            impact: ReleaseMergeImpact {
                moved_tracks,
                moved_observations,
                moved_contributions,
                shared_audio_identities,
                detached_release_artist_observations: detach,
                operator_images_adopted,
                operator_images_preserved,
                provider_images_adopted,
                provider_image_conflicts,
                moved_user_states: source_user_states - retained_user_states,
                retained_user_states,
            },
            target,
            sources: preview_sources,
            metadata_conflicts,
            warnings,
        })
    }

    /// Reverses one Release merge using its immutable Track ordinal map.
    pub async fn split_release_merge(
        &self,
        pool: &PgPool,
        merge_event_id: Uuid,
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        if actor.trim().is_empty() || reason.trim().is_empty() {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mut tx = pool.begin().await?;
        let (source_release_id, target_release_id, moved_payload, assets_payload) =
            sqlx::query_as::<_, (Uuid, Uuid, serde_json::Value, serde_json::Value)>(
                "SELECT source_release_id, target_release_id, moved_tracks, moved_assets
             FROM release_merge_events WHERE id=$1 FOR UPDATE",
            )
            .bind(merge_event_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(CatalogStoreError::MutationConflict)?;
        let moved: Vec<ReleaseMovedTrack> = serde_json::from_value(moved_payload.clone())?;
        let assets: ReleaseMovedAssets = serde_json::from_value(assets_payload)?;
        let statuses = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, status FROM releases
             WHERE id=ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind([source_release_id, target_release_id])
        .fetch_all(&mut *tx)
        .await?;
        if statuses.len() != 2 {
            return Err(CatalogStoreError::MutationConflict);
        }
        let source_status = statuses
            .iter()
            .find(|(id, _)| *id == source_release_id)
            .map(|(_, status)| status.as_str());
        let target_status = statuses
            .iter()
            .find(|(id, _)| *id == target_release_id)
            .map(|(_, status)| status.as_str());
        if source_status != Some("merged") || target_status != Some("active") {
            return Err(CatalogStoreError::MutationConflict);
        }
        let track_ids = moved.iter().map(|track| track.track_id).collect::<Vec<_>>();
        for track in &moved {
            for observation in &track.observations {
                sqlx::query(
                    "UPDATE observed_artists SET release_id=$2, source_ordinal=$3
                     WHERE id=$1 AND release_id=$4",
                )
                .bind(observation.observation_id)
                .bind(source_release_id)
                .bind(observation.original_ordinal)
                .bind(target_release_id)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query(
                "UPDATE tracks SET release_id=$2, ordinal=$3,
                    revision=revision+1, updated_at=$4
                 WHERE id=$1 AND release_id=$5",
            )
            .bind(track.track_id)
            .bind(source_release_id)
            .bind(track.original_ordinal)
            .bind(now)
            .bind(target_release_id)
            .execute(&mut *tx)
            .await?;
        }
        if !track_ids.is_empty() {
            sqlx::query("UPDATE contributions SET release_id=$2 WHERE track_id=ANY($1)")
                .bind(&track_ids)
                .bind(source_release_id)
                .execute(&mut *tx)
                .await?;
        }
        restore_release_merge_assets(&mut tx, source_release_id, target_release_id, &assets, now)
            .await?;
        let split_event_id = Uuid::now_v7();
        sqlx::query(
            "UPDATE releases SET status='active', merged_into_release_id=NULL,
                revision=revision+1, updated_at=$2 WHERE id=$1",
        )
        .bind(source_release_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE releases SET revision=revision+1, updated_at=$2 WHERE id=$1")
            .bind(target_release_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        refresh_release_catalog_aggregate(&mut tx, source_release_id, now).await?;
        refresh_release_catalog_aggregate(&mut tx, target_release_id, now).await?;
        refresh_artist_catalog_aggregates(&mut tx, &track_ids, now).await?;
        sqlx::query(
            "INSERT INTO release_split_events (
                id, merge_event_id, source_release_id, target_release_id,
                actor, reason, restored_tracks, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(split_event_id)
        .bind(merge_event_id)
        .bind(source_release_id)
        .bind(target_release_id)
        .bind(actor)
        .bind(reason)
        .bind(moved_payload)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        enqueue_release_revision(&mut tx, source_release_id, now).await?;
        enqueue_release_revision(&mut tx, target_release_id, now).await?;
        enqueue_track_revisions(&mut tx, &track_ids, now).await?;
        audit_release_event(
            &mut tx,
            source_release_id,
            "release_split",
            actor,
            reason,
            split_event_id,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// One administrative Release merge: fold `source` into `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseMerge {
    pub source_release_id: Uuid,
    pub target_release_id: Uuid,
    pub actor: String,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

/// One committed Release merge event: which source folded into which target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseMergeEvent {
    pub source_release_id: Uuid,
    pub target_release_id: Uuid,
    pub merge_event_id: Uuid,
}

/// One Release rendered in an administrative merge impact preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseMergePreviewRelease {
    pub public_api_id: Uuid,
    pub title: String,
    pub display_artist: Option<String>,
    pub release_date: Option<String>,
    pub release_date_text: Option<String>,
    pub status: String,
    pub track_count: i64,
    pub release_artist_count: i64,
    pub has_operator_image: bool,
    pub provider_image_count: i64,
}

/// Exact bounded impact of folding the selected source Releases into the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseMergeImpact {
    pub moved_tracks: i64,
    pub moved_observations: i64,
    pub moved_contributions: i64,
    pub shared_audio_identities: i64,
    /// Source Release-level billing rows detached from observations that move
    /// with their Tracks; the split restores these links.
    pub detached_release_artist_observations: i64,
    pub operator_images_adopted: i64,
    /// Source artwork that stays on a merge tombstone because the target (or a
    /// lower-id source) already supplies it.
    pub operator_images_preserved: i64,
    pub provider_images_adopted: i64,
    pub provider_image_conflicts: i64,
    /// Per-user Release state rows (star/rating/pin/play count) moved to the
    /// target; a user who already has target state keeps the target's row.
    pub moved_user_states: i64,
    pub retained_user_states: i64,
}

/// Impact preview shared by the Admin API and the Admin UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseMergePreview {
    pub target: ReleaseMergePreviewRelease,
    pub sources: Vec<ReleaseMergePreviewRelease>,
    pub impact: ReleaseMergeImpact,
    /// Track count the target holds after every source Track has moved.
    pub target_track_count_after: i64,
    /// Target-wins metadata conflicts: `title`, `displayArtist`,
    /// `releaseDate`, `releaseDateText`, `releaseArtist`.
    pub metadata_conflicts: Vec<String>,
    pub warnings: Vec<String>,
}

type ReleaseMergePreviewRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    bool,
    i64,
);

/// Non-Track data folded by one Release merge, recorded for the reversible split.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ReleaseMovedAssets {
    #[serde(default)]
    detached_release_artist_observations: Vec<DetachedReleaseArtistObservation>,
    /// Users whose per-Release state row moved to the target.
    #[serde(default)]
    moved_release_user_states: Vec<Uuid>,
    #[serde(default)]
    adopted_operator_image: Option<AdoptedOperatorImage>,
    #[serde(default)]
    adopted_provider_images: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DetachedReleaseArtistObservation {
    release_artist_id: Uuid,
    observed_artist_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdoptedOperatorImage {
    source_release_id: Uuid,
    /// Adoption timestamp; the split only takes the image back when the target
    /// override is still the adopted one (an operator upload after the merge
    /// wins and stays).
    updated_at: OffsetDateTime,
}

fn validate_release_merge_selection(
    source_release_ids: &[Uuid],
    target_release_id: Uuid,
) -> Result<Vec<Uuid>, CatalogStoreError> {
    if source_release_ids.is_empty() || source_release_ids.len() > MAX_RELEASE_MERGE_SOURCES {
        return Err(CatalogStoreError::InvalidRequest);
    }
    let sources = source_release_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if sources.len() != source_release_ids.len() || sources.contains(&target_release_id) {
        return Err(CatalogStoreError::InvalidRequest);
    }
    Ok(sources.into_iter().collect())
}

fn validate_release_merge_batch(merges: &[ReleaseMerge]) -> Result<(), CatalogStoreError> {
    if merges.is_empty() || merges.len() > MAX_RELEASE_MERGE_SOURCES {
        return Err(CatalogStoreError::InvalidRequest);
    }
    let target_release_id = merges[0].target_release_id;
    let mut sources = std::collections::BTreeSet::new();
    for merge in merges {
        if merge.target_release_id != target_release_id
            || merge.source_release_id == target_release_id
            || merge.actor.trim().is_empty()
            || merge.reason.trim().is_empty()
            || !sources.insert(merge.source_release_id)
        {
            return Err(CatalogStoreError::InvalidRequest);
        }
    }
    Ok(())
}

/// One source merge inside the caller's transaction.
pub(crate) async fn merge_review_selection(
    tx: &mut Transaction<'_, Postgres>,
    merge: &ReleaseMerge,
) -> Result<Uuid, CatalogStoreError> {
    let (event, _) = merge_release_in_transaction(tx, merge, &mut false).await?;
    Ok(event)
}

async fn merge_release_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    merge: &ReleaseMerge,
    operator_image_adopted: &mut bool,
) -> Result<(Uuid, ReleaseMovedAssets), CatalogStoreError> {
    let source_release_id = merge.source_release_id;
    let target_release_id = merge.target_release_id;
    let actor = merge.actor.as_str();
    let reason = merge.reason.as_str();
    let now = merge.occurred_at;
    let mut assets = ReleaseMovedAssets::default();
    let statuses = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, status FROM releases
             WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind([source_release_id, target_release_id])
    .fetch_all(&mut **tx)
    .await?;
    if statuses.len() != 2 || statuses.iter().any(|(_, status)| status != "active") {
        return Err(CatalogStoreError::MutationConflict);
    }
    let source_tracks = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT id, ordinal FROM tracks WHERE release_id=$1 ORDER BY ordinal, id",
    )
    .bind(source_release_id)
    .fetch_all(&mut **tx)
    .await?;
    ensure_correction_scope(
        source_tracks.len(),
        CatalogCorrectionDimension::Tracks,
        MAX_CATALOG_CORRECTION_TRACKS,
    )?;
    let source_track_ids = source_tracks
        .iter()
        .map(|(track_id, _)| *track_id)
        .collect::<Vec<_>>();
    let source_observations = sqlx::query_as::<_, (Uuid, Uuid, i32)>(
        "SELECT id, track_id, source_ordinal FROM observed_artists
             WHERE release_id=$1 AND track_id=ANY($2)
             ORDER BY track_id, id",
    )
    .bind(source_release_id)
    .bind(&source_track_ids)
    .fetch_all(&mut **tx)
    .await?;
    ensure_correction_scope(
        source_observations.len(),
        CatalogCorrectionDimension::IdentityRows,
        MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
    )?;
    // The surviving Release keeps the target's Release-level billing. Source
    // billing rows stay on the tombstone as evidence, but a link to an
    // observation that moves with its Track must be detached first: the
    // composite `release_artists_observation_fk` is not deferrable. The
    // split restores the exact links from the merge event.
    let moved_observation_ids = source_observations
        .iter()
        .map(|(observation_id, _, _)| *observation_id)
        .collect::<Vec<_>>();
    let detached = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT id, observed_artist_id FROM release_artists
              WHERE release_id=$1 AND observed_artist_id = ANY($2)
              ORDER BY id FOR UPDATE",
    )
    .bind(source_release_id)
    .bind(&moved_observation_ids)
    .fetch_all(&mut **tx)
    .await?;
    ensure_correction_scope(
        detached.len(),
        CatalogCorrectionDimension::IdentityRows,
        MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
    )?;
    if !detached.is_empty() {
        sqlx::query(
            "UPDATE release_artists SET observed_artist_id=NULL
                  WHERE release_id=$1 AND observed_artist_id = ANY($2)",
        )
        .bind(source_release_id)
        .bind(&moved_observation_ids)
        .execute(&mut **tx)
        .await?;
        assets.detached_release_artist_observations = detached
            .into_iter()
            .map(
                |(release_artist_id, observed_artist_id)| DetachedReleaseArtistObservation {
                    release_artist_id,
                    observed_artist_id,
                },
            )
            .collect();
    }
    let target_max =
        sqlx::query_scalar::<_, Option<i32>>("SELECT max(ordinal) FROM tracks WHERE release_id=$1")
            .bind(target_release_id)
            .fetch_one(&mut **tx)
            .await?
            .unwrap_or(0);
    let target_observation_max = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT max(source_ordinal) FROM observed_artists WHERE release_id=$1",
    )
    .bind(target_release_id)
    .fetch_one(&mut **tx)
    .await?
    .unwrap_or(0);
    let mut moved = Vec::with_capacity(source_tracks.len());
    let mut next_observation_ordinal = target_observation_max;
    for (index, (track_id, original_ordinal)) in source_tracks.iter().enumerate() {
        let merged_ordinal = target_max
            .checked_add(i32::try_from(index + 1).map_err(|_| CatalogStoreError::NumericOverflow)?)
            .ok_or(CatalogStoreError::NumericOverflow)?;
        sqlx::query(
            "UPDATE tracks SET release_id=$2, ordinal=$3,
                    revision=revision+1, updated_at=$4 WHERE id=$1 AND release_id=$5",
        )
        .bind(track_id)
        .bind(target_release_id)
        .bind(merged_ordinal)
        .bind(now)
        .bind(source_release_id)
        .execute(&mut **tx)
        .await?;
        let mut observations = Vec::new();
        for (observation_id, _observation_track_id, original_observation_ordinal) in
            source_observations
                .iter()
                .filter(|(_, observation_track_id, _)| observation_track_id == track_id)
        {
            next_observation_ordinal = next_observation_ordinal
                .checked_add(1)
                .ok_or(CatalogStoreError::NumericOverflow)?;
            sqlx::query(
                "UPDATE observed_artists SET release_id=$2, source_ordinal=$3
                     WHERE id=$1 AND release_id=$4",
            )
            .bind(observation_id)
            .bind(target_release_id)
            .bind(next_observation_ordinal)
            .bind(source_release_id)
            .execute(&mut **tx)
            .await?;
            observations.push(ReleaseMovedObservation {
                observation_id: *observation_id,
                original_ordinal: *original_observation_ordinal,
                merged_ordinal: next_observation_ordinal,
            });
        }
        moved.push(ReleaseMovedTrack {
            track_id: *track_id,
            original_ordinal: *original_ordinal,
            merged_ordinal,
            observations,
        });
    }
    let track_ids = moved.iter().map(|track| track.track_id).collect::<Vec<_>>();
    if !track_ids.is_empty() {
        sqlx::query("UPDATE contributions SET release_id=$2 WHERE track_id=ANY($1)")
            .bind(&track_ids)
            .bind(target_release_id)
            .execute(&mut **tx)
            .await?;
    }
    // Policy novelty evidence (`track_novelty`, `release_novelty_evaluations`)
    // is immutable history: it keeps the Release whose evaluation recorded it
    // and is never rewritten or deleted by a merge.
    // Per-user Release state (star, hate, rating, pin, play count): a user who
    // already has state on the surviving Release keeps the target's row; every
    // other user's row moves, so nothing is lost and the split can move it back.
    assets.moved_release_user_states = sqlx::query_scalar::<_, Uuid>(
        "UPDATE native_release_user_states source SET release_id=$2
          WHERE source.release_id=$1
            AND NOT EXISTS (
                SELECT 1 FROM native_release_user_states target
                 WHERE target.release_id=$2 AND target.user_id=source.user_id)
         RETURNING source.user_id",
    )
    .bind(source_release_id)
    .bind(target_release_id)
    .fetch_all(&mut **tx)
    .await?;
    // Artwork: the target's own cover always wins. A source operator
    // override is adopted only when the target has none, and only from the
    // first (lowest id) source, so the choice is deterministic. Provider
    // artwork is adopted per provider the target has no image for.
    if !*operator_image_adopted {
        let target_has_image = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM catalog_entity_images
                                 WHERE entity_type='release' AND release_id=$1)",
        )
        .bind(target_release_id)
        .fetch_one(&mut **tx)
        .await?;
        if !target_has_image {
            let adopted = sqlx::query(
                "UPDATE catalog_entity_images SET release_id=$2, updated_at=$3
                      WHERE entity_type='release' AND release_id=$1",
            )
            .bind(source_release_id)
            .bind(target_release_id)
            .bind(now)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            if adopted == 1 {
                *operator_image_adopted = true;
                assets.adopted_operator_image = Some(AdoptedOperatorImage {
                    source_release_id,
                    updated_at: now,
                });
            }
        }
    }
    let source_providers = sqlx::query_scalar::<_, String>(
        "SELECT provider FROM catalog_provider_images
              WHERE entity_type='release' AND entity_id=$1 ORDER BY provider",
    )
    .bind(source_release_id)
    .fetch_all(&mut **tx)
    .await?;
    for provider in source_providers {
        let target_has_provider = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM catalog_provider_images
                                 WHERE entity_type='release' AND entity_id=$1 AND provider=$2)",
        )
        .bind(target_release_id)
        .bind(&provider)
        .fetch_one(&mut **tx)
        .await?;
        if target_has_provider {
            continue;
        }
        if let Some(image_id) = sqlx::query_scalar::<_, Uuid>(
            "UPDATE catalog_provider_images SET entity_id=$2
                  WHERE entity_type='release' AND entity_id=$1 AND provider=$3
                 RETURNING id",
        )
        .bind(source_release_id)
        .bind(target_release_id)
        .bind(&provider)
        .fetch_optional(&mut **tx)
        .await?
        {
            assets.adopted_provider_images.push(image_id);
        }
    }
    sqlx::query(
        "UPDATE releases SET status='merged', merged_into_release_id=$2,
                revision=revision+1, updated_at=$3 WHERE id=$1",
    )
    .bind(source_release_id)
    .bind(target_release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE releases SET revision=revision+1, updated_at=$2 WHERE id=$1")
        .bind(target_release_id)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    refresh_release_catalog_aggregate(tx, source_release_id, now).await?;
    refresh_release_catalog_aggregate(tx, target_release_id, now).await?;
    refresh_artist_catalog_aggregates(tx, &track_ids, now).await?;
    let event_id = Uuid::now_v7();
    let moved_payload = serde_json::to_value(&moved)?;
    let assets_payload = serde_json::to_value(&assets)?;
    sqlx::query(
        "INSERT INTO release_merge_events (
                id, source_release_id, target_release_id, actor, reason,
                moved_tracks, moved_assets, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(event_id)
    .bind(source_release_id)
    .bind(target_release_id)
    .bind(actor)
    .bind(reason)
    .bind(&moved_payload)
    .bind(&assets_payload)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    enqueue_release_revision(tx, source_release_id, now).await?;
    enqueue_release_revision(tx, target_release_id, now).await?;
    enqueue_track_revisions(tx, &track_ids, now).await?;
    audit_release_event(
        tx,
        source_release_id,
        "release_merged",
        actor,
        reason,
        event_id,
        now,
    )
    .await?;
    audit_release_event(
        tx,
        target_release_id,
        "release_merged_into",
        actor,
        reason,
        Uuid::now_v7(),
        now,
    )
    .await?;
    Ok((event_id, assets))
}

/// Restores the non-Track data recorded by one Release merge.
async fn restore_release_merge_assets(
    tx: &mut Transaction<'_, Postgres>,
    source_release_id: Uuid,
    target_release_id: Uuid,
    assets: &ReleaseMovedAssets,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    for link in &assets.detached_release_artist_observations {
        sqlx::query(
            "UPDATE release_artists SET observed_artist_id=$2
              WHERE id=$1 AND release_id=$3",
        )
        .bind(link.release_artist_id)
        .bind(link.observed_artist_id)
        .bind(source_release_id)
        .execute(&mut **tx)
        .await?;
    }
    if !assets.moved_release_user_states.is_empty() {
        sqlx::query(
            "UPDATE native_release_user_states SET release_id=$2
              WHERE release_id=$1 AND user_id = ANY($3)",
        )
        .bind(target_release_id)
        .bind(source_release_id)
        .bind(&assets.moved_release_user_states)
        .execute(&mut **tx)
        .await?;
    }
    if let Some(image) = &assets.adopted_operator_image {
        // Only take the override back while it is still the row adopted by the
        // merge; an operator upload after the merge wins and stays on target.
        sqlx::query(
            "UPDATE catalog_entity_images SET release_id=$2, updated_at=$3
              WHERE entity_type='release' AND release_id=$1 AND updated_at=$4",
        )
        .bind(target_release_id)
        .bind(image.source_release_id)
        .bind(now)
        .bind(image.updated_at)
        .execute(&mut **tx)
        .await?;
    }
    if !assets.adopted_provider_images.is_empty() {
        sqlx::query(
            "UPDATE catalog_provider_images target SET entity_id=$2
              WHERE target.entity_type='release' AND target.entity_id=$1
                AND target.id = ANY($3)
                AND NOT EXISTS (
                    SELECT 1 FROM catalog_provider_images existing
                     WHERE existing.entity_type='release' AND existing.entity_id=$2
                       AND existing.provider = target.provider)",
        )
        .bind(target_release_id)
        .bind(source_release_id)
        .bind(&assets.adopted_provider_images)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn enqueue_release_revision(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    let revision = sqlx::query_scalar::<_, i64>("SELECT revision FROM releases WHERE id=$1")
        .bind(release_id)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog','release',$1,'upsert',$3,$2,0,$2)
         ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
         DO NOTHING",
    )
    .bind(release_id)
    .bind(now)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn enqueue_track_revisions(
    tx: &mut Transaction<'_, Postgres>,
    track_ids: &[Uuid],
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query_as::<_, (Uuid, i64)>("SELECT id, revision FROM tracks WHERE id=ANY($1)")
        .bind(track_ids)
        .fetch_all(&mut **tx)
        .await?;
    for (track_id, revision) in rows {
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation, entity_revision,
                available_at, attempt_count, created_at
             ) VALUES ('tantivy.catalog','track',$1,'upsert',$3,$2,0,$2)
             ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
             DO NOTHING",
        )
        .bind(track_id)
        .bind(now)
        .bind(revision)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn refresh_release_catalog_aggregate(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
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
                ), 0),
                (SELECT revision FROM releases WHERE id=$1), $2
         FROM tracks t WHERE t.release_id = $1
         ON CONFLICT (release_id) DO UPDATE SET
            track_count=EXCLUDED.track_count,
            duration_ms=EXCLUDED.duration_ms,
            media_bytes=EXCLUDED.media_bytes,
            revision=EXCLUDED.revision,
            updated_at=EXCLUDED.updated_at",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn refresh_artist_catalog_aggregates(
    tx: &mut Transaction<'_, Postgres>,
    track_ids: &[Uuid],
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO artist_catalog_aggregates (
            artist_id, release_count, billed_track_count,
            performance_track_count, all_credit_track_count, revision, updated_at
         )
         SELECT associations.artist_id, count(DISTINCT tracks.release_id),
                count(DISTINCT associations.track_id),
                count(DISTINCT associations.track_id) FILTER (WHERE associations.is_performance),
                count(DISTINCT associations.track_id), 1, $2
         FROM artist_track_associations associations
         JOIN tracks ON tracks.id = associations.track_id
         WHERE associations.artist_id IN (
             SELECT DISTINCT artist_id
             FROM artist_track_associations
             WHERE track_id=ANY($1)
         )
         GROUP BY associations.artist_id
         ON CONFLICT (artist_id) DO UPDATE SET
            release_count=EXCLUDED.release_count,
            billed_track_count=EXCLUDED.billed_track_count,
            performance_track_count=EXCLUDED.performance_track_count,
            all_credit_track_count=EXCLUDED.all_credit_track_count,
            revision=artist_catalog_aggregates.revision+1,
            updated_at=EXCLUDED.updated_at",
    )
    .bind(track_ids)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn audit_release_event(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    event_type: &str,
    actor: &str,
    reason: &str,
    event_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "INSERT INTO catalog_audit_events (
            event_id, entity_type, entity_id, event_type, actor, reason,
            payload, created_at
         ) VALUES ($1,'release',$2,$3,$4,$5,$6,$7)",
    )
    .bind(event_id)
    .bind(release_id)
    .bind(event_type)
    .bind(actor)
    .bind(reason)
    .bind(serde_json::json!({"event_id": event_id}))
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
    musicbrainz_ids: Vec<(String, String)>,
    /// Chromaprint fingerprint of this Track's own audio, when the normalized
    /// output is a whole file. CUE-governed segments share one source
    /// fingerprint, so they carry none (the fingerprint would describe another
    /// Track's audio and poison acoustic duplicate detection).
    acoustic_fingerprint: Option<String>,
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
    reused_release_artist: bool,
    observation_id: Uuid,
    evidence_id: Uuid,
    decision_id: Uuid,
    presentation_id: Uuid,
    track_id: Option<Uuid>,
    source_ordinal: i32,
    presentation_role: PresentationRole,
    join_phrase: String,
    raw_value: String,
    normalized_value: String,
    source_pointer: String,
    source_payload: serde_json::Value,
    musicbrainz_ids: Vec<(String, String)>,
}

#[derive(Clone, Copy)]
enum ArtistBillingSyntax {
    Literal,
    Explicit,
    Slash,
}

impl ArtistBillingSyntax {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Explicit => "explicit_phrase",
            Self::Slash => "release_context_slash",
        }
    }
}

struct PreparedBillingCandidate {
    candidate: ParsedArtistCandidate,
    role: PresentationRole,
    syntax: ArtistBillingSyntax,
}

struct PreparedArtistPresentation {
    ordinal: i32,
    role: PresentationRole,
    join_phrase: String,
}

#[derive(Clone)]
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
        let release_artists = release_artist_observations(&manifest);
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

        // Acoustic fingerprints are observed per *source file* during
        // normalization and retained in the sealed manifest; index them by source
        // path so each Track can carry its own audio's fingerprint into durable
        // identity evidence (PRD-FP-001/PRD-FP-003) without re-reading audio.
        let fingerprints_by_source = normalization
            .fingerprints
            .iter()
            .filter(|fingerprint| !fingerprint.fingerprint.trim().is_empty())
            .map(|fingerprint| (fingerprint.source_file.as_str(), fingerprint))
            .collect::<BTreeMap<_, _>>();

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
                musicbrainz_ids: track
                    .observed_metadata
                    .as_ref()
                    .map(extract_musicbrainz_ids)
                    .unwrap_or_default(),
                acoustic_fingerprint: output
                    .segment
                    .is_none()
                    .then(|| {
                        fingerprints_by_source
                            .get(output.source_file.as_str())
                            .map(|fingerprint| fingerprint.fingerprint.trim().to_owned())
                    })
                    .flatten(),
            });
        }
        if tracks.is_empty() {
            return Err(CatalogStoreError::ImportConflict(work_item_id));
        }

        let mut resolutions = Vec::new();
        for (index, raw) in release_artists.iter().enumerate() {
            resolutions.push(PreparedResolution::new(
                None,
                None,
                PreparedArtistPresentation {
                    ordinal: i32::try_from(index + 1)
                        .map_err(|_| CatalogStoreError::NumericOverflow)?,
                    role: PresentationRole::Primary,
                    join_phrase: String::new(),
                },
                raw,
                format!("/release_observation/artists/{index}"),
                serde_json::json!({ "value": raw }),
            ));
        }
        for resolution in &mut resolutions {
            resolution.musicbrainz_ids = manifest
                .tracks
                .iter()
                .filter(|track| {
                    track
                        .observed_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.artist.as_deref())
                        .is_some_and(|artist| {
                            normalize_lookup(artist) == resolution.normalized_value
                        })
                })
                .filter_map(|track| track.observed_metadata.as_ref())
                .flat_map(extract_musicbrainz_ids)
                .collect();
            resolution.musicbrainz_ids.sort();
            resolution.musicbrainz_ids.dedup();
        }
        // ADR-0170: additional billed Artists do not make an exact match
        // ambiguous. Reuse only when one Release observation owns the Track's
        // normalized value; equal normalized Release observations deliberately
        // collapse this map entry to None rather than becoming an identity key.
        let matching_release_artists = resolutions.iter().fold(
            BTreeMap::<String, Option<Uuid>>::new(),
            |mut matches, resolution| {
                matches
                    .entry(resolution.normalized_value.clone())
                    .and_modify(|artist_id| *artist_id = None)
                    .or_insert(Some(resolution.artist_id));
                matches
            },
        );
        for (index, (manifest_track, prepared_track)) in
            manifest.tracks.iter().zip(tracks.iter()).enumerate()
        {
            if let Some(raw) = manifest_track
                .observed_metadata
                .as_ref()
                .and_then(|metadata| metadata.artist.as_ref())
                .filter(|value| !value.trim().is_empty())
            {
                let billing = prepare_track_billing(raw, &matching_release_artists)?;
                let preserves_embedded_ids = billing.len() == 1;
                for (candidate_index, prepared_candidate) in billing.into_iter().enumerate() {
                    let normalized = normalize_lookup(&prepared_candidate.candidate.raw_value);
                    let release_artist_id =
                        matching_release_artists.get(&normalized).copied().flatten();
                    let source_pointer = if preserves_embedded_ids {
                        format!("/tracks/{index}/observed_metadata/artist")
                    } else {
                        format!(
                            "/tracks/{index}/observed_metadata/artist#billing/{candidate_index}"
                        )
                    };
                    resolutions.push(PreparedResolution::new(
                        release_artist_id,
                        Some(prepared_track.id),
                        PreparedArtistPresentation {
                            ordinal: i32::try_from(candidate_index + 1)
                                .map_err(|_| CatalogStoreError::NumericOverflow)?,
                            role: prepared_candidate.role,
                            join_phrase: prepared_candidate.candidate.join_phrase.clone(),
                        },
                        &prepared_candidate.candidate.raw_value,
                        source_pointer,
                        serde_json::json!({
                            "value": prepared_candidate.candidate.raw_value,
                            "display_artist": raw,
                            "source_file": manifest_track.file_path,
                            "candidate_index": candidate_index,
                            "role": presentation_role_name(prepared_candidate.role),
                            "join_phrase": prepared_candidate.candidate.join_phrase,
                            "billing_syntax": prepared_candidate.syntax.as_str(),
                        }),
                    ));
                    if preserves_embedded_ids && let Some(resolution) = resolutions.last_mut() {
                        resolution.musicbrainz_ids = manifest_track
                            .observed_metadata
                            .as_ref()
                            .map(extract_musicbrainz_ids)
                            .unwrap_or_default();
                    }
                }
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
        artist_id: Option<Uuid>,
        track_id: Option<Uuid>,
        presentation: PreparedArtistPresentation,
        raw: &str,
        source_pointer: String,
        source_payload: serde_json::Value,
    ) -> Self {
        Self {
            artist_id: artist_id.unwrap_or_else(Uuid::now_v7),
            reused_release_artist: artist_id.is_some(),
            observation_id: Uuid::now_v7(),
            evidence_id: Uuid::now_v7(),
            decision_id: Uuid::now_v7(),
            presentation_id: Uuid::now_v7(),
            track_id,
            source_ordinal: presentation.ordinal,
            presentation_role: presentation.role,
            join_phrase: presentation.join_phrase,
            raw_value: raw.to_owned(),
            normalized_value: normalize_lookup(raw),
            source_pointer,
            source_payload,
            musicbrainz_ids: Vec::new(),
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
            Option<serde_json::Value>,
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
    // ADR-0213: catalog import is an authorized lifecycle read of the full
    // evidence object; routine policy/browse paths use the compact facts.
    let payload = match row.3 {
        Some(payload) => payload,
        None => crate::manifest_evidence::load_manifest_on_connection(&mut *tx, row.0)
            .await
            .map_err(|error| CatalogStoreError::ManifestEvidence(error.to_string()))?,
    };
    Ok(ManifestBinding {
        manifest_row_id: row.0,
        schema_version: row.1,
        semantic_hash: row.2,
        payload,
        receipt_release_id: row.4,
        plan_hash: row.5,
        plan_payload: row.6,
        sealed_manifest_hash: row.7,
    })
}

fn normalize_lookup(value: &str) -> String {
    value.trim().to_lowercase()
}

fn release_artist_observations(manifest: &ReleaseManifest) -> Vec<String> {
    let source_labels = &manifest.release_observation.source_labels;
    let inspector_manifest = source_labels.get("observed_titles").is_some()
        || source_labels.get("release_artist_source").is_some();
    if !inspector_manifest
        || source_labels
            .get("reviewed_release_billing")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    {
        return manifest
            .release_observation
            .artists
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect();
    }

    let mut album_artists = Vec::new();
    let mut track_artists = Vec::new();
    for track in &manifest.tracks {
        let Some(metadata) = &track.observed_metadata else {
            continue;
        };
        push_unique_nonblank(&mut album_artists, metadata.album_artist.as_ref());
        push_unique_nonblank(&mut track_artists, metadata.artist.as_ref());
    }
    if album_artists.is_empty() {
        track_artists
    } else {
        album_artists
    }
}

fn push_unique_nonblank(values: &mut Vec<String>, candidate: Option<&String>) {
    let Some(candidate) = candidate.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    if !values.iter().any(|value| value == candidate) {
        values.push(candidate.clone());
    }
}

fn prepare_track_billing(
    raw: &str,
    matching_release_artists: &BTreeMap<String, Option<Uuid>>,
) -> Result<Vec<PreparedBillingCandidate>, CatalogStoreError> {
    let literal = || {
        ParsedArtistCandidate::new(raw.trim().to_owned(), String::new()).map(|candidate| {
            vec![PreparedBillingCandidate {
                candidate,
                role: PresentationRole::Primary,
                syntax: ArtistBillingSyntax::Literal,
            }]
        })
    };
    if matching_release_artists
        .get(&normalize_lookup(raw))
        .is_some_and(Option::is_some)
    {
        return literal().map_err(Into::into);
    }

    let explicit = parse_explicit_artist_billing(raw)?;
    if explicit.len() > 1 {
        return Ok(explicit
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| PreparedBillingCandidate {
                candidate,
                role: if index == 0 {
                    PresentationRole::Primary
                } else {
                    PresentationRole::Featured
                },
                syntax: ArtistBillingSyntax::Explicit,
            })
            .collect());
    }

    let slash = parse_slash_artist_billing(raw)?;
    let slash_has_release_context = slash.len() > 1
        && slash.iter().any(|candidate| {
            matching_release_artists
                .get(&normalize_lookup(&candidate.raw_value))
                .is_some_and(Option::is_some)
        });
    if slash_has_release_context {
        return Ok(slash
            .into_iter()
            .map(|candidate| {
                let role = if matching_release_artists
                    .get(&normalize_lookup(&candidate.raw_value))
                    .is_some_and(Option::is_some)
                {
                    PresentationRole::Primary
                } else {
                    PresentationRole::Featured
                };
                PreparedBillingCandidate {
                    candidate,
                    role,
                    syntax: ArtistBillingSyntax::Slash,
                }
            })
            .collect());
    }
    literal().map_err(Into::into)
}

const fn presentation_role_name(role: PresentationRole) -> &'static str {
    match role {
        PresentationRole::Primary => "primary",
        PresentationRole::Featured => "featured",
        PresentationRole::Guest => "guest",
        PresentationRole::Other => "other",
    }
}

const fn association_type(role: PresentationRole) -> &'static str {
    match role {
        PresentationRole::Primary => "primary_artist",
        PresentationRole::Featured => "featured_artist",
        PresentationRole::Guest => "guest_artist",
        PresentationRole::Other => "other",
    }
}

fn extract_musicbrainz_ids(metadata: &melodee_domain::ObservedMetadata) -> Vec<(String, String)> {
    let Some(ids) = metadata
        .raw
        .get("musicbrainz_ids")
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    ids.iter()
        .filter_map(|(family, value)| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(|value| (family.clone(), value.to_owned()))
        })
        .collect()
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

/// Rewrites each resolution's Artist id to an existing Artist with the exact same
/// normalized name when that is unambiguous (ADR-0189).
///
/// Returns a cloned import because every downstream insert (credits, external
/// identifiers, observations, enrichment tasks) uses the resolution's Artist id;
/// remapping once here keeps them consistent.
async fn reuse_existing_artists(
    tx: &mut Transaction<'_, Postgres>,
    prepared: &PreparedImport,
) -> Result<(PreparedImport, std::collections::BTreeSet<Uuid>), CatalogStoreError> {
    let mut prepared = prepared.clone();
    let names = prepared
        .resolutions
        .iter()
        .map(|resolution| resolution.normalized_value.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut reused = std::collections::BTreeSet::new();
    if names.is_empty() {
        return Ok((prepared, reused));
    }
    // A check-then-insert race would recreate the duplicate this decision exists to
    // prevent: two imports of the same name running concurrently both find no
    // candidate and both insert. Serialize them on the name itself, in ascending
    // order so two imports sharing several names cannot deadlock. The lock is
    // transaction-scoped and released by the import commit.
    for name in &names {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(name)
            .execute(&mut **tx)
            .await?;
    }
    // Count every live homonym before checking the sole candidate's eligibility.
    // Aggregate provider rows per Artist: multiple aliases for one provider identity
    // do not imply multiple Artist identities (ADR-0194).
    let candidates: Vec<(Uuid, String, Option<String>, Vec<String>)> = sqlx::query_as(
        "SELECT artist.id, artist.normalized_name, artist.disambiguation,
                COALESCE(array_agg(DISTINCT external.external_id)
                         FILTER (WHERE external.external_id IS NOT NULL), ARRAY[]::text[])
           FROM artists artist
           LEFT JOIN artist_external_identities external
             ON external.artist_id = artist.id AND external.provider = 'musicbrainz'
          WHERE artist.normalized_name = ANY($1)
            AND artist.status IN ('active','verified')
            AND artist.merged_into_artist_id IS NULL
          GROUP BY artist.id",
    )
    .bind(&names)
    .fetch_all(&mut **tx)
    .await?;
    let mut by_name = BTreeMap::<_, Vec<_>>::new();
    for (id, name, disambiguation, external_ids) in candidates {
        by_name
            .entry(name)
            .or_default()
            .push((id, disambiguation, external_ids));
    }
    for resolution in &mut prepared.resolutions {
        let Some(candidates) = by_name.get(&resolution.normalized_value) else {
            continue;
        };
        let [candidate] = candidates.as_slice() else {
            continue;
        };
        if candidate
            .1
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            continue;
        }
        let embedded = resolution
            .musicbrainz_ids
            .iter()
            .filter(|(family, _)| matches!(family.as_str(), "artist" | "release_artist"))
            .map(|(_, external_id)| external_id);
        if embedded
            .into_iter()
            .any(|external_id| !candidate.2.contains(external_id))
        {
            continue;
        }
        if candidate.0 != resolution.artist_id {
            tracing::debug!(
                normalized_name = %resolution.normalized_value,
                reused_artist_id = %candidate.0,
                "reusing an existing Artist for an exact normalized name"
            );
            resolution.artist_id = candidate.0;
            reused.insert(candidate.0);
        }
    }
    // Preparation allocates provisional IDs before the transaction can resolve reuse.
    // Rebuild this not-yet-persisted snapshot from the final decisions before hashing;
    // never modify an already committed immutable overlay.
    let overlay = &prepared.overlay;
    let mut references = overlay.references().clone();
    references.artist_resolutions = prepared
        .resolutions
        .iter()
        .map(|resolution| OverlayArtistResolution {
            observed_artist_id: ObservedArtistId::from_uuid(resolution.observation_id),
            artist_id: ArtistId::from_uuid(resolution.artist_id),
            decision_id: Some(melodee_domain::ArtistMatchDecisionId::from_uuid(
                resolution.decision_id,
            )),
        })
        .collect();
    prepared.overlay = CatalogSemanticOverlay::new(NewCatalogSemanticOverlay {
        id: overlay.id(),
        release_id: overlay.release_id(),
        source_manifest_id: overlay.source_manifest_id(),
        source_semantic_manifest_hash: overlay.source_semantic_manifest_hash().to_owned(),
        publication_plan_hash: overlay.publication_plan_hash().to_owned(),
        catalog_import_id: overlay.catalog_import_id(),
        resolver_version: overlay.resolver_version().to_owned(),
        predecessor_id: overlay.predecessor_id(),
        references,
        created_at: overlay.created_at(),
        created_by: overlay.created_by().to_owned(),
    })?;
    Ok((prepared, reused))
}

const SET_BASED_MUTATION_STATEMENT_COUNT: u32 = 19;

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

    persist_embedded_audio_identity_evidence(tx, prepared, now).await?;
    persist_acoustic_audio_identity_evidence(tx, prepared, now).await?;

    // ADR-0189: reuse an existing Artist whose normalized name is exactly equal
    // instead of creating a second row for the same name. C# resolves credits
    // against a normalized-name lookup at import (`LibraryInsertJob`), so a
    // featured artist appearing on several Releases is one Artist there; creating
    // one row per Release is what produced the duplicates. Reuse is
    // deliberately narrow: exactly one candidate may carry the name, it must not
    // be a tombstone, a disambiguation hint refuses reuse, and a conflicting
    // embedded MusicBrainz Artist identifier blocks the reuse (two same-name
    // artists with different external identity).
    let (reused_import, reused_artist_ids) = reuse_existing_artists(tx, prepared).await?;
    let prepared = &reused_import;

    // Reused Artists already exist, so they must not be inserted again.
    let artist_identities = prepared.resolutions.iter().fold(
        BTreeMap::<Uuid, &PreparedResolution>::new(),
        |mut rows, resolution| {
            if !reused_artist_ids.contains(&resolution.artist_id) {
                rows.entry(resolution.artist_id).or_insert(resolution);
            }
            rows
        },
    );
    // An import can reuse every Artist it observes (ADR-0189), in which case there
    // is nothing to insert.
    if !artist_identities.is_empty() {
        let mut artist_query: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO artists (
                id, public_api_id, canonical_name, normalized_name, sort_name,
                artist_type, status, revision, created_at, updated_at
             ) ",
        );
        artist_query.push_values(artist_identities.values(), |mut row, resolution| {
            row.push_bind(resolution.artist_id)
                .push_bind(resolution.artist_id)
                .push_bind(&resolution.raw_value)
                .push_bind(&resolution.normalized_value)
                .push_bind(&resolution.raw_value)
                .push_bind("unknown")
                .push_bind("active")
                .push_bind(1_i64)
                .push_bind(now)
                .push_bind(now);
        });
        artist_query.build().execute(&mut **tx).await?;
    }

    for resolution in &prepared.resolutions {
        for (family, external_id) in &resolution.musicbrainz_ids {
            if !matches!(family.as_str(), "artist" | "release_artist") {
                continue;
            }
            let trusted_shape = Uuid::parse_str(external_id).is_ok();
            sqlx::query(
                "INSERT INTO artist_external_identities (
                    id, artist_id, entity_type, provider, external_id,
                    provider_unique_scope, confidence, evidence_source,
                    verified_at, is_locked, created_at
                 )
                 SELECT $1,$2,'artist','musicbrainz',$3,$4,$5,$6,NULL,false,$7
                 WHERE NOT EXISTS (
                     SELECT 1 FROM artist_external_identities existing
                     WHERE existing.artist_id = $2
                       AND existing.provider = 'musicbrainz'
                       AND existing.external_id = $3
                 )",
            )
            .bind(Uuid::now_v7())
            .bind(resolution.artist_id)
            .bind(external_id)
            .bind(false)
            .bind(if trusted_shape { 9500_i32 } else { 1000_i32 })
            .bind(if trusted_shape {
                "manifest-v3:embedded-musicbrainz"
            } else {
                "manifest-v3:malformed-musicbrainz-observation"
            })
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
    }

    let mut release_external_ids = prepared
        .resolutions
        .iter()
        .filter(|resolution| resolution.track_id.is_none())
        .flat_map(|resolution| resolution.musicbrainz_ids.iter())
        .filter(|(family, _)| matches!(family.as_str(), "release" | "release_group"))
        .cloned()
        .collect::<Vec<_>>();
    release_external_ids.sort();
    release_external_ids.dedup();
    for (family, external_id) in release_external_ids {
        sqlx::query(
            "INSERT INTO release_external_identities (
                id, release_id, provider, external_id, trusted,
                confidence, evidence_source, is_locked, observed_at
             ) VALUES ($1,$2,'musicbrainz',$3,$4,$5,$6,false,$7)
             ON CONFLICT (release_id, provider, external_id) DO UPDATE
             SET trusted = release_external_identities.trusted OR EXCLUDED.trusted,
                 confidence = GREATEST(release_external_identities.confidence, EXCLUDED.confidence),
                 evidence_source = EXCLUDED.evidence_source,
                 observed_at = EXCLUDED.observed_at",
        )
        .bind(Uuid::now_v7())
        .bind(release_id)
        .bind(&external_id)
        .bind(Uuid::parse_str(&external_id).is_ok())
        .bind(if Uuid::parse_str(&external_id).is_ok() {
            9500_i32
        } else {
            1000_i32
        })
        .bind(format!("manifest-v3:embedded-musicbrainz:{family}"))
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }

    schedule_provider_enrichment(tx, release_id, artist_identities.keys().copied(), now).await?;

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
            .push_bind(if resolution.reused_release_artist {
                9000_i32
            } else {
                6000_i32
            })
            .push_bind("manifest-v3")
            .push_bind(serde_json::json!({
                "source_pointer": resolution.source_pointer,
                "provisional_bias": !resolution.reused_release_artist && !reused_artist_ids.contains(&resolution.artist_id),
                "same_release_identity": resolution.reused_release_artist,
                "reused_catalog_identity": reused_artist_ids.contains(&resolution.artist_id)
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
            .push_bind(if resolution.reused_release_artist {
                9000_i32
            } else {
                6000_i32
            })
            .push_bind(if reused_artist_ids.contains(&resolution.artist_id) {
                "reused the unambiguous existing Artist by exact normalized name and compatible evidence"
            } else if resolution.reused_release_artist {
                "resolved to the sole matching Release Artist from manifest context"
            } else {
                "created a distinct active identity from manifest context"
            })
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
            .push_bind(presentation_role_name(resolution.presentation_role))
            .push_bind(resolution.source_ordinal)
            .push_bind(&resolution.raw_value)
            .push_bind(&resolution.join_phrase)
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
                .push_bind(presentation_role_name(resolution.presentation_role))
                .push_bind(resolution.source_ordinal)
                .push_bind(&resolution.raw_value)
                .push_bind(&resolution.join_phrase)
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
            .push_bind(association_type(resolution.presentation_role))
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
         FROM tracks t WHERE t.release_id = $1
         ON CONFLICT (release_id) DO UPDATE SET
            track_count=EXCLUDED.track_count,
            duration_ms=EXCLUDED.duration_ms,
            media_bytes=EXCLUDED.media_bytes,
            revision=release_catalog_aggregates.revision+1,
            updated_at=EXCLUDED.updated_at",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let imported_track_ids = prepared
        .tracks
        .iter()
        .map(|track| track.id)
        .collect::<Vec<_>>();
    refresh_artist_catalog_aggregates(tx, &imported_track_ids, now).await?;

    // Advance reused identities once per import, then publish their actual revisions.
    // New Artists retain revision one; retries return before any mutation.
    let reused_ids = reused_artist_ids.iter().copied().collect::<Vec<_>>();
    sqlx::query("UPDATE artists SET revision=revision+1, updated_at=$2 WHERE id=ANY($1)")
        .bind(&reused_ids)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    let resolved_artist_ids = prepared
        .resolutions
        .iter()
        .map(|resolution| resolution.artist_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let resolved_artist_count = resolved_artist_ids.len();
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation, entity_revision,
            available_at, attempt_count, created_at
         )
         SELECT 'tantivy.catalog', input.entity_type, input.id, 'upsert', input.revision, $4, 0, $4
         FROM (
             SELECT 'release'::text AS entity_type, id, revision FROM releases WHERE id=$1
             UNION ALL SELECT 'track', id, revision FROM tracks WHERE id=ANY($2)
             UNION ALL SELECT 'artist', id, revision FROM artists WHERE id=ANY($3)
         ) input",
    )
    .bind(release_id)
    .bind(&imported_track_ids)
    .bind(&resolved_artist_ids)
    .bind(now)
    .execute(&mut **tx)
    .await?;

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
            "artist_count": resolved_artist_count,
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
        artist_count: u32::try_from(resolved_artist_count)
            .map_err(|_| CatalogStoreError::NumericOverflow)?,
        media_asset_count: u32::try_from(prepared.assets.len())
            .map_err(|_| CatalogStoreError::NumericOverflow)?,
        set_based_mutation_statement_count: SET_BASED_MUTATION_STATEMENT_COUNT,
        inserted: true,
    })
}

const MAX_AUDIO_IDENTITY_PROPOSALS_PER_IMPORT: usize = 256;

/// Schedules provider work for the Artists and Release an import created.
///
/// ADR-0212: scheduling is two set-based statements regardless of how many
/// Artists or providers exist. Eligibility is joined from the installation
/// record inside this transaction; there is no per-entity capability lookup,
/// network call or catalog scan on the import path.
async fn schedule_provider_enrichment(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    artist_ids: impl IntoIterator<Item = Uuid>,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    let mut artists = artist_ids.into_iter().collect::<Vec<_>>();
    artists.sort_unstable();
    artists.dedup();
    schedule_catalog_enrichment(tx, release_id, &artists, now).await?;
    Ok(())
}

/// Retains embedded MusicBrainz observations on the AudioIdentity that owns a
/// Track. Matching recording IDs are only proposals: a tag is evidence, not a
/// license to silently collapse two catalog occurrences.
/// Records the durable acoustic identity evidence an import already paid for.
///
/// Normalization runs `fpcalc` on every source file and the fingerprint is
/// retained in the sealed manifest, but nothing promoted it into
/// `audio_identity_evidence`, so `audio_identity_confirmed` could never be true
/// from acoustic data and every Track's novelty stayed `uncertain` ("lacks
/// durable fingerprint"). The digest is the catalogue-wide comparison key
/// (PRD-FP-002/PRD-FP-003); the raw fingerprint stays in the manifest.
async fn persist_acoustic_audio_identity_evidence(
    tx: &mut Transaction<'_, Postgres>,
    prepared: &PreparedImport,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    for track in &prepared.tracks {
        let Some(fingerprint) = track.acoustic_fingerprint.as_deref() else {
            continue;
        };
        if fingerprint.trim().is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO audio_identity_evidence (
                id, source_audio_identity_id, candidate_audio_identity_id,
                track_id, evidence_kind, polarity, confidence, source,
                payload, created_at
             ) VALUES ($1,$2,NULL,$3,'acoustic_fingerprint','supports',9000,
                       'manifest-v3:chromaprint',$4,$5)",
        )
        .bind(Uuid::now_v7())
        .bind(track.audio_identity_id)
        .bind(track.id)
        .bind(serde_json::json!({
            "fingerprint_digest": acoustic_fingerprint_digest(fingerprint),
            "fingerprint_length": fingerprint.trim().len(),
            "media_asset_id": track.media_asset_id,
        }))
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn persist_embedded_audio_identity_evidence(
    tx: &mut Transaction<'_, Postgres>,
    prepared: &PreparedImport,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    for track in &prepared.tracks {
        for (family, external_id) in &track.musicbrainz_ids {
            let trusted_shape = Uuid::parse_str(external_id).is_ok();
            sqlx::query(
                "INSERT INTO audio_identity_evidence (
                    id, source_audio_identity_id, candidate_audio_identity_id,
                    track_id, evidence_kind, polarity, confidence, source,
                    payload, created_at
                 ) VALUES ($1,$2,NULL,$3,'external_identifier','supports',$4,$5,$6,$7)",
            )
            .bind(Uuid::now_v7())
            .bind(track.audio_identity_id)
            .bind(track.id)
            .bind(if trusted_shape { 9500_i32 } else { 1000_i32 })
            .bind(if trusted_shape {
                "manifest-v3:embedded-musicbrainz"
            } else {
                "manifest-v3:malformed-musicbrainz-observation"
            })
            .bind(serde_json::json!({
                "family": family,
                "external_id": external_id,
                "media_asset_id": track.media_asset_id,
                "trusted_identifier_shape": trusted_shape,
            }))
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
    }

    let mut recording_groups = BTreeMap::<String, Vec<(&PreparedTrack, String)>>::new();
    for track in &prepared.tracks {
        for (family, external_id) in &track.musicbrainz_ids {
            if family == "recording" && Uuid::parse_str(external_id).is_ok() {
                recording_groups
                    .entry(external_id.to_ascii_lowercase())
                    .or_default()
                    .push((track, external_id.clone()));
            }
        }
    }
    let mut proposal_count = 0_usize;
    for (recording_id, tracks) in recording_groups {
        for left_index in 0..tracks.len() {
            for right_index in (left_index + 1)..tracks.len() {
                if proposal_count >= MAX_AUDIO_IDENTITY_PROPOSALS_PER_IMPORT {
                    return Ok(());
                }
                let source = tracks[left_index].0;
                let candidate = tracks[right_index].0;
                let already_exists = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (
                         SELECT 1 FROM audio_identity_match_decisions
                         WHERE source_audio_identity_id = $1
                           AND candidate_audio_identity_id = $2
                     )",
                )
                .bind(source.audio_identity_id)
                .bind(candidate.audio_identity_id)
                .fetch_one(&mut **tx)
                .await?;
                if already_exists {
                    continue;
                }
                let evidence_id = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO audio_identity_evidence (
                        id, source_audio_identity_id, candidate_audio_identity_id,
                        track_id, evidence_kind, polarity, confidence, source,
                        payload, created_at
                     ) VALUES ($1,$2,$3,$4,'external_identifier','supports',9500,
                               'manifest-v3:recording-id-proposal',$5,$6)",
                )
                .bind(evidence_id)
                .bind(source.audio_identity_id)
                .bind(candidate.audio_identity_id)
                .bind(source.id)
                .bind(serde_json::json!({
                    "family": "recording",
                    "external_id": recording_id,
                    "candidate_track_id": candidate.id,
                }))
                .bind(now)
                .execute(&mut **tx)
                .await?;
                let decision_id = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO audio_identity_match_decisions (
                        id, source_audio_identity_id, candidate_audio_identity_id,
                        outcome, authority, is_locked, reason,
                        predecessor_decision_id, evidence_ids, payload, created_at
                     ) VALUES ($1,$2,$3,'proposed','automated',false,
                               'matching embedded MusicBrainz recording observation',
                               NULL,$4,$5,$6)",
                )
                .bind(decision_id)
                .bind(source.audio_identity_id)
                .bind(candidate.audio_identity_id)
                .bind(serde_json::json!([evidence_id]))
                .bind(serde_json::json!({
                    "resolver_version": prepared.resolver_version,
                    "automatic_confirmation": false,
                }))
                .bind(now)
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    "INSERT INTO audio_identity_match_current (
                        source_audio_identity_id, candidate_audio_identity_id,
                        decision_id, updated_at
                     ) VALUES ($1,$2,$3,$4)",
                )
                .bind(source.audio_identity_id)
                .bind(candidate.audio_identity_id)
                .bind(decision_id)
                .bind(now)
                .execute(&mut **tx)
                .await?;
                proposal_count += 1;
            }
        }
    }
    Ok(())
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

async fn load_existing_release_result(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    semantic_hash: &str,
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
         WHERE ci.release_id = $1 AND ci.semantic_manifest_hash = $2
         ORDER BY ci.imported_at, ci.id
         LIMIT 1",
    )
    .bind(release_id)
    .bind(semantic_hash)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.3 != request.resolver_version {
        return Err(CatalogStoreError::ImportConflict(release_id));
    }
    let overlay_id = row.2.ok_or(CatalogStoreError::ImportConflict(release_id))?;
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

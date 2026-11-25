//! Durable Release work-item store: transactional claiming, leases, recovery,
//! retries, failure evidence, and append-only history.
//!
//! Concurrency model (PRD-INGEST-002, SPEC 11.5): PostgreSQL row locking with
//! `FOR UPDATE SKIP LOCKED`; every mutation and its history event commit in
//! the same transaction, so a crash never leaves a half-applied claim or an
//! untracked state change.
//!
//! Time is always supplied by the caller (`now: OffsetDateTime`) — no hidden
//! clocks in this module (testable, deterministic).

use melodee_domain::{
    FailureClass, IngestionState, ManifestPublication, PublicationAuthorization,
    PublicationJournalStep, PublicationPlan, ReleaseId, ReleaseManifest, ReleaseWorkItem,
    ReleaseWorkItemEvent, RetryPolicy, SourceDisposition, StructuralValidation, TransitionKind,
    ValidationOutcome, WorkErrorCode, WorkEventType,
};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::config::WorkEngineConfig;
use crate::error::WorkStoreError;
use crate::worker::WorkerId;

/// Rolling-window failure/completion counts for dashboard aggregation
///. Durable facts only: a "failed in window" item is one
/// currently in `failed` whose last update is inside the window; a
/// "completed in window" item carries a `completed_at` inside the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkItemWindowCounts {
    pub failed: i64,
    pub completed: i64,
}

/// One work-item list row with its catalog binding for operator queues.
///
/// The added columns are display values only: the work-item id is the
/// identity, and a bound Release never becomes the work item's identity. Every
/// added field is `Option` because a work item legitimately has no import
/// binding until it has been imported.
#[derive(Debug, Clone)]
pub struct WorkItemListRow {
    /// The durable work item (all operator-visible state and lease evidence).
    pub item: ReleaseWorkItem,
    /// Catalog Release this work item was imported into, when bound.
    pub release_id: Option<Uuid>,
    /// Display title of that Release, when bound.
    pub release_title: Option<String>,
    /// Billed display artist of that Release, when bound.
    pub display_artist: Option<String>,
    /// Total media bytes recorded by the release aggregates, when bound.
    pub media_bytes: Option<i64>,
    /// Client-facing Release status (`active` when clients can see and play it),
    /// when the work item is bound to a catalog Release.
    pub release_status: Option<String>,
}

/// One hourly bucket of completed work items for the dashboard activity
/// series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HourlyCompletionCount {
    pub hour: OffsetDateTime,
    pub count: i64,
}

/// Per-state row counts for operator diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCount {
    pub state: IngestionState,
    pub count: i64,
}

/// Sort keys accepted by the operator work-item listing. Each variant
/// maps to a constant `ORDER BY` fragment over plain indexed columns — the
/// whitelist is the injection boundary, so client input never reaches SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemSort {
    AddedAtAsc,
    AddedAtDesc,
    UpdatedAtAsc,
    UpdatedAtDesc,
    StateAsc,
    StateDesc,
    AttemptCountAsc,
    AttemptCountDesc,
}

impl WorkItemSort {
    /// Parses a strict `key:asc|desc` sort expression. Keys are camelCase API
    /// names (`addedAt`, `updatedAt`, `state`, `attemptCount`); anything else
    /// — including a missing or unknown direction suffix — is rejected.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (key, direction) = raw.split_once(':')?;
        match (key, direction) {
            ("addedAt", "asc") => Some(Self::AddedAtAsc),
            ("addedAt", "desc") => Some(Self::AddedAtDesc),
            ("updatedAt", "asc") => Some(Self::UpdatedAtAsc),
            ("updatedAt", "desc") => Some(Self::UpdatedAtDesc),
            ("state", "asc") => Some(Self::StateAsc),
            ("state", "desc") => Some(Self::StateDesc),
            ("attemptCount", "asc") => Some(Self::AttemptCountAsc),
            ("attemptCount", "desc") => Some(Self::AttemptCountDesc),
            _ => None,
        }
    }

    /// The historical default ordering (newest first).
    #[must_use]
    pub const fn default_order() -> Self {
        Self::AddedAtDesc
    }

    /// Constant SQL fragment; every variant ends with the unique `id`
    /// tiebreaker so paging is deterministic.
    const fn order_by(self) -> &'static str {
        match self {
            Self::AddedAtAsc => "created_at ASC, id",
            Self::AddedAtDesc => "created_at DESC, id",
            Self::UpdatedAtAsc => "updated_at ASC, id",
            Self::UpdatedAtDesc => "updated_at DESC, id",
            Self::StateAsc => "state ASC, created_at DESC, id",
            Self::StateDesc => "state DESC, created_at DESC, id",
            Self::AttemptCountAsc => "attempt_count ASC, id",
            Self::AttemptCountDesc => "attempt_count DESC, id",
        }
    }
}

/// Table-qualified ordering fragment for queries that join other tables owning
/// `created_at`/`updated_at` (see [`WorkItemStore::list_page_enriched`]).
///
/// Injection audit: built from the [`WorkItemSort`] whitelist only, never from
/// client text.
const fn qualified_order_by(sort: WorkItemSort) -> &'static str {
    match sort {
        WorkItemSort::AddedAtAsc => "w.created_at ASC, w.id",
        WorkItemSort::AddedAtDesc => "w.created_at DESC, w.id",
        WorkItemSort::UpdatedAtAsc => "w.updated_at ASC, w.id",
        WorkItemSort::UpdatedAtDesc => "w.updated_at DESC, w.id",
        WorkItemSort::StateAsc => "w.state ASC, w.created_at DESC, w.id",
        WorkItemSort::StateDesc => "w.state DESC, w.created_at DESC, w.id",
        WorkItemSort::AttemptCountAsc => "w.attempt_count ASC, w.id",
        WorkItemSort::AttemptCountDesc => "w.attempt_count DESC, w.id",
    }
}

/// Escapes LIKE wildcards in an operator-supplied substring so `%` and `_` are
/// matched literally (`ESCAPE '\\'` is applied by the query).
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for WorkItemListRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row as _;
        let base = WorkItemRow::from_row(row)?;
        Ok(Self {
            item: ReleaseWorkItem::try_from(base)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            release_id: row.try_get("release_id")?,
            release_title: row.try_get("title")?,
            display_artist: row.try_get("display_artist")?,
            media_bytes: row.try_get("media_bytes")?,
            release_status: row.try_get("status")?,
        })
    }
}

/// An active lease proven by PostgreSQL evidence (PRD-WORK-002, PRD-INGEST-010).
///
/// This is what the durable store can prove: a row currently holds an
/// unexpired lease. It is **not** a live-worker liveness claim — there is no
/// heartbeat/liveness store yet, so a lease owner string proves "some worker
/// claimed this item and the lease has not expired", not "that worker process
/// is alive right now". The diagnostics API exposes exactly this and nothing
/// more, so operators are never misled about worker liveness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLease {
    pub work_item_id: Uuid,
    pub api_key: Uuid,
    pub state: IngestionState,
    pub lease_owner: String,
    pub lease_acquired_at: OffsetDateTime,
    pub lease_expires_at: OffsetDateTime,
    /// Most recent durable activity (state update timestamp).
    pub last_activity_at: OffsetDateTime,
}

/// Durable inbound-to-staging ownership evidence. This is separate from the
/// logical ingestion state `Staged`: it records filesystem custody only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingHandoff {
    pub work_item_id: Uuid,
    pub staging_relative_path: String,
    pub source_generation: String,
    pub status: StagingHandoffStatus,
    pub method: Option<StagingHandoffMethod>,
    pub tree_hash: Option<String>,
    pub intended_at: OffsetDateTime,
    pub staged_at: Option<OffsetDateTime>,
    pub inbound_removed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingHandoffStatus {
    Intent,
    Staged,
    InboundRemoved,
}

impl StagingHandoffStatus {
    fn parse(value: &str) -> Result<Self, WorkStoreError> {
        match value {
            "intent" => Ok(Self::Intent),
            "staged" => Ok(Self::Staged),
            "inbound_removed" => Ok(Self::InboundRemoved),
            _ => Err(WorkStoreError::CorruptState(format!(
                "invalid staging handoff status {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingHandoffMethod {
    Rename,
    VerifiedCopy,
}

impl StagingHandoffMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::VerifiedCopy => "verified_copy",
        }
    }

    fn parse(value: &str) -> Result<Self, WorkStoreError> {
        match value {
            "rename" => Ok(Self::Rename),
            "verified_copy" => Ok(Self::VerifiedCopy),
            _ => Err(WorkStoreError::CorruptState(format!(
                "invalid staging handoff method {value}"
            ))),
        }
    }
}

#[derive(Debug, FromRow)]
struct StagingHandoffRow {
    work_item_id: Uuid,
    staging_relative_path: String,
    source_generation: String,
    status: String,
    method: Option<String>,
    tree_hash: Option<String>,
    intended_at: OffsetDateTime,
    staged_at: Option<OffsetDateTime>,
    inbound_removed_at: Option<OffsetDateTime>,
}

impl TryFrom<StagingHandoffRow> for StagingHandoff {
    type Error = WorkStoreError;

    fn try_from(row: StagingHandoffRow) -> Result<Self, Self::Error> {
        Ok(Self {
            work_item_id: row.work_item_id,
            staging_relative_path: row.staging_relative_path,
            source_generation: row.source_generation,
            status: StagingHandoffStatus::parse(&row.status)?,
            method: row
                .method
                .as_deref()
                .map(StagingHandoffMethod::parse)
                .transpose()?,
            tree_hash: row.tree_hash,
            intended_at: row.intended_at,
            staged_at: row.staged_at,
            inbound_removed_at: row.inbound_removed_at,
        })
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct WorkItemRow {
    id: Uuid,
    api_key: Uuid,
    source_library_id: Option<Uuid>,
    target_library_id: Option<Uuid>,
    source_path: String,
    current_path: String,
    target_path: Option<String>,
    source_fingerprint: Option<String>,
    manifest_hash: Option<String>,
    state: String,
    state_reason: Option<String>,
    attempt_count: i32,
    next_attempt_at: Option<OffsetDateTime>,
    lease_owner: Option<String>,
    lease_acquired_at: Option<OffsetDateTime>,
    lease_expires_at: Option<OffsetDateTime>,
    latest_settling_snapshot_id: Option<i64>,
    stable_since: Option<OffsetDateTime>,
    last_observed_at: Option<OffsetDateTime>,
    last_error_code: Option<String>,
    last_error_message: Option<String>,
    correlation_id: Uuid,
    discovered_at: OffsetDateTime,
    first_started_at: Option<OffsetDateTime>,
    last_attempt_at: Option<OffsetDateTime>,
    updated_at: OffsetDateTime,
    created_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
}

impl TryFrom<WorkItemRow> for ReleaseWorkItem {
    type Error = WorkStoreError;

    fn try_from(row: WorkItemRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            api_key: row.api_key,
            source_library_id: row.source_library_id,
            target_library_id: row.target_library_id,
            source_path: row.source_path,
            current_path: row.current_path,
            target_path: row.target_path,
            source_fingerprint: row.source_fingerprint,
            manifest_hash: row.manifest_hash,
            state: IngestionState::parse(&row.state)
                .ok_or_else(|| WorkStoreError::CorruptState(row.state.clone()))?,
            state_reason: row.state_reason,
            attempt_count: u32::try_from(row.attempt_count).map_err(|_| {
                WorkStoreError::CorruptState(format!("negative attempt_count on {}", row.id))
            })?,
            next_attempt_at: row.next_attempt_at,
            lease_owner: row.lease_owner,
            lease_acquired_at: row.lease_acquired_at,
            lease_expires_at: row.lease_expires_at,
            latest_settling_snapshot_id: row.latest_settling_snapshot_id,
            stable_since: row.stable_since,
            last_observed_at: row.last_observed_at,
            last_error_code: row.last_error_code.map(WorkErrorCode::new),
            last_error_message: row.last_error_message,
            correlation_id: row.correlation_id,
            discovered_at: row.discovered_at,
            first_started_at: row.first_started_at,
            last_attempt_at: row.last_attempt_at,
            updated_at: row.updated_at,
            created_at: row.created_at,
            completed_at: row.completed_at,
        })
    }
}

#[derive(Debug, FromRow)]
struct EventRow {
    id: i64,
    work_item_id: Uuid,
    event_type: String,
    from_state: Option<String>,
    to_state: Option<String>,
    reason_code: Option<String>,
    worker_id: Option<String>,
    correlation_id: Uuid,
    created_at: OffsetDateTime,
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, FromRow)]
struct SettlingSnapshotRow {
    id: i64,
    work_item_id: Uuid,
    observed_at: OffsetDateTime,
    signature: String,
    file_count: i32,
    total_bytes: i64,
    ready_marker_seen: bool,
    bypassed_settling: bool,
}

#[derive(Debug, Clone)]
pub struct SettlingSnapshot {
    pub id: i64,
    pub work_item_id: Uuid,
    pub observed_at: OffsetDateTime,
    pub signature: String,
    pub file_count: i32,
    pub total_bytes: i64,
    pub ready_marker_seen: bool,
    pub bypassed_settling: bool,
}

#[derive(Debug, Clone)]
pub struct SettlingSnapshotFile {
    pub relative_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub modified_at: OffsetDateTime,
}

/// Durable manifest payload and integrity identity published for one work item.
#[derive(Debug, Clone)]
pub struct StoredManifest {
    pub id: i64,
    pub work_item_id: Uuid,
    pub schema_version: i32,
    pub manifest_hash: String,
    pub payload: serde_json::Value,
    pub source_snapshot_id: Option<i64>,
    pub source_signature: Option<String>,
    pub generated_at: OffsetDateTime,
}

/// Durable immutable publication intent plus an optional completion receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPublication {
    pub work_item_id: Uuid,
    pub release_id: ReleaseId,
    pub authorization: PublicationAuthorization,
    pub validation: StructuralValidation,
    pub source_disposition: SourceDisposition,
    pub plan: PublicationPlan,
    pub plan_hash: String,
    pub authorized_at: OffsetDateTime,
    pub published_at: Option<OffsetDateTime>,
    pub sealed_manifest_hash: Option<String>,
}

/// One append-only, idempotency-keyed publication progress record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationJournalEntry {
    pub id: i64,
    pub work_item_id: Uuid,
    pub journal_key: String,
    pub step: PublicationJournalStep,
    pub asset_path: Option<String>,
    pub size_bytes: Option<u64>,
    pub byte_hash: Option<String>,
    pub worker_id: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, FromRow)]
struct PublicationRow {
    work_item_id: Uuid,
    authorization_id: Uuid,
    release_id: Uuid,
    authorization_name: String,
    authorization_version: i32,
    validation_profile_name: String,
    validation_profile_version: i32,
    validation_outcome: String,
    validation_payload: serde_json::Value,
    source_disposition: String,
    authorized_at: OffsetDateTime,
    plan_hash: String,
    plan_payload: serde_json::Value,
    published_at: Option<OffsetDateTime>,
    sealed_manifest_hash: Option<String>,
}

#[derive(Debug, FromRow)]
struct PublicationJournalRow {
    id: i64,
    work_item_id: Uuid,
    journal_key: String,
    step: String,
    asset_path: Option<String>,
    size_bytes: Option<i64>,
    byte_hash: Option<String>,
    worker_id: String,
    metadata: Option<serde_json::Value>,
    created_at: OffsetDateTime,
}

impl TryFrom<PublicationRow> for StoredPublication {
    type Error = WorkStoreError;

    fn try_from(row: PublicationRow) -> Result<Self, Self::Error> {
        let authorization_version = u16::try_from(row.authorization_version).map_err(|_| {
            WorkStoreError::CorruptState(format!(
                "invalid publication authorization version on {}",
                row.work_item_id
            ))
        })?;
        let plan: PublicationPlan = serde_json::from_value(row.plan_payload)?;
        let validation: StructuralValidation = serde_json::from_value(row.validation_payload)?;
        validation.validate()?;
        let authorization = PublicationAuthorization {
            id: row.authorization_id,
            name: row.authorization_name,
            version: authorization_version,
        };
        if plan.release_id != ReleaseId::from_uuid(row.release_id)
            || plan.authorization != authorization
            || row.source_disposition != "retained"
            || validation.profile_name != row.validation_profile_name
            || i32::from(validation.profile_version) != row.validation_profile_version
            || row.validation_outcome != "passed"
            || validation.outcome != ValidationOutcome::Passed
            || plan.plan_hash()? != row.plan_hash
        {
            return Err(WorkStoreError::CorruptState(format!(
                "publication identity mismatch on {}",
                row.work_item_id
            )));
        }
        plan.validate()?;
        Ok(Self {
            work_item_id: row.work_item_id,
            release_id: plan.release_id,
            authorization,
            validation,
            source_disposition: SourceDisposition::Retained,
            plan,
            plan_hash: row.plan_hash,
            authorized_at: row.authorized_at,
            published_at: row.published_at,
            sealed_manifest_hash: row.sealed_manifest_hash,
        })
    }
}

impl TryFrom<PublicationJournalRow> for PublicationJournalEntry {
    type Error = WorkStoreError;

    fn try_from(row: PublicationJournalRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            work_item_id: row.work_item_id,
            journal_key: row.journal_key,
            step: PublicationJournalStep::parse(&row.step)
                .ok_or_else(|| WorkStoreError::CorruptState(row.step.clone()))?,
            asset_path: row.asset_path,
            size_bytes: row
                .size_bytes
                .map(|value| {
                    u64::try_from(value).map_err(|_| {
                        WorkStoreError::CorruptState(format!(
                            "negative publication journal size on {}",
                            row.work_item_id
                        ))
                    })
                })
                .transpose()?,
            byte_hash: row.byte_hash,
            worker_id: row.worker_id,
            metadata: row.metadata,
            created_at: row.created_at,
        })
    }
}

impl TryFrom<EventRow> for ReleaseWorkItemEvent {
    type Error = WorkStoreError;

    fn try_from(row: EventRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            work_item_id: row.work_item_id,
            event_type: WorkEventType::parse(&row.event_type)
                .ok_or_else(|| WorkStoreError::CorruptState(row.event_type.clone()))?,
            from_state: row
                .from_state
                .map(|s| IngestionState::parse(&s).ok_or(WorkStoreError::CorruptState(s)))
                .transpose()?,
            to_state: row
                .to_state
                .map(|s| IngestionState::parse(&s).ok_or(WorkStoreError::CorruptState(s)))
                .transpose()?,
            reason_code: row.reason_code,
            worker_id: row.worker_id,
            correlation_id: row.correlation_id,
            created_at: row.created_at,
            metadata: row.metadata,
        })
    }
}

/// The work-item repository. Stateless; every method takes its dependencies
/// explicitly. All methods are async over a shared `PgPool`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkItemStore;

impl WorkItemStore {
    /// Persists a newly discovered work item plus its `created` event,
    /// atomically.
    #[instrument(skip_all, fields(work_item_id = %item.id, correlation_id = %item.correlation_id))]
    pub async fn create(
        &self,
        pool: &PgPool,
        item: &ReleaseWorkItem,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO release_work_items (
                id, api_key, source_library_id, target_library_id, source_path,
                current_path, target_path, source_fingerprint, manifest_hash, state,
                state_reason, attempt_count, next_attempt_at, lease_owner,
                lease_acquired_at, lease_expires_at, latest_settling_snapshot_id,
                stable_since, last_observed_at, last_error_code, last_error_message,
                correlation_id, discovered_at, first_started_at, last_attempt_at,
                updated_at, created_at, completed_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28)",
        )
        .bind(item.id)
        .bind(item.api_key)
        .bind(item.source_library_id)
        .bind(item.target_library_id)
        .bind(&item.source_path)
        .bind(&item.current_path)
        .bind(&item.target_path)
        .bind(&item.source_fingerprint)
        .bind(&item.manifest_hash)
        .bind(item.state.as_str())
        .bind(&item.state_reason)
        .bind(i32::try_from(item.attempt_count).unwrap_or(i32::MAX))
        .bind(item.next_attempt_at)
        .bind(&item.lease_owner)
        .bind(item.lease_acquired_at)
        .bind(item.lease_expires_at)
        .bind(item.latest_settling_snapshot_id)
        .bind(item.stable_since)
        .bind(item.last_observed_at)
        .bind(item.last_error_code.as_ref().map(|c| c.as_str().to_string()))
        .bind(&item.last_error_message)
        .bind(item.correlation_id)
        .bind(item.discovered_at)
        .bind(item.first_started_at)
        .bind(item.last_attempt_at)
        .bind(item.updated_at)
        .bind(item.created_at)
        .bind(item.completed_at)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            item.id,
            WorkEventType::Created,
            None,
            Some(item.state),
            None,
            None,
            item.correlation_id,
            item.created_at,
            None,
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %item.id, state = %item.state.as_str(), "work item created");
        Ok(())
    }

    /// Creates the work item if no active inbound occurrence owns this path.
    /// A successful handoff releases the path for a later inbound occurrence.
    #[instrument(
        skip_all,
        fields(work_item_id = %item.id, source_path = %item.source_path)
    )]
    pub async fn create_or_get(
        &self,
        pool: &PgPool,
        item: &ReleaseWorkItem,
    ) -> Result<(ReleaseWorkItem, bool), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let inserted = sqlx::query_as::<_, WorkItemRow>(
            "INSERT INTO release_work_items (
                id, api_key, source_library_id, target_library_id, source_path,
                current_path, target_path, source_fingerprint, manifest_hash, state,
                state_reason, attempt_count, next_attempt_at, lease_owner,
                lease_acquired_at, lease_expires_at, latest_settling_snapshot_id,
                stable_since, last_observed_at, last_error_code, last_error_message,
                correlation_id, discovered_at, first_started_at, last_attempt_at,
                updated_at, created_at, completed_at
            ) VALUES (
                $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28
            )
            ON CONFLICT DO NOTHING
            RETURNING *",
        )
        .bind(item.id)
        .bind(item.api_key)
        .bind(item.source_library_id)
        .bind(item.target_library_id)
        .bind(&item.source_path)
        .bind(&item.current_path)
        .bind(&item.target_path)
        .bind(&item.source_fingerprint)
        .bind(&item.manifest_hash)
        .bind(item.state.as_str())
        .bind(&item.state_reason)
        .bind(i32::try_from(item.attempt_count).unwrap_or(i32::MAX))
        .bind(item.next_attempt_at)
        .bind(&item.lease_owner)
        .bind(item.lease_acquired_at)
        .bind(item.lease_expires_at)
        .bind(item.latest_settling_snapshot_id)
        .bind(item.stable_since)
        .bind(item.last_observed_at)
        .bind(item.last_error_code.as_ref().map(|c| c.as_str().to_string()))
        .bind(&item.last_error_message)
        .bind(item.correlation_id)
        .bind(item.discovered_at)
        .bind(item.first_started_at)
        .bind(item.last_attempt_at)
        .bind(item.updated_at)
        .bind(item.created_at)
        .bind(item.completed_at)
        .fetch_optional(&mut *tx)
        .await?;

        let row = match inserted {
            Some(row) => {
                crate::processing::assign_registered_arrival(
                    &mut tx,
                    row.id,
                    row.source_library_id,
                    &row.source_path,
                )
                .await?;
                let row_state = row.state.clone();
                let state = IngestionState::parse(&row_state)
                    .ok_or_else(|| WorkStoreError::CorruptState(row_state))?;
                insert_event(
                    &mut tx,
                    row.id,
                    WorkEventType::Created,
                    None,
                    Some(state),
                    None,
                    None,
                    row.correlation_id,
                    row.created_at,
                    Some(serde_json::json!({
                        "source_library_id": row.source_library_id,
                        "source_path": row.source_path,
                    })),
                )
                .await?;
                (row, true)
            }
            None => {
                let row = sqlx::query_as::<_, WorkItemRow>(
                    "SELECT * FROM release_work_items
                     WHERE source_library_id IS NOT DISTINCT FROM $1
                       AND source_path = $2 AND inbound_active
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                )
                .bind(item.source_library_id)
                .bind(&item.source_path)
                .fetch_optional(&mut *tx)
                .await?;
                (row.ok_or(WorkStoreError::NotFound(item.id))?, false)
            }
        };

        tx.commit().await?;
        Ok((ReleaseWorkItem::try_from(row.0)?, row.1))
    }

    /// Looks up a work item by durable source identity.
    pub async fn get_by_source_identity(
        &self,
        pool: &PgPool,
        source_library_id: Option<Uuid>,
        source_path: &str,
    ) -> Result<Option<ReleaseWorkItem>, WorkStoreError> {
        let row = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE source_library_id IS NOT DISTINCT FROM $1 AND source_path = $2
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(source_library_id)
        .bind(source_path)
        .fetch_optional(pool)
        .await?;
        row.map(ReleaseWorkItem::try_from).transpose()
    }

    /// Lists active discovery work items used by the reconciler (`discovered`,
    /// `settling`, `ready`).
    pub async fn list_active_for_reconciliation(
        &self,
        pool: &PgPool,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE inbound_active AND state IN ('discovered', 'settling', 'ready')
             ORDER BY updated_at DESC",
        )
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(ReleaseWorkItem::try_from).collect()
    }

    /// Persists handoff intent before any filesystem operation. A live lease
    /// is required for each later receipt or cleanup mutation.
    pub async fn begin_staging_handoff(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        staging_relative_path: &str,
        source_generation: &str,
        now: OffsetDateTime,
    ) -> Result<StagingHandoff, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        let inserted = sqlx::query_as::<_, StagingHandoffRow>(
            "INSERT INTO release_work_item_staging_handoffs (
                work_item_id, staging_relative_path, source_generation, status, intended_at
             ) VALUES ($1,$2,$3,'intent',$4)
             ON CONFLICT (work_item_id) DO NOTHING
             RETURNING *",
        )
        .bind(id)
        .bind(staging_relative_path)
        .bind(source_generation)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let row = match inserted {
            Some(row) => row,
            None => sqlx::query_as::<_, StagingHandoffRow>(
                "SELECT * FROM release_work_item_staging_handoffs WHERE work_item_id = $1 FOR UPDATE",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?,
        };
        if row.staging_relative_path != staging_relative_path {
            return Err(WorkStoreError::CorruptState(format!(
                "staging path mismatch for work item {id}"
            )));
        }
        tx.commit().await?;
        StagingHandoff::try_from(row)
    }

    pub async fn staging_handoff(
        &self,
        pool: &PgPool,
        id: Uuid,
    ) -> Result<Option<StagingHandoff>, WorkStoreError> {
        sqlx::query_as::<_, StagingHandoffRow>(
            "SELECT * FROM release_work_item_staging_handoffs WHERE work_item_id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(StagingHandoff::try_from)
        .transpose()
    }

    /// Records a verified final staging tree. The inbound path remains active
    /// until it is removed or proven to have been replaced by a new arrival.
    pub async fn record_staging_receipt(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        method: StagingHandoffMethod,
        tree_hash: &str,
        now: OffsetDateTime,
    ) -> Result<StagingHandoff, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        let row = sqlx::query_as::<_, StagingHandoffRow>(
            "UPDATE release_work_item_staging_handoffs
             SET status = 'staged', method = $2, tree_hash = $3,
                 staged_at = COALESCE(staged_at, $4)
             WHERE work_item_id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(method.as_str())
        .bind(tree_hash)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        StagingHandoff::try_from(row)
    }

    pub async fn record_inbound_removed(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.finish_inbound_claim(pool, id, worker, now, "inbound_removed")
            .await
    }

    pub async fn record_inbound_replaced(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.finish_inbound_claim(pool, id, worker, now, "staged")
            .await
    }

    async fn finish_inbound_claim(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
        status: &str,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        sqlx::query(
            "UPDATE release_work_item_staging_handoffs
             SET status = $2,
                 inbound_removed_at = CASE WHEN $2 = 'inbound_removed'
                     THEN COALESCE(inbound_removed_at, $3) ELSE inbound_removed_at END
             WHERE work_item_id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE release_work_items SET inbound_active = false, updated_at = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Suppresses only the still-present acquired directory while cleanup is
    /// pending. A different generation releases the historic path for a new
    /// work item without letting the old cleanup delete it.
    pub async fn inbound_occurrence_is_acquired(
        &self,
        pool: &PgPool,
        source_library_id: Option<Uuid>,
        source_path: &str,
        source_generation: &str,
        now: OffsetDateTime,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT w.id, h.source_generation
             FROM release_work_items w
             JOIN release_work_item_staging_handoffs h ON h.work_item_id = w.id
             WHERE w.inbound_active
               AND w.source_library_id IS NOT DISTINCT FROM $1
               AND w.source_path = $2
               AND h.status IN ('staged', 'inbound_removed')
             ORDER BY w.created_at DESC, w.id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(source_library_id)
        .bind(source_path)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((id, recorded_generation)) = row else {
            tx.commit().await?;
            return Ok(false);
        };
        if recorded_generation == source_generation {
            tx.commit().await?;
            return Ok(true);
        }
        sqlx::query(
            "UPDATE release_work_items SET inbound_active = false, updated_at = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(false)
    }

    /// Inserts a new snapshot for a work item and returns the new snapshot id.
    /// Snapshot files are persisted for operator observability and to keep
    /// comparison deterministic.
    #[allow(clippy::too_many_arguments)]
    pub async fn persist_settling_snapshot(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        signature: &str,
        file_count: i32,
        total_bytes: i64,
        ready_marker_seen: bool,
        bypassed_settling: bool,
        observed_at: OffsetDateTime,
        files: &[SettlingSnapshotFile],
    ) -> Result<i64, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let snapshot = sqlx::query_as::<_, (i64,)>(
            "INSERT INTO release_work_item_settling_snapshots (
                work_item_id, observed_at, signature, file_count, total_bytes,
                ready_marker_seen, bypassed_settling
            ) VALUES ($1,$2,$3,$4,$5,$6,$7)
             RETURNING id",
        )
        .bind(work_item_id)
        .bind(observed_at)
        .bind(signature)
        .bind(file_count)
        .bind(total_bytes)
        .bind(ready_marker_seen)
        .bind(bypassed_settling)
        .fetch_one(&mut *tx)
        .await?;
        let snapshot_id = snapshot.0;

        for file in files {
            sqlx::query(
                "INSERT INTO release_work_item_settling_snapshot_files (
                    snapshot_id, relative_path, file_type, file_size, modified_at
                ) VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(snapshot_id)
            .bind(&file.relative_path)
            .bind(&file.file_type)
            .bind(file.file_size)
            .bind(file.modified_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(snapshot_id)
    }

    /// Fetches the latest persisted snapshot for an item.
    pub async fn latest_snapshot(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Option<SettlingSnapshot>, WorkStoreError> {
        let row = sqlx::query_as::<_, SettlingSnapshotRow>(
            "SELECT * FROM release_work_item_settling_snapshots
             WHERE work_item_id = $1
             ORDER BY observed_at DESC, id DESC
             LIMIT 1",
        )
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(SettlingSnapshot {
            id: row.id,
            work_item_id: row.work_item_id,
            observed_at: row.observed_at,
            signature: row.signature,
            file_count: row.file_count,
            total_bytes: row.total_bytes,
            ready_marker_seen: row.ready_marker_seen,
            bypassed_settling: row.bypassed_settling,
        }))
    }

    /// Publishes a complete manifest and advances `inspecting -> normalizing`
    /// in one transaction. The unique work-item constraint makes publication
    /// idempotent and prevents competing workers from becoming authoritative.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_manifest(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        manifest: &ReleaseManifest,
        manifest_hash: &str,
        source_snapshot_id: Option<i64>,
        source_signature: Option<&str>,
        generation_duration_ms: i64,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Inspecting {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        item.state
            .ensure_transition(IngestionState::Normalizing, TransitionKind::Worker)?;
        let payload = serde_json::to_value(manifest)?;
        let existing = sqlx::query_as::<_, (String,)>(
            "SELECT manifest_hash FROM release_work_item_manifests WHERE work_item_id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((existing_hash,)) = existing {
            if existing_hash != manifest_hash {
                return Err(WorkStoreError::ManifestConflict(id));
            }
        } else {
            sqlx::query(
                "INSERT INTO release_work_item_manifests (
                    work_item_id, schema_version, manifest_hash, source_snapshot_id,
                    source_signature, manifest_payload, ffprobe_path, ffprobe_version,
                    tool_metadata, source_stable, generation_duration_ms, generated_at
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            )
            .bind(id)
            .bind(i32::from(manifest.schema_version))
            .bind(manifest_hash)
            .bind(source_snapshot_id)
            .bind(source_signature)
            .bind(payload)
            .bind(manifest.inspection.ffprobe_path.clone())
            .bind(manifest.inspection.ffprobe_version.clone())
            .bind(serde_json::json!({ "schema_version": manifest.schema_version }))
            .bind(manifest.inspection.source_stable)
            .bind(generation_duration_ms.max(0))
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "UPDATE release_work_items
             SET state = 'normalizing', state_reason = 'inspection-complete',
                 manifest_hash = $2, lease_owner = NULL, lease_acquired_at = NULL,
                 lease_expires_at = NULL, updated_at = $3
             WHERE id = $1",
        )
        .bind(id)
        .bind(manifest_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(IngestionState::Inspecting),
            Some(IngestionState::Normalizing),
            Some("inspection-complete"),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "manifest_hash": manifest_hash,
                "schema_version": manifest.schema_version,
            })),
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, manifest_hash, "manifest published");
        Ok(())
    }

    /// Atomically claims up to `config.claim_batch_size` work items in the
    /// `normalizing` state that have no active lease, establishing leases for
    /// `worker`. Returns only items actually owned by this worker.
    #[instrument(skip_all, fields(worker_id = %worker, batch = config.claim_batch_size))]
    pub async fn claim_normalizing(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let lease_expiry = now + config.lease_duration;
        let mut tx = pool.begin().await?;
        // Select candidates with row locks (skipped when locked by rivals).
        // Only normalizing items with expired/no leases are eligible.
        let candidates =
            crate::processing::claim_candidates(&mut tx, "normalization", config, now).await?;

        let mut items = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let row = sqlx::query_as::<_, WorkItemRow>(
                "UPDATE release_work_items w
                 SET lease_owner = $1,
                     lease_acquired_at = $2,
                     lease_expires_at = $3,
                     first_started_at = COALESCE(w.first_started_at, $2),
                     updated_at = $2
                 WHERE w.id = $4
                 RETURNING w.*",
            )
            .bind(worker.as_str())
            .bind(now)
            .bind(lease_expiry)
            .bind(candidate.id)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE release_work_item_manifests
                 SET normalization_started_at = $2,
                     normalization_completed_at = NULL,
                     normalization_provenance = NULL
                 WHERE work_item_id = $1",
            )
            .bind(candidate.id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            let item = ReleaseWorkItem::try_from(row)?;
            insert_event(
                &mut tx,
                item.id,
                WorkEventType::Claimed,
                None,
                None,
                None,
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({ "lease_expires_at": lease_expiry.to_string() })),
            )
            .await?;
            items.push(item);
        }
        tx.commit().await?;
        if items.is_empty() {
            debug!(worker_id = %worker, "claim_normalizing: no eligible work");
        } else {
            info!(worker_id = %worker, claimed = items.len(), "normalization lease acquired");
        }
        Ok(items)
    }

    /// Claims normalized manifests awaiting structural validation. Validation
    /// has its own lease so a completed normalization never retains authority
    /// across the stage boundary.
    #[instrument(skip_all, fields(worker_id = %worker, batch = config.claim_batch_size))]
    pub async fn claim_validating(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let lease_expiry = now + config.lease_duration;
        let mut tx = pool.begin().await?;
        let candidates =
            crate::processing::claim_candidates(&mut tx, "validation", config, now).await?;

        let mut items = Vec::with_capacity(candidates.len());
        for candidate in candidates {
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
            .bind(candidate.id)
            .fetch_one(&mut *tx)
            .await?;
            let item = ReleaseWorkItem::try_from(row)?;
            insert_event(
                &mut tx,
                item.id,
                WorkEventType::Claimed,
                None,
                None,
                Some("validation.claimed"),
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({ "lease_expires_at": lease_expiry.to_string() })),
            )
            .await?;
            items.push(item);
        }
        tx.commit().await?;
        Ok(items)
    }

    /// Parks structurally uncertain work for operator review without treating
    /// uncertainty as a failed attempt. Full file-level findings remain in
    /// the atomic transition event.
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn record_validation_review(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        validation: &StructuralValidation,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        validation.validate()?;
        if validation.outcome != ValidationOutcome::NeedsReview {
            return Err(WorkStoreError::PublicationConflict(id));
        }
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Validating {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        item.state
            .ensure_transition(IngestionState::NeedsReview, TransitionKind::Worker)?;
        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = 'needs_review', state_reason = 'validation.needs_review',
                 lease_owner = NULL, lease_acquired_at = NULL,
                 lease_expires_at = NULL, updated_at = $2
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(IngestionState::Validating),
            Some(IngestionState::NeedsReview),
            Some("validation.needs_review"),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::to_value(validation)?),
        )
        .await?;
        tx.commit().await?;
        ReleaseWorkItem::try_from(row)
    }

    /// Publishes normalization observations for a work item and transitions
    /// `normalizing -> validating` in one transaction. Updates the manifest
    /// payload with the new normalization observations and provenance,
    /// recomputes the semantic hash, and verifies the lease to prevent stale
    /// workers from publishing authoritative state.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn publish_normalization(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        manifest: &ReleaseManifest,
        manifest_hash: &str,
        source_file: &str,
        duration_ms: u64,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Normalizing {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }

        let payload = serde_json::to_value(manifest)?;
        let provenance = manifest
            .normalization
            .as_ref()
            .map(|n| serde_json::to_value(&n.provenance))
            .transpose()?
            .unwrap_or(serde_json::Value::Null);

        item.state
            .ensure_transition(IngestionState::Validating, TransitionKind::Worker)?;
        let manifest_update = sqlx::query(
            "UPDATE release_work_item_manifests
             SET schema_version = $2,
                 manifest_payload = $3,
                 manifest_hash = $4,
                 normalization_provenance = $5,
                 normalization_completed_at = $6
             WHERE work_item_id = $1",
        )
        .bind(id)
        .bind(i32::from(manifest.schema_version))
        .bind(payload)
        .bind(manifest_hash)
        .bind(provenance)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if manifest_update.rows_affected() != 1 {
            return Err(WorkStoreError::ManifestNotFound(id));
        }

        sqlx::query(
            "UPDATE release_work_items
             SET state = 'validating',
                 state_reason = 'normalization-complete',
                 manifest_hash = $2,
                 lease_owner = NULL,
                 lease_acquired_at = NULL,
                 lease_expires_at = NULL,
                 updated_at = $3
             WHERE id = $1",
        )
        .bind(id)
        .bind(manifest_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(IngestionState::Normalizing),
            Some(IngestionState::Validating),
            Some("normalization-complete"),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "manifest_hash": manifest_hash,
                "normalization_duration_ms": duration_ms,
                "source_file": source_file,
            })),
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, manifest_hash, "normalization published");
        Ok(())
    }

    /// Returns the authoritative manifest for a work item, if inspection has
    /// completed successfully.
    pub async fn get_manifest(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Option<StoredManifest>, WorkStoreError> {
        let row = sqlx::query_as::<
            _,
            (
                i64,
                Uuid,
                i32,
                String,
                serde_json::Value,
                Option<i64>,
                Option<String>,
                OffsetDateTime,
            ),
        >(
            "SELECT id, work_item_id, schema_version, manifest_hash, manifest_payload,
                    source_snapshot_id, source_signature, generated_at
             FROM release_work_item_manifests WHERE work_item_id = $1",
        )
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(
            |(
                id,
                work_item_id,
                schema_version,
                manifest_hash,
                payload,
                source_snapshot_id,
                source_signature,
                generated_at,
            )| StoredManifest {
                id,
                work_item_id,
                schema_version,
                manifest_hash,
                payload,
                source_snapshot_id,
                source_signature,
                generated_at,
            },
        ))
    }

    /// Atomically persists immutable authorization and publication intent,
    /// upgrades the authoritative manifest to v3, and transitions
    /// `validating -> staged`. Staged work intentionally holds no lease.
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn stage_authorized_publication(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        manifest: &ReleaseManifest,
        plan: &PublicationPlan,
        now: OffsetDateTime,
    ) -> Result<StoredPublication, WorkStoreError> {
        if manifest.schema_version != melodee_domain::PUBLICATION_MANIFEST_VERSION
            || manifest.normalization.is_none()
        {
            return Err(WorkStoreError::PublicationConflict(id));
        }
        let publication = manifest
            .publication
            .as_ref()
            .ok_or(WorkStoreError::PublicationConflict(id))?;
        publication.validate_against(plan)?;
        if manifest.work_item_id != id {
            return Err(WorkStoreError::PublicationConflict(id));
        }

        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Validating {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        item.state
            .ensure_transition(IngestionState::Staged, TransitionKind::Worker)?;

        let stored_manifest = sqlx::query_as::<_, (i32, String)>(
            "SELECT schema_version, manifest_hash FROM release_work_item_manifests
             WHERE work_item_id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(WorkStoreError::ManifestNotFound(id))?;
        if stored_manifest.0 != i32::from(melodee_domain::NORMALIZATION_MANIFEST_VERSION)
            || stored_manifest.1 != plan.input_manifest_hash
            || item.manifest_hash.as_deref() != Some(plan.input_manifest_hash.as_str())
        {
            return Err(WorkStoreError::PublicationConflict(id));
        }

        let validation_payload = serde_json::to_value(&publication.validation)?;
        let plan_payload = serde_json::to_value(plan)?;
        let manifest_payload = serde_json::to_value(manifest)?;
        sqlx::query(
            "INSERT INTO release_publication_authorizations (
                authorization_id, work_item_id, release_id, authorization_name,
                authorization_version, validation_profile_name,
                validation_profile_version, validation_outcome, validation_payload,
                source_disposition, authorized_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,'passed',$8,'retained',$9)",
        )
        .bind(publication.authorization.id)
        .bind(id)
        .bind(publication.release_id.into_uuid())
        .bind(&publication.authorization.name)
        .bind(i32::from(publication.authorization.version))
        .bind(&publication.validation.profile_name)
        .bind(i32::from(publication.validation.profile_version))
        .bind(validation_payload)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO release_publication_plans (
                work_item_id, authorization_id, release_id, layout_version,
                input_manifest_hash, target_relative_path, manifest_relative_path,
                plan_hash, plan_payload, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(id)
        .bind(plan.authorization.id)
        .bind(plan.release_id.into_uuid())
        .bind(i32::from(plan.layout_version))
        .bind(&plan.input_manifest_hash)
        .bind(&plan.target_relative_path)
        .bind(&plan.manifest_relative_path)
        .bind(&publication.plan_hash)
        .bind(plan_payload)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE release_work_item_manifests
             SET schema_version = $2, manifest_payload = $3
             WHERE work_item_id = $1",
        )
        .bind(id)
        .bind(i32::from(manifest.schema_version))
        .bind(manifest_payload)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE release_work_items
             SET state = 'staged', state_reason = 'phase3.structural-publication',
                 target_path = $2, lease_owner = NULL, lease_acquired_at = NULL,
                 lease_expires_at = NULL, updated_at = $3
             WHERE id = $1",
        )
        .bind(id)
        .bind(&plan.target_relative_path)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(IngestionState::Validating),
            Some(IngestionState::Staged),
            Some(PublicationAuthorization::PHASE3_STRUCTURAL_NAME),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "release_id": publication.release_id,
                "authorization_id": publication.authorization.id,
                "plan_hash": publication.plan_hash,
                "source_disposition": publication.source_disposition,
            })),
        )
        .await?;
        tx.commit().await?;

        Ok(stored_publication_from_staged(id, publication, plan, now))
    }

    /// Claims new staged publications and directly reclaims convergent
    /// `publishing` work after lease expiry. Reclamation never routes through
    /// the ordinary Failed retry queue.
    #[instrument(skip_all, fields(worker_id = %worker, batch = config.claim_batch_size))]
    pub async fn claim_publication(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let lease_expiry = now + config.lease_duration;
        let mut tx = pool.begin().await?;
        let candidates =
            crate::processing::claim_candidates(&mut tx, "publication", config, now).await?;

        let mut items = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let from = IngestionState::parse(&candidate.state)
                .ok_or_else(|| WorkStoreError::CorruptState(candidate.state.clone()))?;
            if from == IngestionState::Staged {
                from.ensure_transition(IngestionState::Publishing, TransitionKind::Worker)?;
            }
            let row = sqlx::query_as::<_, WorkItemRow>(
                "UPDATE release_work_items
                 SET state = 'publishing', state_reason = $1,
                     lease_owner = $2, lease_acquired_at = $3,
                     lease_expires_at = $4, updated_at = $3
                 WHERE id = $5
                 RETURNING *",
            )
            .bind(if from == IngestionState::Staged {
                "publication.started"
            } else {
                "publication.reclaimed"
            })
            .bind(worker.as_str())
            .bind(now)
            .bind(lease_expiry)
            .bind(candidate.id)
            .fetch_one(&mut *tx)
            .await?;
            let item = ReleaseWorkItem::try_from(row)?;
            if from == IngestionState::Staged {
                insert_event(
                    &mut tx,
                    item.id,
                    WorkEventType::Transitioned,
                    Some(IngestionState::Staged),
                    Some(IngestionState::Publishing),
                    Some("publication.started"),
                    Some(worker.as_str()),
                    item.correlation_id,
                    now,
                    None,
                )
                .await?;
            }
            insert_event(
                &mut tx,
                item.id,
                WorkEventType::Claimed,
                None,
                None,
                Some(if from == IngestionState::Staged {
                    "publication.claimed"
                } else {
                    "publication.reclaimed"
                }),
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({ "lease_expires_at": lease_expiry.to_string() })),
            )
            .await?;
            items.push(item);
        }
        tx.commit().await?;
        Ok(items)
    }

    /// Appends an idempotency-keyed publication milestone under the active
    /// publishing lease. Reusing a key with different evidence fails closed.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker, step = step.as_str()))]
    pub async fn record_publication_progress(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        journal_key: &str,
        step: PublicationJournalStep,
        asset_path: Option<&str>,
        size_bytes: Option<u64>,
        byte_hash: Option<&str>,
        metadata: Option<serde_json::Value>,
        now: OffsetDateTime,
    ) -> Result<PublicationJournalEntry, WorkStoreError> {
        if journal_key.is_empty() {
            return Err(WorkStoreError::PublicationJournalConflict(
                id,
                journal_key.to_owned(),
            ));
        }
        let stored_size = size_bytes
            .map(|value| i64::try_from(value).map_err(|_| WorkStoreError::PublicationConflict(id)))
            .transpose()?;
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Publishing {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }

        let existing = sqlx::query_as::<_, PublicationJournalRow>(
            "SELECT * FROM release_publication_journal
             WHERE work_item_id = $1 AND journal_key = $2",
        )
        .bind(id)
        .bind(journal_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = existing {
            let entry = PublicationJournalEntry::try_from(row)?;
            if entry.step != step
                || entry.asset_path.as_deref() != asset_path
                || entry.size_bytes != size_bytes
                || entry.byte_hash.as_deref() != byte_hash
                || entry.metadata != metadata
            {
                return Err(WorkStoreError::PublicationJournalConflict(
                    id,
                    journal_key.to_owned(),
                ));
            }
            tx.commit().await?;
            return Ok(entry);
        }

        if step == PublicationJournalStep::AssetCopied {
            let plan_payload: (serde_json::Value,) = sqlx::query_as(
                "SELECT plan_payload FROM release_publication_plans WHERE work_item_id = $1",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(WorkStoreError::PublicationNotFound(id))?;
            let plan: PublicationPlan = serde_json::from_value(plan_payload.0)?;
            let matches_plan = plan.assets.iter().any(|asset| {
                Some(asset.canonical_path.as_str()) == asset_path
                    && Some(asset.size_bytes) == size_bytes
                    && Some(asset.byte_hash.as_str()) == byte_hash
            });
            if !matches_plan {
                return Err(WorkStoreError::PublicationConflict(id));
            }
        }

        let row = sqlx::query_as::<_, PublicationJournalRow>(
            "INSERT INTO release_publication_journal (
                work_item_id, journal_key, step, asset_path, size_bytes,
                byte_hash, worker_id, metadata, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
             RETURNING *",
        )
        .bind(id)
        .bind(journal_key)
        .bind(step.as_str())
        .bind(asset_path)
        .bind(stored_size)
        .bind(byte_hash)
        .bind(worker.as_str())
        .bind(metadata)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        PublicationJournalEntry::try_from(row)
    }

    /// Records a convergent publication problem without discarding durable
    /// intent or routing the item through the ordinary Failed retry queue.
    /// The cleared lease makes Publishing directly reclaimable.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker, step = step.as_str(), error_code = %code.as_str()))]
    pub async fn record_publication_problem(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        step: PublicationJournalStep,
        code: WorkErrorCode,
        message: &str,
        metadata: Option<serde_json::Value>,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        if !matches!(
            step,
            PublicationJournalStep::TargetConflict | PublicationJournalStep::Failed
        ) {
            return Err(WorkStoreError::PublicationConflict(id));
        }
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Publishing {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        let durable_plan_hash: (String,) = sqlx::query_as(
            "SELECT plan_hash FROM release_publication_plans WHERE work_item_id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(WorkStoreError::PublicationNotFound(id))?;
        let journal_key = format!("{}:{}", step.as_str(), durable_plan_hash.0);
        let journal_metadata = Some(serde_json::json!({
            "error_code": code.as_str(),
            "evidence": metadata,
        }));
        let existing = sqlx::query_as::<_, PublicationJournalRow>(
            "SELECT * FROM release_publication_journal
             WHERE work_item_id = $1 AND journal_key = $2",
        )
        .bind(id)
        .bind(&journal_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            let entry = PublicationJournalEntry::try_from(existing)?;
            if entry.step != step
                || entry.asset_path.is_some()
                || entry.size_bytes.is_some()
                || entry.byte_hash.is_some()
                || entry.metadata != journal_metadata
            {
                return Err(WorkStoreError::PublicationJournalConflict(id, journal_key));
            }
        } else {
            sqlx::query(
                "INSERT INTO release_publication_journal (
                    work_item_id, journal_key, step, worker_id, metadata, created_at
                 ) VALUES ($1,$2,$3,$4,$5,$6)",
            )
            .bind(id)
            .bind(&journal_key)
            .bind(step.as_str())
            .bind(worker.as_str())
            .bind(&journal_metadata)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state_reason = $2, last_error_code = $2,
                 last_error_message = $3, lease_owner = NULL,
                 lease_acquired_at = NULL, lease_expires_at = NULL,
                 updated_at = $4
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(code.as_str())
        .bind(message)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(IngestionState::Publishing),
            Some(IngestionState::Publishing),
            Some(code.as_str()),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            journal_metadata,
        )
        .await?;
        tx.commit().await?;
        ReleaseWorkItem::try_from(row)
    }

    /// Commits a final verified canonical publication receipt and transitions
    /// `publishing -> published`, atomically clearing the publication lease.
    /// Repeating an identical completion after uncertain commit is success.
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn complete_publication(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        plan_hash: &str,
        sealed_manifest_hash: &str,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        let plan_row = sqlx::query_as::<_, (Uuid, String, String, serde_json::Value)>(
            "SELECT release_id, plan_hash, target_relative_path, plan_payload
             FROM release_publication_plans WHERE work_item_id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(WorkStoreError::PublicationNotFound(id))?;
        let plan: PublicationPlan = serde_json::from_value(plan_row.3.clone())?;
        plan.validate()?;
        if plan_row.0 != plan.release_id.into_uuid()
            || plan_row.1 != plan_hash
            || plan.plan_hash()? != plan_hash
        {
            return Err(WorkStoreError::PublicationConflict(id));
        }

        let journal_rows = sqlx::query_as::<_, PublicationJournalRow>(
            "SELECT * FROM release_publication_journal
             WHERE work_item_id = $1 ORDER BY created_at, id",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let journal = journal_rows
            .into_iter()
            .map(PublicationJournalEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        if !has_complete_publication_evidence(&plan, plan_hash, sealed_manifest_hash, &journal) {
            return Err(WorkStoreError::PublicationConflict(id));
        }

        if item.state == IngestionState::Published {
            let receipt = sqlx::query_as::<_, (String, String)>(
                "SELECT plan_hash, sealed_manifest_hash
                 FROM release_publication_receipts WHERE work_item_id = $1",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(WorkStoreError::PublicationNotFound(id))?;
            if receipt.0 != plan_hash || receipt.1 != sealed_manifest_hash {
                return Err(WorkStoreError::PublicationConflict(id));
            }
            tx.commit().await?;
            return Ok(item);
        }

        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Publishing {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        item.state
            .ensure_transition(IngestionState::Published, TransitionKind::Worker)?;
        sqlx::query(
            "INSERT INTO release_publication_receipts (
                work_item_id, release_id, plan_hash, sealed_manifest_hash,
                source_disposition, published_at
             ) VALUES ($1,$2,$3,$4,'retained',$5)",
        )
        .bind(id)
        .bind(plan_row.0)
        .bind(plan_hash)
        .bind(sealed_manifest_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = 'published', state_reason = 'publication.verified',
                 target_path = $2, lease_owner = NULL, lease_acquired_at = NULL,
                 lease_expires_at = NULL, updated_at = $3
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(&plan_row.2)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(IngestionState::Publishing),
            Some(IngestionState::Published),
            Some("publication.verified"),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "release_id": plan_row.0,
                "plan_hash": plan_hash,
                "sealed_manifest_hash": sealed_manifest_hash,
                "source_disposition": "retained",
            })),
        )
        .await?;
        tx.commit().await?;
        ReleaseWorkItem::try_from(row)
    }

    /// Fetches immutable publication intent and its optional completion receipt.
    pub async fn get_publication(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Option<StoredPublication>, WorkStoreError> {
        let row = sqlx::query_as::<_, PublicationRow>(
            "SELECT p.work_item_id, a.authorization_id, p.release_id,
                    a.authorization_name, a.authorization_version,
                    a.validation_profile_name, a.validation_profile_version,
                    a.validation_outcome, a.validation_payload,
                    a.source_disposition, a.authorized_at,
                    p.plan_hash, p.plan_payload, r.published_at,
                    r.sealed_manifest_hash
             FROM release_publication_plans p
             JOIN release_publication_authorizations a
               ON a.authorization_id = p.authorization_id
             LEFT JOIN release_publication_receipts r
               ON r.work_item_id = p.work_item_id
             WHERE p.work_item_id = $1",
        )
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?;
        row.map(StoredPublication::try_from).transpose()
    }

    /// Hard-deletes one failed, pre-catalog work item and its processing
    /// evidence. The caller has already removed the owned storage trees; this
    /// transaction refuses a cataloged item so a purge can never orphan or
    /// rewrite catalog identity. An operator audit row remains in
    /// `catalog_audit_events`, deliberately independent of the deleted rows.
    pub async fn purge_pre_catalog_work_item(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        actor: &str,
        reason: &str,
        deleted_paths: serde_json::Value,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        if actor.trim().is_empty() || reason.trim().is_empty() {
            return Err(WorkStoreError::InvalidRequest("purge actor or reason"));
        }
        if !deleted_paths.is_object() {
            return Err(WorkStoreError::InvalidRequest("purge path evidence"));
        }
        let mut tx = pool.begin().await?;
        let row = sqlx::query_as::<_, (String,)>(
            "SELECT state FROM release_work_items WHERE id = $1 FOR UPDATE",
        )
        .bind(work_item_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((state,)) = row else {
            tx.rollback().await?;
            return Err(WorkStoreError::NotFound(work_item_id));
        };
        // ADR-0211: an operator may hard-delete a pre-catalog work item at any
        // lifecycle state. Parsing still fails closed on a corrupt state
        // value; the row lock above fences a concurrent transition, and the
        // catalog binding check below still refuses to remove a Release's
        // processing record through this path.
        let _state = IngestionState::parse(&state)
            .ok_or_else(|| WorkStoreError::CorruptState(state.clone()))?;
        let cataloged = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM catalog_imports WHERE work_item_id = $1)",
        )
        .bind(work_item_id)
        .fetch_one(&mut *tx)
        .await?;
        if cataloged {
            tx.rollback().await?;
            return Err(WorkStoreError::Cataloged(work_item_id));
        }
        // Publication evidence has RESTRICT foreign keys and is removed in
        // dependency order before the work item.
        for statement in [
            "DELETE FROM release_publication_journal WHERE work_item_id = $1",
            "DELETE FROM release_publication_receipts WHERE work_item_id = $1",
            "DELETE FROM release_publication_plans WHERE work_item_id = $1",
            "DELETE FROM release_publication_authorizations WHERE work_item_id = $1",
        ] {
            sqlx::query(statement)
                .bind(work_item_id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO catalog_audit_events
                (event_id, entity_type, entity_id, event_type, actor, reason, payload, created_at)
             VALUES ($1, 'release_work_item', $2, 'work_item_purged', $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(work_item_id)
        .bind(actor)
        .bind(reason)
        .bind(&deleted_paths)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let deleted = sqlx::query("DELETE FROM release_work_items WHERE id = $1")
            .bind(work_item_id)
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(WorkStoreError::NotFound(work_item_id));
        }
        tx.commit().await?;
        info!(work_item_id = %work_item_id, actor, "pre-catalog work item purged");
        Ok(())
    }

    /// Lists append-only canonical publication progress, oldest first.
    pub async fn list_publication_journal(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Vec<PublicationJournalEntry>, WorkStoreError> {
        let rows = sqlx::query_as::<_, PublicationJournalRow>(
            "SELECT * FROM release_publication_journal
             WHERE work_item_id = $1 ORDER BY created_at, id",
        )
        .bind(work_item_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(PublicationJournalEntry::try_from)
            .collect()
    }

    /// Updates durable settling progress fields without changing core state.
    pub async fn upsert_settling_progress(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        stable_since: Option<OffsetDateTime>,
        latest_snapshot_id: Option<i64>,
        last_observed_at: OffsetDateTime,
        state_reason: Option<&str>,
    ) -> Result<(), WorkStoreError> {
        sqlx::query(
            "UPDATE release_work_items
             SET stable_since = $2,
                 latest_settling_snapshot_id = $3,
                 last_observed_at = $4,
                 state_reason = COALESCE($5, state_reason),
                 updated_at = $4
             WHERE id = $1",
        )
        .bind(work_item_id)
        .bind(stable_since)
        .bind(latest_snapshot_id)
        .bind(last_observed_at)
        .bind(state_reason)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Atomically claims up to `config.claim_batch_size` eligible items for
    /// `worker`, establishing leases. Eligible: `ready` (not scheduled in the
    /// future) plus `failed` items whose retry time has arrived and whose
    /// attempts remain — those are transitioned `failed -> ready` (System) in
    /// the same transaction. Items with active leases are never claimed.
    ///
    /// Returns only work actually owned by this worker.
    #[instrument(skip_all, fields(worker_id = %worker, batch = config.claim_batch_size))]
    pub async fn claim(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let lease_expiry = now + config.lease_duration;
        let mut tx = pool.begin().await?;
        // Select candidates with row locks (skipped when locked by rivals),
        // then update each within the same transaction. Two steps so the
        // pre-update state is known for the failed->ready requeue event.
        // A continuous flow of fresh Ready work must not starve a recovered inspection.
        let candidates =
            crate::processing::claim_candidates(&mut tx, "inspection", config, now).await?;

        let mut items = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let was_failed = candidate.state == "failed";
            let row = sqlx::query_as::<_, WorkItemRow>(
                "UPDATE release_work_items w
                 SET state = CASE WHEN w.state = 'failed' THEN 'ready' ELSE w.state END,
                     state_reason = CASE WHEN w.state = 'failed' THEN NULL ELSE w.state_reason END,
                     next_attempt_at = NULL,
                     lease_owner = $1,
                     lease_acquired_at = $2,
                     lease_expires_at = $3,
                     first_started_at = COALESCE(w.first_started_at, $2),
                     updated_at = $2
                 WHERE w.id = $4
                 RETURNING w.*",
            )
            .bind(worker.as_str())
            .bind(now)
            .bind(lease_expiry)
            .bind(candidate.id)
            .fetch_one(&mut *tx)
            .await?;
            let item = ReleaseWorkItem::try_from(row)?;
            if was_failed {
                insert_event(
                    &mut tx,
                    item.id,
                    WorkEventType::Transitioned,
                    Some(IngestionState::Failed),
                    Some(IngestionState::Ready),
                    Some("retry.eligible"),
                    Some(worker.as_str()),
                    item.correlation_id,
                    now,
                    None,
                )
                .await?;
            }
            insert_event(
                &mut tx,
                item.id,
                WorkEventType::Claimed,
                None,
                None,
                None,
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({ "lease_expires_at": lease_expiry.to_string() })),
            )
            .await?;
            items.push(item);
        }
        tx.commit().await?;
        if items.is_empty() {
            debug!(worker_id = %worker, "claim: no eligible work");
        } else {
            info!(worker_id = %worker, claimed = items.len(), "lease acquired");
        }
        Ok(items)
    }

    /// System-kind transitions (discovery, settling, retry requeue): no lease
    /// required, but an active lease held by a worker blocks system moves.
    /// Used by discovery/settling and by tests/diagnostics.
    #[instrument(skip_all, fields(work_item_id = %id, to_state = to.as_str()))]
    pub async fn system_transition(
        &self,
        pool: &PgPool,
        id: Uuid,
        to: IngestionState,
        reason: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        if matches!(
            item.state,
            IngestionState::Cataloging | IngestionState::Cataloged
        ) || matches!(to, IngestionState::Cataloging | IngestionState::Cataloged)
        {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        if item.has_active_lease(now) {
            return Err(WorkStoreError::LeaseConflict(id));
        }
        item.state.ensure_transition(to, TransitionKind::System)?;
        let from = item.state;
        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = $2, state_reason = $3, updated_at = $4
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(to.as_str())
        .bind(reason)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(from),
            Some(to),
            reason,
            None,
            item.correlation_id,
            now,
            None,
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, from_state = %from.as_str(), to_state = %to.as_str(), "state transitioned");
        ReleaseWorkItem::try_from(row)
    }

    /// Advances state for a leased worker or an operator. Illegal transitions
    /// fail atomically without changing persisted state.
    // The parameter list mirrors the persisted transition contract; bundling
    // into a request struct would only rename the same surface.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(work_item_id = %id, to_state = to.as_str(), ?kind))]
    pub async fn transition(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: Option<&WorkerId>,
        kind: TransitionKind,
        to: IngestionState,
        reason: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;

        if item.state == IngestionState::NeedsReview && sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM review_deletion_authorizations WHERE work_item_id=$1)"
        ).bind(id).fetch_one(&mut *tx).await? {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }

        // These boundaries have additional durable invariants and must use
        // stage_authorized_publication, claim_publication, and
        // complete_publication respectively. A generic state-only transition
        // could otherwise create unpublishable or falsely published work.
        if matches!(
            item.state,
            IngestionState::Cataloging | IngestionState::Cataloged
        ) || matches!(
            to,
            IngestionState::Staged
                | IngestionState::Publishing
                | IngestionState::Published
                | IngestionState::Cataloging
                | IngestionState::Cataloged
                | IngestionState::Deleted
        ) {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }

        // Ownership rules: workers must hold an active lease; operators must
        // not race someone else's active lease.
        match kind {
            TransitionKind::Worker => {
                let worker = worker.ok_or(WorkStoreError::InvalidState(id, item.state))?;
                ensure_active_lease(&item, worker, now)?;
            }
            TransitionKind::Operator => {
                if let Some(owner) = &item.lease_owner
                    && item.has_active_lease(now)
                    && Some(owner.as_str()) != worker.map(WorkerId::as_str)
                {
                    return Err(WorkStoreError::LeaseConflict(id));
                }
            }
            TransitionKind::System => {
                return Err(WorkStoreError::InvalidState(id, item.state));
            }
        }

        item.state.ensure_transition(to, kind)?;
        let from = item.state;
        let terminal = to.is_terminal();
        let completed_at = if terminal { Some(now) } else { None };
        // Leases belong to active processing: terminal states, operator
        // decisions, and parking in NeedsReview all release the lease so the
        // item is never stranded behind a dead lease.
        let clear_lease = terminal
            || kind == TransitionKind::Operator
            || matches!(
                to,
                IngestionState::NeedsReview | IngestionState::Staged | IngestionState::Published
            );

        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = $2,
                 state_reason = $3,
                 updated_at = $4,
                 completed_at = $5,
                 lease_owner = CASE WHEN $6 THEN NULL ELSE lease_owner END,
                 lease_acquired_at = CASE WHEN $6 THEN NULL ELSE lease_acquired_at END,
                 lease_expires_at = CASE WHEN $6 THEN NULL ELSE lease_expires_at END
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(to.as_str())
        .bind(reason)
        .bind(now)
        .bind(completed_at)
        .bind(clear_lease)
        .fetch_one(&mut *tx)
        .await?;

        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(from),
            Some(to),
            reason,
            worker.map(WorkerId::as_str),
            item.correlation_id,
            now,
            None,
        )
        .await?;
        if terminal {
            insert_event(
                &mut tx,
                id,
                WorkEventType::Completed,
                None,
                Some(to),
                None,
                worker.map(WorkerId::as_str),
                item.correlation_id,
                now,
                None,
            )
            .await?;
        }
        tx.commit().await?;
        info!(work_item_id = %id, from_state = %from.as_str(), to_state = %to.as_str(), "state transitioned");
        ReleaseWorkItem::try_from(row)
    }

    /// Completes the Release lifecycle once its search projection is durable.
    ///
    /// `SPEC.md` §11.2 and `PRD.md` define `Cataloged → Indexing → Indexed`, and
    /// `IngestionState::is_terminal` names `Indexed` as the successful end
    /// state. The search worker owns the durable projection (and therefore the
    /// only evidence that indexing finished), but it leases *outbox rows*, not
    /// work items, so [`Self::transition`] — which requires a work-item lease
    /// for worker transitions — cannot express this hop. This method performs it
    /// under its own invariant instead:
    ///
    /// * only from `indexing` (never re-completes an item);
    /// * only when the projection outbox holds **no pending row** for the
    ///   Release, any of its Tracks, or any of its billed Artists, so the
    ///   queryable index is complete for that Release;
    /// * bounded by `limit` and ordered oldest-updated first, so a backlog
    ///   cannot produce one unbounded statement;
    /// * records the same `transitioned` + `completed` evidence a worker
    ///   transition would, so history, timelines, and the dashboard activity
    ///   series see the completion.
    ///
    /// Idempotent: an item that already left `indexing` is never touched.
    pub async fn complete_indexed(
        &self,
        pool: &PgPool,
        now: OffsetDateTime,
        limit: u32,
        reason: &str,
    ) -> Result<Vec<Uuid>, WorkStoreError> {
        let limit = i64::from(limit.max(1));
        let mut tx = pool.begin().await?;
        // Set-based and bounded: candidates are the unfinished tail of the
        // lifecycle, and only those whose projection rows are all processed
        // advance. `NOT IN` over a DISTINCT sub-select avoids the row-multiply a
        // LEFT JOIN against three entity families would cause.
        let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "WITH candidates AS (
                 SELECT w.id, w.correlation_id, ci.release_id, COALESCE(s.target_release_id, ci.release_id) AS survivor, w.updated_at
                   FROM release_work_items w
                   JOIN catalog_imports ci ON ci.work_item_id = w.id
                  LEFT JOIN release_review_identifications s ON s.work_item_id=w.id
                  WHERE w.state = 'indexing' AND (s.work_item_id IS NULL OR s.applied_at IS NOT NULL)
                  ORDER BY w.updated_at, w.id
                  LIMIT $2
             ),
             pending AS (
                 SELECT DISTINCT c.id
                   FROM candidates c
                   LEFT JOIN tracks t ON t.release_id = c.survivor
                   LEFT JOIN release_artists ra ON ra.release_id = c.survivor
                   JOIN catalog_projection_outbox o
                     ON o.processed_at IS NULL
                    AND (
                           (o.entity_type = 'release' AND o.entity_id IN (c.release_id,c.survivor))
                        OR (o.entity_type = 'track' AND o.entity_id = t.id)
                        OR (o.entity_type = 'artist' AND o.entity_id = ra.artist_id)
                    )
             )
             UPDATE release_work_items w
                SET state = 'indexed',
                    state_reason = $1,
                    completed_at = $3,
                    updated_at = $3
               FROM candidates c
              WHERE w.id = c.id AND w.state='indexing'
                AND c.id NOT IN (SELECT id FROM pending)
             RETURNING w.id, w.correlation_id",
        )
        .bind(reason)
        .bind(limit)
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;

        for (id, correlation_id) in &rows {
            insert_event(
                &mut tx,
                *id,
                WorkEventType::Transitioned,
                Some(IngestionState::Indexing),
                Some(IngestionState::Indexed),
                Some(reason),
                None,
                *correlation_id,
                now,
                None,
            )
            .await?;
            insert_event(
                &mut tx,
                *id,
                WorkEventType::Completed,
                None,
                Some(IngestionState::Indexed),
                None,
                None,
                *correlation_id,
                now,
                None,
            )
            .await?;
        }
        tx.commit().await?;
        let completed: Vec<Uuid> = rows.into_iter().map(|(id, _)| id).collect();
        if !completed.is_empty() {
            info!(
                completed = completed.len(),
                "work items completed indexing and reached the terminal Indexed state"
            );
        }
        Ok(completed)
    }

    /// Records a processing failure with durable evidence and computes retry
    /// eligibility. Clears the lease. Classification decides the outcome:
    /// retryable (scheduled or exhausted), permanent, or review-required.
    // Parameters mirror the durable failure record; see transition().
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker, error_code = %code.as_str()))]
    pub async fn record_failure(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        code: WorkErrorCode,
        message: &str,
        class: FailureClass,
        policy: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if !item.state.can_fail() {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }

        let attempt_count = item.attempt_count + 1;
        let (new_state, next_attempt_at): (IngestionState, Option<OffsetDateTime>) = match class {
            FailureClass::Retryable if policy.can_retry(attempt_count) => (
                IngestionState::Failed,
                policy.next_attempt_at(attempt_count, now),
            ),
            FailureClass::Retryable => (IngestionState::Failed, None),
            FailureClass::Permanent => (IngestionState::Failed, None),
            FailureClass::NeedsReview => {
                // SPEC 11.2: NeedsReview is reached from Validating only.
                item.state
                    .ensure_transition(IngestionState::NeedsReview, TransitionKind::Worker)?;
                (IngestionState::NeedsReview, None)
            }
        };

        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = $2,
                 state_reason = $3,
                 attempt_count = $4,
                 last_attempt_at = $5,
                 last_error_code = $6,
                 last_error_message = $7,
                 next_attempt_at = $8,
                 lease_owner = NULL,
                 lease_acquired_at = NULL,
                 lease_expires_at = NULL,
                 updated_at = $5
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(new_state.as_str())
        .bind(message)
        .bind(i32::try_from(attempt_count).unwrap_or(i32::MAX))
        .bind(now)
        .bind(code.as_str())
        .bind(message)
        .bind(next_attempt_at)
        .fetch_one(&mut *tx)
        .await?;

        insert_event(
            &mut tx,
            id,
            WorkEventType::Failed,
            Some(item.state),
            Some(new_state),
            Some(code.as_str()),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "attempt_count": attempt_count,
                "failure_class": class,
            })),
        )
        .await?;
        if let Some(next) = next_attempt_at {
            insert_event(
                &mut tx,
                id,
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
                })),
            )
            .await?;
        }
        tx.commit().await?;
        warn!(
            work_item_id = %id,
            attempt_count,
            next_attempt_at = ?next_attempt_at,
            failure_class = ?class,
            error_code = %code.as_str(),
            error_message = ?message,
            "work item failure recorded"
        );
        ReleaseWorkItem::try_from(row)
    }

    /// Explicitly releases a held lease without changing state (work becomes
    /// claimable again).
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn release_lease(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        if item.lease_owner.as_deref() != Some(worker.as_str()) {
            return Err(WorkStoreError::NotLeaseOwner(id));
        }
        sqlx::query(
            "UPDATE release_work_items
             SET lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL, updated_at = $2
             WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::LeaseReleased,
            None,
            None,
            None,
            Some(worker.as_str()),
            item.correlation_id,
            now,
            None,
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, "lease released");
        Ok(())
    }

    /// Renews a held, unexpired lease (heartbeat). Deferred long-running stages
    /// can call this without any schema change — proof the durable-work design
    /// supports renewal without redesign.
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn renew_lease(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        lease_duration: time::Duration,
        now: OffsetDateTime,
    ) -> Result<OffsetDateTime, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        let new_expiry = now + lease_duration;
        sqlx::query(
            "UPDATE release_work_items
             SET lease_expires_at = $2, updated_at = $3
             WHERE id = $1",
        )
        .bind(id)
        .bind(new_expiry)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::LeaseRenewed,
            None,
            None,
            None,
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({ "lease_expires_at": new_expiry.to_string() })),
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, lease_expires_at = %new_expiry, "lease renewed");
        Ok(new_expiry)
    }

    /// Requeue interrupted inspections through the normal retry policy. Other
    /// stages own their recovery and a concurrent pass skips locked rows.
    pub async fn recover_expired_inspections(
        &self,
        pool: &PgPool,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<Uuid>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE state = 'inspecting' AND lease_expires_at <= $1
             ORDER BY lease_expires_at, id LIMIT $2 FOR UPDATE SKIP LOCKED",
        )
        .bind(now)
        .bind(i64::from(config.claim_batch_size))
        .fetch_all(&mut *tx)
        .await?;
        Self::recover_locked_leases(tx, rows, &config.retry_policy, now).await
    }

    /// Recovers items whose leases expired mid-processing: marks them failed
    /// with `lease.lost` and schedules retry per policy (crash-loop protection
    /// via attempt counting). Returns recovered ids.
    #[instrument(skip_all)]
    pub async fn recover_expired_leases(
        &self,
        pool: &PgPool,
        policy: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Result<Vec<Uuid>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE lease_expires_at IS NOT NULL AND lease_expires_at <= $1
             FOR UPDATE SKIP LOCKED",
        )
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;

        Self::recover_locked_leases(tx, rows, policy, now).await
    }

    async fn recover_locked_leases(
        mut tx: Transaction<'_, Postgres>,
        rows: Vec<WorkItemRow>,
        policy: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Result<Vec<Uuid>, WorkStoreError> {
        let mut recovered = Vec::with_capacity(rows.len());
        for row in rows {
            let item = ReleaseWorkItem::try_from(row)?;
            let attempt_count = item.attempt_count + 1;
            let convergent_stage = matches!(
                item.state,
                IngestionState::Publishing | IngestionState::Cataloging
            );
            let interrupted_processing = item.state.is_active_processing() && !convergent_stage;
            let next_attempt_at = if interrupted_processing && policy.can_retry(attempt_count) {
                policy.next_attempt_at(attempt_count, now)
            } else {
                None
            };
            // Items merely claimed (still ready) return to the queue without a
            // failure mark; items interrupted mid-stage become failed/lease.lost.
            // Publishing and Cataloging are convergent stages. Publication has
            // durable plan/filesystem truth; Cataloging is one PostgreSQL
            // transaction. Expiry only fences the stale worker and leaves each
            // stage directly reclaimable instead of restarting at inspection.
            let (new_state, error_code): (IngestionState, Option<&str>) = if interrupted_processing
            {
                (IngestionState::Failed, Some(WorkErrorCode::LEASE_LOST))
            } else {
                (item.state, None)
            };
            sqlx::query(
                "UPDATE release_work_items
                 SET state = $2,
                     state_reason = CASE WHEN $3 THEN 'worker lease expired' ELSE state_reason END,
                     attempt_count = CASE WHEN $3 THEN $4 ELSE attempt_count END,
                     last_attempt_at = CASE WHEN $3 THEN $5 ELSE last_attempt_at END,
                     last_error_code = CASE WHEN $3 THEN $6 ELSE last_error_code END,
                     last_error_message = CASE WHEN $3 THEN 'worker lease expired' ELSE last_error_message END,
                     next_attempt_at = CASE WHEN $3 THEN $7 ELSE NULL END,
                     lease_owner = NULL,
                     lease_acquired_at = NULL,
                     lease_expires_at = NULL,
                     updated_at = $5
                 WHERE id = $1",
            )
            .bind(item.id)
            .bind(new_state.as_str())
            .bind(interrupted_processing)
            .bind(i32::try_from(attempt_count).unwrap_or(i32::MAX))
            .bind(now)
            .bind(error_code)
            .bind(next_attempt_at)
            .execute(&mut *tx)
            .await?;

            insert_event(
                &mut tx,
                item.id,
                WorkEventType::LeaseExpiredRecovered,
                Some(item.state),
                Some(new_state),
                error_code,
                item.lease_owner.as_deref(),
                item.correlation_id,
                now,
                None,
            )
            .await?;
            if interrupted_processing {
                insert_event(
                    &mut tx,
                    item.id,
                    WorkEventType::Failed,
                    Some(item.state),
                    Some(new_state),
                    error_code,
                    item.lease_owner.as_deref(),
                    item.correlation_id,
                    now,
                    Some(serde_json::json!({ "attempt_count": attempt_count })),
                )
                .await?;
                if let Some(next) = next_attempt_at {
                    insert_event(
                        &mut tx,
                        item.id,
                        WorkEventType::RetryScheduled,
                        None,
                        None,
                        error_code,
                        item.lease_owner.as_deref(),
                        item.correlation_id,
                        now,
                        Some(serde_json::json!({
                            "attempt_count": attempt_count,
                            "next_attempt_at": next.to_string(),
                        })),
                    )
                    .await?;
                }
            }
            recovered.push(item.id);
        }
        tx.commit().await?;
        if !recovered.is_empty() {
            info!(recovered = recovered.len(), "expired leases recovered");
        }
        Ok(recovered)
    }

    /// Fetches one work item by internal id or operator-safe api key.
    pub async fn get(
        &self,
        pool: &PgPool,
        id_or_api_key: Uuid,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let row = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items WHERE id = $1 OR api_key = $1",
        )
        .bind(id_or_api_key)
        .fetch_optional(pool)
        .await?
        .ok_or(WorkStoreError::NotFound(id_or_api_key))?;
        ReleaseWorkItem::try_from(row)
    }

    /// Lists work items, optionally filtered by state, newest first, bounded.
    pub async fn list(
        &self,
        pool: &PgPool,
        state: Option<IngestionState>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE ($1::text IS NULL OR state = $1)
             ORDER BY created_at DESC, id
             LIMIT $2 OFFSET $3",
        )
        .bind(state.map(IngestionState::as_str))
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(ReleaseWorkItem::try_from)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Lists one page of work items, optionally filtered by state, ordered by
    /// a whitelisted sort, bounded — and returns the total row count for the
    /// filter (ignoring the page) so callers can render pagination honestly.
    pub async fn list_page(
        &self,
        pool: &PgPool,
        state: Option<IngestionState>,
        limit: u32,
        offset: u32,
        sort: WorkItemSort,
    ) -> Result<(Vec<ReleaseWorkItem>, i64), WorkStoreError> {
        let total = self.count(pool, state).await?;
        // Injection audit: `order_by` returns a constant `&'static str`
        // fragment selected from the WorkItemSort whitelist; only bind
        // parameters carry client-influenced values.
        let rows = sqlx::query_as::<_, WorkItemRow>(sqlx::AssertSqlSafe(format!(
            "SELECT * FROM release_work_items
             WHERE ($1::text IS NULL OR state = $1)
             ORDER BY {}
             LIMIT $2 OFFSET $3",
            sort.order_by()
        )))
        .bind(state.map(IngestionState::as_str))
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        let items = rows
            .into_iter()
            .map(ReleaseWorkItem::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    /// Lists one page of work items with their catalog binding, plus the total
    /// row count for the filter (ignoring the page).
    ///
    /// The queue surface shows the Release a work item belongs to, so this
    /// query joins the import binding (`catalog_imports`, unique on
    /// `work_item_id`) and the release aggregates that already carry
    /// `media_bytes`. All added columns are display values only: the work-item
    /// id remains the identity, and a work item never inherits identity from a
    /// path or a title (SPEC).
    ///
    /// `query` is an optional case-insensitive substring filter over the
    /// Release title, billed artist, and current path. It is a bounded
    /// operator convenience over the active queue, not a catalog search: the
    /// scan is capped by the page limit and never becomes the catalog browse
    /// path (PRD-UI-010).
    pub async fn list_page_enriched(
        &self,
        pool: &PgPool,
        states: Option<&[IngestionState]>,
        limit: u32,
        offset: u32,
        sort: WorkItemSort,
        query: Option<&str>,
    ) -> Result<(Vec<WorkItemListRow>, i64), WorkStoreError> {
        let pattern = query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", escape_like(value)));
        // A phase filter is a state *set*; `NULL`/empty means every state.
        let state_names: Option<Vec<&str>> = states
            .filter(|list| !list.is_empty())
            .map(|list| list.iter().map(|state| state.as_str()).collect());
        let filter = "($1::text[] IS NULL OR w.state = ANY($1))
               AND (
                   $2::text IS NULL
                   OR r.title ILIKE $2 ESCAPE '\\'
                   OR r.display_artist ILIKE $2 ESCAPE '\\'
                   OR w.current_path ILIKE $2 ESCAPE '\\'
               )";
        // Injection audit: `order_by` and `filter` are constants; only bind
        // parameters carry client-influenced values. The ordering fragment is
        // table-qualified because the query joins other tables that also own
        // `created_at`/`updated_at` columns.
        let total = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*)::bigint
             FROM release_work_items w
             LEFT JOIN catalog_imports ci ON ci.work_item_id = w.id
             LEFT JOIN releases r ON r.id = ci.release_id
             WHERE {filter}"
        )))
        .bind(state_names.as_deref())
        .bind(pattern.as_deref())
        .fetch_one(pool)
        .await?;
        let rows = sqlx::query_as::<_, WorkItemListRow>(sqlx::AssertSqlSafe(format!(
            "SELECT w.*, ci.release_id, r.title, r.display_artist, rca.media_bytes, r.status
             FROM release_work_items w
             LEFT JOIN catalog_imports ci ON ci.work_item_id = w.id
             LEFT JOIN releases r ON r.id = ci.release_id
             LEFT JOIN release_catalog_aggregates rca ON rca.release_id = r.id
             WHERE {filter}
             ORDER BY {}
             LIMIT $3 OFFSET $4",
            qualified_order_by(sort)
        )))
        .bind(state_names.as_deref())
        .bind(pattern.as_deref())
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok((rows, total))
    }

    /// Lists one page of work items whose state is in `states`, ordered by a
    /// whitelisted sort, bounded — plus the total row count for the filter
    /// (ignoring the page). Read-side companion to [`Self::list_page`] for
    /// multi-state operator queues (release review). An empty state
    /// set short-circuits to an empty page without touching the database.
    pub async fn list_page_states(
        &self,
        pool: &PgPool,
        states: &[IngestionState],
        limit: u32,
        offset: u32,
        sort: WorkItemSort,
    ) -> Result<(Vec<ReleaseWorkItem>, i64), WorkStoreError> {
        if states.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let state_names: Vec<&str> = states.iter().map(|state| state.as_str()).collect();
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM release_work_items WHERE state = ANY($1)",
        )
        .bind(&state_names)
        .fetch_one(pool)
        .await?;
        // Injection audit: `order_by` returns a constant `&'static str`
        // fragment selected from the WorkItemSort whitelist; only bind
        // parameters carry client-influenced values.
        let rows = sqlx::query_as::<_, WorkItemRow>(sqlx::AssertSqlSafe(format!(
            "SELECT * FROM release_work_items
             WHERE state = ANY($1)
             ORDER BY {}
             LIMIT $2 OFFSET $3",
            sort.order_by()
        )))
        .bind(&state_names)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        let items = rows
            .into_iter()
            .map(ReleaseWorkItem::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    /// Total work-item rows, optionally filtered by state.
    pub async fn count(
        &self,
        pool: &PgPool,
        state: Option<IngestionState>,
    ) -> Result<i64, WorkStoreError> {
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM release_work_items
             WHERE ($1::text IS NULL OR state = $1)",
        )
        .bind(state.map(IngestionState::as_str))
        .fetch_one(pool)
        .await?;
        Ok(total)
    }

    /// Per-state counts for operator diagnostics (PRD-WORK-002 shape).
    pub async fn counts_by_state(&self, pool: &PgPool) -> Result<Vec<StateCount>, WorkStoreError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT state, COUNT(*)::bigint FROM release_work_items GROUP BY state ORDER BY state",
        )
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(|(state, count)| {
                Ok(StateCount {
                    state: IngestionState::parse(&state)
                        .ok_or(WorkStoreError::CorruptState(state))?,
                    count,
                })
            })
            .collect()
    }

    /// Failure/completion counts since `since` (set-based, single pass).
    pub async fn window_counts(
        &self,
        pool: &PgPool,
        since: OffsetDateTime,
    ) -> Result<WorkItemWindowCounts, WorkStoreError> {
        let row: (i64, i64) = sqlx::query_as(
            "SELECT
                 COUNT(*) FILTER (WHERE state = 'failed' AND updated_at >= $1)::bigint,
                 COUNT(*) FILTER (WHERE completed_at IS NOT NULL AND completed_at >= $1)::bigint
             FROM release_work_items",
        )
        .bind(since)
        .fetch_one(pool)
        .await?;
        Ok(WorkItemWindowCounts {
            failed: row.0,
            completed: row.1,
        })
    }

    /// Per-hour completed counts since `since`, oldest first (set-based;
    /// bounded by the window, never by row cardinality beyond one row per
    /// hour).
    pub async fn completed_hourly(
        &self,
        pool: &PgPool,
        since: OffsetDateTime,
    ) -> Result<Vec<HourlyCompletionCount>, WorkStoreError> {
        // Zero-filled hour spine: the dashboard plots a trailing-24h axis, so an
        // hour with no completions is a real data point (0), not a gap. A sparse
        // series made the chart draw one long diagonal between two distant
        // samples. generate_series is bounded by the same window.
        let rows: Vec<(OffsetDateTime, i64)> = sqlx::query_as(
            "WITH spine AS (
               SELECT generate_series(
                        date_trunc('hour', $1::timestamptz),
                        date_trunc('hour', now()),
                        interval '1 hour'
                      ) AS hour
             ),
             counted AS (
               SELECT date_trunc('hour', completed_at) AS hour, COUNT(*)::bigint AS count
                 FROM release_work_items
                WHERE completed_at IS NOT NULL AND completed_at >= $1
                GROUP BY 1
             )
             SELECT spine.hour, COALESCE(counted.count, 0)
               FROM spine LEFT JOIN counted USING (hour)
              ORDER BY 1",
        )
        .bind(since)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(hour, count)| HourlyCompletionCount { hour, count })
            .collect())
    }

    /// Per-hour counts of work items that transitioned into `to_state` since
    /// `since`, oldest first.
    ///
    /// Postgres, not the process: it reads the append-only
    /// `release_work_item_events` log, whose `created_at` index backs the
    /// window range, and groups to one row per hour (bounded by the window,
    /// never by row cardinality).
    pub async fn transitions_hourly(
        &self,
        pool: &PgPool,
        since: OffsetDateTime,
        to_state: &str,
    ) -> Result<Vec<HourlyCompletionCount>, WorkStoreError> {
        let rows: Vec<(OffsetDateTime, i64)> = sqlx::query_as(
            "SELECT date_trunc('hour', created_at) AS hour, COUNT(*)::bigint AS count
             FROM release_work_item_events
             WHERE to_state = $1 AND created_at >= $2
             GROUP BY 1 ORDER BY 1",
        )
        .bind(to_state)
        .bind(since)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(hour, count)| HourlyCompletionCount { hour, count })
            .collect())
    }

    /// Number of work items that transitioned into `to_state` since `since`.
    /// Same evidence source and bounded range as [`Self::transitions_hourly`].
    pub async fn count_transitions_since(
        &self,
        pool: &PgPool,
        since: OffsetDateTime,
        to_state: &str,
    ) -> Result<i64, WorkStoreError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM release_work_item_events
             WHERE to_state = $1 AND created_at >= $2",
        )
        .bind(to_state)
        .bind(since)
        .fetch_one(pool)
        .await?;
        Ok(count)
    }

    /// Number of work items that entered the pipeline (`created` events) since
    /// `since`.
    pub async fn count_created_since(
        &self,
        pool: &PgPool,
        since: OffsetDateTime,
    ) -> Result<i64, WorkStoreError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM release_work_item_events
             WHERE event_type = 'created' AND created_at >= $1",
        )
        .bind(since)
        .fetch_one(pool)
        .await?;
        Ok(count)
    }

    /// Active (unexpired) leases proven by PostgreSQL evidence, bounded by
    /// `limit` (newest expiry first). This is lease evidence only — it proves a
    /// row holds an unexpired lease, not that the owner process is alive (there
    /// is no heartbeat store yet). Used by the diagnostics API to show what is
    /// currently claimed without overclaiming worker liveness.
    pub async fn list_active_leases(
        &self,
        pool: &PgPool,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<ActiveLease>, WorkStoreError> {
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE lease_expires_at IS NOT NULL AND lease_expires_at > $1
             ORDER BY lease_expires_at DESC, id
             LIMIT $2",
        )
        .bind(now)
        .bind(i64::from(limit.min(500)))
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let lease_owner =
                    row.lease_owner
                        .clone()
                        .ok_or(WorkStoreError::CorruptState(format!(
                            "missing lease_owner on {}",
                            row.id
                        )))?;
                let lease_acquired_at =
                    row.lease_acquired_at
                        .ok_or(WorkStoreError::CorruptState(format!(
                            "missing lease_acquired_at on {}",
                            row.id
                        )))?;
                let lease_expires_at =
                    row.lease_expires_at
                        .ok_or(WorkStoreError::CorruptState(format!(
                            "missing lease_expires_at on {}",
                            row.id
                        )))?;
                Ok(ActiveLease {
                    work_item_id: row.id,
                    api_key: row.api_key,
                    state: IngestionState::parse(&row.state)
                        .ok_or_else(|| WorkStoreError::CorruptState(row.state.clone()))?,
                    lease_owner,
                    lease_acquired_at,
                    lease_expires_at,
                    last_activity_at: row.updated_at,
                })
            })
            .collect()
    }

    /// Append-only history for one work item, oldest first.
    pub async fn list_events(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Vec<ReleaseWorkItemEvent>, WorkStoreError> {
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT * FROM release_work_item_events
             WHERE work_item_id = $1 ORDER BY created_at, id",
        )
        .bind(work_item_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(ReleaseWorkItemEvent::try_from)
            .collect::<Result<Vec<_>, _>>()
    }
}

fn stored_publication_from_staged(
    work_item_id: Uuid,
    publication: &ManifestPublication,
    plan: &PublicationPlan,
    authorized_at: OffsetDateTime,
) -> StoredPublication {
    StoredPublication {
        work_item_id,
        release_id: publication.release_id,
        authorization: publication.authorization.clone(),
        validation: publication.validation.clone(),
        source_disposition: publication.source_disposition,
        plan: plan.clone(),
        plan_hash: publication.plan_hash.clone(),
        authorized_at,
        published_at: None,
        sealed_manifest_hash: None,
    }
}

fn has_complete_publication_evidence(
    plan: &PublicationPlan,
    plan_hash: &str,
    sealed_manifest_hash: &str,
    journal: &[PublicationJournalEntry],
) -> bool {
    let bundle_key = format!("bundle_sealed:{plan_hash}");
    let bundle_sealed = journal.iter().any(|entry| {
        entry.journal_key == bundle_key
            && entry.step == PublicationJournalStep::BundleSealed
            && entry.asset_path.is_none()
            && entry.size_bytes.is_none()
            && entry.byte_hash.is_none()
    });
    if !bundle_sealed {
        return false;
    }

    let mut unique_paths = std::collections::BTreeSet::new();
    for asset in &plan.assets {
        if !unique_paths.insert(asset.canonical_path.as_str()) {
            continue;
        }
        let asset_key = format!("asset:{}", asset.canonical_path);
        if !journal.iter().any(|entry| {
            entry.journal_key == asset_key
                && entry.step == PublicationJournalStep::AssetCopied
                && entry.asset_path.as_deref() == Some(asset.canonical_path.as_str())
                && entry.size_bytes == Some(asset.size_bytes)
                && entry.byte_hash.as_deref() == Some(asset.byte_hash.as_str())
        }) {
            return false;
        }
    }

    [
        (
            PublicationJournalStep::Finalized,
            format!("finalized:{plan_hash}"),
        ),
        (
            PublicationJournalStep::FinalAdopted,
            format!("final_adopted:{plan_hash}"),
        ),
    ]
    .into_iter()
    .any(|(step, key)| {
        journal.iter().any(|entry| {
            entry.journal_key == key
                && entry.step == step
                && entry.asset_path.as_deref() == Some(plan.manifest_relative_path.as_str())
                && entry.size_bytes.is_some_and(|size| size > 0)
                && entry.byte_hash.as_deref() == Some(sealed_manifest_hash)
        })
    })
}

/// Locks a work item row for update inside a transaction.
pub(crate) async fn lock_for_update(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<ReleaseWorkItem, WorkStoreError> {
    let row = sqlx::query_as::<_, WorkItemRow>(
        "SELECT * FROM release_work_items WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WorkStoreError::NotFound(id))?;
    ReleaseWorkItem::try_from(row)
}

/// Requires an active lease held by `worker` at `now`.
pub(crate) fn ensure_active_lease(
    item: &ReleaseWorkItem,
    worker: &WorkerId,
    now: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    if item.lease_owner.as_deref() != Some(worker.as_str()) {
        return Err(WorkStoreError::NotLeaseOwner(item.id));
    }
    if !item.has_active_lease(now) {
        return Err(WorkStoreError::LeaseExpired(item.id));
    }
    Ok(())
}

/// Inserts one history event. Always called inside the mutation's transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
    event_type: WorkEventType,
    from_state: Option<IngestionState>,
    to_state: Option<IngestionState>,
    reason_code: Option<&str>,
    worker_id: Option<&str>,
    correlation_id: Uuid,
    now: OffsetDateTime,
    metadata: Option<serde_json::Value>,
) -> Result<(), WorkStoreError> {
    sqlx::query(
        "INSERT INTO release_work_item_events (
            work_item_id, event_type, from_state, to_state, reason_code,
            worker_id, correlation_id, created_at, metadata
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(work_item_id)
    .bind(event_type.as_str())
    .bind(from_state.map(IngestionState::as_str))
    .bind(to_state.map(IngestionState::as_str))
    .bind(reason_code)
    .bind(worker_id)
    .bind(correlation_id)
    .bind(now)
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

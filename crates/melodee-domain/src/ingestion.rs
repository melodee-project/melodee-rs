//! Durable per-Release ingestion work model: state machine, retry policy,
//! failure classification, and transition events.
//!
//! Framework-independent by design (no SQLx/Tokio/Axum/filesystem). The
//! persistence crate maps these types to PostgreSQL rows; every legal
//! transition is decided HERE so no other layer can invent state changes.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Durable per-Release ingestion state (SPEC.md section 11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionState {
    Discovered,
    Settling,
    Ready,
    Inspecting,
    Normalizing,
    Validating,
    NeedsReview,
    Staged,
    Publishing,
    Published,
    Cataloging,
    Cataloged,
    Indexing,
    Indexed,
    Failed,
    Quarantined,
    Ignored,
    Deleted,
}

impl IngestionState {
    /// Every persisted representation, in canonical order. Used by the schema
    /// CHECK constraint and by diagnostics.
    pub const ALL: [Self; 18] = [
        Self::Discovered,
        Self::Settling,
        Self::Ready,
        Self::Inspecting,
        Self::Normalizing,
        Self::Validating,
        Self::NeedsReview,
        Self::Staged,
        Self::Publishing,
        Self::Published,
        Self::Cataloging,
        Self::Cataloged,
        Self::Indexing,
        Self::Indexed,
        Self::Failed,
        Self::Quarantined,
        Self::Ignored,
        Self::Deleted,
    ];

    /// Stable lowercase wire/storage form (matches serde snake_case).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Settling => "settling",
            Self::Ready => "ready",
            Self::Inspecting => "inspecting",
            Self::Normalizing => "normalizing",
            Self::Validating => "validating",
            Self::NeedsReview => "needs_review",
            Self::Staged => "staged",
            Self::Publishing => "publishing",
            Self::Published => "published",
            Self::Cataloging => "cataloging",
            Self::Cataloged => "cataloged",
            Self::Indexing => "indexing",
            Self::Indexed => "indexed",
            Self::Failed => "failed",
            Self::Quarantined => "quarantined",
            Self::Ignored => "ignored",
            Self::Deleted => "deleted",
        }
    }

    /// Parses the storage form.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }

    /// Terminal states: no advance without an explicit operator action.
    /// `Quarantined` is parked, not terminal (operator reprocess exits it).
    /// `Failed` is not terminal: retry or operator reprocess exits it.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Indexed | Self::Ignored | Self::Deleted)
    }

    /// The operator-facing phase this state belongs to.
    ///
    /// Total over [`Self::ALL`] and a partition: every state maps to exactly one
    /// phase, and [`IngestionPhase::states`] is its inverse. Used by the Admin
    /// UI queue filters and labels so operators reason in phases, not states.
    #[must_use]
    pub const fn phase(self) -> IngestionPhase {
        match self {
            Self::Discovered | Self::Settling | Self::Ready => IngestionPhase::Inbound,
            Self::Inspecting | Self::Normalizing | Self::Validating => IngestionPhase::Processing,
            Self::Staged | Self::Publishing | Self::Published => IngestionPhase::Publishing,
            Self::Cataloging | Self::Cataloged => IngestionPhase::Catalog,
            Self::Indexing | Self::Indexed => IngestionPhase::Active,
            Self::NeedsReview | Self::Quarantined => IngestionPhase::Review,
            Self::Ignored | Self::Deleted => IngestionPhase::Disposition,
            Self::Failed => IngestionPhase::Failed,
        }
    }

    /// True when client surfaces can already see and stream the Release.
    ///
    /// Entering `indexing` is the policy `accept` commit, which sets
    /// `releases.status = 'active'` in the same transaction; every client route
    /// (native, OpenSubsonic, Jellyfin) filters on that status and a playable
    /// media asset. `indexed` adds only the search projection, which no client
    /// browse or playback path depends on.
    #[must_use]
    pub const fn is_client_visible(self) -> bool {
        matches!(self, Self::Indexing | Self::Indexed)
    }

    /// States a worker may claim from the queue.
    #[must_use]
    pub const fn is_claimable(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Normalizing | Self::Validating | Self::Staged | Self::Publishing
        )
    }

    /// States that may hold an active lease.
    #[must_use]
    pub const fn is_active_processing(self) -> bool {
        matches!(
            self,
            Self::Settling
                | Self::Inspecting
                | Self::Normalizing
                | Self::Validating
                | Self::Publishing
                | Self::Cataloging
                // policy evaluates an already-cataloged Release under
                // the same lease/fencing contract before it advances or parks.
                | Self::Cataloged
                | Self::Indexing
        )
    }

    /// States from which a failure may be recorded (worker was processing).
    #[must_use]
    pub const fn can_fail(self) -> bool {
        self.is_active_processing()
    }
}

/// Who is performing a transition. The legal transition set differs per kind
/// (SPEC.md section 11.2 plus operator/reprocess semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// System/discovery flow (discovery, settling, retry requeue at claim).
    System,
    /// A leased worker advancing or failing a stage.
    Worker,
    /// An operator decision (review accept/quarantine/ignore/delete, reprocess).
    Operator,
    /// Explicitly retire an ambiguous legacy occurrence after reviewed binding.
    LegacyPathResolution,
}

/// Operator-facing phase: the coarse grouping of [`IngestionState`].
///
/// Fourteen lifecycle states is the right granularity for the durable work
/// model — each one is a claim partition with its own fencing and evidence —
/// but it is the wrong granularity for an operator, who thinks in terms of
/// "is this importing, being checked, publishing, cataloged, searchable?".
/// Phases are a pure projection of state (every state belongs to exactly one
/// phase); they are never stored and never affect transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionPhase {
    /// Arrived, waiting for a stable snapshot, or queued for a worker.
    Inbound,
    /// Being read, converted, and checked by the pipeline.
    Processing,
    /// Canonical publication in progress or verified.
    Publishing,
    /// Catalog import in progress or complete.
    Catalog,
    /// Live to clients: the Release is `active` and playable. The search
    /// projection may still be finishing inside this phase, because client
    /// browse/search/playback read PostgreSQL — the projection only feeds smart
    /// playlists and therefore never gates availability.
    Active,
    /// Parked on an operator decision.
    Review,
    /// Final non-accepted disposition.
    Disposition,
    /// Processing failed; retry or operator action required.
    Failed,
}

impl IngestionPhase {
    /// Every phase, in lifecycle order.
    pub const ALL: [Self; 8] = [
        Self::Inbound,
        Self::Processing,
        Self::Publishing,
        Self::Catalog,
        Self::Active,
        Self::Review,
        Self::Disposition,
        Self::Failed,
    ];

    /// Stable lowercase wire/storage form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Processing => "processing",
            Self::Publishing => "publishing",
            Self::Catalog => "catalog",
            Self::Active => "active",
            Self::Review => "review",
            Self::Disposition => "disposition",
            Self::Failed => "failed",
        }
    }

    /// Parses the wire form.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }

    /// The states that belong to this phase, in canonical order.
    #[must_use]
    pub const fn states(self) -> &'static [IngestionState] {
        match self {
            Self::Inbound => &[
                IngestionState::Discovered,
                IngestionState::Settling,
                IngestionState::Ready,
            ],
            Self::Processing => &[
                IngestionState::Inspecting,
                IngestionState::Normalizing,
                IngestionState::Validating,
            ],
            Self::Publishing => &[
                IngestionState::Staged,
                IngestionState::Publishing,
                IngestionState::Published,
            ],
            Self::Catalog => &[IngestionState::Cataloging, IngestionState::Cataloged],
            Self::Active => &[IngestionState::Indexing, IngestionState::Indexed],
            Self::Review => &[IngestionState::NeedsReview, IngestionState::Quarantined],
            Self::Disposition => &[IngestionState::Ignored, IngestionState::Deleted],
            Self::Failed => &[IngestionState::Failed],
        }
    }
}

impl IngestionState {
    /// Validates a transition for the given actor kind.
    pub fn ensure_transition(
        self,
        next: Self,
        kind: TransitionKind,
    ) -> Result<(), InvalidStateTransition> {
        let allowed = match kind {
            TransitionKind::System => matches!(
                (self, next),
                (Self::Discovered, Self::Settling)
                    | (Self::Settling, Self::Settling) // source changed; re-settle
                    | (Self::Settling, Self::Ready) // source stable
                    | (Self::Ready, Self::Settling) // source changed after readiness
                    | (Self::Failed, Self::Ready) // retry eligibility reached (at claim)
            ),
            TransitionKind::Worker => matches!(
                (self, next),
                (Self::Ready, Self::Inspecting)
                    | (Self::Inspecting, Self::Settling)
                    | (Self::Inspecting, Self::Normalizing)
                    | (Self::Normalizing, Self::Validating)
                    | (Self::Validating, Self::Staged)
                    | (Self::Validating, Self::NeedsReview)
                    | (Self::Staged, Self::Publishing)
                    | (Self::Publishing, Self::Published)
                    | (Self::Published, Self::Cataloging)
                    | (Self::Cataloging, Self::Cataloged)
                    | (Self::Cataloged, Self::NeedsReview)
                    | (Self::Cataloged, Self::Quarantined)
                    | (Self::Cataloged, Self::Ignored)
                    | (Self::Cataloged, Self::Indexing)
                    | (Self::Indexing, Self::NeedsReview)
                    | (Self::Indexing, Self::Indexed)
                    | (Self::Inspecting, Self::Failed)
                    | (Self::Normalizing, Self::Failed)
                    | (Self::Validating, Self::Failed)
                    | (Self::Publishing, Self::Failed)
                    | (Self::Cataloging, Self::Failed)
                    | (Self::Cataloged, Self::Failed)
                    | (Self::Indexing, Self::Failed)
            ),
            TransitionKind::Operator => matches!(
                (self, next),
                (Self::NeedsReview, Self::Validating)
                    | (Self::NeedsReview, Self::Staged)
                    | (Self::NeedsReview, Self::Indexing)
                    | (Self::NeedsReview, Self::Quarantined)
                    | (Self::NeedsReview, Self::Ignored)
                    | (Self::NeedsReview, Self::Deleted)
                    | (Self::Quarantined, Self::Ready) // operator reprocess
                    | (Self::Failed, Self::Ready) // operator retry/reprocess
            ),
            TransitionKind::LegacyPathResolution => matches!(
                (self, next),
                (
                    Self::Discovered | Self::Settling | Self::Ready,
                    Self::Ignored
                )
            ),
        };

        if allowed {
            Ok(())
        } else {
            Err(InvalidStateTransition {
                current: self,
                next,
                kind,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid {kind:?} ingestion transition from {current:?} to {next:?}")]
pub struct InvalidStateTransition {
    pub current: IngestionState,
    pub next: IngestionState,
    pub kind: TransitionKind,
}

/// Failure classification (PRD: distinguish retryable / permanent / review).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Transient; eligible for automatic retry with backoff.
    Retryable,
    /// Not retryable automatically; stays Failed until an operator reprocesses.
    Permanent,
    /// Uncertainty that requires an operator decision; parks in NeedsReview.
    NeedsReview,
}

/// Machine-readable, operator-safe error code. New codes are added as stages
/// land; the string form is stable for persisted history.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkErrorCode(String);

impl WorkErrorCode {
    pub const STAGE_FAILED: &'static str = "stage.failed";
    pub const STAGE_TIMEOUT: &'static str = "stage.timeout";
    pub const LEASE_LOST: &'static str = "lease.lost";
    pub const VALIDATION_FAILED: &'static str = "validation.failed";
    pub const POLICY_REVIEW: &'static str = "policy.review";

    #[must_use]
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Bounded exponential backoff retry policy (PRD-INGEST-001, SPEC 11.5).
///
/// `next = base * 2^(attempt-1)`, capped at `max_delay`, giving up after
/// `max_attempts` automatic attempts. All inputs are configuration-driven;
/// the policy itself is pure and tested with fixed clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base_delay: Duration::seconds(60),
            max_delay: Duration::hours(1),
            max_attempts: 5,
        }
    }
}

impl RetryPolicy {
    /// Computes the next eligible attempt time after the given failure count,
    /// or `None` when automatic attempts are exhausted.
    #[must_use]
    pub fn next_attempt_at(
        &self,
        attempt_count: u32,
        now: OffsetDateTime,
    ) -> Option<OffsetDateTime> {
        if attempt_count == 0 || attempt_count > self.max_attempts {
            return None;
        }
        let shift = attempt_count.saturating_sub(1).min(30);
        let delay = self
            .base_delay
            .saturating_mul(1_i32.checked_shl(shift).unwrap_or(0));
        let capped = if delay > self.max_delay {
            self.max_delay
        } else {
            delay
        };
        Some(now + capped)
    }

    /// Whether another automatic attempt is allowed after `attempt_count` failures.
    #[must_use]
    pub fn can_retry(&self, attempt_count: u32) -> bool {
        attempt_count < self.max_attempts
    }
}

/// Policy outcome for a Release (SPEC section 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Accept,
    Review,
    Ignore,
    Quarantine,
    Delete,
}

impl Disposition {
    /// Every disposition in canonical order.
    pub const ALL: [Self; 5] = [
        Self::Accept,
        Self::Review,
        Self::Ignore,
        Self::Quarantine,
        Self::Delete,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Review => "review",
            Self::Ignore => "ignore",
            Self::Quarantine => "quarantine",
            Self::Delete => "delete",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }

    /// Whether this disposition is destructive (Delete).
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Delete)
    }
}

/// A durable Release work item (SPEC 11.1). Processing state only — never
/// conflated with the catalog `Release` entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseWorkItem {
    pub id: Uuid,
    /// Operator-safe public identity (stable across systems; safe to display/log).
    pub api_key: Uuid,
    pub source_library_id: Option<Uuid>,
    pub target_library_id: Option<Uuid>,
    pub source_path: String,
    pub current_path: String,
    pub target_path: Option<String>,
    pub source_fingerprint: Option<String>,
    pub manifest_hash: Option<String>,
    pub state: IngestionState,
    pub state_reason: Option<String>,
    pub attempt_count: u32,
    pub next_attempt_at: Option<OffsetDateTime>,
    pub lease_owner: Option<String>,
    pub lease_acquired_at: Option<OffsetDateTime>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub latest_settling_snapshot_id: Option<i64>,
    pub stable_since: Option<OffsetDateTime>,
    pub last_observed_at: Option<OffsetDateTime>,
    pub last_error_code: Option<WorkErrorCode>,
    pub last_error_message: Option<String>,
    pub correlation_id: Uuid,
    pub discovered_at: OffsetDateTime,
    pub first_started_at: Option<OffsetDateTime>,
    pub last_attempt_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

impl ReleaseWorkItem {
    /// A newly discovered candidate (SPEC 11.1).
    #[must_use]
    pub fn discovered(
        source_path: String,
        source_library_id: Option<Uuid>,
        correlation_id: Uuid,
        now: OffsetDateTime,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            api_key: Uuid::now_v7(),
            source_library_id,
            target_library_id: None,
            target_path: None,
            current_path: source_path.clone(),
            source_path,
            source_fingerprint: None,
            manifest_hash: None,
            state: IngestionState::Discovered,
            state_reason: None,
            attempt_count: 0,
            next_attempt_at: None,
            lease_owner: None,
            lease_acquired_at: None,
            lease_expires_at: None,
            latest_settling_snapshot_id: None,
            stable_since: None,
            last_observed_at: None,
            last_error_code: None,
            last_error_message: None,
            correlation_id,
            discovered_at: now,
            first_started_at: None,
            last_attempt_at: None,
            updated_at: now,
            created_at: now,
            completed_at: None,
        }
    }

    /// Whether the lease is currently held and unexpired at `now`.
    #[must_use]
    pub fn has_active_lease(&self, now: OffsetDateTime) -> bool {
        self.lease_owner.is_some() && self.lease_expires_at.is_some_and(|expiry| expiry > now)
    }
}

/// Append-only transition/audit event types (SPEC 11; PRD-INGEST-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkEventType {
    Created,
    Claimed,
    LeaseRenewed,
    LeaseReleased,
    LeaseExpiredRecovered,
    Transitioned,
    Failed,
    RetryScheduled,
    Completed,
}

impl WorkEventType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Claimed => "claimed",
            Self::LeaseRenewed => "lease_renewed",
            Self::LeaseReleased => "lease_released",
            Self::LeaseExpiredRecovered => "lease_expired_recovered",
            Self::Transitioned => "transitioned",
            Self::Failed => "failed",
            Self::RetryScheduled => "retry_scheduled",
            Self::Completed => "completed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        const ALL: [WorkEventType; 9] = [
            WorkEventType::Created,
            WorkEventType::Claimed,
            WorkEventType::LeaseRenewed,
            WorkEventType::LeaseReleased,
            WorkEventType::LeaseExpiredRecovered,
            WorkEventType::Transitioned,
            WorkEventType::Failed,
            WorkEventType::RetryScheduled,
            WorkEventType::Completed,
        ];
        ALL.into_iter().find(|e| e.as_str() == value)
    }
}

/// One append-only history row (PRD-INGEST-010: "what happened and why").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseWorkItemEvent {
    pub id: i64,
    pub work_item_id: Uuid,
    pub event_type: WorkEventType,
    pub from_state: Option<IngestionState>,
    pub to_state: Option<IngestionState>,
    pub reason_code: Option<String>,
    pub worker_id: Option<String>,
    pub correlation_id: Uuid,
    pub created_at: OffsetDateTime,
    /// Structured, operator-safe extras only (attempt counts, lease expiry).
    /// Never secrets, paths with credentials, or stack traces.
    pub metadata: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

    #[test]
    fn worker_happy_path_is_valid() {
        let path = [
            IngestionState::Ready,
            IngestionState::Inspecting,
            IngestionState::Normalizing,
            IngestionState::Validating,
            IngestionState::Staged,
            IngestionState::Publishing,
            IngestionState::Published,
            IngestionState::Cataloging,
            IngestionState::Cataloged,
            IngestionState::Indexing,
            IngestionState::Indexed,
        ];
        for pair in path.windows(2) {
            assert!(
                pair[0]
                    .ensure_transition(pair[1], TransitionKind::Worker)
                    .is_ok(),
                "{:?} -> {:?} must be a legal worker transition",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn inspect_to_settling_is_valid_worker_transition() {
        assert!(
            IngestionState::Inspecting
                .ensure_transition(IngestionState::Settling, TransitionKind::Worker)
                .is_ok(),
            "source changes during inspection must allow inspect->settling"
        );
    }

    #[test]
    fn discovery_flow_is_system_only() {
        assert!(
            IngestionState::Discovered
                .ensure_transition(IngestionState::Settling, TransitionKind::System)
                .is_ok()
        );
        assert!(
            IngestionState::Discovered
                .ensure_transition(IngestionState::Settling, TransitionKind::Worker)
                .is_err()
        );
    }

    #[test]
    fn settling_self_transition_is_system_only() {
        assert!(
            IngestionState::Settling
                .ensure_transition(IngestionState::Settling, TransitionKind::System)
                .is_ok()
        );
        assert!(
            IngestionState::Settling
                .ensure_transition(IngestionState::Settling, TransitionKind::Worker)
                .is_err()
        );
    }

    #[test]
    fn failure_states_are_worker_transitions() {
        for from in [
            IngestionState::Inspecting,
            IngestionState::Normalizing,
            IngestionState::Validating,
            IngestionState::Publishing,
            IngestionState::Cataloging,
            IngestionState::Indexing,
        ] {
            assert!(
                from.ensure_transition(IngestionState::Failed, TransitionKind::Worker)
                    .is_ok(),
                "{from:?} -> Failed"
            );
        }
        assert!(
            IngestionState::Ready
                .ensure_transition(IngestionState::Failed, TransitionKind::Worker)
                .is_err()
        );
    }

    #[test]
    fn operator_review_transitions() {
        for to in [
            IngestionState::Staged,
            IngestionState::Quarantined,
            IngestionState::Ignored,
            IngestionState::Deleted,
        ] {
            assert!(
                IngestionState::NeedsReview
                    .ensure_transition(to, TransitionKind::Operator)
                    .is_ok(),
                "NeedsReview -> {to:?}"
            );
            assert!(
                IngestionState::NeedsReview
                    .ensure_transition(to, TransitionKind::Worker)
                    .is_err()
            );
        }
    }

    #[test]
    fn reprocess_semantics() {
        assert!(
            IngestionState::Failed
                .ensure_transition(IngestionState::Ready, TransitionKind::System)
                .is_ok(),
            "retry requeue at claim"
        );
        assert!(
            IngestionState::Failed
                .ensure_transition(IngestionState::Ready, TransitionKind::Operator)
                .is_ok(),
            "operator retry"
        );
        assert!(
            IngestionState::Quarantined
                .ensure_transition(IngestionState::Ready, TransitionKind::Operator)
                .is_ok()
        );
        assert!(
            IngestionState::Quarantined
                .ensure_transition(IngestionState::Ready, TransitionKind::Worker)
                .is_err()
        );
    }

    #[test]
    fn policy_worker_can_route_from_cataloged() {
        for to in [
            IngestionState::NeedsReview,
            IngestionState::Quarantined,
            IngestionState::Ignored,
            IngestionState::Indexing,
            IngestionState::Failed,
        ] {
            assert!(
                IngestionState::Cataloged
                    .ensure_transition(to, TransitionKind::Worker)
                    .is_ok(),
                "Cataloged -> {to:?} must be a legal worker (policy) transition"
            );
        }
        assert!(
            IngestionState::Cataloged
                .ensure_transition(IngestionState::Deleted, TransitionKind::Worker)
                .is_err(),
            "Cataloged -> Deleted must require operator authorization"
        );
    }

    #[test]
    fn cataloged_may_hold_policy_lease_but_parked_outcomes_may_not() {
        assert!(IngestionState::Cataloged.is_active_processing());
        assert!(IngestionState::Cataloged.can_fail());
        assert!(!IngestionState::NeedsReview.is_active_processing());
        assert!(!IngestionState::Quarantined.is_active_processing());
        assert!(!IngestionState::Ignored.is_active_processing());
    }

    #[test]
    fn skipping_work_is_rejected() {
        for kind in [
            TransitionKind::System,
            TransitionKind::Worker,
            TransitionKind::Operator,
        ] {
            assert!(
                IngestionState::Discovered
                    .ensure_transition(IngestionState::Indexed, kind)
                    .is_err()
            );
        }
    }

    #[test]
    fn terminal_states() {
        assert!(IngestionState::Indexed.is_terminal());
        assert!(IngestionState::Ignored.is_terminal());
        assert!(IngestionState::Deleted.is_terminal());
        assert!(!IngestionState::Quarantined.is_terminal());
        assert!(!IngestionState::Failed.is_terminal());
    }

    #[test]
    fn state_storage_round_trip() {
        for state in IngestionState::ALL {
            assert_eq!(IngestionState::parse(state.as_str()), Some(state));
        }
        assert_eq!(IngestionState::parse("bogus"), None);
    }

    #[test]
    fn retry_backoff_is_bounded_exponential() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.next_attempt_at(1, T0),
            Some(T0 + Duration::seconds(60))
        );
        assert_eq!(
            policy.next_attempt_at(2, T0),
            Some(T0 + Duration::seconds(120))
        );
        assert_eq!(
            policy.next_attempt_at(3, T0),
            Some(T0 + Duration::seconds(240))
        );
        assert_eq!(
            policy.next_attempt_at(4, T0),
            Some(T0 + Duration::seconds(480))
        );
        assert_eq!(
            policy.next_attempt_at(5, T0),
            Some(T0 + Duration::seconds(960))
        );
        assert_eq!(
            policy.next_attempt_at(6, T0),
            None,
            "exhausted after max_attempts"
        );
        assert!(!policy.can_retry(5));
        assert!(policy.can_retry(4));
    }

    #[test]
    fn retry_backoff_caps_at_max_delay() {
        let policy = RetryPolicy {
            base_delay: Duration::seconds(3600),
            max_delay: Duration::hours(1),
            max_attempts: 10,
        };
        assert_eq!(policy.next_attempt_at(3, T0), Some(T0 + Duration::hours(1)));
    }

    #[test]
    fn event_type_round_trip() {
        for event in [
            WorkEventType::Created,
            WorkEventType::Claimed,
            WorkEventType::LeaseRenewed,
            WorkEventType::LeaseReleased,
            WorkEventType::LeaseExpiredRecovered,
            WorkEventType::Transitioned,
            WorkEventType::Failed,
            WorkEventType::RetryScheduled,
            WorkEventType::Completed,
        ] {
            assert_eq!(WorkEventType::parse(event.as_str()), Some(event));
        }
    }

    #[test]
    fn active_lease_semantics() {
        let mut item = ReleaseWorkItem::discovered("/inbound/x".into(), None, Uuid::now_v7(), T0);
        assert!(!item.has_active_lease(T0));
        item.lease_owner = Some("worker-1".into());
        item.lease_expires_at = Some(T0 + Duration::seconds(60));
        assert!(item.has_active_lease(T0));
        assert!(!item.has_active_lease(T0 + Duration::seconds(61)));
    }
}

#[cfg(test)]
mod phase_tests {
    use super::{IngestionPhase, IngestionState};
    use std::collections::BTreeSet;

    #[test]
    fn every_state_has_exactly_one_phase() {
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        for state in IngestionState::ALL {
            assert!(
                seen.insert(state.as_str()),
                "state {state:?} appeared twice while mapping phases"
            );
            // Totality is enforced by the exhaustive match in `phase()`, but the
            // phase must also be a declared member of IngestionPhase::ALL.
            assert!(IngestionPhase::ALL.contains(&state.phase()));
        }
        assert_eq!(seen.len(), IngestionState::ALL.len());
    }

    #[test]
    fn phases_partition_the_state_set() {
        let mut from_phases: BTreeSet<&'static str> = BTreeSet::new();
        for phase in IngestionPhase::ALL {
            for state in phase.states() {
                assert!(
                    from_phases.insert(state.as_str()),
                    "state {state:?} is listed by more than one phase"
                );
                assert_eq!(
                    state.phase(),
                    phase,
                    "IngestionPhase::states() and IngestionState::phase() disagree for {state:?}"
                );
            }
        }
        assert_eq!(
            from_phases.len(),
            IngestionState::ALL.len(),
            "phases must cover every state exactly once"
        );
    }

    #[test]
    fn only_the_active_phase_is_client_visible() {
        for state in IngestionState::ALL {
            assert_eq!(
                state.is_client_visible(),
                state.phase() == IngestionPhase::Active,
                "{state:?} disagrees between is_client_visible and its phase"
            );
        }
        // The last state a client cannot see is cataloged; the first it can is
        // indexing (the policy accept commit).
        assert!(!IngestionState::Cataloged.is_client_visible());
        assert!(IngestionState::Indexing.is_client_visible());
    }

    #[test]
    fn phase_wire_form_round_trips() {
        for phase in IngestionPhase::ALL {
            assert_eq!(IngestionPhase::parse(phase.as_str()), Some(phase));
        }
        assert_eq!(IngestionPhase::parse("nonsense"), None);
        assert_eq!(IngestionPhase::parse(""), None);
    }

    #[test]
    fn terminal_states_report_a_finished_phase() {
        // A terminal state must never project onto an "in progress" phase.
        for state in IngestionState::ALL {
            if state.is_terminal() {
                assert!(
                    matches!(
                        state.phase(),
                        IngestionPhase::Active | IngestionPhase::Disposition
                    ),
                    "terminal state {state:?} maps to {:?}",
                    state.phase()
                );
            }
        }
    }
}

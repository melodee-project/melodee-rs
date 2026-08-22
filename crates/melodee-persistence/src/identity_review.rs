//! read-side operator queries over the Artist identity schema
//! (tables, ADR-0134 parity domain): unresolved observation listing,
//! observation detail with candidate evidence, the identity decision audit
//! feed, and public-id resolution for merge/split entry points.
//!
//! Every query here is a pure read over existing tables — no new tables, no
//! recomputation of resolver logic. Mutations continue to live in
//! `catalog_mutations.rs` / `catalog_identity.rs`; this module only surfaces
//! what those paths durably recorded.

use melodee_domain::ArtistId;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CatalogStore, CatalogStoreError};

/// Sort keys accepted by the unresolved-observation listing. Each variant
/// maps to a constant `ORDER BY` fragment over indexed plain columns — the
/// whitelist is the injection boundary, so client input never reaches SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedArtistSort {
    CreatedAtAsc,
    CreatedAtDesc,
    DisplayValueAsc,
    DisplayValueDesc,
}

impl ObservedArtistSort {
    /// Parses a strict `key:asc|desc` sort expression. Keys are camelCase API
    /// names (`createdAt`, `displayValue`); anything else is rejected.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (key, direction) = raw.split_once(':')?;
        match (key, direction) {
            ("createdAt", "asc") => Some(Self::CreatedAtAsc),
            ("createdAt", "desc") => Some(Self::CreatedAtDesc),
            ("displayValue", "asc") => Some(Self::DisplayValueAsc),
            ("displayValue", "desc") => Some(Self::DisplayValueDesc),
            _ => None,
        }
    }

    /// Constant SQL fragment; every variant ends with the unique `id`
    /// tiebreaker so paging is deterministic.
    const fn order_by(self) -> &'static str {
        match self {
            Self::CreatedAtAsc => "o.created_at ASC, o.id",
            Self::CreatedAtDesc => "o.created_at DESC, o.id",
            Self::DisplayValueAsc => "o.display_value ASC, o.id",
            Self::DisplayValueDesc => "o.display_value DESC, o.id",
        }
    }
}

/// One observed Artist string awaiting an authoritative resolution, with its
/// Release/Track source context and pending-match counts. An observation is
/// unresolved while `artist_match_current` has no authoritative decision for
/// it (proposals never advance the current pointer).
#[derive(Debug, Clone)]
pub struct UnresolvedObservedArtist {
    pub id: Uuid,
    pub release_id: Uuid,
    pub release_public_id: Uuid,
    pub release_title: String,
    pub track_id: Option<Uuid>,
    pub track_public_id: Option<Uuid>,
    pub track_title: Option<String>,
    pub scope: String,
    pub source_ordinal: i32,
    pub raw_value: String,
    pub normalized_value: String,
    pub display_value: String,
    pub source_pointer: String,
    /// Distinct candidate Artists with a proposed (not yet authoritative)
    /// match decision.
    pub pending_match_count: i64,
    /// Immutable match-evidence rows recorded for this observation.
    pub evidence_count: i64,
    pub created_at: OffsetDateTime,
}

/// The authoritative (current) match decision for an observation, if any.
#[derive(Debug, Clone)]
pub struct ObservedArtistCurrentDecision {
    pub artist_id: Uuid,
    pub artist_public_id: Uuid,
    pub artist_name: String,
    pub outcome: String,
    pub authority: String,
    pub confidence: i32,
    pub reason: String,
    pub locked: bool,
    pub decided_at: OffsetDateTime,
}

/// One immutable evidence fact persisted with an observation/candidate pair.
#[derive(Debug, Clone)]
pub struct ObservedArtistMatchEvidence {
    pub kind: String,
    pub polarity: String,
    pub confidence: i32,
    pub source: String,
    pub payload: serde_json::Value,
    pub created_at: OffsetDateTime,
}

/// One candidate Artist for an observation: the latest decision state plus
/// every persisted evidence row for the pair.
#[derive(Debug, Clone)]
pub struct ObservedArtistCandidate {
    pub artist_id: Uuid,
    pub artist_public_id: Uuid,
    pub artist_name: String,
    pub artist_status: String,
    pub latest_outcome: String,
    pub latest_confidence: i32,
    pub latest_reason: String,
    pub latest_locked: bool,
    pub latest_decided_at: OffsetDateTime,
    pub decision_count: i64,
    pub evidence: Vec<ObservedArtistMatchEvidence>,
}

/// One durable do-not-match assertion targeting this observation.
#[derive(Debug, Clone)]
pub struct ObservedArtistNegativeAssertion {
    pub id: Uuid,
    pub candidate_artist_id: Option<Uuid>,
    pub candidate_artist_public_id: Option<Uuid>,
    pub candidate_artist_name: Option<String>,
    pub candidate_provider: Option<String>,
    pub candidate_external_id: Option<String>,
    pub authority: String,
    pub reason: String,
    pub locked: bool,
    /// False once a `revoke` successor supersedes this assertion.
    pub active: bool,
    pub created_at: OffsetDateTime,
}

/// Full operator review image of one observed Artist.
#[derive(Debug, Clone)]
pub struct ObservedArtistDetail {
    pub observation: UnresolvedObservedArtist,
    pub current: Option<ObservedArtistCurrentDecision>,
    pub candidates: Vec<ObservedArtistCandidate>,
    pub negative_assertions: Vec<ObservedArtistNegativeAssertion>,
}

/// One identity-domain audit event (who/what/when/reason/outcome).
#[derive(Debug, Clone)]
pub struct IdentityDecisionFeedEntry {
    pub event_id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub event_type: String,
    pub actor: String,
    pub reason: String,
    pub payload: serde_json::Value,
    pub created_at: OffsetDateTime,
}

/// Internal + public Artist identity resolution for merge/split entry points.
#[derive(Debug, Clone)]
pub struct ArtistIdentityRef {
    pub id: ArtistId,
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub status: String,
}

#[derive(Debug, sqlx::FromRow)]
struct UnresolvedRow {
    id: Uuid,
    release_id: Uuid,
    release_public_id: Uuid,
    release_title: String,
    track_id: Option<Uuid>,
    track_public_id: Option<Uuid>,
    track_title: Option<String>,
    scope: String,
    source_ordinal: i32,
    raw_value: String,
    normalized_value: String,
    display_value: String,
    source_pointer: String,
    pending_match_count: i64,
    evidence_count: i64,
    created_at: OffsetDateTime,
}

impl From<UnresolvedRow> for UnresolvedObservedArtist {
    fn from(row: UnresolvedRow) -> Self {
        Self {
            id: row.id,
            release_id: row.release_id,
            release_public_id: row.release_public_id,
            release_title: row.release_title,
            track_id: row.track_id,
            track_public_id: row.track_public_id,
            track_title: row.track_title,
            scope: row.scope,
            source_ordinal: row.source_ordinal,
            raw_value: row.raw_value,
            normalized_value: row.normalized_value,
            display_value: row.display_value,
            source_pointer: row.source_pointer,
            pending_match_count: row.pending_match_count,
            evidence_count: row.evidence_count,
            created_at: row.created_at,
        }
    }
}

/// Shared observation/context projection used by both the listing (filtered
/// to unresolved) and the detail read (unfiltered).
const OBSERVATION_SELECT: &str = "SELECT o.id, o.release_id, r.public_api_id AS release_public_id,
            r.title AS release_title, o.track_id,
            t.public_api_id AS track_public_id, t.title AS track_title,
            o.scope, o.source_ordinal, o.raw_value, o.normalized_value,
            o.display_value, o.source_pointer,
            (SELECT count(DISTINCT proposed.artist_id)
               FROM artist_match_decisions proposed
              WHERE proposed.observed_artist_id = o.id
                AND proposed.outcome = 'proposed') AS pending_match_count,
            (SELECT count(*)
               FROM artist_match_evidence evidence
              WHERE evidence.observed_artist_id = o.id) AS evidence_count,
            o.created_at
       FROM observed_artists o
       JOIN releases r ON r.id = o.release_id
       LEFT JOIN tracks t ON t.id = o.track_id";

impl CatalogStore {
    /// Counts observed Artists without an authoritative match decision (the
    /// unresolved identity queue depth) without loading any rows — the
    /// dashboard attention count.
    pub async fn count_unresolved_observations(
        &self,
        pool: &PgPool,
    ) -> Result<i64, CatalogStoreError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM observed_artists o
             WHERE NOT EXISTS (
                 SELECT 1 FROM artist_match_current current
                 WHERE current.observed_artist_id = o.id)",
        )
        .fetch_one(pool)
        .await?)
    }

    /// Lists observed Artists without an authoritative match decision
    /// (the unresolved identity queue), paged and whitelist-sorted, plus the
    /// total row count for the filter.
    pub async fn list_unresolved_observations(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
        sort: ObservedArtistSort,
    ) -> Result<(Vec<UnresolvedObservedArtist>, i64), CatalogStoreError> {
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM observed_artists o
             WHERE NOT EXISTS (
                 SELECT 1 FROM artist_match_current current
                 WHERE current.observed_artist_id = o.id)",
        )
        .fetch_one(pool)
        .await?;
        // Injection audit: `order_by` returns a constant `&'static str`
        // fragment selected from the ObservedArtistSort whitelist; only bind
        // parameters carry client-influenced values.
        let rows = sqlx::query_as::<_, UnresolvedRow>(sqlx::AssertSqlSafe(format!(
            "{OBSERVATION_SELECT}
             WHERE NOT EXISTS (
                 SELECT 1 FROM artist_match_current current
                 WHERE current.observed_artist_id = o.id)
             ORDER BY {}
             LIMIT $1 OFFSET $2",
            sort.order_by()
        )))
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok((
            rows.into_iter()
                .map(UnresolvedObservedArtist::from)
                .collect(),
            total,
        ))
    }

    /// Loads the full operator review image of one observed Artist: source
    /// context, authoritative decision (if any), candidate Artists with
    /// their persisted evidence, and durable negative assertions.
    pub async fn observed_artist_detail(
        &self,
        pool: &PgPool,
        observed_artist_id: Uuid,
    ) -> Result<Option<ObservedArtistDetail>, CatalogStoreError> {
        let observation = sqlx::query_as::<_, UnresolvedRow>(sqlx::AssertSqlSafe(format!(
            "{OBSERVATION_SELECT} WHERE o.id = $1"
        )))
        .bind(observed_artist_id)
        .fetch_optional(pool)
        .await?;
        let Some(observation) = observation else {
            return Ok(None);
        };

        let current = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                String,
                String,
                String,
                i32,
                String,
                bool,
                OffsetDateTime,
            ),
        >(
            "SELECT decision.artist_id, artist.public_api_id, artist.canonical_name,
                    decision.outcome, decision.authority, decision.confidence,
                    decision.reason, decision.is_locked, decision.created_at
               FROM artist_match_current current
               JOIN artist_match_decisions decision ON decision.id = current.decision_id
               JOIN artists artist ON artist.id = decision.artist_id
              WHERE current.observed_artist_id = $1",
        )
        .bind(observed_artist_id)
        .fetch_optional(pool)
        .await?
        .map(
            |(artist_id, public_id, name, outcome, authority, confidence, reason, locked, at)| {
                ObservedArtistCurrentDecision {
                    artist_id,
                    artist_public_id: public_id,
                    artist_name: name,
                    outcome,
                    authority,
                    confidence,
                    reason,
                    locked,
                    decided_at: at,
                }
            },
        );

        // Latest decision per candidate Artist (DISTINCT ON keeps the newest
        // image); the per-candidate decision count is the proposal history
        // depth. Candidates are then ordered by confidence, newest first.
        let mut candidates: Vec<ObservedArtistCandidate> = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                String,
                String,
                String,
                i32,
                String,
                bool,
                OffsetDateTime,
                i64,
            ),
        >(
            "SELECT DISTINCT ON (decision.artist_id)
                    decision.artist_id, artist.public_api_id, artist.canonical_name,
                    artist.status, decision.outcome, decision.confidence,
                    decision.reason, decision.is_locked, decision.created_at,
                    (SELECT count(*) FROM artist_match_decisions history
                      WHERE history.observed_artist_id = $1
                        AND history.artist_id = decision.artist_id) AS decision_count
               FROM artist_match_decisions decision
               JOIN artists artist ON artist.id = decision.artist_id
              WHERE decision.observed_artist_id = $1
              ORDER BY decision.artist_id, decision.created_at DESC, decision.id DESC",
        )
        .bind(observed_artist_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(
                artist_id,
                public_id,
                name,
                status,
                outcome,
                confidence,
                reason,
                locked,
                at,
                decision_count,
            )| {
                ObservedArtistCandidate {
                    artist_id,
                    artist_public_id: public_id,
                    artist_name: name,
                    artist_status: status,
                    latest_outcome: outcome,
                    latest_confidence: confidence,
                    latest_reason: reason,
                    latest_locked: locked,
                    latest_decided_at: at,
                    decision_count,
                    evidence: Vec::new(),
                }
            },
        )
        .collect();
        candidates.sort_by(|left, right| {
            right
                .latest_confidence
                .cmp(&left.latest_confidence)
                .then_with(|| right.latest_decided_at.cmp(&left.latest_decided_at))
                .then_with(|| left.artist_id.cmp(&right.artist_id))
        });

        let evidence_rows = sqlx::query_as::<
            _,
            (
                Option<Uuid>,
                String,
                String,
                i32,
                String,
                serde_json::Value,
                OffsetDateTime,
            ),
        >(
            "SELECT candidate_artist_id, evidence_kind, polarity, confidence,
                    source, payload, created_at
               FROM artist_match_evidence
              WHERE observed_artist_id = $1
              ORDER BY created_at, id",
        )
        .bind(observed_artist_id)
        .fetch_all(pool)
        .await?;
        for (candidate_id, kind, polarity, confidence, source, payload, created_at) in evidence_rows
        {
            let Some(candidate_id) = candidate_id else {
                continue;
            };
            if let Some(candidate) = candidates
                .iter_mut()
                .find(|candidate| candidate.artist_id == candidate_id)
            {
                candidate.evidence.push(ObservedArtistMatchEvidence {
                    kind,
                    polarity,
                    confidence,
                    source,
                    payload,
                    created_at,
                });
            }
        }

        let negative_assertions = sqlx::query_as::<
            _,
            (
                Uuid,
                Option<Uuid>,
                Option<Uuid>,
                Option<String>,
                Option<String>,
                Option<String>,
                String,
                String,
                bool,
                bool,
                OffsetDateTime,
            ),
        >(
            "SELECT assertion.id, assertion.candidate_artist_id,
                    artist.public_api_id, artist.canonical_name,
                    assertion.candidate_provider, assertion.candidate_external_id,
                    assertion.authority, assertion.reason, assertion.is_locked,
                    NOT EXISTS (
                        SELECT 1 FROM artist_negative_assertions successor
                        WHERE successor.predecessor_assertion_id = assertion.id
                    ) AS active,
                    assertion.created_at
               FROM artist_negative_assertions assertion
               LEFT JOIN artists artist ON artist.id = assertion.candidate_artist_id
              WHERE assertion.observed_artist_id = $1
                AND assertion.assertion_kind = 'do_not_match'
              ORDER BY assertion.created_at DESC, assertion.id DESC",
        )
        .bind(observed_artist_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(
                id,
                candidate_artist_id,
                candidate_public_id,
                candidate_name,
                provider,
                external_id,
                authority,
                reason,
                locked,
                active,
                created_at,
            )| {
                ObservedArtistNegativeAssertion {
                    id,
                    candidate_artist_id,
                    candidate_artist_public_id: candidate_public_id,
                    candidate_artist_name: candidate_name,
                    candidate_provider: provider,
                    candidate_external_id: external_id,
                    authority,
                    reason,
                    locked,
                    active,
                    created_at,
                }
            },
        )
        .collect();

        Ok(Some(ObservedArtistDetail {
            observation: UnresolvedObservedArtist::from(observation),
            current,
            candidates,
            negative_assertions,
        }))
    }

    /// Lists the identity-domain audit feed (observed-Artist and Artist
    /// events: match decisions, negative assertions, merges, splits), newest
    /// first, plus the total row count.
    pub async fn list_identity_decisions(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<IdentityDecisionFeedEntry>, i64), CatalogStoreError> {
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM catalog_audit_events
             WHERE entity_type IN ('observed_artist', 'artist')",
        )
        .fetch_one(pool)
        .await?;
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Uuid,
                String,
                String,
                String,
                serde_json::Value,
                OffsetDateTime,
            ),
        >(
            "SELECT event_id, entity_type, entity_id, event_type, actor,
                    reason, payload, created_at
               FROM catalog_audit_events
              WHERE entity_type IN ('observed_artist', 'artist')
              ORDER BY created_at DESC, id DESC
              LIMIT $1 OFFSET $2",
        )
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        let entries = rows
            .into_iter()
            .map(
                |(event_id, entity_type, entity_id, event_type, actor, reason, payload, at)| {
                    IdentityDecisionFeedEntry {
                        event_id,
                        entity_type,
                        entity_id,
                        event_type,
                        actor,
                        reason,
                        payload,
                        created_at: at,
                    }
                },
            )
            .collect();
        Ok((entries, total))
    }

    /// Resolves an Artist public id to its internal identity and status for
    /// merge/split/resolve entry points. Public ids are preserved verbatim
    /// (legacy UUIDs included); they are lookup keys, never regenerated.
    pub async fn artist_by_public_id(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<ArtistIdentityRef>, CatalogStoreError> {
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, canonical_name, status FROM artists WHERE public_api_id = $1",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(|(id, canonical_name, status)| ArtistIdentityRef {
            id: ArtistId::from_uuid(id),
            public_api_id,
            canonical_name,
            status,
        }))
    }
}

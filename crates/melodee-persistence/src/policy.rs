//! PostgreSQL persistence for classification, novelty, policy
//! rules, durable decisions, and dry-run results (ADR-0120).
//!
//! This crate never implements classification, novelty, or policy logic —
//! it persists and retrieves the durable facts those engines produce.

use melodee_domain::{
    DecisionCompletionState, Disposition, FailureClass, IngestionState, NoveltyEvaluation,
    PolicyActor, PolicyRuleSet, ReleaseClassification, ReleaseId, ReleasePolicyDecision,
    ReleaseWorkItem, RetryPolicy, TrackNovelty, TransitionKind, WorkErrorCode, WorkEventType,
};
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use tracing::instrument;
use uuid::Uuid;

use crate::config::WorkEngineConfig;
use crate::error::WorkStoreError;
use crate::work_items::{WorkItemRow, ensure_active_lease, insert_event, lock_for_update};
use crate::worker::WorkerId;

/// Maximum sample Release IDs returned by one dry-run (operator-friendly).
pub const MAX_POLICY_TEST_SAMPLES: usize = 20;
/// Maximum Releases loaded by one read-only dry-run snapshot query.
pub const MAX_POLICY_TEST_RELEASES: usize = 10_000;
/// Maximum Track facts materialized across one dry-run sample.
pub const MAX_POLICY_TEST_TRACK_FACTS: usize = 10_000;
/// Maximum stored classification trait/evidence facts scanned by one dry run.
pub const MAX_POLICY_TEST_CLASSIFICATION_FACTS: usize = 10_000;
/// Maximum Track facts evaluated and persisted for one Release.
pub const MAX_POLICY_TRACKS_PER_RELEASE: usize = 10_000;
/// Maximum evidence rows accepted for one Release classification.
pub const MAX_POLICY_CLASSIFICATION_EVIDENCE: usize = 1_000;

const SUPPORTED_CLASSIFIER_VERSION: &str = "melodee.policy.classifier-v1";
const SUPPORTED_NOVELTY_EVALUATOR_VERSION: &str = "melodee.policy.novelty-v1";

/// Typed persistence failures.
#[derive(Debug, Error)]
pub enum PolicyStoreError {
    #[error("policy database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("policy serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("policy domain invariant failed")]
    Domain(#[from] melodee_domain::DomainInvariantError),
    #[error("policy work-item operation failed")]
    Work(#[from] WorkStoreError),
    #[error("policy rule set not found: {0}")]
    RuleSetNotFound(String),
    #[error("release not cataloged: {0}")]
    NotCataloged(Uuid),
    #[error("policy value exceeds the PostgreSQL representation")]
    NumericOverflow,
    #[error("persisted policy value is unknown: {0}")]
    CorruptValue(String),
    #[error("policy rule set conflicts with immutable stored version")]
    RuleSetConflict,
    #[error("policy source fingerprint does not match the immutable catalog import")]
    FingerprintMismatch,
    #[error("policy facts do not belong to the claimed Release or its authoritative Tracks")]
    ForeignFacts,
    #[error("policy evidence is incomplete")]
    IncompleteFacts,
    #[error("policy decision conflicts with an existing durable decision")]
    DecisionConflict,
    #[error("administrator authorization is required for this policy action")]
    AuthorizationRequired,
    #[error("policy operation exceeds the {dimension} limit of {limit}")]
    ScopeExceeded {
        dimension: &'static str,
        limit: usize,
    },
}

/// One Track's catalog presence used for novelty assessment.
#[derive(Debug, Clone, serde::Deserialize, sqlx::FromRow)]
pub struct CatalogTrackFact {
    pub release_id: Uuid,
    pub track_id: Uuid,
    pub audio_identity_id: Uuid,
    pub media_asset_id: Option<Uuid>,
    pub byte_hash: Option<String>,
    /// Nonblank Chromaprint observation from the immutable v3 manifest.
    pub audio_fingerprint: Option<String>,
    /// Durable comparison key for `audio_fingerprint` (see
    /// [`melodee_domain::acoustic_fingerprint_digest`]).
    pub audio_fingerprint_digest: Option<String>,
    pub has_confirmed_audio_identity: bool,
}

/// One fenced Cataloged work item and its immutable catalog source binding.
#[derive(Debug, Clone)]
pub struct PolicyClaim {
    pub item: ReleaseWorkItem,
    pub release_id: ReleaseId,
    pub title: String,
    /// The publication receipt's immutable sealed-manifest hash.
    pub source_fingerprint: String,
    pub semantic_manifest_hash: String,
}

/// Bounded authoritative facts consumed by the pure engines.
#[derive(Debug, Clone)]
pub struct PolicyCatalogContext {
    pub claim: PolicyClaim,
    pub tracks: Vec<CatalogTrackFact>,
    pub current_classification: Option<StoredReleaseClassification>,
    pub current_decision: Option<StoredPolicyDecision>,
    pub structured_tag_traits: Vec<melodee_domain::ReleaseTrait>,
    pub embedded_external_id_traits: Vec<melodee_domain::ReleaseTrait>,
    pub catalog_known_release_traits: Vec<melodee_domain::ReleaseTrait>,
    pub existing_disposition: Option<Disposition>,
    pub has_corrupt_media: bool,
    /// Byte hashes from this Release that also occur in another Release.
    pub catalog_exact_byte_hashes: std::collections::BTreeSet<String>,
    /// Audio identities referenced by another Release.
    pub catalog_known_audio_identities: std::collections::BTreeSet<melodee_domain::AudioIdentityId>,
    /// Audio identities with a durable confirmed distinct-recording decision.
    pub catalog_alternate_audio_identities:
        std::collections::BTreeSet<melodee_domain::AudioIdentityId>,
    /// Acoustic fingerprint digests already present in *another* Release, so a
    /// re-encoded or retagged copy is recognised instead of being called new.
    pub catalog_acoustic_fingerprints: std::collections::BTreeSet<String>,
}

/// Atomic, lease-fenced policy evaluation write.
#[derive(Debug, Clone)]
pub struct PolicyEvaluationCommit {
    pub work_item_id: Uuid,
    pub release_id: ReleaseId,
    pub source_fingerprint: String,
    pub classification: ReleaseClassification,
    pub novelty_evaluation: NoveltyEvaluation,
    pub track_novelty: Vec<TrackNovelty>,
    pub decision: ReleasePolicyDecision,
    pub now: OffsetDateTime,
}

/// Durable outcome of applying or adopting an identical policy evaluation.
#[derive(Debug, Clone)]
pub struct PolicyCommitResult {
    pub item: ReleaseWorkItem,
    pub decision: StoredPolicyDecision,
    pub inserted: bool,
    /// Set-based mutation statements (not reads/fencing) executed by this path.
    pub set_based_mutation_statement_count: u32,
}

/// Explicit administrator override of the current pending outcome.
#[derive(Debug, Clone)]
pub struct PolicyAdminOverrideRequest {
    pub work_item_id: Uuid,
    pub decision_id: melodee_domain::ReleasePolicyDecisionId,
    pub chosen_disposition: Disposition,
    pub actor: String,
    pub reason: String,
    pub source_fingerprint: String,
    pub now: OffsetDateTime,
}

/// Authorization evidence recorded after destructive execution succeeds.
#[derive(Debug, Clone)]
pub struct PolicyDestructiveCompletion {
    pub work_item_id: Uuid,
    pub decision_id: melodee_domain::ReleasePolicyDecisionId,
    pub actor: String,
    pub reason: String,
    pub source_fingerprint: String,
    /// Durable publication directory that was removed below the configured
    /// canonical root. This remains audit evidence after catalog tombstoning.
    pub canonical_relative_path: String,
    /// Durable source directory removed below the configured staging root,
    /// when this Release originated in the current ingestion pipeline.
    pub staging_relative_path: Option<String>,
    pub completed_at: OffsetDateTime,
}

/// Immutable audit image for one policy decision change.
#[derive(Debug, Clone)]
pub struct PolicyDecisionAuditEntry {
    pub decision: StoredPolicyDecision,
    pub event: String,
    pub created_at: OffsetDateTime,
}

/// Audited resolver/operator evidence that no catalog identity matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAudioIdentityNoMatchAssertion {
    pub id: Uuid,
    pub audio_identity_id: melodee_domain::AudioIdentityId,
    pub actor: String,
    pub reason: String,
    pub evidence: serde_json::Value,
    pub created_at: OffsetDateTime,
}

/// Audited assertion that two identities are distinct recordings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAudioIdentityAlternateAssertion {
    pub source_audio_identity_id: melodee_domain::AudioIdentityId,
    pub alternate_audio_identity_id: melodee_domain::AudioIdentityId,
    pub evidence_id: Uuid,
    pub actor: String,
    pub reason: String,
    pub created_at: OffsetDateTime,
}

/// Raw `policy_rule_sets` row shared by the rule-set loaders.
type PolicyRuleSetRow = (
    Uuid,
    String,
    i32,
    serde_json::Value,
    serde_json::Value,
    OffsetDateTime,
);

fn parse_rule_set_row(row: PolicyRuleSetRow) -> Result<PolicyRuleSet, PolicyStoreError> {
    let (id, name, version, rules_json, actor_json, created_at) = row;
    let rules: Vec<melodee_domain::PolicyRule> = serde_json::from_value(rules_json)?;
    let created_by: PolicyActor = serde_json::from_value(actor_json)?;
    Ok(PolicyRuleSet {
        id: melodee_domain::PolicyRuleSetId::from_uuid(id),
        name,
        version: u32::try_from(version).map_err(|_| PolicyStoreError::NumericOverflow)?,
        rules,
        created_by,
        created_at,
    })
}

/// Title and primary-Artist display value for one dry-run sample Release.
/// A display string is never an Artist identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyTestSampleRelease {
    pub release_id: Uuid,
    pub title: String,
    pub artist: Option<String>,
}

/// Independently derived catalog knowledge supporting one Release trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyReleaseCatalogTraitAssertion {
    pub id: Uuid,
    pub release_id: ReleaseId,
    pub trait_: melodee_domain::ReleaseTrait,
    pub source_version: String,
    pub actor: String,
    pub reason: String,
    pub evidence: serde_json::Value,
    pub created_at: OffsetDateTime,
}

/// Stateless PostgreSQL policy repository.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyStore;

impl PolicyStore {
    // ------------------------------------------------------------------
    // Policy rule sets
    // ------------------------------------------------------------------

    /// Persists a new versioned policy rule set. Fails if `(name, version)`
    /// already exists (ON CONFLICT DO NOTHING makes the insert idempotent).
    #[instrument(skip_all)]
    pub async fn create_rule_set(
        &self,
        pool: &PgPool,
        rule_set: &PolicyRuleSet,
        now: OffsetDateTime,
    ) -> Result<(), PolicyStoreError> {
        self.ensure_rule_set(pool, rule_set, now).await.map(|_| ())
    }

    /// Inserts an immutable rule-set version or adopts an exactly identical
    /// retry. A `(name, version)` collision with different content fails.
    #[instrument(skip_all, fields(rule_set_id = %rule_set.id.into_uuid(), version = rule_set.version))]
    pub async fn ensure_rule_set(
        &self,
        pool: &PgPool,
        rule_set: &PolicyRuleSet,
        _now: OffsetDateTime,
    ) -> Result<PolicyRuleSet, PolicyStoreError> {
        let mut validated = PolicyRuleSet::new(rule_set.clone())?;
        let micros = (validated.created_at.nanosecond() / 1_000) * 1_000;
        validated.created_at = validated
            .created_at
            .replace_nanosecond(micros)
            .map_err(|error| PolicyStoreError::CorruptValue(error.to_string()))?;
        let rules_json = serde_json::to_value(&validated.rules)?;
        let actor_json = serde_json::to_value(&validated.created_by)?;
        sqlx::query(
            r#"INSERT INTO policy_rule_sets
                (id, name, version, rules, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (name, version) DO NOTHING"#,
        )
        .bind(validated.id.into_uuid())
        .bind(&validated.name)
        .bind(i32::try_from(validated.version).map_err(|_| PolicyStoreError::NumericOverflow)?)
        .bind(rules_json)
        .bind(actor_json)
        .bind(validated.created_at)
        .execute(pool)
        .await?;

        let stored = self
            .load_rule_set_by_id(pool, validated.id)
            .await?
            .ok_or(PolicyStoreError::RuleSetConflict)?;
        if stored != validated {
            return Err(PolicyStoreError::RuleSetConflict);
        }
        Ok(stored)
    }

    /// Loads the latest (highest version) rule set by name.
    #[instrument(skip_all)]
    pub async fn load_rule_set(
        &self,
        pool: &PgPool,
        name: &str,
    ) -> Result<Option<PolicyRuleSet>, PolicyStoreError> {
        let row: Option<PolicyRuleSetRow> = sqlx::query_as(
            r#"SELECT id, name, version, rules, created_by, created_at
                   FROM policy_rule_sets
                  WHERE name = $1
                  ORDER BY version DESC
                  LIMIT 1"#,
        )
        .bind(name)
        .fetch_optional(pool)
        .await?;
        row.map(parse_rule_set_row).transpose()
    }

    /// Loads a rule set by its UUID.
    #[instrument(skip_all)]
    pub async fn load_rule_set_by_id(
        &self,
        pool: &PgPool,
        id: melodee_domain::PolicyRuleSetId,
    ) -> Result<Option<PolicyRuleSet>, PolicyStoreError> {
        let row: Option<PolicyRuleSetRow> = sqlx::query_as(
            r#"SELECT id, name, version, rules, created_by, created_at
               FROM policy_rule_sets
               WHERE id = $1"#,
        )
        .bind(id.into_uuid())
        .fetch_optional(pool)
        .await?;
        row.map(parse_rule_set_row).transpose()
    }

    /// Lists every persisted version of a named rule set, oldest first. Rule
    /// sets are immutable; this history is how the policy editor finds the
    /// last-seen definition of a rule removed from the current version.
    #[instrument(skip_all)]
    pub async fn list_rule_set_versions(
        &self,
        pool: &PgPool,
        name: &str,
    ) -> Result<Vec<PolicyRuleSet>, PolicyStoreError> {
        let rows: Vec<PolicyRuleSetRow> = sqlx::query_as(
            r#"SELECT id, name, version, rules, created_by, created_at
               FROM policy_rule_sets
               WHERE name = $1
               ORDER BY version"#,
        )
        .bind(name)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(parse_rule_set_row).collect()
    }

    /// Set-based per-rule match counts from durable policy decisions (how
    /// often each rule id decided a Release). Powers the admin rule listing's
    /// match column.
    pub async fn rule_match_counts(
        &self,
        pool: &PgPool,
    ) -> Result<Vec<(String, i64)>, PolicyStoreError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            r#"SELECT matched.rule_id, COUNT(*)::bigint
                 FROM policy_decisions decision,
                      jsonb_array_elements_text(decision.matched_rule_ids) AS matched(rule_id)
                GROUP BY matched.rule_id
                ORDER BY matched.rule_id"#,
        )
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// Counts durable decisions still awaiting completion — the dashboard
    /// "pending policy decisions" attention count.
    pub async fn count_pending_decisions(&self, pool: &PgPool) -> Result<i64, PolicyStoreError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM policy_decisions WHERE completion_state = 'pending'",
        )
        .fetch_one(pool)
        .await?)
    }

    /// Loads title and primary-Artist display name for a bounded set of
    /// Release ids (dry-run sample enrichment). Bounded by
    /// the caller; the admin surface passes at most `MAX_POLICY_TEST_SAMPLES`.
    pub async fn test_sample_releases(
        &self,
        pool: &PgPool,
        release_ids: &[ReleaseId],
    ) -> Result<Vec<PolicyTestSampleRelease>, PolicyStoreError> {
        if release_ids.len() > MAX_POLICY_TEST_SAMPLES {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "policy test sample Releases",
                limit: MAX_POLICY_TEST_SAMPLES,
            });
        }
        let ids: Vec<Uuid> = release_ids.iter().map(|id| id.into_uuid()).collect();
        let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            r#"SELECT release.id, release.title,
                      (SELECT artist.canonical_name
                         FROM release_artists billing
                         JOIN artists artist ON artist.id = billing.artist_id
                        WHERE billing.release_id = release.id
                          AND billing.role = 'primary'
                        ORDER BY billing.presentation_order, billing.id
                        LIMIT 1) AS artist
                 FROM releases release
                WHERE release.id = ANY($1)
                ORDER BY release.id"#,
        )
        .bind(&ids)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(release_id, title, artist)| PolicyTestSampleRelease {
                release_id,
                title,
                artist,
            })
            .collect())
    }

    /// Inserts an immutable no-catalog-match fact or adopts an exact retry.
    pub async fn record_audio_identity_no_match(
        &self,
        pool: &PgPool,
        assertion: &PolicyAudioIdentityNoMatchAssertion,
    ) -> Result<(), PolicyStoreError> {
        if assertion.actor.trim().is_empty()
            || assertion.reason.trim().is_empty()
            || !assertion.audio_identity_id.is_uuid_v7()
            || assertion.evidence.as_object().is_none()
            || assertion.id.get_version_num() != 7
        {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let created_at = assertion
            .created_at
            .replace_nanosecond((assertion.created_at.nanosecond() / 1_000) * 1_000)
            .map_err(|error| PolicyStoreError::CorruptValue(error.to_string()))?;
        sqlx::query(
            r#"INSERT INTO audio_identity_catalog_assertions
                (id, audio_identity_id, outcome, actor, reason, evidence, created_at)
               VALUES ($1, $2, 'no_catalog_match', $3, $4, $5, $6)
               ON CONFLICT (audio_identity_id) DO NOTHING"#,
        )
        .bind(assertion.id)
        .bind(assertion.audio_identity_id.into_uuid())
        .bind(&assertion.actor)
        .bind(&assertion.reason)
        .bind(&assertion.evidence)
        .bind(created_at)
        .execute(pool)
        .await?;
        let stored: Option<(Uuid, String, String, serde_json::Value, OffsetDateTime)> =
            sqlx::query_as(
                r#"SELECT id, actor, reason, evidence, created_at
                     FROM audio_identity_catalog_assertions
                    WHERE audio_identity_id = $1"#,
            )
            .bind(assertion.audio_identity_id.into_uuid())
            .fetch_optional(pool)
            .await?;
        if stored
            != Some((
                assertion.id,
                assertion.actor.clone(),
                assertion.reason.clone(),
                assertion.evidence.clone(),
                created_at,
            ))
        {
            return Err(PolicyStoreError::DecisionConflict);
        }
        Ok(())
    }

    /// Inserts an immutable distinct-recording assertion backed by the exact
    /// supporting manual positive evidence tuple, or adopts an exact retry.
    pub async fn record_audio_identity_alternate(
        &self,
        pool: &PgPool,
        assertion: &PolicyAudioIdentityAlternateAssertion,
    ) -> Result<(), PolicyStoreError> {
        if assertion.source_audio_identity_id == assertion.alternate_audio_identity_id
            || assertion.actor.trim().is_empty()
            || assertion.reason.trim().is_empty()
        {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let created_at = assertion
            .created_at
            .replace_nanosecond((assertion.created_at.nanosecond() / 1_000) * 1_000)
            .map_err(|error| PolicyStoreError::CorruptValue(error.to_string()))?;
        let mut tx = pool.begin().await?;
        let evidence: Option<(Uuid,)> = sqlx::query_as(
            r#"SELECT id FROM audio_identity_evidence
                WHERE id = $1 AND source_audio_identity_id = $2
                  AND candidate_audio_identity_id = $3
                  AND polarity = 'supports' AND evidence_kind = 'manual'
                FOR SHARE"#,
        )
        .bind(assertion.evidence_id)
        .bind(assertion.source_audio_identity_id.into_uuid())
        .bind(assertion.alternate_audio_identity_id.into_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if evidence.is_none() {
            return Err(PolicyStoreError::IncompleteFacts);
        }
        sqlx::query(
            r#"INSERT INTO audio_identity_alternate_recordings
                (source_audio_identity_id, alternate_audio_identity_id,
                 evidence_id, actor, reason, created_at)
               VALUES ($1, $2, $3, $4, $5, $6)
               ON CONFLICT (source_audio_identity_id, alternate_audio_identity_id)
               DO NOTHING"#,
        )
        .bind(assertion.source_audio_identity_id.into_uuid())
        .bind(assertion.alternate_audio_identity_id.into_uuid())
        .bind(assertion.evidence_id)
        .bind(&assertion.actor)
        .bind(&assertion.reason)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        let stored: Option<(Uuid, String, String, OffsetDateTime)> = sqlx::query_as(
            r#"SELECT evidence_id, actor, reason, created_at
                 FROM audio_identity_alternate_recordings
                WHERE source_audio_identity_id = $1
                  AND alternate_audio_identity_id = $2"#,
        )
        .bind(assertion.source_audio_identity_id.into_uuid())
        .bind(assertion.alternate_audio_identity_id.into_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if stored
            != Some((
                assertion.evidence_id,
                assertion.actor.clone(),
                assertion.reason.clone(),
                created_at,
            ))
        {
            return Err(PolicyStoreError::DecisionConflict);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Inserts independently derived catalog classification evidence or
    /// adopts an exact retry. A different assertion for the same
    /// `(Release, trait)` is an immutable-fact conflict.
    pub async fn record_release_catalog_trait(
        &self,
        pool: &PgPool,
        assertion: &PolicyReleaseCatalogTraitAssertion,
    ) -> Result<(), PolicyStoreError> {
        if assertion.id.get_version_num() != 7
            || !assertion.release_id.is_uuid_v7()
            || assertion.source_version.trim().is_empty()
            || assertion.actor.trim().is_empty()
            || assertion.reason.trim().is_empty()
            || assertion.evidence.as_object().is_none()
        {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let created_at = assertion
            .created_at
            .replace_nanosecond((assertion.created_at.nanosecond() / 1_000) * 1_000)
            .map_err(|error| PolicyStoreError::CorruptValue(error.to_string()))?;
        sqlx::query(
            r#"INSERT INTO release_catalog_trait_assertions
                (id, release_id, trait, source_version, actor, reason, evidence, created_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
               ON CONFLICT (release_id, trait) DO NOTHING"#,
        )
        .bind(assertion.id)
        .bind(assertion.release_id.into_uuid())
        .bind(assertion.trait_.as_str())
        .bind(&assertion.source_version)
        .bind(&assertion.actor)
        .bind(&assertion.reason)
        .bind(&assertion.evidence)
        .bind(created_at)
        .execute(pool)
        .await?;
        let stored: Option<(
            Uuid,
            String,
            String,
            String,
            serde_json::Value,
            OffsetDateTime,
        )> = sqlx::query_as(
            r#"SELECT id, source_version, actor, reason, evidence, created_at
                     FROM release_catalog_trait_assertions
                    WHERE release_id = $1 AND trait = $2"#,
        )
        .bind(assertion.release_id.into_uuid())
        .bind(assertion.trait_.as_str())
        .fetch_optional(pool)
        .await?;
        if stored
            != Some((
                assertion.id,
                assertion.source_version.clone(),
                assertion.actor.clone(),
                assertion.reason.clone(),
                assertion.evidence.clone(),
                created_at,
            ))
        {
            return Err(PolicyStoreError::DecisionConflict);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Cataloged policy work
    // ------------------------------------------------------------------

    /// Claims a bounded batch of Cataloged work, including direct reclaim of
    /// expired leases. Cataloged remains the durable state while the
    /// policy worker owns the lease (ADR-0120).
    #[instrument(skip_all, fields(worker_id = %worker, batch = config.claim_batch_size))]
    pub async fn claim_cataloged(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<PolicyClaim>, PolicyStoreError> {
        let mut tx = pool.begin().await?;
        let selected = crate::processing::claim_candidates(&mut tx, "policy", config, now).await?;
        if selected.is_empty() {
            tx.commit().await?;
            return Ok(Vec::new());
        }
        let work_item_ids: Vec<_> = selected.iter().map(|row| row.id).collect();
        let candidates: Vec<(Uuid, Uuid, String, String, String)> = sqlx::query_as(
            "SELECT ci.work_item_id,ci.release_id,r.title,ci.sealed_manifest_hash,ci.semantic_manifest_hash
               FROM catalog_imports ci JOIN releases r ON r.id=ci.release_id
              WHERE ci.work_item_id=ANY($1)")
            .bind(&work_item_ids).fetch_all(&mut *tx).await?;
        let lease_expires_at = now + config.lease_duration;
        let rows: Vec<WorkItemRow> = sqlx::query_as(
            r#"UPDATE release_work_items
                  SET lease_owner = $1, lease_acquired_at = $2,
                      lease_expires_at = $3, next_attempt_at = NULL,
                      last_attempt_at = $2, updated_at = $2
                WHERE id = ANY($4) AND state = 'cataloged'
                RETURNING *"#,
        )
        .bind(worker.as_str())
        .bind(now)
        .bind(lease_expires_at)
        .bind(&work_item_ids)
        .fetch_all(&mut *tx)
        .await?;
        let items: Vec<ReleaseWorkItem> = rows
            .into_iter()
            .map(ReleaseWorkItem::try_from)
            .collect::<Result<_, _>>()?;
        let event_ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();
        let correlations: Vec<Uuid> = items.iter().map(|item| item.correlation_id).collect();
        sqlx::query(
            r#"INSERT INTO release_work_item_events
                (work_item_id, event_type, reason_code, worker_id,
                 correlation_id, created_at, metadata)
               SELECT work_item_id, 'claimed', 'policy.claimed', $1,
                      correlation_id, $2,
                      jsonb_build_object('lease_expires_at', $3::text)
                 FROM unnest($4::uuid[], $5::uuid[])
                      AS claimed(work_item_id, correlation_id)"#,
        )
        .bind(worker.as_str())
        .bind(now)
        .bind(lease_expires_at.to_string())
        .bind(&event_ids)
        .bind(&correlations)
        .execute(&mut *tx)
        .await?;

        let metadata: std::collections::BTreeMap<Uuid, (Uuid, String, String, String)> = candidates
            .into_iter()
            .map(|(work_item_id, release_id, title, sealed, semantic)| {
                (work_item_id, (release_id, title, sealed, semantic))
            })
            .collect();
        let mut claims = Vec::with_capacity(items.len());
        for item in items {
            let (release_id, title, sealed, semantic) = metadata
                .get(&item.id)
                .cloned()
                .ok_or(PolicyStoreError::ForeignFacts)?;
            claims.push(PolicyClaim {
                item,
                release_id: ReleaseId::from_uuid(release_id),
                title,
                source_fingerprint: sealed,
                semantic_manifest_hash: semantic,
            });
        }
        claims.sort_by_key(|claim| claim.item.id);
        tx.commit().await?;
        Ok(claims)
    }

    /// Renews one active policy lease.
    pub async fn renew_lease(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        worker: &WorkerId,
        lease_duration: time::Duration,
        now: OffsetDateTime,
    ) -> Result<OffsetDateTime, PolicyStoreError> {
        Ok(crate::WorkItemStore
            .renew_lease(pool, work_item_id, worker, lease_duration, now)
            .await?)
    }

    /// Releases one active policy lease without changing Cataloged state.
    pub async fn release_lease(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<(), PolicyStoreError> {
        Ok(crate::WorkItemStore
            .release_lease(pool, work_item_id, worker, now)
            .await?)
    }

    /// Clears expired Cataloged leases in a bounded maintenance pass. Normal
    /// claims can also directly reclaim these rows.
    pub async fn recover_expired_claims(
        &self,
        pool: &PgPool,
        limit: u32,
        now: OffsetDateTime,
    ) -> Result<Vec<Uuid>, PolicyStoreError> {
        let mut tx = pool.begin().await?;
        let rows: Vec<(Uuid, Uuid, Option<String>)> = sqlx::query_as(
            r#"WITH candidates AS (
                   SELECT id, lease_owner FROM release_work_items
                    WHERE state = 'cataloged' AND lease_expires_at <= $1
                    ORDER BY lease_expires_at, id
                    LIMIT $2 FOR UPDATE SKIP LOCKED
               ), recovered AS (
                 UPDATE release_work_items w
                  SET lease_owner = NULL, lease_acquired_at = NULL,
                      lease_expires_at = NULL, updated_at = $1
                 FROM candidates c WHERE w.id = c.id
               RETURNING w.id, w.correlation_id
               )
               SELECT recovered.id, recovered.correlation_id, candidates.lease_owner
                 FROM recovered JOIN candidates ON candidates.id = recovered.id"#,
        )
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        if !rows.is_empty() {
            let ids: Vec<Uuid> = rows.iter().map(|row| row.0).collect();
            let correlations: Vec<Uuid> = rows.iter().map(|row| row.1).collect();
            let owners: Vec<Option<String>> = rows.iter().map(|row| row.2.clone()).collect();
            sqlx::query(
                r#"INSERT INTO release_work_item_events
                    (work_item_id, event_type, reason_code, worker_id,
                     correlation_id, created_at, metadata)
                   SELECT work_item_id, 'lease_expired_recovered',
                          'policy.lease_expired', owner, correlation_id, $1,
                          jsonb_build_object('resume_state', 'cataloged')
                     FROM unnest($2::uuid[], $3::uuid[], $4::text[])
                          AS recovered(work_item_id, correlation_id, owner)"#,
            )
            .bind(now)
            .bind(&ids)
            .bind(&correlations)
            .bind(&owners)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }

    // ------------------------------------------------------------------
    // Classification storage
    // ------------------------------------------------------------------

    async fn store_classification_for_rule_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        classification: &ReleaseClassification,
        rule: (melodee_domain::PolicyRuleSetId, u32),
    ) -> Result<(), PolicyStoreError> {
        let classification = ReleaseClassification::new(classification.clone())?;
        if classification.evidence.len() > MAX_POLICY_CLASSIFICATION_EVIDENCE {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "classification evidence",
                limit: MAX_POLICY_CLASSIFICATION_EVIDENCE,
            });
        }
        let (rule_set_id, version) = rule;
        let rule_set_version =
            i32::try_from(version).map_err(|_| PolicyStoreError::NumericOverflow)?;
        sqlx::query(
            r#"INSERT INTO release_classifications
                (id, release_id, confidence, classifier_version, classified_at, created_at,
                 rule_set_id, rule_set_version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        )
        .bind(classification.id.into_uuid())
        .bind(classification.release_id.into_uuid())
        .bind(i64::from(classification.confidence.basis_points()))
        .bind(&classification.classifier_version)
        .bind(classification.classified_at)
        .bind(classification.classified_at)
        .bind(rule_set_id.into_uuid())
        .bind(rule_set_version)
        .execute(&mut **tx)
        .await?;

        let traits: Vec<String> = classification
            .traits
            .iter()
            .map(|value| value.as_str().to_owned())
            .collect();
        sqlx::query(
            r#"INSERT INTO release_traits (classification_id, trait, created_at)
               SELECT $1, trait, $2
                 FROM unnest($3::text[]) AS trait"#,
        )
        .bind(classification.id.into_uuid())
        .bind(classification.classified_at)
        .bind(&traits)
        .execute(&mut **tx)
        .await?;

        let evidence_traits: Vec<String> = classification
            .evidence
            .iter()
            .map(|evidence| evidence.trait_.as_str().to_owned())
            .collect();
        let evidence_sources: Vec<String> = classification
            .evidence
            .iter()
            .map(|evidence| evidence.source.as_str().to_owned())
            .collect();
        let evidence_kinds: Vec<String> = classification
            .evidence
            .iter()
            .map(|evidence| evidence.kind.as_str().to_owned())
            .collect();
        let evidence_confidences: Vec<i32> = classification
            .evidence
            .iter()
            .map(|evidence| i32::from(evidence.confidence.basis_points()))
            .collect();
        let evidence_details: Vec<String> = classification
            .evidence
            .iter()
            .map(|evidence| evidence.detail.clone())
            .collect();
        sqlx::query(
            r#"INSERT INTO release_classification_evidence
                (id, trait, source, kind, confidence, detail, created_at)
               SELECT $1, trait, source, kind, confidence, detail, $2
                 FROM unnest($3::text[], $4::text[], $5::text[], $6::integer[], $7::text[])
                      AS evidence(trait, source, kind, confidence, detail)"#,
        )
        .bind(classification.id.into_uuid())
        .bind(classification.classified_at)
        .bind(&evidence_traits)
        .bind(&evidence_sources)
        .bind(&evidence_kinds)
        .bind(&evidence_confidences)
        .bind(&evidence_details)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Novelty storage
    // ------------------------------------------------------------------

    /// Persists per-Track novelty assessments within an existing transaction.
    #[instrument(skip_all, fields(release_id = %evaluation.release_id))]
    async fn store_novelty_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        evaluation: &NoveltyEvaluation,
        track_novelty: &[TrackNovelty],
    ) -> Result<Uuid, PolicyStoreError> {
        let evaluation = NoveltyEvaluation::new(evaluation.clone())?;
        if track_novelty.len() > MAX_POLICY_TRACKS_PER_RELEASE {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "Tracks per Release",
                limit: MAX_POLICY_TRACKS_PER_RELEASE,
            });
        }
        if usize::try_from(evaluation.total_track_count)
            .map_err(|_| PolicyStoreError::NumericOverflow)?
            != track_novelty.len()
        {
            return Err(PolicyStoreError::IncompleteFacts);
        }
        for novelty in track_novelty {
            TrackNovelty::new(novelty.clone())?;
            if novelty.release_id != evaluation.release_id || novelty.media_asset_id.is_none() {
                return Err(PolicyStoreError::ForeignFacts);
            }
        }

        let evaluation_id = Uuid::now_v7();
        let ids: Vec<Uuid> = track_novelty
            .iter()
            .map(|value| value.id.into_uuid())
            .collect();
        let release_ids: Vec<Uuid> = track_novelty
            .iter()
            .map(|value| value.release_id.into_uuid())
            .collect();
        let track_ids: Vec<Uuid> = track_novelty
            .iter()
            .map(|value| value.track_id.into_uuid())
            .collect();
        let audio_ids: Vec<Uuid> = track_novelty
            .iter()
            .map(|value| value.audio_identity_id.into_uuid())
            .collect();
        let media_ids: Vec<Uuid> = track_novelty
            .iter()
            .filter_map(|value| value.media_asset_id.map(|id| id.into_uuid()))
            .collect();
        let categories: Vec<String> = track_novelty
            .iter()
            .map(|value| value.category.as_str().to_owned())
            .collect();
        let confidences: Vec<i32> = track_novelty
            .iter()
            .map(|value| i32::from(value.confidence.basis_points()))
            .collect();
        let evidence_kinds: Vec<String> = track_novelty
            .iter()
            .map(|value| value.evidence_kind.as_str().to_owned())
            .collect();
        let details: Vec<String> = track_novelty
            .iter()
            .map(|value| value.detail.clone())
            .collect();
        let evaluated_at: Vec<OffsetDateTime> = track_novelty
            .iter()
            .map(|value| value.evaluated_at)
            .collect();

        sqlx::query(
            r#"INSERT INTO track_novelty
                (id, evaluation_id, release_id, track_id, audio_identity_id,
                 media_asset_id, category, confidence, evidence_kind, detail,
                 evaluated_at, created_at)
               SELECT id, $1, release_id, track_id, audio_identity_id,
                      media_asset_id, category, confidence, evidence_kind,
                      detail, evaluated_at, evaluated_at
                 FROM unnest(
                    $2::uuid[], $3::uuid[], $4::uuid[], $5::uuid[], $6::uuid[],
                    $7::text[], $8::integer[], $9::text[], $10::text[], $11::timestamptz[]
                 ) AS facts(id, release_id, track_id, audio_identity_id,
                            media_asset_id, category, confidence, evidence_kind,
                            detail, evaluated_at)"#,
        )
        .bind(evaluation_id)
        .bind(&ids)
        .bind(&release_ids)
        .bind(&track_ids)
        .bind(&audio_ids)
        .bind(&media_ids)
        .bind(&categories)
        .bind(&confidences)
        .bind(&evidence_kinds)
        .bind(&details)
        .bind(&evaluated_at)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            r#"INSERT INTO release_novelty_evaluations
                (id, release_id, policy_rule_set_id, exact_duplicate_count, known_audio_identity_count,
                 alternate_recording_count, new_track_count, uncertain_count,
                 total_track_count, evaluated_at, evaluator_version, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        )
        .bind(evaluation_id)
        .bind(evaluation.release_id.into_uuid())
        .bind(evaluation.policy_rule_set_id.into_uuid())
        .bind(
            i32::try_from(evaluation.exact_duplicate_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(
            i32::try_from(evaluation.known_audio_identity_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(
            i32::try_from(evaluation.alternate_recording_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(
            i32::try_from(evaluation.new_track_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(
            i32::try_from(evaluation.uncertain_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(
            i32::try_from(evaluation.total_track_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(evaluation.evaluated_at)
        .bind(&evaluation.evaluator_version)
        .bind(evaluation.evaluated_at)
        .execute(&mut **tx)
        .await?;
        Ok(evaluation_id)
    }

    // ------------------------------------------------------------------
    // Policy decisions (durable, before destructive action)
    // ------------------------------------------------------------------

    /// Atomically persists validated policy facts and the durable decision,
    /// then performs the lease-fenced state transition. An existing decision
    /// for the same work item and source fingerprint is authoritative and is
    /// adopted after crash recovery instead of being replaced by caller IDs.
    #[instrument(skip_all, fields(work_item_id = %request.work_item_id, worker_id = %worker))]
    pub async fn commit_evaluation(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        request: &PolicyEvaluationCommit,
    ) -> Result<PolicyCommitResult, PolicyStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, request.work_item_id).await?;
        if item.state != IngestionState::Cataloged {
            return Err(PolicyStoreError::NotCataloged(request.work_item_id));
        }
        ensure_active_lease(&item, worker, request.now)?;

        let binding: Option<(Uuid, String, bool, serde_json::Value)> = sqlx::query_as(
            r#"SELECT import.release_id, import.sealed_manifest_hash,
                      jsonb_path_exists(
                          manifest.manifest_payload,
                          '$.files[*] ? (@.bytes_read_errors == true)'
                      ), manifest.manifest_payload
                 FROM catalog_imports import
                 JOIN release_work_item_manifests manifest
                   ON manifest.id = import.manifest_row_id
                WHERE import.work_item_id = $1"#,
        )
        .bind(request.work_item_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (release_id, fingerprint, has_corrupt_media, manifest_payload) =
            binding.ok_or(PolicyStoreError::NotCataloged(request.work_item_id))?;
        if release_id != request.release_id.into_uuid()
            || fingerprint != request.source_fingerprint
            || request.decision.source_fingerprint != request.source_fingerprint
        {
            return Err(PolicyStoreError::FingerprintMismatch);
        }

        let decision = ReleasePolicyDecision::new(request.decision.clone())?;
        let classification = ReleaseClassification::new(request.classification.clone())?;
        let novelty = NoveltyEvaluation::new(request.novelty_evaluation.clone())?;
        if classification.release_id != request.release_id
            || novelty.release_id != request.release_id
            || decision.release_id != request.release_id
            || novelty.policy_rule_set_id != decision.rule_set_id
        {
            return Err(PolicyStoreError::ForeignFacts);
        }
        let catalog_trait_rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT trait FROM release_catalog_trait_assertions
                WHERE release_id = $1 ORDER BY trait LIMIT $2"#,
        )
        .bind(request.release_id.into_uuid())
        .bind(
            i64::try_from(melodee_domain::MAX_RELEASE_CLASSIFICATION_TRAITS + 1)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .fetch_all(&mut *tx)
        .await?;
        if catalog_trait_rows.len() > melodee_domain::MAX_RELEASE_CLASSIFICATION_TRAITS {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "catalog Release trait assertions",
                limit: melodee_domain::MAX_RELEASE_CLASSIFICATION_TRAITS,
            });
        }
        let catalog_traits = catalog_trait_rows
            .into_iter()
            .map(|(value,)| {
                melodee_domain::ReleaseTrait::parse(&value)
                    .ok_or(PolicyStoreError::CorruptValue(value))
            })
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        validate_manifest_classification_evidence(
            &classification,
            &manifest_payload,
            &catalog_traits,
        )?;
        if novelty.evaluator_version != SUPPORTED_NOVELTY_EVALUATOR_VERSION {
            return Err(PolicyStoreError::ForeignFacts);
        }
        if !matches!(decision.actor, PolicyActor::System { .. }) {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let expected_rule_set: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT rules FROM policy_rule_sets WHERE id = $1 AND version = $2")
                .bind(decision.rule_set_id.into_uuid())
                .bind(
                    i32::try_from(decision.rule_set_version)
                        .map_err(|_| PolicyStoreError::NumericOverflow)?,
                )
                .fetch_optional(&mut *tx)
                .await?;
        let Some((rules_json,)) = expected_rule_set else {
            return Err(PolicyStoreError::RuleSetNotFound(format!(
                "{}@{}",
                decision.rule_set_id.into_uuid(),
                decision.rule_set_version
            )));
        };
        let rules: Vec<melodee_domain::PolicyRule> = serde_json::from_value(rules_json)?;
        let existing = load_decision_tx(&mut tx, request.release_id).await?;
        if existing.is_none() {
            validate_decision_rule(
                &decision,
                &rules,
                &classification,
                &novelty,
                None,
                has_corrupt_media,
            )?;
        }

        self.validate_track_facts_tx(&mut tx, request.release_id, &request.track_novelty)
            .await?;

        let (stored, inserted) = if let Some(existing) = existing {
            if existing.work_item_id != request.work_item_id
                || existing.source_fingerprint != request.source_fingerprint
                || existing.rule_set_id != decision.rule_set_id
                || existing.rule_set_version != decision.rule_set_version
            {
                return Err(PolicyStoreError::DecisionConflict);
            }
            if !decision_semantically_matches(&existing, &decision) {
                return Err(PolicyStoreError::DecisionConflict);
            }
            validate_existing_evaluation_tx(
                &mut tx,
                &classification,
                &novelty,
                &request.track_novelty,
                decision.rule_set_id,
                decision.rule_set_version,
            )
            .await?;
            (existing, false)
        } else {
            self.store_classification_for_rule_tx(
                &mut tx,
                &classification,
                (decision.rule_set_id, decision.rule_set_version),
            )
            .await?;
            let novelty_evaluation_id = self
                .store_novelty_tx(&mut tx, &novelty, &request.track_novelty)
                .await?;
            let durable_decision = completed_non_destructive_decision(decision, request.now)?;
            insert_decision_tx(
                &mut tx,
                request.work_item_id,
                classification.id.into_uuid(),
                novelty_evaluation_id,
                &durable_decision,
            )
            .await?;
            (
                stored_decision(
                    &durable_decision,
                    request.work_item_id,
                    classification.id.into_uuid(),
                    novelty_evaluation_id,
                    None,
                ),
                true,
            )
        };

        let (to, clear_lease) = policy_target(stored.disposition);
        set_release_client_status_tx(&mut tx, stored.release_id, stored.disposition, request.now)
            .await?;
        item.state
            .ensure_transition(to, TransitionKind::Worker)
            .map_err(WorkStoreError::from)?;
        let completed_at = if to.is_terminal() {
            Some(request.now)
        } else {
            None
        };
        let row: Option<WorkItemRow> = sqlx::query_as(
            r#"UPDATE release_work_items
                  SET state = $2, state_reason = $3, updated_at = $4,
                      completed_at = $5,
                      lease_owner = CASE WHEN $6 THEN NULL ELSE lease_owner END,
                      lease_acquired_at = CASE WHEN $6 THEN NULL ELSE lease_acquired_at END,
                      lease_expires_at = CASE WHEN $6 THEN NULL ELSE lease_expires_at END
                WHERE id = $1 AND state = 'cataloged' AND lease_owner = $7
                  AND lease_expires_at > clock_timestamp()
                RETURNING *"#,
        )
        .bind(request.work_item_id)
        .bind(to.as_str())
        .bind(format!(
            "policy.disposition.{}",
            stored.disposition.as_str()
        ))
        .bind(request.now)
        .bind(completed_at)
        .bind(clear_lease)
        .bind(worker.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let row = row.ok_or(WorkStoreError::LeaseExpired(request.work_item_id))?;
        insert_event(
            &mut tx,
            request.work_item_id,
            WorkEventType::Transitioned,
            Some(IngestionState::Cataloged),
            Some(to),
            Some("policy.decision_applied"),
            Some(worker.as_str()),
            item.correlation_id,
            request.now,
            Some(serde_json::json!({
                "decision_id": stored.id.into_uuid(),
                "disposition": stored.disposition.as_str(),
                "source_fingerprint": stored.source_fingerprint,
            })),
        )
        .await?;
        let item = crate::review_correction::apply_accepted_identification(
            &mut tx,
            ReleaseWorkItem::try_from(row)?,
            request.now,
        )
        .await?;
        tx.commit().await?;
        Ok(PolicyCommitResult {
            item,
            decision: stored,
            inserted,
            set_based_mutation_statement_count: if inserted { 8 } else { 3 },
        })
    }

    async fn validate_track_facts_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        release_id: ReleaseId,
        facts: &[TrackNovelty],
    ) -> Result<(), PolicyStoreError> {
        if facts.len() > MAX_POLICY_TRACKS_PER_RELEASE {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "Tracks per Release",
                limit: MAX_POLICY_TRACKS_PER_RELEASE,
            });
        }
        let rows: Vec<NoveltyRevalidationRow> = sqlx::query_as(
            r#"SELECT t.id AS track_id,
                      t.audio_identity_id,
                      t.playable_media_asset_id AS media_asset_id,
                      EXISTS (
                          SELECT 1 FROM tracks other
                          JOIN media_assets other_media
                            ON other_media.id = other.playable_media_asset_id
                          JOIN media_assets own_media
                            ON own_media.id = t.playable_media_asset_id
                          WHERE other.release_id <> t.release_id
                            AND other_media.byte_hash = own_media.byte_hash
                      ) AS exact_duplicate,
                      EXISTS (
                          SELECT 1
                            FROM audio_identity_evidence own_evidence
                            JOIN audio_identity_evidence other_evidence
                              ON other_evidence.evidence_kind = 'acoustic_fingerprint'
                             AND (other_evidence.payload->>'fingerprint_digest')
                                 = (own_evidence.payload->>'fingerprint_digest')
                            JOIN tracks other_track
                              ON other_track.id = other_evidence.track_id
                             AND other_track.release_id <> t.release_id
                           WHERE own_evidence.track_id = t.id
                             AND own_evidence.evidence_kind = 'acoustic_fingerprint'
                      ) AS acoustic_match,
                      EXISTS (
                          SELECT 1 FROM tracks other
                           WHERE other.release_id <> t.release_id
                             AND other.audio_identity_id = t.audio_identity_id
                      ) OR EXISTS (
                          SELECT 1
                            FROM audio_identity_match_current current_match
                            JOIN audio_identity_match_decisions match_decision
                              ON match_decision.id = current_match.decision_id
                             AND match_decision.outcome = 'confirmed'
                            JOIN tracks other
                              ON other.audio_identity_id = current_match.candidate_audio_identity_id
                             AND other.release_id <> t.release_id
                           WHERE current_match.source_audio_identity_id = t.audio_identity_id
                      ) AS known_identity,
                      EXISTS (
                          SELECT 1 FROM audio_identity_alternate_recordings alternate
                          JOIN tracks other
                            ON other.audio_identity_id = CASE
                                   WHEN alternate.source_audio_identity_id = t.audio_identity_id
                                   THEN alternate.alternate_audio_identity_id
                                   ELSE alternate.source_audio_identity_id
                               END
                           AND other.release_id <> t.release_id
                          WHERE t.audio_identity_id IN (
                              alternate.source_audio_identity_id,
                              alternate.alternate_audio_identity_id
                          )
                      ) AS alternate_recording,
                      (EXISTS (
                          SELECT 1 FROM audio_identity_catalog_assertions assertion
                           WHERE assertion.audio_identity_id = t.audio_identity_id
                             AND assertion.outcome = 'no_catalog_match'
                      ) OR EXISTS (
                          SELECT 1 FROM audio_identity_alternate_recordings alternate
                          JOIN tracks other
                            ON other.audio_identity_id = CASE
                                   WHEN alternate.source_audio_identity_id = t.audio_identity_id
                                   THEN alternate.alternate_audio_identity_id
                                   ELSE alternate.source_audio_identity_id
                               END
                           AND other.release_id <> t.release_id
                          WHERE t.audio_identity_id IN (
                              alternate.source_audio_identity_id,
                              alternate.alternate_audio_identity_id
                          )
                      )) AS identity_confirmed
                 FROM tracks t WHERE t.release_id = $1 ORDER BY t.id LIMIT $2"#,
        )
        .bind(release_id.into_uuid())
        .bind(
            i64::try_from(MAX_POLICY_TRACKS_PER_RELEASE + 1)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .fetch_all(&mut **tx)
        .await?;
        if rows.len() > MAX_POLICY_TRACKS_PER_RELEASE {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "Tracks per Release",
                limit: MAX_POLICY_TRACKS_PER_RELEASE,
            });
        }
        if rows.len() != facts.len() {
            return Err(PolicyStoreError::IncompleteFacts);
        }
        let expected: std::collections::BTreeMap<
            Uuid,
            (
                Uuid,
                Uuid,
                melodee_domain::TrackNoveltyCategory,
                melodee_domain::NoveltyEvidenceKind,
                u16,
                &'static str,
            ),
        > = rows
            .into_iter()
            .map(
                |NoveltyRevalidationRow {
                     track_id: track,
                     audio_identity_id: audio,
                     media_asset_id: media,
                     exact_duplicate: exact,
                     acoustic_match: acoustic,
                     known_identity: known,
                     alternate_recording: alternate,
                     identity_confirmed: confirmed,
                 }| {
                    let (category, evidence_kind, confidence, detail) = if exact {
                        (
                            melodee_domain::TrackNoveltyCategory::ExactDuplicate,
                            melodee_domain::NoveltyEvidenceKind::ByteHashMatch,
                            10_000,
                            "Canonical byte hash exists in another Release",
                        )
                    } else if acoustic {
                        (
                            melodee_domain::TrackNoveltyCategory::KnownAudioIdentity,
                            melodee_domain::NoveltyEvidenceKind::AudioIdentityMatch,
                            9_000,
                            "Canonical acoustic fingerprint exists in another Release",
                        )
                    } else if known {
                        (
                            melodee_domain::TrackNoveltyCategory::KnownAudioIdentity,
                            melodee_domain::NoveltyEvidenceKind::AudioIdentityMatch,
                            9_000,
                            "AudioIdentity is durably referenced by another Release",
                        )
                    } else if alternate {
                        (
                            melodee_domain::TrackNoveltyCategory::AlternateRecording,
                            melodee_domain::NoveltyEvidenceKind::AudioIdentityDistinct,
                            8_500,
                            "Catalog evidence confirms a distinct alternate recording",
                        )
                    } else if confirmed {
                        (
                            melodee_domain::TrackNoveltyCategory::NewTrack,
                            melodee_domain::NoveltyEvidenceKind::NoCatalogMatch,
                            8_500,
                            "Confirmed AudioIdentity has no catalog match",
                        )
                    } else {
                        (
                            melodee_domain::TrackNoveltyCategory::Uncertain,
                            melodee_domain::NoveltyEvidenceKind::InsufficientEvidence,
                            0,
                            "No catalog comparison conclusion for this AudioIdentity",
                        )
                    };
                    (
                        track,
                        (audio, media, category, evidence_kind, confidence, detail),
                    )
                },
            )
            .collect();
        for fact in facts {
            let media = fact
                .media_asset_id
                .ok_or(PolicyStoreError::IncompleteFacts)?;
            let actual = expected.get(&fact.track_id.into_uuid());
            if fact.release_id != release_id
                || actual.map(|value| (value.0, value.1))
                    != Some((fact.audio_identity_id.into_uuid(), media.into_uuid()))
                || actual.map(|value| value.2) != Some(fact.category)
                || actual.map(|value| value.3) != Some(fact.evidence_kind)
                || actual.map(|value| value.4) != Some(fact.confidence.basis_points())
                || actual.map(|value| value.5) != Some(fact.detail.as_str())
            {
                return Err(PolicyStoreError::ForeignFacts);
            }
        }
        Ok(())
    }

    /// Records a policy-stage failure without sending retryable work back
    /// through inspection/catalog import. Retryable work remains Cataloged,
    /// unleased, and directly reclaimable after its backoff.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_failure(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        worker: &WorkerId,
        code: WorkErrorCode,
        message: &str,
        class: FailureClass,
        retry: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, PolicyStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, work_item_id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Cataloged {
            return Err(PolicyStoreError::NotCataloged(work_item_id));
        }
        let attempt_count = item.attempt_count.saturating_add(1);
        let (to, next_attempt_at) = match class {
            FailureClass::Retryable if retry.can_retry(attempt_count) => (
                IngestionState::Cataloged,
                retry.next_attempt_at(attempt_count, now),
            ),
            FailureClass::NeedsReview => (IngestionState::NeedsReview, None),
            FailureClass::Retryable | FailureClass::Permanent => (IngestionState::Failed, None),
        };
        if to != IngestionState::Cataloged {
            item.state
                .ensure_transition(to, TransitionKind::Worker)
                .map_err(WorkStoreError::from)?;
        }
        let row: WorkItemRow = sqlx::query_as(
            r#"UPDATE release_work_items
                  SET state = $2, state_reason = $3, attempt_count = $4,
                      last_attempt_at = $5, last_error_code = $6,
                      last_error_message = $7, next_attempt_at = $8,
                      lease_owner = NULL, lease_acquired_at = NULL,
                      lease_expires_at = NULL, updated_at = $5
                WHERE id = $1 AND state = 'cataloged' AND lease_owner = $9
                  AND lease_expires_at > clock_timestamp()
                RETURNING *"#,
        )
        .bind(work_item_id)
        .bind(to.as_str())
        .bind("policy.evaluation_failed")
        .bind(i32::try_from(attempt_count).map_err(|_| PolicyStoreError::NumericOverflow)?)
        .bind(now)
        .bind(code.as_str())
        .bind(message)
        .bind(next_attempt_at)
        .bind(worker.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(WorkStoreError::LeaseExpired(work_item_id))?;
        insert_event(
            &mut tx,
            work_item_id,
            WorkEventType::Failed,
            Some(IngestionState::Cataloged),
            Some(to),
            Some(code.as_str()),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "failure_class": class,
                "attempt_count": attempt_count,
                "policy_retry": next_attempt_at.is_some(),
            })),
        )
        .await?;
        if let Some(next) = next_attempt_at {
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
                Some(serde_json::json!({ "next_attempt_at": next.to_string() })),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(ReleaseWorkItem::try_from(row)?)
    }

    /// Applies the exact administrator-selected disposition and routes the
    /// parked work item in the same transaction as the audited update.
    pub async fn apply_admin_override(
        &self,
        pool: &PgPool,
        request: &PolicyAdminOverrideRequest,
    ) -> Result<PolicyCommitResult, PolicyStoreError> {
        self.apply_review_override(
            pool,
            request,
            PolicyActor::Administrator {
                actor: request.actor.clone(),
            },
        )
        .await
    }

    /// Editors may explicitly delete a reviewed Release.
    pub async fn apply_editor_delete(
        &self,
        pool: &PgPool,
        request: &PolicyAdminOverrideRequest,
    ) -> Result<PolicyCommitResult, PolicyStoreError> {
        if request.chosen_disposition != Disposition::Delete {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        self.apply_review_override(
            pool,
            request,
            PolicyActor::Editor {
                actor: request.actor.clone(),
            },
        )
        .await
    }

    async fn apply_review_override(
        &self,
        pool: &PgPool,
        request: &PolicyAdminOverrideRequest,
        actor: PolicyActor,
    ) -> Result<PolicyCommitResult, PolicyStoreError> {
        if request.actor.trim().is_empty() || request.reason.trim().is_empty() {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, request.work_item_id).await?;
        if item.state != IngestionState::NeedsReview || item.has_active_lease(request.now) {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let mut stored = load_decision_by_id_tx(&mut tx, request.decision_id)
            .await?
            .ok_or(PolicyStoreError::DecisionConflict)?;
        if stored.work_item_id != request.work_item_id
            || stored.source_fingerprint != request.source_fingerprint
            || stored.completion_state != DecisionCompletionState::Pending
        {
            return Err(PolicyStoreError::DecisionConflict);
        }
        verify_current_fingerprint_tx(&mut tx, request.work_item_id, &request.source_fingerprint)
            .await?;
        let completion_state = if matches!(
            request.chosen_disposition,
            Disposition::Review | Disposition::Delete
        ) {
            DecisionCompletionState::Pending
        } else {
            DecisionCompletionState::Completed
        };
        let completed_at =
            (completion_state == DecisionCompletionState::Completed).then_some(request.now);
        // A committed human delete may already have removed part of the storage.
        // Retries keep the original authorizer. A competing disposition cannot reopen it.
        if stored.disposition == Disposition::Delete
            && matches!(
                stored.actor,
                PolicyActor::Administrator { .. } | PolicyActor::Editor { .. }
            )
        {
            if request.chosen_disposition != Disposition::Delete {
                return Err(PolicyStoreError::DecisionConflict);
            }
            tx.commit().await?;
            return Ok(PolicyCommitResult {
                item,
                decision: stored,
                inserted: false,
                set_based_mutation_statement_count: 0,
            });
        }
        let rule_payload: (serde_json::Value,) =
            sqlx::query_as("SELECT rules FROM policy_rule_sets WHERE id = $1 AND version = $2")
                .bind(stored.rule_set_id.into_uuid())
                .bind(
                    i32::try_from(stored.rule_set_version)
                        .map_err(|_| PolicyStoreError::NumericOverflow)?,
                )
                .fetch_one(&mut *tx)
                .await?;
        let rules: Vec<melodee_domain::PolicyRule> = serde_json::from_value(rule_payload.0)?;
        let admin_rule = rules
            .iter()
            .find(|rule| rule.rule_type == melodee_domain::PolicyRuleType::AdministratorOverride)
            .ok_or(PolicyStoreError::AuthorizationRequired)?;
        let matched_rule_ids = vec![admin_rule.id.clone()];
        let evidence_summary = format!(
            "{} '{}' selected {}: {}",
            if actor.is_administrator() {
                "Administrator"
            } else {
                "Editor"
            },
            request.actor,
            request.chosen_disposition.as_str(),
            request.reason
        );
        let changed = sqlx::query(
            r#"UPDATE policy_decisions
                  SET disposition = $2, actor = $3, override_reason = $4,
                      decided_at = $5, completion_state = $6, completed_at = $7,
                      matched_rule_ids = $8, evidence_summary = $9
                WHERE id = $1 AND completion_state = 'pending'"#,
        )
        .bind(request.decision_id.into_uuid())
        .bind(request.chosen_disposition.as_str())
        .bind(serde_json::to_value(&actor)?)
        .bind(&request.reason)
        .bind(request.now)
        .bind(completion_state.as_str())
        .bind(completed_at)
        .bind(sqlx::types::Json(&matched_rule_ids))
        .bind(&evidence_summary)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(PolicyStoreError::DecisionConflict);
        }
        stored.disposition = request.chosen_disposition;
        stored.actor = actor;
        stored.override_reason = Some(request.reason.clone());
        stored.decided_at = request.now;
        stored.completion_state = completion_state;
        stored.completed_at = completed_at;
        stored.matched_rule_ids = matched_rule_ids;
        stored.evidence_summary = evidence_summary;
        set_release_client_status_tx(&mut tx, stored.release_id, stored.disposition, request.now)
            .await?;
        let (to, _) = policy_target(request.chosen_disposition);
        let row = route_operator_item_tx(&mut tx, &item, to, request.now).await?;
        let item = crate::review_correction::apply_accepted_identification(
            &mut tx,
            ReleaseWorkItem::try_from(row)?,
            request.now,
        )
        .await?;
        tx.commit().await?;
        Ok(PolicyCommitResult {
            item,
            decision: stored,
            inserted: false,
            set_based_mutation_statement_count: 3,
        })
    }

    /// Records successful destructive execution, then and only then advances
    /// the administrator-authorized Delete decision to Deleted.
    pub async fn complete_destructive_action(
        &self,
        pool: &PgPool,
        request: &PolicyDestructiveCompletion,
    ) -> Result<PolicyCommitResult, PolicyStoreError> {
        if request.actor.trim().is_empty() || request.reason.trim().is_empty() {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, request.work_item_id).await?;
        if item.state != IngestionState::NeedsReview || item.has_active_lease(request.completed_at)
        {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let mut stored = load_decision_by_id_tx(&mut tx, request.decision_id)
            .await?
            .ok_or(PolicyStoreError::DecisionConflict)?;
        let authorization_matches = matches!(
            &stored.actor,
            PolicyActor::Administrator { actor } | PolicyActor::Editor { actor }
                if actor == &request.actor
                    && stored.override_reason.as_deref() == Some(request.reason.as_str())
        );
        if stored.work_item_id != request.work_item_id
            || stored.source_fingerprint != request.source_fingerprint
            || stored.disposition != Disposition::Delete
            || stored.completion_state != DecisionCompletionState::Pending
            || !authorization_matches
            || request.completed_at < stored.decided_at
        {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        verify_current_fingerprint_tx(&mut tx, request.work_item_id, &request.source_fingerprint)
            .await?;
        if request.canonical_relative_path.trim().is_empty()
            || request
                .staging_relative_path
                .as_deref()
                .is_some_and(|path| path.trim().is_empty())
        {
            return Err(PolicyStoreError::AuthorizationRequired);
        }
        let changed = sqlx::query(
            r#"UPDATE policy_decisions
                  SET completion_state = 'completed', completed_at = $2
                WHERE id = $1 AND disposition = 'delete'
                  AND completion_state = 'pending'"#,
        )
        .bind(request.decision_id.into_uuid())
        .bind(request.completed_at)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(PolicyStoreError::DecisionConflict);
        }
        stored.completion_state = DecisionCompletionState::Completed;
        stored.completed_at = Some(request.completed_at);
        let release_revision = sqlx::query_scalar::<_, i64>(
            "UPDATE releases
                SET status = 'deleted', revision = revision + 1, updated_at = $2
              WHERE id = $1
              RETURNING revision",
        )
        .bind(stored.release_id.into_uuid())
        .bind(request.completed_at)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE tracks
                SET revision = revision + 1, updated_at = $2
              WHERE release_id = $1",
        )
        .bind(stored.release_id.into_uuid())
        .bind(request.completed_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, attempt_count, created_at
             ) VALUES ('tantivy.catalog','release',$1,'delete',$2,$3,0,$3)
             ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
             DO NOTHING",
        )
        .bind(stored.release_id.into_uuid())
        .bind(release_revision)
        .bind(request.completed_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, attempt_count, created_at
             )
             SELECT 'tantivy.catalog','track', track.id, 'delete',
                    track.revision, $2, 0, $2
               FROM tracks track WHERE track.release_id = $1
             ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
             DO NOTHING",
        )
        .bind(stored.release_id.into_uuid())
        .bind(request.completed_at)
        .execute(&mut *tx)
        .await?;
        recompute_artist_aggregates_after_release_deletion(
            &mut tx,
            stored.release_id.into_uuid(),
            request.completed_at,
        )
        .await?;
        sqlx::query(
            "INSERT INTO catalog_audit_events (
                event_id, entity_type, entity_id, event_type, actor, reason,
                payload, created_at
             ) VALUES ($1,'release',$2,'release_deleted',$3,$4,$5,$6)",
        )
        .bind(Uuid::now_v7())
        .bind(stored.release_id.into_uuid())
        .bind(&request.actor)
        .bind(&request.reason)
        .bind(serde_json::json!({
            "work_item_id": request.work_item_id,
            "decision_id": request.decision_id.into_uuid(),
            "source_fingerprint": request.source_fingerprint,
            "canonical_relative_path": request.canonical_relative_path,
            "staging_relative_path": request.staging_relative_path,
            "retained_catalog_tombstone": true
        }))
        .bind(request.completed_at)
        .execute(&mut *tx)
        .await?;
        item.state
            .ensure_transition(IngestionState::Deleted, TransitionKind::Operator)
            .map_err(WorkStoreError::from)?;
        let row: WorkItemRow = sqlx::query_as(
            r#"UPDATE release_work_items
                  SET state = 'deleted', state_reason = $2, completed_at = $3,
                      lease_owner = NULL, lease_acquired_at = NULL,
                      lease_expires_at = NULL, updated_at = $3
                WHERE id = $1 AND state = 'needs_review'
                RETURNING *"#,
        )
        .bind(request.work_item_id)
        .bind(format!("policy.delete_completed:{}", request.reason))
        .bind(request.completed_at)
        .fetch_one(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            request.work_item_id,
            WorkEventType::Completed,
            Some(IngestionState::NeedsReview),
            Some(IngestionState::Deleted),
            Some("policy.delete_completed"),
            Some(&request.actor),
            item.correlation_id,
            request.completed_at,
            Some(serde_json::json!({
                "decision_id": request.decision_id.into_uuid(),
                "release_id": stored.release_id.into_uuid(),
                "canonical_relative_path": request.canonical_relative_path,
                "staging_relative_path": request.staging_relative_path
            })),
        )
        .await?;
        tx.commit().await?;
        Ok(PolicyCommitResult {
            item: ReleaseWorkItem::try_from(row)?,
            decision: stored,
            inserted: false,
            set_based_mutation_statement_count: 9,
        })
    }

    /// Loads an existing policy decision for a Release, if any.
    #[instrument(skip_all)]
    pub async fn get_decision(
        &self,
        pool: &PgPool,
        release_id: melodee_domain::ReleaseId,
    ) -> Result<Option<StoredPolicyDecision>, PolicyStoreError> {
        let row: Option<StoredPolicyDecisionRow> = sqlx::query_as(
            r#"SELECT id, classification_id, novelty_evaluation_id, work_item_id,
                   release_id, rule_set_id, rule_set_version, matched_rule_ids,
                   disposition, evidence_summary, source_fingerprint, actor,
                   decided_at, completion_state, completed_at, override_reason
              FROM policy_decisions
             WHERE release_id = $1"#,
        )
        .bind(release_id.into_uuid())
        .fetch_optional(pool)
        .await?;

        row.map(|r| r.try_into()).transpose()
    }

    /// Returns the current decision and its immutable audit images.
    pub async fn inspect_decision(
        &self,
        pool: &PgPool,
        release_id: ReleaseId,
    ) -> Result<(Option<StoredPolicyDecision>, Vec<PolicyDecisionAuditEntry>), PolicyStoreError>
    {
        let current = self.get_decision(pool, release_id).await?;
        let rows: Vec<PolicyDecisionAuditRow> = sqlx::query_as(
            r#"SELECT id, classification_id, novelty_evaluation_id, work_item_id,
                      release_id, rule_set_id, rule_set_version,
                      matched_rule_ids, disposition, evidence_summary,
                      source_fingerprint, actor, decided_at, completion_state,
                      completed_at, override_reason, event, created_at
                 FROM policy_decision_audit
                WHERE release_id = $1
                ORDER BY created_at, id"#,
        )
        .bind(release_id.into_uuid())
        .fetch_all(pool)
        .await?;
        let history = rows
            .into_iter()
            .map(PolicyDecisionAuditEntry::try_from)
            .collect::<Result<_, _>>()?;
        Ok((current, history))
    }

    /// Lists immutable audit images for CLI/operator inspection.
    pub async fn list_decision_audit(
        &self,
        pool: &PgPool,
        release_id: ReleaseId,
    ) -> Result<Vec<PolicyDecisionAuditEntry>, PolicyStoreError> {
        Ok(self.inspect_decision(pool, release_id).await?.1)
    }

    // ------------------------------------------------------------------
    // Dry-run / test results
    // ------------------------------------------------------------------

    /// Persists a dry-run/test-against-catalog result (PRD-POLICY-008).
    #[instrument(skip_all)]
    pub async fn store_test_run(
        &self,
        pool: &PgPool,
        summary: &melodee_domain::PolicyTestSummary,
    ) -> Result<(), PolicyStoreError> {
        let summary = melodee_domain::PolicyTestSummary::new(summary.clone())?;
        let disposition_counts = serde_json::to_value(
            summary
                .disposition_counts
                .iter()
                .map(|(d, c)| (d.as_str(), c))
                .collect::<Vec<_>>(),
        )?;
        let matched_rule_counts = serde_json::to_value(&summary.matched_rule_counts)?;
        let existing_disposition_counts = serde_json::to_value(
            summary
                .existing_disposition_counts
                .iter()
                .map(|(disposition, count)| (disposition.map(Disposition::as_str), count))
                .collect::<Vec<_>>(),
        )?;
        let sample_ids: Vec<Uuid> = summary
            .sample_release_ids
            .iter()
            .map(|r| r.into_uuid())
            .collect();
        let ran_by = serde_json::to_value(&summary.ran_by)?;

        sqlx::query(
            r#"INSERT INTO policy_test_runs
                (id, rule_set_id, rule_set_version, total_releases_evaluated,
                 disposition_counts, matched_rule_counts,
                 existing_disposition_counts, destructive_impact_count,
                 sample_release_ids, ran_by, ran_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        )
        .bind(summary.test_run_id.into_uuid())
        .bind(summary.rule_set_id.into_uuid())
        .bind(
            i32::try_from(summary.rule_set_version)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(
            i32::try_from(summary.total_releases_evaluated)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(disposition_counts)
        .bind(matched_rule_counts)
        .bind(existing_disposition_counts)
        .bind(
            i32::try_from(summary.destructive_impact_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .bind(&sample_ids)
        .bind(ran_by)
        .bind(summary.ran_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Catalog reads for novelty analysis
    // ------------------------------------------------------------------

    /// Loads all catalog Tracks for a Release along with their AudioIdentity
    /// and playable MediaAsset byte hash, for novelty assessment.
    #[instrument(skip_all, fields(release_id = %release_id))]
    pub async fn load_release_tracks(
        &self,
        pool: &PgPool,
        release_id: melodee_domain::ReleaseId,
    ) -> Result<Vec<CatalogTrackFact>, PolicyStoreError> {
        let rows: Vec<CatalogTrackFactRow> = sqlx::query_as(
            r#"SELECT t.release_id, t.id as track_id, t.audio_identity_id,
                   t.playable_media_asset_id as media_asset_id,
                   m.byte_hash, fp.fingerprint AS audio_fingerprint,
                   (EXISTS (
                       SELECT 1 FROM audio_identity_catalog_assertions assertion
                        WHERE assertion.audio_identity_id = t.audio_identity_id
                          AND assertion.outcome = 'no_catalog_match'
                   ) OR EXISTS (
                       SELECT 1 FROM audio_identity_alternate_recordings alternate
                       JOIN tracks other
                         ON other.audio_identity_id = CASE
                                WHEN alternate.source_audio_identity_id = t.audio_identity_id
                                THEN alternate.alternate_audio_identity_id
                                ELSE alternate.source_audio_identity_id
                            END
                        AND other.release_id <> t.release_id
                       WHERE t.audio_identity_id IN (
                           alternate.source_audio_identity_id,
                           alternate.alternate_audio_identity_id
                       )
                   )) as has_confirmed_audio_identity
              FROM tracks t
              LEFT JOIN media_assets m ON m.id = t.playable_media_asset_id
              LEFT JOIN catalog_imports ci ON ci.release_id = t.release_id
              LEFT JOIN release_work_item_manifests wm ON wm.id = ci.manifest_row_id
              LEFT JOIN LATERAL (
                    SELECT NULLIF(btrim(value->>'fingerprint'), '') AS fingerprint
                      FROM jsonb_array_elements(
                           COALESCE(wm.manifest_payload->'normalization'->'fingerprints', '[]'::jsonb)
                      ) value
                     WHERE value->>'source_file' = t.source_path
                     LIMIT 1
              ) fp ON true
             WHERE t.release_id = $1
             ORDER BY t.ordinal
             LIMIT $2"#,
        )
        .bind(release_id.into_uuid())
        .bind(
            i64::try_from(MAX_POLICY_TRACKS_PER_RELEASE + 1)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .fetch_all(pool)
        .await?;

        if rows.len() > MAX_POLICY_TRACKS_PER_RELEASE {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "Tracks per Release",
                limit: MAX_POLICY_TRACKS_PER_RELEASE,
            });
        }

        Ok(rows
            .into_iter()
            .map(|row| CatalogTrackFact {
                release_id: row.release_id,
                track_id: row.track_id,
                audio_identity_id: row.audio_identity_id,
                media_asset_id: row.media_asset_id,
                byte_hash: row.byte_hash,
                audio_fingerprint_digest: row
                    .audio_fingerprint
                    .as_deref()
                    .map(melodee_domain::acoustic_fingerprint_digest),
                audio_fingerprint: row.audio_fingerprint,
                has_confirmed_audio_identity: row.has_confirmed_audio_identity,
            })
            .collect())
    }

    /// Loads and revalidates the complete bounded catalog context for a held
    /// claim. No source or canonical media path is opened.
    pub async fn load_context(
        &self,
        pool: &PgPool,
        claim: &PolicyClaim,
    ) -> Result<PolicyCatalogContext, PolicyStoreError> {
        let persisted: Option<(Uuid, String, String, String, bool, serde_json::Value)> =
            sqlx::query_as(
                r#"SELECT ci.release_id, r.title, ci.sealed_manifest_hash,
                      ci.semantic_manifest_hash,
                      jsonb_path_exists(
                          wm.manifest_payload,
                          '$.files[*] ? (@.bytes_read_errors == true)'
                      ), wm.manifest_payload
                 FROM catalog_imports ci
                 JOIN releases r ON r.id = ci.release_id
                 JOIN release_work_item_manifests wm ON wm.id = ci.manifest_row_id
                WHERE ci.work_item_id = $1"#,
            )
            .bind(claim.item.id)
            .fetch_optional(pool)
            .await?;
        let (release_id, title, sealed, semantic, has_corrupt_media, manifest_payload) =
            persisted.ok_or(PolicyStoreError::NotCataloged(claim.item.id))?;
        if release_id != claim.release_id.into_uuid()
            || sealed != claim.source_fingerprint
            || semantic != claim.semantic_manifest_hash
        {
            return Err(PolicyStoreError::FingerprintMismatch);
        }

        let tracks = self.load_release_tracks(pool, claim.release_id).await?;
        if tracks.is_empty() {
            return Err(PolicyStoreError::IncompleteFacts);
        }
        let audio_ids: Vec<Uuid> = tracks.iter().map(|track| track.audio_identity_id).collect();
        let hashes: Vec<String> = tracks
            .iter()
            .filter_map(|track| track.byte_hash.clone())
            .collect();
        let exact_hash_rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT DISTINCT m.byte_hash
                 FROM tracks t
                 JOIN media_assets m ON m.id = t.playable_media_asset_id
                WHERE t.release_id <> $1 AND m.byte_hash = ANY($2)"#,
        )
        .bind(release_id)
        .bind(&hashes)
        .fetch_all(pool)
        .await?;
        let known_rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"SELECT DISTINCT own.audio_identity_id
                 FROM tracks own
                WHERE own.release_id = $1 AND own.audio_identity_id = ANY($2)
                  AND (
                      EXISTS (
                          SELECT 1 FROM tracks other
                           WHERE other.release_id <> $1
                             AND other.audio_identity_id = own.audio_identity_id
                      ) OR EXISTS (
                          SELECT 1
                            FROM audio_identity_match_current current_match
                            JOIN audio_identity_match_decisions match_decision
                              ON match_decision.id = current_match.decision_id
                             AND match_decision.outcome = 'confirmed'
                            JOIN tracks other
                              ON other.audio_identity_id = current_match.candidate_audio_identity_id
                             AND other.release_id <> $1
                           WHERE current_match.source_audio_identity_id = own.audio_identity_id
                      )
                  )"#,
        )
        .bind(release_id)
        .bind(&audio_ids)
        .fetch_all(pool)
        .await?;
        let alternate_rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"SELECT DISTINCT own.audio_identity_id
                 FROM tracks own
                WHERE own.release_id = $2
                  AND own.audio_identity_id = ANY($1)
                  AND EXISTS (
                      SELECT 1
                        FROM audio_identity_alternate_recordings alternate
                        JOIN tracks other
                          ON other.audio_identity_id = CASE
                                 WHEN alternate.source_audio_identity_id = own.audio_identity_id
                                 THEN alternate.alternate_audio_identity_id
                                 ELSE alternate.source_audio_identity_id
                             END
                         AND other.release_id <> $2
                       WHERE own.audio_identity_id IN (
                           alternate.source_audio_identity_id,
                           alternate.alternate_audio_identity_id
                       )
                  )"#,
        )
        .bind(&audio_ids)
        .bind(release_id)
        .fetch_all(pool)
        .await?;
        let acoustic_rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT DISTINCT evidence.payload->>'fingerprint_digest'
                 FROM audio_identity_evidence evidence
                 JOIN tracks owner ON owner.id = evidence.track_id
                WHERE evidence.evidence_kind = 'acoustic_fingerprint'
                  AND evidence.payload ? 'fingerprint_digest'
                  AND owner.release_id <> $1"#,
        )
        .bind(release_id)
        .fetch_all(pool)
        .await?;
        let catalog_trait_rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT trait FROM release_catalog_trait_assertions
                WHERE release_id = $1 ORDER BY trait LIMIT $2"#,
        )
        .bind(release_id)
        .bind(
            i64::try_from(melodee_domain::MAX_RELEASE_CLASSIFICATION_TRAITS + 1)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .fetch_all(pool)
        .await?;
        if catalog_trait_rows.len() > melodee_domain::MAX_RELEASE_CLASSIFICATION_TRAITS {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "catalog Release trait assertions",
                limit: melodee_domain::MAX_RELEASE_CLASSIFICATION_TRAITS,
            });
        }
        let catalog_known_release_traits = catalog_trait_rows
            .into_iter()
            .map(|(value,)| {
                melodee_domain::ReleaseTrait::parse(&value)
                    .ok_or(PolicyStoreError::CorruptValue(value))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let current_classification = self.load_classification(pool, claim.release_id).await?;
        let current_decision = self.get_decision(pool, claim.release_id).await?;
        let existing_disposition = current_decision
            .as_ref()
            .map(|decision| decision.disposition);
        let (structured_tag_traits, embedded_external_id_traits) =
            extract_manifest_traits(&manifest_payload);
        Ok(PolicyCatalogContext {
            claim: PolicyClaim {
                item: claim.item.clone(),
                release_id: claim.release_id,
                title,
                source_fingerprint: sealed,
                semantic_manifest_hash: semantic,
            },
            tracks,
            current_classification,
            current_decision,
            structured_tag_traits,
            embedded_external_id_traits,
            catalog_known_release_traits,
            existing_disposition,
            has_corrupt_media,
            catalog_exact_byte_hashes: exact_hash_rows.into_iter().map(|row| row.0).collect(),
            catalog_known_audio_identities: known_rows
                .into_iter()
                .map(|row| melodee_domain::AudioIdentityId::from_uuid(row.0))
                .collect(),
            catalog_alternate_audio_identities: alternate_rows
                .into_iter()
                .map(|row| melodee_domain::AudioIdentityId::from_uuid(row.0))
                .collect(),
            catalog_acoustic_fingerprints: acoustic_rows.into_iter().map(|row| row.0).collect(),
        })
    }

    /// Loads the same immutable context without claiming work. Intended for
    /// read-only CLI inspection and dry-run evaluation.
    pub async fn load_snapshot(
        &self,
        pool: &PgPool,
        release_id: ReleaseId,
    ) -> Result<Option<PolicyCatalogContext>, PolicyStoreError> {
        let binding: Option<(Uuid, String, String, String)> = sqlx::query_as(
            r#"SELECT ci.work_item_id, r.title, ci.sealed_manifest_hash,
                      ci.semantic_manifest_hash
                 FROM catalog_imports ci
                 JOIN releases r ON r.id = ci.release_id
                WHERE ci.release_id = $1"#,
        )
        .bind(release_id.into_uuid())
        .fetch_optional(pool)
        .await?;
        let Some((work_item_id, title, source_fingerprint, semantic_manifest_hash)) = binding
        else {
            return Ok(None);
        };
        let item = crate::WorkItemStore.get(pool, work_item_id).await?;
        let claim = PolicyClaim {
            item,
            release_id,
            title,
            source_fingerprint,
            semantic_manifest_hash,
        };
        Ok(Some(self.load_context(pool, &claim).await?))
    }

    /// Loads bounded read-only contexts with fixed set-based preflights
    /// and set-based snapshot. This never claims work or scales statements
    /// with the requested Release count.
    pub async fn load_test_contexts(
        &self,
        pool: &PgPool,
        limit: u32,
    ) -> Result<Vec<PolicyCatalogContext>, PolicyStoreError> {
        let limit = usize::try_from(limit).map_err(|_| PolicyStoreError::NumericOverflow)?;
        if limit > MAX_POLICY_TEST_RELEASES {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "dry-run Releases",
                limit: MAX_POLICY_TEST_RELEASES,
            });
        }
        let track_scope: (i64,) = sqlx::query_as(
            r#"SELECT count(*) FROM (
                   SELECT 1
                     FROM tracks track
                     JOIN (
                         SELECT release_id FROM catalog_imports
                          ORDER BY imported_at, release_id LIMIT $1
                     ) selected ON selected.release_id = track.release_id
                    LIMIT $2
               ) bounded"#,
        )
        .bind(i64::try_from(limit).map_err(|_| PolicyStoreError::NumericOverflow)?)
        .bind(
            i64::try_from(MAX_POLICY_TEST_TRACK_FACTS + 1)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .fetch_one(pool)
        .await?;
        if usize::try_from(track_scope.0).map_err(|_| PolicyStoreError::NumericOverflow)?
            > MAX_POLICY_TEST_TRACK_FACTS
        {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "dry-run Track facts",
                limit: MAX_POLICY_TEST_TRACK_FACTS,
            });
        }
        let classification_scope: (i64,) = sqlx::query_as(
            r#"WITH selected AS (
                   SELECT release_id FROM catalog_imports
                    ORDER BY imported_at, release_id LIMIT $1
               )
               SELECT count(*) FROM (
                   SELECT trait.classification_id
                     FROM release_traits trait
                     JOIN release_classifications classification
                       ON classification.id = trait.classification_id
                     JOIN selected ON selected.release_id = classification.release_id
                   UNION ALL
                   SELECT evidence.id
                     FROM release_classification_evidence evidence
                     JOIN release_classifications classification
                       ON classification.id = evidence.id
                     JOIN selected ON selected.release_id = classification.release_id
                   UNION ALL
                   SELECT assertion.id
                     FROM release_catalog_trait_assertions assertion
                     JOIN selected ON selected.release_id = assertion.release_id
                   LIMIT $2
               ) bounded"#,
        )
        .bind(i64::try_from(limit).map_err(|_| PolicyStoreError::NumericOverflow)?)
        .bind(
            i64::try_from(MAX_POLICY_TEST_CLASSIFICATION_FACTS + 1)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
        )
        .fetch_one(pool)
        .await?;
        if usize::try_from(classification_scope.0).map_err(|_| PolicyStoreError::NumericOverflow)?
            > MAX_POLICY_TEST_CLASSIFICATION_FACTS
        {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "dry-run classification facts",
                limit: MAX_POLICY_TEST_CLASSIFICATION_FACTS,
            });
        }
        let rows: Vec<DryRunContextRow> = sqlx::query_as(
            r#"SELECT w.*, ci.release_id, r.title,
                      ci.sealed_manifest_hash, ci.semantic_manifest_hash,
                      wm.manifest_payload,
                      jsonb_path_exists(
                          wm.manifest_payload,
                          '$.files[*] ? (@.bytes_read_errors == true)'
                      ) AS has_corrupt_media,
                      COALESCE((
                          SELECT jsonb_agg(to_jsonb(fact) ORDER BY fact.track_id)
                            FROM (
                                SELECT t.release_id, t.id AS track_id,
                                       t.audio_identity_id,
                                       t.playable_media_asset_id AS media_asset_id,
                                       media.byte_hash,
                                       (SELECT NULLIF(btrim(fp->>'fingerprint'), '')
                                          FROM jsonb_array_elements(COALESCE(
                                               wm.manifest_payload->'normalization'->'fingerprints',
                                               '[]'::jsonb
                                          )) fp
                                         WHERE fp->>'source_file' = t.source_path LIMIT 1
                                       ) AS audio_fingerprint,
                                       (EXISTS (
                                           SELECT 1 FROM audio_identity_catalog_assertions assertion
                                            WHERE assertion.audio_identity_id = t.audio_identity_id
                                              AND assertion.outcome = 'no_catalog_match'
                                       ) OR EXISTS (
                                           SELECT 1 FROM audio_identity_alternate_recordings alternate
                                           JOIN tracks other
                                             ON other.audio_identity_id = CASE
                                                    WHEN alternate.source_audio_identity_id = t.audio_identity_id
                                                    THEN alternate.alternate_audio_identity_id
                                                    ELSE alternate.source_audio_identity_id
                                                END
                                            AND other.release_id <> t.release_id
                                           WHERE t.audio_identity_id IN (
                                               alternate.source_audio_identity_id,
                                               alternate.alternate_audio_identity_id
                                           )
                                       )) AS has_confirmed_audio_identity
                                  FROM tracks t
                                  JOIN media_assets media
                                    ON media.id = t.playable_media_asset_id
                                 WHERE t.release_id = ci.release_id
                                 ORDER BY t.id LIMIT $2
                            ) fact
                      ), '[]'::jsonb) AS tracks,
                      COALESCE((
                          SELECT array_agg(assertion.trait ORDER BY assertion.trait)
                            FROM release_catalog_trait_assertions assertion
                           WHERE assertion.release_id = ci.release_id
                      ), ARRAY[]::text[]) AS catalog_traits,
                      decision.disposition AS existing_disposition,
                      COALESCE((
                          SELECT array_agg(DISTINCT own_media.byte_hash)
                            FROM tracks own
                            JOIN media_assets own_media
                              ON own_media.id = own.playable_media_asset_id
                           WHERE own.release_id = ci.release_id
                             AND EXISTS (
                                 SELECT 1 FROM tracks other
                                 JOIN media_assets other_media
                                   ON other_media.id = other.playable_media_asset_id
                                 WHERE other.release_id <> ci.release_id
                                   AND other_media.byte_hash = own_media.byte_hash
                             )
                      ), ARRAY[]::text[]) AS exact_hashes,
                      COALESCE((
                          SELECT array_agg(DISTINCT own.audio_identity_id)
                            FROM tracks own WHERE own.release_id = ci.release_id
                              AND (
                                EXISTS (
                                  SELECT 1 FROM tracks other
                                   WHERE other.release_id <> ci.release_id
                                     AND other.audio_identity_id = own.audio_identity_id
                                ) OR EXISTS (
                                  SELECT 1
                                    FROM audio_identity_match_current current_match
                                    JOIN audio_identity_match_decisions match_decision
                                      ON match_decision.id = current_match.decision_id
                                     AND match_decision.outcome = 'confirmed'
                                    JOIN tracks other
                                      ON other.audio_identity_id = current_match.candidate_audio_identity_id
                                     AND other.release_id <> ci.release_id
                                   WHERE current_match.source_audio_identity_id = own.audio_identity_id
                                )
                              )
                      ), ARRAY[]::uuid[]) AS known_audio_ids,
                      COALESCE((
                          SELECT array_agg(DISTINCT own.audio_identity_id)
                            FROM tracks own WHERE own.release_id = ci.release_id
                              AND EXISTS (
                                  SELECT 1
                                    FROM audio_identity_alternate_recordings alternate
                                    JOIN tracks other
                                      ON other.audio_identity_id = CASE
                                             WHEN alternate.source_audio_identity_id = own.audio_identity_id
                                             THEN alternate.alternate_audio_identity_id
                                             ELSE alternate.source_audio_identity_id
                                         END
                                     AND other.release_id <> ci.release_id
                                   WHERE own.audio_identity_id IN (
                                       alternate.source_audio_identity_id,
                                       alternate.alternate_audio_identity_id
                                   )
                              )
                      ), ARRAY[]::uuid[]) AS alternate_audio_ids,
                      COALESCE((
                          SELECT array_agg(DISTINCT evidence.payload->>'fingerprint_digest')
                            FROM audio_identity_evidence evidence
                            JOIN tracks owner
                              ON owner.id = evidence.track_id
                           WHERE evidence.evidence_kind = 'acoustic_fingerprint'
                             AND evidence.payload ? 'fingerprint_digest'
                             AND owner.release_id <> ci.release_id
                      ), ARRAY[]::text[]) AS acoustic_fingerprints
                 FROM catalog_imports ci
                 JOIN release_work_items w ON w.id = ci.work_item_id
                 JOIN releases r ON r.id = ci.release_id
                 JOIN release_work_item_manifests wm ON wm.id = ci.manifest_row_id
                 LEFT JOIN policy_decisions decision ON decision.release_id = ci.release_id
                ORDER BY ci.imported_at, ci.release_id
                LIMIT $1"#,
        )
        .bind(i64::try_from(limit).map_err(|_| PolicyStoreError::NumericOverflow)?)
        .bind(i64::try_from(MAX_POLICY_TRACKS_PER_RELEASE + 1)
            .map_err(|_| PolicyStoreError::NumericOverflow)?)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    /// Loads the catalog novelty context for a Release: the set of AudioIdentity
    /// IDs that have exact byte-duplicate media elsewhere in the catalog, and
    /// the set of AudioIdentity IDs that are referenced by Tracks in other
    /// Releases (known identities).
    #[instrument(skip_all, fields(release_id = %release_id))]
    pub async fn load_catalog_novelty_context(
        &self,
        pool: &PgPool,
        release_id: melodee_domain::ReleaseId,
    ) -> Result<
        (
            std::collections::BTreeSet<melodee_domain::AudioIdentityId>,
            std::collections::BTreeSet<melodee_domain::AudioIdentityId>,
        ),
        PolicyStoreError,
    > {
        let release_uuid = release_id.into_uuid();

        let exact_dup_rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"SELECT DISTINCT t.audio_identity_id
               FROM tracks t
               JOIN media_assets m ON m.id = t.playable_media_asset_id
               JOIN track_media_assets tma ON tma.track_id = t.id
               JOIN media_assets m2 ON m2.id = tma.media_asset_id
               WHERE t.release_id = $1
                 AND m2.byte_hash = m.byte_hash
                 AND EXISTS (
                     SELECT 1 FROM tracks t3
                     JOIN track_media_assets tma3 ON tma3.track_id = t3.id
                     WHERE tma3.media_asset_id = m2.id
                       AND t3.release_id <> $1
                 )"#,
        )
        .bind(release_uuid)
        .fetch_all(pool)
        .await?;

        let known_rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"SELECT DISTINCT t.audio_identity_id
               FROM tracks t
               WHERE t.release_id = $1
                 AND EXISTS (
                     SELECT 1 FROM tracks t2
                     WHERE t2.audio_identity_id = t.audio_identity_id
                       AND t2.release_id <> $1
                 )"#,
        )
        .bind(release_uuid)
        .fetch_all(pool)
        .await?;

        let exact_dup: std::collections::BTreeSet<_> = exact_dup_rows
            .into_iter()
            .map(|(id,)| melodee_domain::AudioIdentityId::from_uuid(id))
            .collect();
        let known: std::collections::BTreeSet<_> = known_rows
            .into_iter()
            .map(|(id,)| melodee_domain::AudioIdentityId::from_uuid(id))
            .collect();

        Ok((exact_dup, known))
    }

    /// Checks whether a given AudioIdentity is already referenced by catalog
    /// Tracks outside of the given Release.
    #[instrument(skip_all)]
    pub async fn count_other_tracks_with_identity(
        &self,
        pool: &PgPool,
        audio_identity_id: melodee_domain::AudioIdentityId,
        exclude_release_id: melodee_domain::ReleaseId,
    ) -> Result<u32, PolicyStoreError> {
        let row: (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*)
               FROM tracks t
               WHERE t.audio_identity_id = $1
                 AND t.release_id <> $2"#,
        )
        .bind(audio_identity_id.into_uuid())
        .bind(exclude_release_id.into_uuid())
        .fetch_one(pool)
        .await?;
        u32::try_from(row.0).map_err(|_| PolicyStoreError::NumericOverflow)
    }

    /// Checks whether a given byte hash exists in any other Release's media.
    #[instrument(skip_all)]
    pub async fn has_known_byte_hash(
        &self,
        pool: &PgPool,
        byte_hash: &str,
        exclude_release_id: melodee_domain::ReleaseId,
    ) -> Result<bool, PolicyStoreError> {
        let row: (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*)
               FROM media_assets m
               JOIN track_media_assets tma ON tma.media_asset_id = m.id
               JOIN tracks t ON t.id = tma.track_id
              WHERE m.byte_hash = $1 AND t.release_id <> $2
              LIMIT 1"#,
        )
        .bind(byte_hash)
        .bind(exclude_release_id.into_uuid())
        .fetch_one(pool)
        .await?;
        Ok(row.0 > 0)
    }

    /// Loads all catalog Release IDs for a set of AudioIdentity IDs (for
    /// cross-Release identity novelty).
    #[instrument(skip_all)]
    pub async fn find_releases_with_audio_identity(
        &self,
        pool: &PgPool,
        audio_identity_id: melodee_domain::AudioIdentityId,
        exclude_release_id: melodee_domain::ReleaseId,
    ) -> Result<Vec<melodee_domain::ReleaseId>, PolicyStoreError> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"SELECT DISTINCT t.release_id
               FROM tracks t
               WHERE t.audio_identity_id = $1
                 AND t.release_id <> $2"#,
        )
        .bind(audio_identity_id.into_uuid())
        .bind(exclude_release_id.into_uuid())
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(r,)| melodee_domain::ReleaseId::from_uuid(r))
            .collect())
    }

    /// Loads a Release classification for a given Release ID.
    #[instrument(skip_all)]
    pub async fn load_classification(
        &self,
        pool: &PgPool,
        release_id: melodee_domain::ReleaseId,
    ) -> Result<Option<StoredReleaseClassification>, PolicyStoreError> {
        let row: Option<StoredClassificationRow> = sqlx::query_as(
            r#"SELECT id, release_id, rule_set_id, rule_set_version,
                      confidence, classifier_version, classified_at
              FROM release_classifications
             WHERE release_id = $1"#,
        )
        .bind(release_id.into_uuid())
        .fetch_optional(pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let trait_rows: Vec<(String,)> =
            sqlx::query_as(r#"SELECT trait FROM release_traits WHERE classification_id = $1"#)
                .bind(row.id)
                .fetch_all(pool)
                .await?;

        let evidence_rows: Vec<StoredEvidenceRow> = sqlx::query_as(
            r#"SELECT trait AS trait_, source, kind, confidence, detail
                FROM release_classification_evidence
               WHERE id = $1"#,
        )
        .bind(row.id)
        .fetch_all(pool)
        .await?;

        let mut traits = std::collections::BTreeSet::new();
        for (trait_str,) in trait_rows {
            let trait_ = melodee_domain::ReleaseTrait::parse(&trait_str)
                .ok_or(PolicyStoreError::CorruptValue(trait_str))?;
            traits.insert(trait_);
        }

        let mut evidence = Vec::with_capacity(evidence_rows.len());
        for row_evidence in evidence_rows {
            let trait_ = melodee_domain::ReleaseTrait::parse(&row_evidence.trait_)
                .ok_or_else(|| PolicyStoreError::CorruptValue(row_evidence.trait_.clone()))?;
            let source = melodee_domain::EvidenceSource::parse(&row_evidence.source)
                .ok_or_else(|| PolicyStoreError::CorruptValue(row_evidence.source.clone()))?;
            let kind = melodee_domain::ClassificationEvidenceKind::parse(&row_evidence.kind)
                .ok_or_else(|| PolicyStoreError::CorruptValue(row_evidence.kind.clone()))?;
            let confidence = melodee_domain::Confidence::new(
                u16::try_from(row_evidence.confidence)
                    .map_err(|_| PolicyStoreError::NumericOverflow)?,
            )?;
            evidence.push(melodee_domain::ClassificationEvidence::new(
                melodee_domain::ClassificationEvidence {
                    id: melodee_domain::ReleaseClassificationId::from_uuid(row.id),
                    release_id: melodee_domain::ReleaseId::from_uuid(row.release_id),
                    trait_,
                    source,
                    kind,
                    confidence,
                    detail: row_evidence.detail,
                },
            )?);
        }

        Ok(Some(StoredReleaseClassification {
            id: melodee_domain::ReleaseClassificationId::from_uuid(row.id),
            release_id: melodee_domain::ReleaseId::from_uuid(row.release_id),
            rule_set_id: melodee_domain::PolicyRuleSetId::from_uuid(row.rule_set_id),
            rule_set_version: u32::try_from(row.rule_set_version)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            traits,
            evidence,
            confidence: melodee_domain::Confidence::new(
                u16::try_from(row.confidence).map_err(|_| PolicyStoreError::NumericOverflow)?,
            )
            .map_err(|_| {
                PolicyStoreError::Domain(melodee_domain::DomainInvariantError::InvalidConfidence)
            })?,
            classified_at: row.classified_at,
            classifier_version: row.classifier_version,
        }))
    }

    /// Loads the persisted novelty evaluation for a Release, including the
    /// per-Track novelty rows, for operator review display. This
    /// is a pure read: it replays no evaluation and computes nothing new.
    #[instrument(skip_all)]
    pub async fn load_novelty(
        &self,
        pool: &PgPool,
        release_id: melodee_domain::ReleaseId,
    ) -> Result<Option<StoredNoveltyEvaluation>, PolicyStoreError> {
        let row: Option<StoredNoveltyEvaluationRow> = sqlx::query_as(
            r#"SELECT id, release_id, exact_duplicate_count,
                      known_audio_identity_count, alternate_recording_count,
                      new_track_count, uncertain_count, total_track_count,
                      evaluated_at, evaluator_version
                 FROM release_novelty_evaluations
                WHERE release_id = $1"#,
        )
        .bind(release_id.into_uuid())
        .fetch_optional(pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let track_rows: Vec<StoredTrackNoveltyRow> = sqlx::query_as(
            r#"SELECT track_id, category, confidence, evidence_kind, detail
                 FROM track_novelty
                WHERE evaluation_id = $1
                ORDER BY track_id"#,
        )
        .bind(row.id)
        .fetch_all(pool)
        .await?;

        let tracks = track_rows
            .into_iter()
            .map(StoredTrackNovelty::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(StoredNoveltyEvaluation {
            id: row.id,
            release_id: melodee_domain::ReleaseId::from_uuid(row.release_id),
            exact_duplicate_count: u32::try_from(row.exact_duplicate_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            known_audio_identity_count: u32::try_from(row.known_audio_identity_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            alternate_recording_count: u32::try_from(row.alternate_recording_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            new_track_count: u32::try_from(row.new_track_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            uncertain_count: u32::try_from(row.uncertain_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            total_track_count: u32::try_from(row.total_track_count)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            evaluated_at: row.evaluated_at,
            evaluator_version: row.evaluator_version,
            tracks,
        }))
    }
}

// ------------------------------------------------------------------
// Stored row types
// ------------------------------------------------------------------

fn completed_non_destructive_decision(
    mut decision: ReleasePolicyDecision,
    now: OffsetDateTime,
) -> Result<ReleasePolicyDecision, PolicyStoreError> {
    if matches!(
        decision.disposition,
        Disposition::Review | Disposition::Delete
    ) {
        decision.completion_state = DecisionCompletionState::Pending;
        decision.completed_at = None;
    } else {
        decision.completion_state = DecisionCompletionState::Completed;
        decision.completed_at = Some(now);
    }
    Ok(ReleasePolicyDecision::new(decision)?)
}

fn policy_target(disposition: Disposition) -> (IngestionState, bool) {
    match disposition {
        Disposition::Accept => (IngestionState::Indexing, true),
        Disposition::Review | Disposition::Delete => (IngestionState::NeedsReview, true),
        Disposition::Ignore => (IngestionState::Ignored, true),
        Disposition::Quarantine => (IngestionState::Quarantined, true),
    }
}

async fn set_release_client_status_tx(
    tx: &mut Transaction<'_, Postgres>,
    release_id: melodee_domain::ReleaseId,
    disposition: Disposition,
    now: OffsetDateTime,
) -> Result<(), PolicyStoreError> {
    let status = if disposition == Disposition::Accept {
        "active"
    } else {
        "deprecated"
    };
    sqlx::query(
        "UPDATE releases
            SET status = $2, revision = revision + 1, updated_at = $3
          WHERE id = $1 AND status IS DISTINCT FROM $2",
    )
    .bind(release_id.into_uuid())
    .bind(status)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn recompute_artist_aggregates_after_release_deletion(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), PolicyStoreError> {
    sqlx::query(
        "WITH affected AS (
            SELECT artist_id FROM release_artists WHERE release_id = $1
            UNION
            SELECT billing.artist_id
              FROM track_artists billing
              JOIN tracks track ON track.id = billing.track_id
             WHERE track.release_id = $1
            UNION
            SELECT artist_id FROM contributions WHERE release_id = $1
         ), recalculated AS (
            SELECT affected.artist_id,
                   count(DISTINCT track.release_id)
                       FILTER (WHERE release.status = 'active') AS release_count,
                   count(DISTINCT association.track_id)
                       FILTER (WHERE release.status = 'active') AS billed_track_count,
                   count(DISTINCT association.track_id)
                       FILTER (WHERE release.status = 'active'
                               AND association.is_performance) AS performance_track_count,
                   count(DISTINCT association.track_id)
                       FILTER (WHERE release.status = 'active') AS all_credit_track_count
              FROM affected
              LEFT JOIN artist_track_associations association
                ON association.artist_id = affected.artist_id
              LEFT JOIN tracks track ON track.id = association.track_id
              LEFT JOIN releases release ON release.id = track.release_id
             GROUP BY affected.artist_id
         )
         UPDATE artist_catalog_aggregates aggregate
            SET release_count = recalculated.release_count,
                billed_track_count = recalculated.billed_track_count,
                performance_track_count = recalculated.performance_track_count,
                all_credit_track_count = recalculated.all_credit_track_count,
                revision = aggregate.revision + 1,
                updated_at = $2
           FROM recalculated
          WHERE aggregate.artist_id = recalculated.artist_id",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_decision_rule(
    decision: &ReleasePolicyDecision,
    rules: &[melodee_domain::PolicyRule],
    classification: &ReleaseClassification,
    novelty: &NoveltyEvaluation,
    existing_disposition: Option<Disposition>,
    has_corrupt_media: bool,
) -> Result<(), PolicyStoreError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Predicate {
        #[serde(default)]
        required_traits: Vec<melodee_domain::ReleaseTrait>,
        existing_dispositions: Option<Vec<Disposition>>,
    }

    let mut first_match = None;
    for rule in rules {
        let predicate: Predicate = serde_json::from_value(rule.predicate_json.clone())?;
        if !predicate
            .required_traits
            .iter()
            .all(|trait_| classification.has_trait(*trait_))
            || predicate
                .existing_dispositions
                .as_ref()
                .is_some_and(|allowed| {
                    existing_disposition.is_none_or(|existing| !allowed.contains(&existing))
                })
        {
            continue;
        }
        let matches = match rule.rule_type {
            melodee_domain::PolicyRuleType::AdministratorOverride => false,
            melodee_domain::PolicyRuleType::CorruptMedia => has_corrupt_media,
            melodee_domain::PolicyRuleType::GreatestHitsUncertain => {
                classification.has_trait(melodee_domain::ReleaseTrait::GreatestHits)
                    && novelty.has_uncertain_tracks()
            }
            melodee_domain::PolicyRuleType::GreatestHitsNovelty => {
                classification.has_trait(melodee_domain::ReleaseTrait::GreatestHits)
                    && novelty.novel_track_count() > 0
            }
            melodee_domain::PolicyRuleType::GreatestHitsRedundancy => {
                let confidence = classification.strongest_non_directory_confidence_for(
                    melodee_domain::ReleaseTrait::GreatestHits,
                );
                classification.has_trait(melodee_domain::ReleaseTrait::GreatestHits)
                    && novelty.is_redundant()
                    && confidence.is_some()
                    && rule
                        .confidence_threshold
                        .is_none_or(|threshold| confidence.is_some_and(|value| value >= threshold))
            }
            melodee_domain::PolicyRuleType::UnknownClassification => {
                classification.has_trait(melodee_domain::ReleaseTrait::Unknown)
            }
            melodee_domain::PolicyRuleType::TraitDisposition => {
                rule.confidence_threshold.is_none_or(|threshold| {
                    predicate.required_traits.iter().all(|trait_| {
                        classification
                            .strongest_non_directory_confidence_for(*trait_)
                            .is_some_and(|confidence| confidence >= threshold)
                    })
                })
            }
        };
        if matches {
            first_match = Some(rule);
            break;
        }
    }
    let canonical_summary = first_match.map_or_else(
        || "No configured policy rule matched; accepting Release".to_string(),
        |rule| match rule.rule_type {
            melodee_domain::PolicyRuleType::AdministratorOverride => {
                "Explicit administrator override".to_string()
            }
            melodee_domain::PolicyRuleType::CorruptMedia => {
                "Corrupt or unreadable media".to_string()
            }
            melodee_domain::PolicyRuleType::GreatestHitsUncertain => format!(
                "Greatest Hits contains {} uncertain Track(s)",
                novelty.uncertain_count
            ),
            melodee_domain::PolicyRuleType::GreatestHitsNovelty => format!(
                "Greatest Hits contains {} new/alternate Track(s)",
                novelty.novel_track_count()
            ),
            melodee_domain::PolicyRuleType::GreatestHitsRedundancy => format!(
                "Greatest Hits contains {} redundant Track(s) and no novel or uncertain Tracks",
                novelty.redundant_track_count()
            ),
            melodee_domain::PolicyRuleType::UnknownClassification => {
                "Release classification is Unknown".to_string()
            }
            melodee_domain::PolicyRuleType::TraitDisposition => {
                format!("Configured trait rule {} matched", rule.id)
            }
        },
    );
    if decision.evidence_summary != canonical_summary {
        return Err(PolicyStoreError::DecisionConflict);
    }
    match (first_match, decision.matched_rule_ids.as_slice()) {
        (None, []) if decision.disposition == Disposition::Accept => Ok(()),
        (Some(rule), [rule_id])
            if rule.id == *rule_id && rule.disposition == decision.disposition =>
        {
            Ok(())
        }
        _ => Err(PolicyStoreError::DecisionConflict),
    }
}

fn decision_semantically_matches(
    stored: &StoredPolicyDecision,
    requested: &ReleasePolicyDecision,
) -> bool {
    stored.release_id == requested.release_id
        && stored.rule_set_id == requested.rule_set_id
        && stored.rule_set_version == requested.rule_set_version
        && stored.matched_rule_ids == requested.matched_rule_ids
        && stored.disposition == requested.disposition
        && stored.evidence_summary == requested.evidence_summary
        && stored.source_fingerprint == requested.source_fingerprint
        && stored.actor == requested.actor
}

fn extract_manifest_traits(
    manifest: &serde_json::Value,
) -> (
    Vec<melodee_domain::ReleaseTrait>,
    Vec<melodee_domain::ReleaseTrait>,
) {
    let mut structured = std::collections::BTreeSet::new();
    let mut embedded = std::collections::BTreeSet::new();
    let metadata = manifest
        .get("files")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("metadata"))
        .chain(
            manifest
                .get("tracks")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|track| track.get("observed_metadata")),
        );
    for raw in metadata.filter_map(|metadata| metadata.get("raw")) {
        let Some(object) = raw.as_object() else {
            continue;
        };
        for (key, value) in object {
            let normalized_key: String = key
                .chars()
                .filter(|char_| char_.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect();
            if normalized_key == "flagcompilation"
                && (value == &serde_json::Value::Bool(true)
                    || value.as_str().is_some_and(|value| {
                        matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true")
                    }))
            {
                structured.insert(melodee_domain::ReleaseTrait::Compilation);
            } else if normalized_key == "musicbrainzreleasetype" {
                for token in manifest_trait_tokens(value) {
                    embedded.insert(token);
                }
            }
        }
    }
    (
        structured.into_iter().collect(),
        embedded.into_iter().collect(),
    )
}

fn manifest_trait_tokens(value: &serde_json::Value) -> Vec<melodee_domain::ReleaseTrait> {
    let values: Vec<&str> = match value {
        serde_json::Value::String(value) => vec![value],
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect(),
        _ => Vec::new(),
    };
    values
        .into_iter()
        .flat_map(|value| value.split([',', ';', '/']))
        .filter_map(|token| {
            let mut normalized = String::new();
            for char_ in token.trim().chars() {
                if char_.is_ascii_alphanumeric() {
                    normalized.extend(char_.to_lowercase());
                } else if !normalized.ends_with('_') {
                    normalized.push('_');
                }
            }
            let normalized = normalized.trim_matches('_');
            melodee_domain::ReleaseTrait::parse(normalized)
        })
        .collect()
}

fn validate_manifest_classification_evidence(
    classification: &ReleaseClassification,
    manifest: &serde_json::Value,
    catalog_traits: &std::collections::BTreeSet<melodee_domain::ReleaseTrait>,
) -> Result<(), PolicyStoreError> {
    if classification.classifier_version != SUPPORTED_CLASSIFIER_VERSION {
        return Err(PolicyStoreError::ForeignFacts);
    }
    let (structured, embedded) = extract_manifest_traits(manifest);
    let structured: std::collections::BTreeSet<_> = structured.into_iter().collect();
    let embedded: std::collections::BTreeSet<_> = embedded.into_iter().collect();
    let title_traits = manifest
        .get("release_observation")
        .and_then(|observation| observation.get("title"))
        .and_then(serde_json::Value::as_str)
        .map(classification_title_traits)
        .unwrap_or_default();
    let source_path = manifest
        .get("source_path")
        .and_then(serde_json::Value::as_str);
    let directory_traits = source_path
        .and_then(|path| std::path::Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .map(classification_title_traits)
        .unwrap_or_default();
    let parent_directory_traits = source_path
        .and_then(|path| std::path::Path::new(path).parent())
        .and_then(std::path::Path::file_name)
        .and_then(|name| name.to_str())
        .map(classification_title_traits)
        .unwrap_or_default();
    let has_real_signal = !structured.is_empty()
        || !embedded.is_empty()
        || !title_traits.is_empty()
        || !directory_traits.is_empty()
        || !parent_directory_traits.is_empty()
        || !catalog_traits.is_empty();
    for evidence in &classification.evidence {
        let (expected_kind, expected_confidence, source_is_authoritative) = match evidence.source {
            melodee_domain::EvidenceSource::Administrator => {
                return Err(PolicyStoreError::AuthorizationRequired);
            }
            melodee_domain::EvidenceSource::EmbeddedExternalId => (
                melodee_domain::ClassificationEvidenceKind::EmbeddedExternalId,
                8_500,
                embedded.contains(&evidence.trait_),
            ),
            melodee_domain::EvidenceSource::StructuredTags => (
                melodee_domain::ClassificationEvidenceKind::StructuredField,
                7_000,
                structured.contains(&evidence.trait_),
            ),
            melodee_domain::EvidenceSource::CatalogContext => (
                melodee_domain::ClassificationEvidenceKind::CatalogKnownRelease,
                6_000,
                catalog_traits.contains(&evidence.trait_),
            ),
            melodee_domain::EvidenceSource::ReleaseTitle
                if evidence.trait_ == melodee_domain::ReleaseTrait::Unknown =>
            {
                (
                    melodee_domain::ClassificationEvidenceKind::TitlePattern,
                    100,
                    !has_real_signal
                        && classification.traits
                            == std::collections::BTreeSet::from([
                                melodee_domain::ReleaseTrait::Unknown,
                            ]),
                )
            }
            melodee_domain::EvidenceSource::ReleaseTitle => (
                melodee_domain::ClassificationEvidenceKind::TitlePattern,
                4_000,
                title_traits.contains(&evidence.trait_),
            ),
            melodee_domain::EvidenceSource::DirectoryName => (
                melodee_domain::ClassificationEvidenceKind::DirectoryName,
                2_000,
                directory_traits.contains(&evidence.trait_),
            ),
            melodee_domain::EvidenceSource::ParentDirectoryName => (
                melodee_domain::ClassificationEvidenceKind::ParentDirectoryName,
                1_500,
                parent_directory_traits.contains(&evidence.trait_),
            ),
        };
        if evidence.kind != expected_kind
            || evidence.confidence.basis_points() != expected_confidence
            || !source_is_authoritative
        {
            return Err(PolicyStoreError::ForeignFacts);
        }
    }
    Ok(())
}

fn classification_title_traits(
    title: &str,
) -> std::collections::BTreeSet<melodee_domain::ReleaseTrait> {
    let words: Vec<String> = title
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let has_word =
        |candidates: &[&str]| words.iter().any(|word| candidates.contains(&word.as_str()));
    let has_phrase = |phrase: &[&str]| {
        words
            .windows(phrase.len())
            .any(|window| window.iter().map(String::as_str).eq(phrase.iter().copied()))
    };
    let mut traits = std::collections::BTreeSet::new();
    if has_phrase(&["greatest", "hit"])
        || has_phrase(&["greatest", "hits"])
        || has_phrase(&["best", "of"])
        || has_word(&["collection", "collections", "compilation", "compilations"])
    {
        traits.insert(melodee_domain::ReleaseTrait::GreatestHits);
        traits.insert(melodee_domain::ReleaseTrait::Compilation);
    }
    if has_phrase(&["box", "set"]) || has_word(&["boxset", "boxsets"]) {
        traits.insert(melodee_domain::ReleaseTrait::BoxSet);
    }
    for (words_to_match, trait_) in [
        (&["live"][..], melodee_domain::ReleaseTrait::Live),
        (
            &["remaster", "remastered", "remasters"][..],
            melodee_domain::ReleaseTrait::Remaster,
        ),
        (
            &["tribute", "tributes", "cover", "covers"][..],
            melodee_domain::ReleaseTrait::Tribute,
        ),
        (
            &["soundtrack", "soundtracks", "ost"][..],
            melodee_domain::ReleaseTrait::Soundtrack,
        ),
        (
            &["bootleg", "bootlegs"][..],
            melodee_domain::ReleaseTrait::Bootleg,
        ),
        (
            &["promo", "promos", "promotional"][..],
            melodee_domain::ReleaseTrait::Promo,
        ),
        (&["demo", "demos"][..], melodee_domain::ReleaseTrait::Demo),
        (
            &["remix", "remixes"][..],
            melodee_domain::ReleaseTrait::Remix,
        ),
        (&["comedy"][..], melodee_domain::ReleaseTrait::Comedy),
    ] {
        if has_word(words_to_match) {
            traits.insert(trait_);
        }
    }
    if has_phrase(&["cast", "recording"]) || has_word(&["soundtrack", "soundtracks", "ost"]) {
        traits.insert(melodee_domain::ReleaseTrait::CastRecording);
    }
    if has_phrase(&["spoken", "word"]) || has_word(&["audiobook", "audiobooks"]) {
        traits.insert(melodee_domain::ReleaseTrait::SpokenWord);
    }
    if has_word(&["radio"]) || has_phrase(&["radio", "show"]) {
        traits.insert(melodee_domain::ReleaseTrait::RadioShow);
    }
    if has_phrase(&["dj", "mix"]) || has_word(&["djmix"]) {
        traits.insert(melodee_domain::ReleaseTrait::DjMix);
    }
    traits
}

async fn validate_existing_evaluation_tx(
    tx: &mut Transaction<'_, Postgres>,
    classification: &ReleaseClassification,
    novelty: &NoveltyEvaluation,
    track_novelty: &[TrackNovelty],
    rule_set_id: melodee_domain::PolicyRuleSetId,
    rule_set_version: u32,
) -> Result<(), PolicyStoreError> {
    let classification_row: Option<(i32, String, Uuid, i32, serde_json::Value, serde_json::Value)> =
        sqlx::query_as(
            r#"SELECT rc.confidence, rc.classifier_version, rc.rule_set_id,
                  rc.rule_set_version,
                  COALESCE((
                      SELECT jsonb_agg(rt.trait ORDER BY rt.trait)
                        FROM release_traits rt WHERE rt.classification_id = rc.id
                  ), '[]'::jsonb),
                  COALESCE((
                      SELECT jsonb_agg(jsonb_build_array(
                                 e.trait, e.source, e.kind, e.confidence, e.detail
                             ) ORDER BY e.trait, e.source, e.kind, e.detail)
                        FROM release_classification_evidence e WHERE e.id = rc.id
                  ), '[]'::jsonb) AS details
             FROM release_classifications rc WHERE rc.release_id = $1"#,
        )
        .bind(classification.release_id.into_uuid())
        .fetch_optional(&mut **tx)
        .await?;
    let Some((confidence, classifier_version, stored_rule_id, stored_version, traits, evidence)) =
        classification_row
    else {
        return Err(PolicyStoreError::IncompleteFacts);
    };
    let expected_traits: Vec<&str> = classification
        .traits
        .iter()
        .map(|value| value.as_str())
        .collect();
    let mut expected_evidence: Vec<serde_json::Value> = classification
        .evidence
        .iter()
        .map(|value| {
            serde_json::json!([
                value.trait_.as_str(),
                value.source.as_str(),
                value.kind.as_str(),
                value.confidence.basis_points(),
                value.detail,
            ])
        })
        .collect();
    expected_evidence.sort_by_key(serde_json::Value::to_string);
    if confidence != i32::from(classification.confidence.basis_points())
        || classifier_version != classification.classifier_version
        || stored_rule_id != rule_set_id.into_uuid()
        || stored_version
            != i32::try_from(rule_set_version).map_err(|_| PolicyStoreError::NumericOverflow)?
        || traits != serde_json::to_value(expected_traits)?
        || evidence != serde_json::Value::Array(expected_evidence)
    {
        return Err(PolicyStoreError::DecisionConflict);
    }

    let novelty_row: Option<StoredNoveltyReplayRow> = sqlx::query_as(
        r#"SELECT evaluation.policy_rule_set_id,
                  evaluation.exact_duplicate_count,
                  evaluation.known_audio_identity_count,
                  evaluation.alternate_recording_count,
                  evaluation.new_track_count, evaluation.uncertain_count,
                  evaluation.total_track_count, evaluation.evaluator_version,
                  COALESCE((
                      SELECT jsonb_agg(jsonb_build_array(
                                 detail.track_id, detail.audio_identity_id,
                                 detail.media_asset_id, detail.category,
                                 detail.confidence, detail.evidence_kind, detail.detail
                             ) ORDER BY detail.track_id)
                        FROM track_novelty detail
                       WHERE detail.evaluation_id = evaluation.id
                  ), '[]'::jsonb) AS details
             FROM release_novelty_evaluations evaluation
            WHERE evaluation.release_id = $1"#,
    )
    .bind(novelty.release_id.into_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(stored) = novelty_row else {
        return Err(PolicyStoreError::IncompleteFacts);
    };
    let mut expected_details = track_novelty.to_vec();
    expected_details.sort_by_key(|value| value.track_id.into_uuid());
    let expected_details: Vec<serde_json::Value> = expected_details
        .iter()
        .map(|value| {
            serde_json::json!([
                value.track_id.into_uuid(),
                value.audio_identity_id.into_uuid(),
                value.media_asset_id.map(|id| id.into_uuid()),
                value.category.as_str(),
                value.confidence.basis_points(),
                value.evidence_kind.as_str(),
                value.detail,
            ])
        })
        .collect();
    let expected_counts = [
        novelty.exact_duplicate_count,
        novelty.known_audio_identity_count,
        novelty.alternate_recording_count,
        novelty.new_track_count,
        novelty.uncertain_count,
        novelty.total_track_count,
    ];
    let stored_counts = [
        stored.exact_duplicate_count,
        stored.known_audio_identity_count,
        stored.alternate_recording_count,
        stored.new_track_count,
        stored.uncertain_count,
        stored.total_track_count,
    ];
    if stored.policy_rule_set_id != novelty.policy_rule_set_id.into_uuid()
        || stored.evaluator_version != novelty.evaluator_version
        || stored_counts != expected_counts.map(|value| i32::try_from(value).unwrap_or(i32::MAX))
        || stored.details != serde_json::Value::Array(expected_details)
    {
        return Err(PolicyStoreError::DecisionConflict);
    }
    Ok(())
}

fn stored_decision(
    decision: &ReleasePolicyDecision,
    work_item_id: Uuid,
    classification_id: Uuid,
    novelty_evaluation_id: Uuid,
    override_reason: Option<String>,
) -> StoredPolicyDecision {
    StoredPolicyDecision {
        id: decision.id,
        classification_id,
        novelty_evaluation_id,
        work_item_id,
        release_id: decision.release_id,
        rule_set_id: decision.rule_set_id,
        rule_set_version: decision.rule_set_version,
        matched_rule_ids: decision.matched_rule_ids.clone(),
        disposition: decision.disposition,
        evidence_summary: decision.evidence_summary.clone(),
        source_fingerprint: decision.source_fingerprint.clone(),
        actor: decision.actor.clone(),
        decided_at: decision.decided_at,
        completion_state: decision.completion_state,
        completed_at: decision.completed_at,
        override_reason,
    }
}

async fn insert_decision_tx(
    tx: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
    classification_id: Uuid,
    novelty_evaluation_id: Uuid,
    decision: &ReleasePolicyDecision,
) -> Result<(), PolicyStoreError> {
    sqlx::query(
        r#"INSERT INTO policy_decisions
            (id, classification_id, novelty_evaluation_id, work_item_id,
             release_id, rule_set_id, rule_set_version,
             matched_rule_ids, disposition, evidence_summary, source_fingerprint,
             actor, decided_at, completion_state, completed_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
    )
    .bind(decision.id.into_uuid())
    .bind(classification_id)
    .bind(novelty_evaluation_id)
    .bind(work_item_id)
    .bind(decision.release_id.into_uuid())
    .bind(decision.rule_set_id.into_uuid())
    .bind(i32::try_from(decision.rule_set_version).map_err(|_| PolicyStoreError::NumericOverflow)?)
    .bind(sqlx::types::Json(&decision.matched_rule_ids))
    .bind(decision.disposition.as_str())
    .bind(&decision.evidence_summary)
    .bind(&decision.source_fingerprint)
    .bind(serde_json::to_value(&decision.actor)?)
    .bind(decision.decided_at)
    .bind(decision.completion_state.as_str())
    .bind(decision.completed_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn load_decision_tx(
    tx: &mut Transaction<'_, Postgres>,
    release_id: ReleaseId,
) -> Result<Option<StoredPolicyDecision>, PolicyStoreError> {
    let row: Option<StoredPolicyDecisionRow> = sqlx::query_as(
        r#"SELECT id, classification_id, novelty_evaluation_id, work_item_id,
                  release_id, rule_set_id, rule_set_version,
                  matched_rule_ids, disposition, evidence_summary,
                  source_fingerprint, actor, decided_at, completion_state,
                  completed_at, override_reason
             FROM policy_decisions WHERE release_id = $1 FOR UPDATE"#,
    )
    .bind(release_id.into_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(TryInto::try_into).transpose()
}

async fn load_decision_by_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    decision_id: melodee_domain::ReleasePolicyDecisionId,
) -> Result<Option<StoredPolicyDecision>, PolicyStoreError> {
    let row: Option<StoredPolicyDecisionRow> = sqlx::query_as(
        r#"SELECT id, classification_id, novelty_evaluation_id, work_item_id,
                  release_id, rule_set_id, rule_set_version,
                  matched_rule_ids, disposition, evidence_summary,
                  source_fingerprint, actor, decided_at, completion_state,
                  completed_at, override_reason
             FROM policy_decisions WHERE id = $1 FOR UPDATE"#,
    )
    .bind(decision_id.into_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(TryInto::try_into).transpose()
}

async fn verify_current_fingerprint_tx(
    tx: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
    expected: &str,
) -> Result<(), PolicyStoreError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT sealed_manifest_hash FROM catalog_imports WHERE work_item_id = $1")
            .bind(work_item_id)
            .fetch_optional(&mut **tx)
            .await?;
    if row.is_none_or(|row| row.0 != expected) {
        return Err(PolicyStoreError::FingerprintMismatch);
    }
    Ok(())
}

async fn route_operator_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    item: &ReleaseWorkItem,
    to: IngestionState,
    now: OffsetDateTime,
) -> Result<WorkItemRow, PolicyStoreError> {
    if to != item.state {
        item.state
            .ensure_transition(to, TransitionKind::Operator)
            .map_err(WorkStoreError::from)?;
    }
    let completed_at = to.is_terminal().then_some(now);
    let row: WorkItemRow = sqlx::query_as(
        r#"UPDATE release_work_items
              SET state = $2, state_reason = $3, completed_at = $4,
                  lease_owner = NULL, lease_acquired_at = NULL,
                  lease_expires_at = NULL, updated_at = $5
            WHERE id = $1 AND state = 'needs_review'
            RETURNING *"#,
    )
    .bind(item.id)
    .bind(to.as_str())
    .bind("policy.admin_override")
    .bind(completed_at)
    .bind(now)
    .fetch_one(&mut **tx)
    .await?;
    if to != item.state {
        insert_event(
            tx,
            item.id,
            WorkEventType::Transitioned,
            Some(item.state),
            Some(to),
            Some("policy.admin_override"),
            None,
            item.correlation_id,
            now,
            None,
        )
        .await?;
    }
    Ok(row)
}

/// Derived novelty signals for one Track, recomputed at commit time so a caller
/// cannot submit a conclusion the catalogue facts do not support.
#[derive(Debug, sqlx::FromRow)]
struct NoveltyRevalidationRow {
    track_id: Uuid,
    audio_identity_id: Uuid,
    /// The Track's playable asset; a Track that reached novelty evaluation always
    /// has one (the decoder rejects NULL exactly as before).
    media_asset_id: Uuid,
    exact_duplicate: bool,
    acoustic_match: bool,
    known_identity: bool,
    alternate_recording: bool,
    identity_confirmed: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct CatalogTrackFactRow {
    release_id: Uuid,
    track_id: Uuid,
    audio_identity_id: Uuid,
    media_asset_id: Option<Uuid>,
    byte_hash: Option<String>,
    audio_fingerprint: Option<String>,
    has_confirmed_audio_identity: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredNoveltyReplayRow {
    policy_rule_set_id: Uuid,
    exact_duplicate_count: i32,
    known_audio_identity_count: i32,
    alternate_recording_count: i32,
    new_track_count: i32,
    uncertain_count: i32,
    total_track_count: i32,
    evaluator_version: String,
    details: serde_json::Value,
}

#[derive(Debug, sqlx::FromRow)]
struct DryRunContextRow {
    #[sqlx(flatten)]
    work_item: WorkItemRow,
    release_id: Uuid,
    title: String,
    sealed_manifest_hash: String,
    semantic_manifest_hash: String,
    manifest_payload: serde_json::Value,
    has_corrupt_media: bool,
    tracks: serde_json::Value,
    catalog_traits: Vec<String>,
    existing_disposition: Option<String>,
    exact_hashes: Vec<String>,
    known_audio_ids: Vec<Uuid>,
    alternate_audio_ids: Vec<Uuid>,
    acoustic_fingerprints: Vec<String>,
}

impl TryFrom<DryRunContextRow> for PolicyCatalogContext {
    type Error = PolicyStoreError;

    fn try_from(row: DryRunContextRow) -> Result<Self, Self::Error> {
        let item = ReleaseWorkItem::try_from(row.work_item)?;
        let tracks: Vec<CatalogTrackFact> = serde_json::from_value(row.tracks)?;
        if tracks.is_empty() {
            return Err(PolicyStoreError::IncompleteFacts);
        }
        if tracks.len() > MAX_POLICY_TRACKS_PER_RELEASE {
            return Err(PolicyStoreError::ScopeExceeded {
                dimension: "Tracks per Release",
                limit: MAX_POLICY_TRACKS_PER_RELEASE,
            });
        }
        let mut traits = Vec::with_capacity(row.catalog_traits.len());
        for value in row.catalog_traits {
            traits.push(
                melodee_domain::ReleaseTrait::parse(&value)
                    .ok_or(PolicyStoreError::CorruptValue(value))?,
            );
        }
        let existing_disposition = row
            .existing_disposition
            .map(|value| Disposition::parse(&value).ok_or(PolicyStoreError::CorruptValue(value)))
            .transpose()?;
        let (structured_tag_traits, embedded_external_id_traits) =
            extract_manifest_traits(&row.manifest_payload);
        Ok(Self {
            claim: PolicyClaim {
                item,
                release_id: ReleaseId::from_uuid(row.release_id),
                title: row.title,
                source_fingerprint: row.sealed_manifest_hash,
                semantic_manifest_hash: row.semantic_manifest_hash,
            },
            tracks,
            current_classification: None,
            current_decision: None,
            structured_tag_traits,
            embedded_external_id_traits,
            catalog_known_release_traits: traits,
            existing_disposition,
            has_corrupt_media: row.has_corrupt_media,
            catalog_exact_byte_hashes: row.exact_hashes.into_iter().collect(),
            catalog_acoustic_fingerprints: row.acoustic_fingerprints.into_iter().collect(),
            catalog_known_audio_identities: row
                .known_audio_ids
                .into_iter()
                .map(melodee_domain::AudioIdentityId::from_uuid)
                .collect(),
            catalog_alternate_audio_identities: row
                .alternate_audio_ids
                .into_iter()
                .map(melodee_domain::AudioIdentityId::from_uuid)
                .collect(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct StoredReleaseClassification {
    pub id: melodee_domain::ReleaseClassificationId,
    pub release_id: melodee_domain::ReleaseId,
    pub rule_set_id: melodee_domain::PolicyRuleSetId,
    pub rule_set_version: u32,
    pub traits: std::collections::BTreeSet<melodee_domain::ReleaseTrait>,
    pub evidence: Vec<melodee_domain::ClassificationEvidence>,
    pub confidence: melodee_domain::Confidence,
    pub classified_at: OffsetDateTime,
    pub classifier_version: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredClassificationRow {
    id: Uuid,
    release_id: Uuid,
    rule_set_id: Uuid,
    rule_set_version: i32,
    confidence: i32,
    classifier_version: String,
    classified_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredEvidenceRow {
    trait_: String,
    source: String,
    kind: String,
    confidence: i32,
    detail: String,
}

/// Persisted per-Track novelty row (operator review read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTrackNovelty {
    pub track_id: melodee_domain::TrackId,
    pub category: melodee_domain::TrackNoveltyCategory,
    pub confidence: melodee_domain::Confidence,
    pub evidence_kind: melodee_domain::NoveltyEvidenceKind,
    pub detail: String,
}

/// Persisted aggregate novelty evaluation plus its per-Track rows for operator
/// review. Counts are the durable evaluation facts, not a
/// recomputation.
#[derive(Debug, Clone)]
pub struct StoredNoveltyEvaluation {
    pub id: Uuid,
    pub release_id: melodee_domain::ReleaseId,
    pub exact_duplicate_count: u32,
    pub known_audio_identity_count: u32,
    pub alternate_recording_count: u32,
    pub new_track_count: u32,
    pub uncertain_count: u32,
    pub total_track_count: u32,
    pub evaluated_at: OffsetDateTime,
    pub evaluator_version: String,
    pub tracks: Vec<StoredTrackNovelty>,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredNoveltyEvaluationRow {
    id: Uuid,
    release_id: Uuid,
    exact_duplicate_count: i32,
    known_audio_identity_count: i32,
    alternate_recording_count: i32,
    new_track_count: i32,
    uncertain_count: i32,
    total_track_count: i32,
    evaluated_at: OffsetDateTime,
    evaluator_version: String,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredTrackNoveltyRow {
    track_id: Uuid,
    category: String,
    confidence: i32,
    evidence_kind: String,
    detail: String,
}

impl TryFrom<StoredTrackNoveltyRow> for StoredTrackNovelty {
    type Error = PolicyStoreError;

    fn try_from(row: StoredTrackNoveltyRow) -> Result<Self, Self::Error> {
        Ok(Self {
            track_id: melodee_domain::TrackId::from_uuid(row.track_id),
            category: melodee_domain::TrackNoveltyCategory::parse(&row.category)
                .ok_or(PolicyStoreError::CorruptValue(row.category))?,
            confidence: melodee_domain::Confidence::new(
                u16::try_from(row.confidence).map_err(|_| PolicyStoreError::NumericOverflow)?,
            )?,
            evidence_kind: melodee_domain::NoveltyEvidenceKind::parse(&row.evidence_kind)
                .ok_or(PolicyStoreError::CorruptValue(row.evidence_kind))?,
            detail: row.detail,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StoredPolicyDecision {
    pub id: melodee_domain::ReleasePolicyDecisionId,
    pub classification_id: Uuid,
    pub novelty_evaluation_id: Uuid,
    pub work_item_id: Uuid,
    pub release_id: melodee_domain::ReleaseId,
    pub rule_set_id: melodee_domain::PolicyRuleSetId,
    pub rule_set_version: u32,
    pub matched_rule_ids: Vec<String>,
    pub disposition: Disposition,
    pub evidence_summary: String,
    pub source_fingerprint: String,
    pub actor: PolicyActor,
    pub decided_at: OffsetDateTime,
    pub completion_state: DecisionCompletionState,
    pub completed_at: Option<OffsetDateTime>,
    pub override_reason: Option<String>,
}

impl std::convert::TryFrom<StoredPolicyDecisionRow> for StoredPolicyDecision {
    type Error = PolicyStoreError;

    fn try_from(row: StoredPolicyDecisionRow) -> Result<Self, Self::Error> {
        let actor: PolicyActor = serde_json::from_value(row.actor)?;
        Ok(Self {
            id: melodee_domain::ReleasePolicyDecisionId::from_uuid(row.id),
            classification_id: row.classification_id,
            novelty_evaluation_id: row.novelty_evaluation_id,
            work_item_id: row.work_item_id,
            release_id: melodee_domain::ReleaseId::from_uuid(row.release_id),
            rule_set_id: melodee_domain::PolicyRuleSetId::from_uuid(row.rule_set_id),
            rule_set_version: u32::try_from(row.rule_set_version)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            matched_rule_ids: row.matched_rule_ids.0,
            disposition: Disposition::parse(&row.disposition)
                .ok_or_else(|| PolicyStoreError::CorruptValue(row.disposition))?,
            evidence_summary: row.evidence_summary,
            source_fingerprint: row.source_fingerprint,
            actor,
            decided_at: row.decided_at,
            completion_state: DecisionCompletionState::parse(&row.completion_state)
                .ok_or_else(|| PolicyStoreError::CorruptValue(row.completion_state))?,
            completed_at: row.completed_at,
            override_reason: row.override_reason,
        })
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredPolicyDecisionRow {
    id: Uuid,
    classification_id: Uuid,
    novelty_evaluation_id: Uuid,
    work_item_id: Uuid,
    release_id: Uuid,
    rule_set_id: Uuid,
    rule_set_version: i32,
    matched_rule_ids: sqlx::types::Json<Vec<String>>,
    disposition: String,
    evidence_summary: String,
    source_fingerprint: String,
    actor: serde_json::Value,
    decided_at: OffsetDateTime,
    completion_state: String,
    completed_at: Option<OffsetDateTime>,
    override_reason: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PolicyDecisionAuditRow {
    id: Uuid,
    classification_id: Uuid,
    novelty_evaluation_id: Uuid,
    work_item_id: Uuid,
    release_id: Uuid,
    rule_set_id: Uuid,
    rule_set_version: i32,
    matched_rule_ids: sqlx::types::Json<Vec<String>>,
    disposition: String,
    evidence_summary: String,
    source_fingerprint: String,
    actor: serde_json::Value,
    decided_at: OffsetDateTime,
    completion_state: String,
    completed_at: Option<OffsetDateTime>,
    override_reason: Option<String>,
    event: String,
    created_at: OffsetDateTime,
}

impl TryFrom<PolicyDecisionAuditRow> for PolicyDecisionAuditEntry {
    type Error = PolicyStoreError;

    fn try_from(row: PolicyDecisionAuditRow) -> Result<Self, Self::Error> {
        let decision = StoredPolicyDecisionRow {
            id: row.id,
            classification_id: row.classification_id,
            novelty_evaluation_id: row.novelty_evaluation_id,
            work_item_id: row.work_item_id,
            release_id: row.release_id,
            rule_set_id: row.rule_set_id,
            rule_set_version: row.rule_set_version,
            matched_rule_ids: row.matched_rule_ids,
            disposition: row.disposition,
            evidence_summary: row.evidence_summary,
            source_fingerprint: row.source_fingerprint,
            actor: row.actor,
            decided_at: row.decided_at,
            completion_state: row.completion_state,
            completed_at: row.completed_at,
            override_reason: row.override_reason,
        }
        .try_into()?;
        Ok(Self {
            decision,
            event: row.event,
            created_at: row.created_at,
        })
    }
}

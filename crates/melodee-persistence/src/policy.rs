//! Phase 5 PostgreSQL persistence for classification, novelty, policy
//! rules, durable decisions, and dry-run results (ADR-0120).
//!
//! This crate never implements classification, novelty, or policy logic —
//! it persists and retrieves the durable facts those engines produce.

use melodee_domain::{
    DecisionCompletionState, Disposition, NoveltyEvaluation, PolicyActor, PolicyRuleSet,
    ReleaseClassification, ReleasePolicyDecision, TrackNovelty,
};
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use tracing::instrument;
use uuid::Uuid;

use crate::error::WorkStoreError;

/// Maximum sample Release IDs returned by one dry-run (operator-friendly).
pub const MAX_POLICY_TEST_SAMPLES: usize = 20;

/// Typed Phase 5 persistence failures.
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
}

/// One Track's catalog presence used for novelty assessment.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogTrackFact {
    pub track_id: Uuid,
    pub audio_identity_id: Uuid,
    pub media_asset_id: Option<Uuid>,
    pub byte_hash: Option<String>,
    pub has_confirmed_audio_identity: bool,
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
        let rules_json = serde_json::to_value(&rule_set.rules)?;
        let actor_json = serde_json::to_value(&rule_set.created_by)?;
        sqlx::query(
            r#"INSERT INTO policy_rule_sets
                (id, name, version, rules, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (name, version) DO NOTHING"#,
        )
        .bind(rule_set.id.into_uuid())
        .bind(&rule_set.name)
        .bind(i64::from(rule_set.version))
        .bind(rules_json)
        .bind(actor_json)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Loads the latest (highest version) rule set by name.
    #[instrument(skip_all)]
    pub async fn load_rule_set(
        &self,
        pool: &PgPool,
        name: &str,
    ) -> Result<Option<PolicyRuleSet>, PolicyStoreError> {
        let row: Option<(
            Uuid,
            String,
            i64,
            serde_json::Value,
            serde_json::Value,
            OffsetDateTime,
        )> = sqlx::query_as(
            r#"SELECT id, name, version, rules, created_by, created_at
                   FROM policy_rule_sets
                  WHERE name = $1
                  ORDER BY version DESC
                  LIMIT 1"#,
        )
        .bind(name)
        .fetch_optional(pool)
        .await?;

        let Some((id, name, version, rules_json, actor_json, created_at)) = row else {
            return Ok(None);
        };
        let rules: Vec<melodee_domain::PolicyRule> = serde_json::from_value(rules_json)?;
        let created_by: PolicyActor = serde_json::from_value(actor_json)?;
        Ok(Some(PolicyRuleSet {
            id: melodee_domain::PolicyRuleSetId::from_uuid(id),
            name,
            version: u32::try_from(version).map_err(|_| PolicyStoreError::NumericOverflow)?,
            rules,
            created_by,
            created_at,
        }))
    }

    /// Loads a rule set by its UUID.
    #[instrument(skip_all)]
    pub async fn load_rule_set_by_id(
        &self,
        pool: &PgPool,
        id: melodee_domain::PolicyRuleSetId,
    ) -> Result<Option<PolicyRuleSet>, PolicyStoreError> {
        let row: Option<(
            Uuid,
            String,
            i64,
            serde_json::Value,
            serde_json::Value,
            OffsetDateTime,
        )> = sqlx::query_as(
            r#"SELECT id, name, version, rules, created_by, created_at
               FROM policy_rule_sets
              WHERE id = $1"#,
        )
        .bind(id.into_uuid())
        .fetch_optional(pool)
        .await?;

        let Some((id, name, version, rules_json, actor_json, created_at)) = row else {
            return Ok(None);
        };
        let rules: Vec<melodee_domain::PolicyRule> = serde_json::from_value(rules_json)?;
        let created_by: PolicyActor = serde_json::from_value(actor_json)?;
        Ok(Some(PolicyRuleSet {
            id: melodee_domain::PolicyRuleSetId::from_uuid(id),
            name,
            version: u32::try_from(version).map_err(|_| PolicyStoreError::NumericOverflow)?,
            rules,
            created_by,
            created_at,
        }))
    }

    // ------------------------------------------------------------------
    // Classification storage
    // ------------------------------------------------------------------

    /// Persists a Release classification with its traits and evidence in one
    /// transaction.
    #[instrument(skip_all, fields(release_id = %classification.release_id))]
    pub async fn store_classification(
        &self,
        pool: &PgPool,
        classification: &ReleaseClassification,
    ) -> Result<(), PolicyStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            r#"INSERT INTO release_classifications
                (id, release_id, confidence, classifier_version, classified_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(classification.id.into_uuid())
        .bind(classification.release_id.into_uuid())
        .bind(i64::from(classification.confidence.basis_points()))
        .bind(&classification.classifier_version)
        .bind(classification.classified_at)
        .bind(classification.classified_at)
        .execute(&mut *tx)
        .await?;

        for trait_value in &classification.traits {
            sqlx::query(
                r#"INSERT INTO release_traits
                    (classification_id, trait, created_at)
                 VALUES ($1, $2, $3)"#,
            )
            .bind(classification.id.into_uuid())
            .bind(trait_value.as_str())
            .bind(classification.classified_at)
            .execute(&mut *tx)
            .await?;
        }

        for evidence in &classification.evidence {
            sqlx::query(
                r#"INSERT INTO release_classification_evidence
                    (id, trait, source, kind, confidence, detail, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            )
            .bind(evidence.id.into_uuid())
            .bind(evidence.trait_.as_str())
            .bind(evidence.source.as_str())
            .bind(evidence.kind.as_str())
            .bind(i64::from(evidence.confidence.basis_points()))
            .bind(&evidence.detail)
            .bind(classification.classified_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Novelty storage
    // ------------------------------------------------------------------

    /// Persists per-Track novelty assessments and the aggregate evaluation.
    #[instrument(skip_all, fields(release_id = %evaluation.release_id))]
    pub async fn store_novelty(
        &self,
        pool: &PgPool,
        evaluation: &NoveltyEvaluation,
        track_novelty: &[TrackNovelty],
    ) -> Result<(), PolicyStoreError> {
        let mut tx = pool.begin().await?;

        sqlx::query(
            r#"INSERT INTO release_novelty_evaluations
                (id, release_id, exact_duplicate_count, known_audio_identity_count,
                 alternate_recording_count, new_track_count, uncertain_count,
                 total_track_count, evaluated_at, evaluator_version, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        )
        .bind(Uuid::now_v7())
        .bind(evaluation.release_id.into_uuid())
        .bind(i64::from(evaluation.exact_duplicate_count))
        .bind(i64::from(evaluation.known_audio_identity_count))
        .bind(i64::from(evaluation.alternate_recording_count))
        .bind(i64::from(evaluation.new_track_count))
        .bind(i64::from(evaluation.uncertain_count))
        .bind(i64::from(evaluation.total_track_count))
        .bind(evaluation.evaluated_at)
        .bind(&evaluation.evaluator_version)
        .bind(evaluation.evaluated_at)
        .execute(&mut *tx)
        .await?;

        for novelty in track_novelty {
            sqlx::query(
                r#"INSERT INTO track_novelty
                    (id, release_id, track_id, audio_identity_id, media_asset_id,
                     category, confidence, evidence_kind, detail, evaluated_at, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
            )
            .bind(novelty.id.into_uuid())
            .bind(novelty.release_id.into_uuid())
            .bind(novelty.track_id.into_uuid())
            .bind(novelty.audio_identity_id.into_uuid())
            .bind(novelty.media_asset_id.map(|m| m.into_uuid()))
            .bind(novelty.category.as_str())
            .bind(i64::from(novelty.confidence.basis_points()))
            .bind(novelty.evidence_kind.as_str())
            .bind(&novelty.detail)
            .bind(novelty.evaluated_at)
            .bind(novelty.evaluated_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Policy decisions (durable, before destructive action)
    // ------------------------------------------------------------------

    /// Records a durable policy decision (PRD-POLICY-007). Must be committed
    /// before any destructive action is executed.
    #[instrument(skip_all, fields(release_id = %decision.release_id))]
    pub async fn record_decision(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        decision: &ReleasePolicyDecision,
    ) -> Result<(), PolicyStoreError> {
        let matched_ids = serde_json::to_value(&decision.matched_rule_ids)?;
        let actor_json = serde_json::to_value(&decision.actor)?;

        sqlx::query(
            r#"INSERT INTO policy_decisions
                (id, release_id, rule_set_id, rule_set_version, matched_rule_ids,
                 disposition, evidence_summary, source_fingerprint, actor,
                 decided_at, completion_state, completed_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        )
        .bind(decision.id.into_uuid())
        .bind(decision.release_id.into_uuid())
        .bind(decision.rule_set_id.into_uuid())
        .bind(i64::from(decision.rule_set_version))
        .bind(matched_ids.clone())
        .bind(decision.disposition.as_str())
        .bind(&decision.evidence_summary)
        .bind(&decision.source_fingerprint)
        .bind(actor_json.clone())
        .bind(decision.decided_at)
        .bind(decision.completion_state.as_str())
        .bind(decision.completed_at)
        .execute(&mut **tx)
        .await?;

        sqlx::query(
            r#"INSERT INTO policy_decision_audit
                (id, release_id, rule_set_id, rule_set_version, matched_rule_ids,
                 disposition, evidence_summary, source_fingerprint, actor,
                 decided_at, completion_state, completed_at, event, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)"#,
        )
        .bind(decision.id.into_uuid())
        .bind(decision.release_id.into_uuid())
        .bind(decision.rule_set_id.into_uuid())
        .bind(i64::from(decision.rule_set_version))
        .bind(matched_ids)
        .bind(decision.disposition.as_str())
        .bind(&decision.evidence_summary)
        .bind(&decision.source_fingerprint)
        .bind(actor_json)
        .bind(decision.decided_at)
        .bind(decision.completion_state.as_str())
        .bind(decision.completed_at)
        .bind("created")
        .bind(decision.decided_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Loads an existing policy decision for a Release, if any.
    #[instrument(skip_all)]
    pub async fn get_decision(
        &self,
        pool: &PgPool,
        release_id: melodee_domain::ReleaseId,
    ) -> Result<Option<StoredPolicyDecision>, PolicyStoreError> {
        let row: Option<StoredPolicyDecisionRow> = sqlx::query_as(
            r#"SELECT id, release_id, rule_set_id, rule_set_version, matched_rule_ids,
                   disposition, evidence_summary, source_fingerprint, actor,
                   decided_at, completion_state, completed_at
              FROM policy_decisions
             WHERE release_id = $1"#,
        )
        .bind(release_id.into_uuid())
        .fetch_optional(pool)
        .await?;

        row.map(|r| r.try_into()).transpose()
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
        let disposition_counts = serde_json::to_value(
            summary
                .disposition_counts
                .iter()
                .map(|(d, c)| (d.as_str(), c))
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
                 disposition_counts, destructive_impact_count, sample_release_ids,
                 ran_by, ran_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(summary.test_run_id.into_uuid())
        .bind(summary.rule_set_id.into_uuid())
        .bind(i64::from(summary.rule_set_version))
        .bind(i64::from(summary.total_releases_evaluated))
        .bind(disposition_counts)
        .bind(i64::from(summary.destructive_impact_count))
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
            r#"SELECT t.id as track_id, t.audio_identity_id,
                   t.playable_media_asset_id as media_asset_id,
                   m.byte_hash, EXISTS (
                       SELECT 1 FROM audio_identity_match_decisions d
                        WHERE d.source_audio_identity_id = t.audio_identity_id
                          AND d.outcome = 'confirmed'
                   ) as has_confirmed_audio_identity
              FROM tracks t
              LEFT JOIN media_assets m ON m.id = t.playable_media_asset_id
             WHERE t.release_id = $1
             ORDER BY t.ordinal"#,
        )
        .bind(release_id.into_uuid())
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| CatalogTrackFact {
                track_id: row.track_id,
                audio_identity_id: row.audio_identity_id,
                media_asset_id: row.media_asset_id,
                byte_hash: row.byte_hash,
                has_confirmed_audio_identity: row.has_confirmed_audio_identity,
            })
            .collect())
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
            r#"SELECT id, release_id, confidence, classifier_version, classified_at
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
            r#"SELECT trait, source, kind, confidence, detail
               FROM release_classification_evidence
              WHERE id = $1"#,
        )
        .bind(row.id)
        .fetch_all(pool)
        .await?;

        let mut traits = std::collections::BTreeSet::new();
        for (trait_str,) in trait_rows {
            if let Some(trait_) = melodee_domain::ReleaseTrait::parse(&trait_str) {
                traits.insert(trait_);
            }
        }

        let evidence: Vec<melodee_domain::ClassificationEvidence> = evidence_rows
            .into_iter()
            .filter_map(|e| {
                let trait_ = melodee_domain::ReleaseTrait::parse(&e.trait_)?;
                let source = melodee_domain::EvidenceSource::parse(&e.source)?;
                let kind = melodee_domain::ClassificationEvidenceKind::parse(&e.kind)?;
                let confidence =
                    melodee_domain::Confidence::new(u16::try_from(e.confidence).ok()?).ok()?;
                Some(melodee_domain::ClassificationEvidence {
                    id: melodee_domain::ReleaseClassificationId::from_uuid(row.id),
                    release_id: melodee_domain::ReleaseId::from_uuid(row.release_id),
                    trait_,
                    source,
                    kind,
                    confidence,
                    detail: e.detail,
                })
            })
            .collect();

        Ok(Some(StoredReleaseClassification {
            id: melodee_domain::ReleaseClassificationId::from_uuid(row.id),
            release_id: melodee_domain::ReleaseId::from_uuid(row.release_id),
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
}

// ------------------------------------------------------------------
// Stored row types
// ------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct CatalogTrackFactRow {
    track_id: Uuid,
    audio_identity_id: Uuid,
    media_asset_id: Option<Uuid>,
    byte_hash: Option<String>,
    has_confirmed_audio_identity: bool,
}

#[derive(Debug, Clone)]
pub struct StoredReleaseClassification {
    pub id: melodee_domain::ReleaseClassificationId,
    pub release_id: melodee_domain::ReleaseId,
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
    confidence: i64,
    classifier_version: String,
    classified_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredEvidenceRow {
    trait_: String,
    source: String,
    kind: String,
    confidence: i64,
    detail: String,
}

#[derive(Debug, Clone)]
pub struct StoredPolicyDecision {
    pub id: melodee_domain::ReleasePolicyDecisionId,
    pub release_id: melodee_domain::ReleaseId,
    pub rule_set_id: melodee_domain::PolicyRuleSetId,
    pub rule_set_version: u32,
    pub matched_rule_ids: Vec<String>,
    pub disposition: Disposition,
    pub evidence_summary: String,
    pub source_fingerprint: Option<String>,
    pub actor: PolicyActor,
    pub decided_at: OffsetDateTime,
    pub completion_state: DecisionCompletionState,
    pub completed_at: Option<OffsetDateTime>,
}

impl std::convert::TryFrom<StoredPolicyDecisionRow> for StoredPolicyDecision {
    type Error = PolicyStoreError;

    fn try_from(row: StoredPolicyDecisionRow) -> Result<Self, Self::Error> {
        let actor: PolicyActor = serde_json::from_value(row.actor)?;
        Ok(Self {
            id: melodee_domain::ReleasePolicyDecisionId::from_uuid(row.id),
            release_id: melodee_domain::ReleaseId::from_uuid(row.release_id),
            rule_set_id: melodee_domain::PolicyRuleSetId::from_uuid(row.rule_set_id),
            rule_set_version: u32::try_from(row.rule_set_version)
                .map_err(|_| PolicyStoreError::NumericOverflow)?,
            matched_rule_ids: row.matched_rule_ids,
            disposition: Disposition::parse(&row.disposition)
                .ok_or_else(|| PolicyStoreError::CorruptValue(row.disposition))?,
            evidence_summary: row.evidence_summary,
            source_fingerprint: row.source_fingerprint,
            actor,
            decided_at: row.decided_at,
            completion_state: DecisionCompletionState::parse(&row.completion_state)
                .ok_or_else(|| PolicyStoreError::CorruptValue(row.completion_state))?,
            completed_at: row.completed_at,
        })
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredPolicyDecisionRow {
    id: Uuid,
    release_id: Uuid,
    rule_set_id: Uuid,
    rule_set_version: i64,
    matched_rule_ids: Vec<String>,
    disposition: String,
    evidence_summary: String,
    source_fingerprint: Option<String>,
    actor: serde_json::Value,
    decided_at: OffsetDateTime,
    completion_state: String,
    completed_at: Option<OffsetDateTime>,
}

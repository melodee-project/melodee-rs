//! Audited Artist and AudioIdentity correction operations.

use melodee_domain::{
    Artist, ArtistId, ArtistMerge, ArtistSplit, ArtistStatus, AudioIdentityEvidence,
    AudioIdentityEvidenceKind, AudioIdentityMatchDecision, AudioIdentityMatchOutcome,
    AudioIdentityMerge, AudioIdentitySplit, DecisionAuthority, EvidencePolarity, SplitAssignment,
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::catalog_mutations::{
    append_overlay_revisions, audit_artist_change, audit_event, ensure_correction_scope,
    insert_artist, rebuild_participation_for_tracks,
};
use crate::{
    ArtistReconciliationEvidence, CatalogCorrectionDimension, CatalogStore, CatalogStoreError,
    MAX_CATALOG_CORRECTION_IDENTITY_ROWS, MAX_CATALOG_CORRECTION_RELEASES,
    MAX_CATALOG_CORRECTION_TRACKS, MAX_CATALOG_SPLIT_ASSIGNMENTS,
};

/// Maximum number of source Artists accepted by one administrative merge.
pub const MAX_ARTIST_MERGE_SOURCES: usize = 100;

/// One resolvable Artist displayed in a merge impact preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistMergePreviewArtist {
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub sort_name: String,
    pub artist_type: String,
    pub disambiguation: Option<String>,
    pub begin_date: Option<time::Date>,
    pub end_date: Option<time::Date>,
    pub status: String,
}

/// Bounded counts of rows and projections affected by an Artist merge.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ArtistMergeImpact {
    pub releases: i64,
    pub tracks: i64,
    pub release_credits: i64,
    pub track_credits: i64,
    pub contributions: i64,
    pub aliases: i64,
    pub external_identities: i64,
    pub relationships: i64,
    pub memberships: i64,
    pub observations: i64,
    pub user_states: i64,
    pub user_state_conflicts: i64,
    pub chart_links: i64,
    pub operator_images: i64,
    pub provider_images: i64,
    pub provider_image_conflicts: i64,
    pub provider_candidates: i64,
    pub provider_tasks: i64,
}

/// Server-authoritative impact and scalar conflict preview for a merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistMergePreview {
    pub target: ArtistMergePreviewArtist,
    pub sources: Vec<ArtistMergePreviewArtist>,
    pub impact: ArtistMergeImpact,
    /// Target-owned scalar fields whose source values differ.
    pub metadata_conflicts: Vec<String>,
}

type ArtistMergePreviewRow = (
    Uuid,
    Uuid,
    String,
    String,
    String,
    Option<String>,
    Option<time::Date>,
    Option<time::Date>,
    String,
);

#[derive(Clone, Copy)]
enum ArtistMergeDecisionAuthority<'a> {
    Admin,
    Automated(&'a ArtistReconciliationEvidence),
}

impl<'a> ArtistMergeDecisionAuthority<'a> {
    const fn database_values(self) -> (&'static str, i32, bool) {
        match self {
            Self::Admin => ("admin", 10_000, true),
            Self::Automated(_) => ("automated", 9_000, false),
        }
    }

    const fn predicate(self) -> &'static str {
        match self {
            Self::Admin => "administrative_merge",
            Self::Automated(evidence) => evidence.predicate(),
        }
    }

    fn audit_payload(self) -> serde_json::Value {
        match self {
            Self::Admin => serde_json::json!({
                "authority": "admin",
                "predicate": self.predicate(),
            }),
            Self::Automated(evidence) => serde_json::json!({
                "authority": "automated",
                "evidence": evidence.payload(),
            }),
        }
    }

    const fn automated_evidence(self) -> Option<&'a ArtistReconciliationEvidence> {
        match self {
            Self::Admin => None,
            Self::Automated(evidence) => Some(evidence),
        }
    }
}

struct ArtistObservationRetarget<'a> {
    expected_artist_id: ArtistId,
    artist_id: ArtistId,
    actor: &'a str,
    reason: &'a str,
    now: OffsetDateTime,
    decision_authority: ArtistMergeDecisionAuthority<'a>,
}

impl CatalogStore {
    /// Appends immutable AudioIdentity comparison evidence.
    pub async fn record_audio_identity_evidence(
        &self,
        pool: &PgPool,
        evidence: &AudioIdentityEvidence,
        payload: serde_json::Value,
        created_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        AudioIdentityEvidence::new(evidence.clone())?;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO audio_identity_evidence (
                id, source_audio_identity_id, candidate_audio_identity_id,
                track_id, evidence_kind, polarity, confidence, source,
                payload, created_at
             ) VALUES ($1,$2,$3,NULL,$4,$5,$6,$7,$8,$9)",
        )
        .bind(evidence.id.into_uuid())
        .bind(evidence.source_audio_identity_id.into_uuid())
        .bind(evidence.candidate_audio_identity_id.into_uuid())
        .bind(audio_evidence_kind_name(evidence.kind))
        .bind(polarity_name(evidence.polarity))
        .bind(i32::from(evidence.confidence.basis_points()))
        .bind(&evidence.source)
        .bind(payload)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        audit_event(
            &mut tx,
            "audio_identity",
            evidence.source_audio_identity_id.into_uuid(),
            "audio_identity_evidence_recorded",
            &evidence.source,
            "immutable AudioIdentity evidence recorded",
            serde_json::json!({
                "evidence_id": evidence.id,
                "candidate_audio_identity_id": evidence.candidate_audio_identity_id,
            }),
            created_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Appends an AudioIdentity decision while honoring locked administration.
    pub async fn record_audio_identity_match_decision(
        &self,
        pool: &PgPool,
        decision: &AudioIdentityMatchDecision,
        payload: serde_json::Value,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;
        // Stable identity rows serialize concurrent first decisions even when
        // no current/head row exists yet. UUID order also matches merge locks.
        let locked_identities = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM audio_identities
             WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind([
            decision.source_audio_identity_id().into_uuid(),
            decision.candidate_audio_identity_id().into_uuid(),
        ])
        .fetch_all(&mut *tx)
        .await?;
        if locked_identities.len() != 2 {
            return Err(CatalogStoreError::MutationConflict);
        }
        let evidence_ids = decision
            .evidence_ids()
            .iter()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        let evidence_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audio_identity_evidence
             WHERE id = ANY($1) AND source_audio_identity_id = $2
               AND candidate_audio_identity_id = $3",
        )
        .bind(&evidence_ids)
        .bind(decision.source_audio_identity_id().into_uuid())
        .bind(decision.candidate_audio_identity_id().into_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(evidence_count).map_err(|_| CatalogStoreError::NumericOverflow)?
            != evidence_ids.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let current = sqlx::query_as::<_, (bool, String)>(
            "SELECT decision.is_locked, decision.authority
             FROM audio_identity_match_current current
             JOIN audio_identity_match_decisions decision ON decision.id = current.decision_id
             WHERE current.source_audio_identity_id = $1
               AND current.candidate_audio_identity_id = $2
             FOR UPDATE OF current, decision",
        )
        .bind(decision.source_audio_identity_id().into_uuid())
        .bind(decision.candidate_audio_identity_id().into_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let predecessor = sqlx::query_scalar::<_, Uuid>(
            "SELECT decision.id FROM audio_identity_match_decisions decision
             WHERE decision.source_audio_identity_id = $1
               AND decision.candidate_audio_identity_id = $2
               AND NOT EXISTS (
                   SELECT 1 FROM audio_identity_match_decisions successor
                   WHERE successor.predecessor_decision_id = decision.id)
             LIMIT 1",
        )
        .bind(decision.source_audio_identity_id().into_uuid())
        .bind(decision.candidate_audio_identity_id().into_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let automated = matches!(decision.authority(), DecisionAuthority::Automated { .. });
        if current
            .as_ref()
            .is_some_and(|(locked, authority)| *locked && authority == "admin" && automated)
        {
            return Err(CatalogStoreError::LockedDecision);
        }
        let (authority, detail) = authority_parts(decision.authority());
        let persisted_payload = serde_json::json!({
            "authority_detail": detail,
            "resolver_payload": payload,
        });
        sqlx::query(
            "INSERT INTO audio_identity_match_decisions (
                id, source_audio_identity_id, candidate_audio_identity_id,
                outcome, authority, is_locked, reason, predecessor_decision_id,
                evidence_ids, payload, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(decision.id().into_uuid())
        .bind(decision.source_audio_identity_id().into_uuid())
        .bind(decision.candidate_audio_identity_id().into_uuid())
        .bind(match decision.outcome() {
            AudioIdentityMatchOutcome::Proposed => "proposed",
            AudioIdentityMatchOutcome::Confirmed => "confirmed",
        })
        .bind(authority)
        .bind(decision.locked())
        .bind(decision.reason())
        .bind(predecessor)
        .bind(serde_json::to_value(decision.evidence_ids())?)
        .bind(persisted_payload)
        .bind(decision.decided_at())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO audio_identity_match_current (
                source_audio_identity_id, candidate_audio_identity_id,
                decision_id, updated_at
             ) VALUES ($1,$2,$3,$4)
             ON CONFLICT (source_audio_identity_id, candidate_audio_identity_id)
             DO UPDATE SET decision_id = EXCLUDED.decision_id,
                           updated_at = EXCLUDED.updated_at",
        )
        .bind(decision.source_audio_identity_id().into_uuid())
        .bind(decision.candidate_audio_identity_id().into_uuid())
        .bind(decision.id().into_uuid())
        .bind(decision.decided_at())
        .execute(&mut *tx)
        .await?;
        audit_event(
            &mut tx,
            "audio_identity",
            decision.source_audio_identity_id().into_uuid(),
            "audio_identity_match_decision_recorded",
            &detail,
            decision.reason(),
            serde_json::json!({
                "decision_id": decision.id(),
                "candidate_audio_identity_id": decision.candidate_audio_identity_id(),
                "outcome": decision.outcome(),
                "locked": decision.locked(),
            }),
            decision.decided_at(),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Converges equivalent AudioIdentities without moving MediaAssets.
    pub async fn merge_audio_identity(
        &self,
        pool: &PgPool,
        merge: &AudioIdentityMerge,
        actor: &str,
        resolver_version: &str,
    ) -> Result<(), CatalogStoreError> {
        AudioIdentityMerge::new(merge.clone())?;
        if actor.trim().is_empty() {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mut tx = pool.begin().await?;
        let statuses = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, status FROM audio_identities
             WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind([merge.source_id.into_uuid(), merge.target_id.into_uuid()])
        .fetch_all(&mut *tx)
        .await?;
        let source_status = statuses
            .iter()
            .find(|(id, _)| *id == merge.source_id.into_uuid())
            .map(|(_, status)| status.as_str());
        let target_status = statuses
            .iter()
            .find(|(id, _)| *id == merge.target_id.into_uuid())
            .map(|(_, status)| status.as_str());
        if statuses.len() != 2
            || matches!(source_status, Some("merged" | "deprecated"))
            || matches!(target_status, Some("merged" | "deprecated"))
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let track_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tracks WHERE audio_identity_id = $1 ORDER BY id LIMIT $2",
        )
        .bind(merge.source_id.into_uuid())
        .bind(limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
        .fetch_all(&mut *tx)
        .await?;
        ensure_correction_scope(
            track_ids.len(),
            CatalogCorrectionDimension::Tracks,
            MAX_CATALOG_CORRECTION_TRACKS,
        )?;
        let release_ids = audio_release_ids(&mut tx, &track_ids).await?;
        ensure_correction_scope(
            release_ids.len(),
            CatalogCorrectionDimension::Releases,
            MAX_CATALOG_CORRECTION_RELEASES,
        )?;
        // Overlay revisions below update Releases. Take those parent locks
        // before Tracks to match Release merge and hard-delete order.
        sqlx::query("SELECT id FROM releases WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(&release_ids)
            .execute(&mut *tx)
            .await?;
        lock_correction_tracks(&mut tx, &track_ids).await?;
        let current_tracks = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tracks WHERE audio_identity_id=$1 ORDER BY id LIMIT $2",
        )
        .bind(merge.source_id.into_uuid())
        .bind(limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
        .fetch_all(&mut *tx)
        .await?;
        if current_tracks != track_ids
            || audio_release_ids(&mut tx, &track_ids).await? != release_ids
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        sqlx::query(
            "UPDATE tracks SET audio_identity_id = $2,
                    revision = revision + 1, updated_at = $3
             WHERE audio_identity_id = $1",
        )
        .bind(merge.source_id.into_uuid())
        .bind(merge.target_id.into_uuid())
        .bind(merge.occurred_at)
        .execute(&mut *tx)
        .await?;
        enqueue_track_revisions(&mut tx, &track_ids, merge.occurred_at, false).await?;
        sqlx::query(
            "UPDATE audio_identities
             SET status = 'merged', merged_into_audio_identity_id = $2,
                 revision = revision + 1, updated_at = $3
             WHERE id = $1",
        )
        .bind(merge.source_id.into_uuid())
        .bind(merge.target_id.into_uuid())
        .bind(merge.occurred_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO audio_identity_merge_events (
                id, source_audio_identity_id, target_audio_identity_id,
                actor, reason, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(merge.id.into_uuid())
        .bind(merge.source_id.into_uuid())
        .bind(merge.target_id.into_uuid())
        .bind(actor)
        .bind(&merge.reason)
        .bind(merge.occurred_at)
        .execute(&mut *tx)
        .await?;
        for (audio_id, bump) in [
            (merge.source_id.into_uuid(), true),
            (merge.target_id.into_uuid(), true),
        ] {
            audit_audio_identity_change(
                &mut tx,
                audio_id,
                "audio_identity_merged",
                actor,
                &merge.reason,
                merge.occurred_at,
                bump,
            )
            .await?;
        }
        append_overlay_revisions(
            &mut tx,
            &release_ids,
            resolver_version,
            actor,
            "audio_identity_merged",
            merge.occurred_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Moves selected Track occurrences to a new AudioIdentity without moving bytes.
    pub async fn split_audio_identity(
        &self,
        pool: &PgPool,
        split: &AudioIdentitySplit,
        actor: &str,
        resolver_version: &str,
    ) -> Result<(), CatalogStoreError> {
        AudioIdentitySplit::new(split.clone())?;
        if actor.trim().is_empty() {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let track_ids = split
            .track_ids
            .iter()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        ensure_correction_scope(
            track_ids.len(),
            CatalogCorrectionDimension::Tracks,
            MAX_CATALOG_CORRECTION_TRACKS,
        )?;
        let mut tx = pool.begin().await?;
        let source_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM audio_identities WHERE id = $1 FOR UPDATE",
        )
        .bind(split.source_id.into_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CatalogStoreError::MutationConflict)?;
        if matches!(source_status.as_str(), "merged" | "deprecated") {
            return Err(CatalogStoreError::MutationConflict);
        }
        let release_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT release_id FROM tracks
             WHERE id = ANY($1) AND audio_identity_id = $2
             ORDER BY release_id LIMIT $3",
        )
        .bind(&track_ids)
        .bind(split.source_id.into_uuid())
        .bind(limit_plus_one(MAX_CATALOG_CORRECTION_RELEASES)?)
        .fetch_all(&mut *tx)
        .await?;
        ensure_correction_scope(
            release_ids.len(),
            CatalogCorrectionDimension::Releases,
            MAX_CATALOG_CORRECTION_RELEASES,
        )?;
        sqlx::query("SELECT id FROM releases WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(&release_ids)
            .execute(&mut *tx)
            .await?;
        lock_correction_tracks(&mut tx, &track_ids).await?;
        let current_releases = sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT release_id FROM tracks WHERE id=ANY($1) AND audio_identity_id=$2 ORDER BY release_id LIMIT $3",
        )
        .bind(&track_ids)
        .bind(split.source_id.into_uuid())
        .bind(limit_plus_one(MAX_CATALOG_CORRECTION_RELEASES)?)
        .fetch_all(&mut *tx)
        .await?;
        if current_releases != release_ids {
            return Err(CatalogStoreError::MutationConflict);
        }
        let matching_tracks = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tracks
             WHERE id = ANY($1) AND audio_identity_id = $2",
        )
        .bind(&track_ids)
        .bind(split.source_id.into_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(matching_tracks).map_err(|_| CatalogStoreError::NumericOverflow)?
            != track_ids.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        sqlx::query(
            "INSERT INTO audio_identities (
                id, status, revision, created_at, updated_at
             ) VALUES ($1,'provisional',1,$2,$2)",
        )
        .bind(split.new_identity_id.into_uuid())
        .bind(split.occurred_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE tracks SET audio_identity_id = $3,
                    revision = revision + 1, updated_at = $4
             WHERE id = ANY($1) AND audio_identity_id = $2",
        )
        .bind(&track_ids)
        .bind(split.source_id.into_uuid())
        .bind(split.new_identity_id.into_uuid())
        .bind(split.occurred_at)
        .execute(&mut *tx)
        .await?;
        enqueue_track_revisions(&mut tx, &track_ids, split.occurred_at, false).await?;
        sqlx::query(
            "INSERT INTO audio_identity_split_events (
                id, source_audio_identity_id, new_audio_identity_id,
                actor, reason, selected_track_ids, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(split.id.into_uuid())
        .bind(split.source_id.into_uuid())
        .bind(split.new_identity_id.into_uuid())
        .bind(actor)
        .bind(&split.reason)
        .bind(serde_json::to_value(&split.track_ids)?)
        .bind(split.occurred_at)
        .execute(&mut *tx)
        .await?;
        for (audio_id, bump) in [
            (split.source_id.into_uuid(), true),
            (split.new_identity_id.into_uuid(), false),
        ] {
            audit_audio_identity_change(
                &mut tx,
                audio_id,
                "audio_identity_split",
                actor,
                &split.reason,
                split.occurred_at,
                bump,
            )
            .await?;
        }
        append_overlay_revisions(
            &mut tx,
            &release_ids,
            resolver_version,
            actor,
            "audio_identity_split",
            split.occurred_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Returns the exact bounded impact of merging the selected source Artists
    /// into the target without mutating catalog state.
    pub async fn preview_artist_merge(
        &self,
        pool: &PgPool,
        source_artist_ids: &[ArtistId],
        target_artist_id: ArtistId,
    ) -> Result<ArtistMergePreview, CatalogStoreError> {
        let source_ids = validate_artist_merge_selection(source_artist_ids, target_artist_id)?;
        let mut all_ids = source_ids.clone();
        all_ids.push(target_artist_id.into_uuid());
        let rows = sqlx::query_as::<_, ArtistMergePreviewRow>(
            "SELECT id, public_api_id, canonical_name, sort_name, artist_type,
                    disambiguation, begin_date, end_date, status
               FROM artists
              WHERE id=ANY($1)
              ORDER BY public_api_id",
        )
        .bind(&all_ids)
        .fetch_all(pool)
        .await?;
        if rows.len() != all_ids.len()
            || rows
                .iter()
                .any(|row| matches!(row.8.as_str(), "merged" | "deprecated"))
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let target_row = rows
            .iter()
            .find(|row| row.0 == target_artist_id.into_uuid())
            .ok_or(CatalogStoreError::MutationConflict)?;
        let to_preview = |row: &ArtistMergePreviewRow| ArtistMergePreviewArtist {
            public_api_id: row.1,
            canonical_name: row.2.clone(),
            sort_name: row.3.clone(),
            artist_type: row.4.clone(),
            disambiguation: row.5.clone(),
            begin_date: row.6,
            end_date: row.7,
            status: row.8.clone(),
        };
        let target = to_preview(target_row);
        let sources = rows
            .iter()
            .filter(|row| source_ids.contains(&row.0))
            .map(to_preview)
            .collect::<Vec<_>>();

        let impact = sqlx::query_as::<_, ArtistMergeImpact>(
            "WITH affected_releases AS (
                 SELECT release_id FROM release_artists WHERE artist_id=ANY($1)
                 UNION SELECT track.release_id FROM track_artists credit
                       JOIN tracks track ON track.id=credit.track_id
                       WHERE credit.artist_id=ANY($1)
                 UNION SELECT release_id FROM contributions WHERE artist_id=ANY($1)
                 UNION SELECT observed.release_id
                       FROM artist_match_current current
                       JOIN artist_match_decisions decision ON decision.id=current.decision_id
                       JOIN observed_artists observed ON observed.id=current.observed_artist_id
                       WHERE decision.artist_id=ANY($1)
             ), affected_tracks AS (
                 SELECT id FROM tracks WHERE release_id IN (SELECT release_id FROM affected_releases)
                 UNION SELECT track_id FROM artist_track_associations
                       WHERE artist_id=ANY($1) OR source_artist_id=ANY($1)
             ), all_selected AS (
                 SELECT unnest($2::uuid[]) AS artist_id
             )
             SELECT
                 (SELECT count(*) FROM affected_releases) AS releases,
                 (SELECT count(*) FROM affected_tracks) AS tracks,
                 (SELECT count(*) FROM release_artists WHERE artist_id=ANY($1)) AS release_credits,
                 (SELECT count(*) FROM track_artists WHERE artist_id=ANY($1)) AS track_credits,
                 (SELECT count(*) FROM contributions WHERE artist_id=ANY($1)) AS contributions,
                 (SELECT count(*) FROM artist_aliases WHERE artist_id=ANY($1)) AS aliases,
                 (SELECT count(*) FROM artist_external_identities WHERE artist_id=ANY($1)) AS external_identities,
                 (SELECT count(*) FROM artist_relationships
                   WHERE source_artist_id=ANY($1) OR target_artist_id=ANY($1)) AS relationships,
                 (SELECT count(*) FROM group_memberships
                   WHERE member_artist_id=ANY($1) OR group_artist_id=ANY($1)) AS memberships,
                 (SELECT count(*) FROM artist_match_current current
                   JOIN artist_match_decisions decision ON decision.id=current.decision_id
                   WHERE decision.artist_id=ANY($1)) AS observations,
                 (SELECT count(*) FROM native_artist_user_states WHERE artist_id=ANY($1)) AS user_states,
                 (SELECT count(*) FROM (
                      SELECT user_id FROM native_artist_user_states
                       WHERE artist_id IN (SELECT artist_id FROM all_selected)
                       GROUP BY user_id HAVING count(*) > 1
                  ) conflicts) AS user_state_conflicts,
                 (SELECT count(*) FROM native_chart_items WHERE linked_artist_id=ANY($1)) AS chart_links,
                 (SELECT count(*) FROM catalog_entity_images
                   WHERE entity_type='artist' AND artist_id=ANY($1)) AS operator_images,
                 (SELECT count(*) FROM catalog_provider_images
                   WHERE entity_type='artist' AND entity_id=ANY($1)) AS provider_images,
                 (SELECT count(*) FROM (
                      SELECT provider FROM catalog_provider_images
                       WHERE entity_type='artist'
                         AND entity_id IN (SELECT artist_id FROM all_selected)
                       GROUP BY provider HAVING count(*) > 1
                  ) conflicts) AS provider_image_conflicts,
                 (SELECT count(*) FROM provider_identity_candidates WHERE artist_id=ANY($1)) AS provider_candidates,
                 (SELECT count(*) FROM provider_enrichment_tasks
                   WHERE entity_type='artist' AND entity_id=ANY($1)) AS provider_tasks",
        )
        .bind(&source_ids)
        .bind(&all_ids)
        .fetch_one(pool)
        .await?;

        let mut metadata_conflicts = Vec::new();
        if sources
            .iter()
            .any(|source| source.canonical_name != target.canonical_name)
        {
            metadata_conflicts.push("canonicalName".to_owned());
        }
        if sources
            .iter()
            .any(|source| source.sort_name != target.sort_name)
        {
            metadata_conflicts.push("sortName".to_owned());
        }
        if sources
            .iter()
            .any(|source| source.artist_type != target.artist_type)
        {
            metadata_conflicts.push("artistType".to_owned());
        }
        if sources
            .iter()
            .any(|source| source.disambiguation != target.disambiguation)
        {
            metadata_conflicts.push("disambiguation".to_owned());
        }
        if sources.iter().any(|source| {
            source.begin_date != target.begin_date || source.end_date != target.end_date
        }) {
            metadata_conflicts.push("activeDates".to_owned());
        }

        Ok(ArtistMergePreview {
            target,
            sources,
            impact,
            metadata_conflicts,
        })
    }

    /// Merges an Artist into a durable redirect and retargets semantic catalog links.
    pub async fn merge_artist(
        &self,
        pool: &PgPool,
        merge: &ArtistMerge,
        resolver_version: &str,
    ) -> Result<(), CatalogStoreError> {
        self.merge_artists_with_authority(
            pool,
            std::slice::from_ref(merge),
            resolver_version,
            ArtistMergeDecisionAuthority::Admin,
        )
        .await
    }

    /// Atomically merges one or more sources into one administrator-selected
    /// target. Every source event commits together or the entire batch rolls
    /// back, including overlays and projection work.
    pub async fn merge_artists(
        &self,
        pool: &PgPool,
        merges: &[ArtistMerge],
        resolver_version: &str,
    ) -> Result<(), CatalogStoreError> {
        self.merge_artists_with_authority(
            pool,
            merges,
            resolver_version,
            ArtistMergeDecisionAuthority::Admin,
        )
        .await
    }

    /// Applies an automated, audited Artist reconciliation without recording
    /// its replacement decisions as locked administrator choices.
    pub async fn reconcile_artist(
        &self,
        pool: &PgPool,
        merge: &ArtistMerge,
        resolver_version: &str,
        evidence: &ArtistReconciliationEvidence,
    ) -> Result<(), CatalogStoreError> {
        evidence.validate()?;
        self.merge_artists_with_authority(
            pool,
            std::slice::from_ref(merge),
            resolver_version,
            ArtistMergeDecisionAuthority::Automated(evidence),
        )
        .await
    }

    async fn merge_artists_with_authority(
        &self,
        pool: &PgPool,
        merges: &[ArtistMerge],
        resolver_version: &str,
        decision_authority: ArtistMergeDecisionAuthority<'_>,
    ) -> Result<(), CatalogStoreError> {
        if merges.is_empty()
            || merges.len() > MAX_ARTIST_MERGE_SOURCES
            || resolver_version.trim().is_empty()
        {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let target_artist_id = merges[0].target_artist_id;
        let source_artist_ids = merges
            .iter()
            .map(|merge| merge.source_artist_id)
            .collect::<Vec<_>>();
        validate_artist_merge_selection(&source_artist_ids, target_artist_id)?;
        for merge in merges {
            ArtistMerge::new(merge.clone())?;
            if merge.target_artist_id != target_artist_id {
                return Err(CatalogStoreError::InvalidRequest);
            }
        }
        let mut ordered_merges = merges.iter().collect::<Vec<_>>();
        ordered_merges.sort_by_key(|merge| merge.source_artist_id.into_uuid());
        let mut tx = pool.begin().await?;
        let release_ids = affected_artist_release_ids(&mut tx, &source_artist_ids).await?;
        ensure_correction_scope(
            release_ids.len(),
            CatalogCorrectionDimension::Releases,
            MAX_CATALOG_CORRECTION_RELEASES,
        )?;
        let mut artists = source_artist_ids
            .iter()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        artists.push(target_artist_id.into_uuid());
        let tracks = artist_merge_scope_tracks(&mut tx, &release_ids, &artists).await?;
        if !crate::catalog_mutations::lock_participation_closure(
            &mut tx,
            &release_ids,
            Some(&tracks),
            &artists,
        )
        .await?
            || affected_artist_release_ids(&mut tx, &source_artist_ids).await? != release_ids
            || artist_merge_scope_tracks(&mut tx, &release_ids, &artists).await? != tracks
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        ensure_artist_merge_batch_scope(&mut tx, &source_artist_ids, target_artist_id).await?;
        for merge in ordered_merges {
            self.merge_artist_in_transaction(&mut tx, merge, resolver_version, decision_authority)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn merge_artist_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        merge: &ArtistMerge,
        resolver_version: &str,
        decision_authority: ArtistMergeDecisionAuthority<'_>,
    ) -> Result<(), CatalogStoreError> {
        let artists = sqlx::query_as::<_, (Uuid, Uuid, String, String, String, i64)>(
            "SELECT id, public_api_id, status, canonical_name, normalized_name, revision FROM artists
             WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind([
            merge.source_artist_id.into_uuid(),
            merge.target_artist_id.into_uuid(),
        ])
        .fetch_all(&mut **tx)
        .await?;
        let source = artists
            .iter()
            .find(|artist| artist.0 == merge.source_artist_id.into_uuid());
        let target = artists
            .iter()
            .find(|artist| artist.0 == merge.target_artist_id.into_uuid());
        if artists.len() != 2
            || source.is_none_or(|artist| {
                artist.1 != merge.source_public_api_id.into_uuid()
                    || matches!(artist.2.as_str(), "merged" | "deprecated")
            })
            || target.is_none_or(|artist| matches!(artist.2.as_str(), "merged" | "deprecated"))
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let source_name = source.map(|artist| artist.3.clone());
        let source_normalized_name = source.map(|artist| artist.4.clone());
        let identity_row_count = affected_artist_identity_row_count(
            tx,
            &[merge.source_artist_id, merge.target_artist_id],
        )
        .await?;
        ensure_correction_scope(
            identity_row_count,
            CatalogCorrectionDimension::IdentityRows,
            MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
        )?;
        let self_link = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM artist_relationships
                WHERE (source_artist_id = $1 AND target_artist_id = $2)
                   OR (source_artist_id = $2 AND target_artist_id = $1)
                UNION ALL
                SELECT 1 FROM group_memberships
                WHERE (member_artist_id = $1 AND group_artist_id = $2)
                   OR (member_artist_id = $2 AND group_artist_id = $1)
             )",
        )
        .bind(merge.source_artist_id.into_uuid())
        .bind(merge.target_artist_id.into_uuid())
        .fetch_one(&mut **tx)
        .await?;
        if self_link {
            return Err(CatalogStoreError::MutationConflict);
        }
        let release_ids = affected_artist_release_ids(tx, &[merge.source_artist_id]).await?;
        ensure_correction_scope(
            release_ids.len(),
            CatalogCorrectionDimension::Releases,
            MAX_CATALOG_CORRECTION_RELEASES,
        )?;
        let mut track_ids = release_track_ids(tx, &release_ids)
            .await?
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        track_ids.extend(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT DISTINCT track_id FROM artist_track_associations
                 WHERE artist_id = ANY($1) OR source_artist_id = ANY($1)
                 ORDER BY track_id LIMIT $2",
            )
            .bind([
                merge.source_artist_id.into_uuid(),
                merge.target_artist_id.into_uuid(),
            ])
            .bind(limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
            .fetch_all(&mut **tx)
            .await?,
        );
        let track_ids = track_ids.into_iter().collect::<Vec<_>>();
        ensure_correction_scope(
            track_ids.len(),
            CatalogCorrectionDimension::Tracks,
            MAX_CATALOG_CORRECTION_TRACKS,
        )?;
        let observation_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT current.observed_artist_id
             FROM artist_match_current current
             JOIN artist_match_decisions decision ON decision.id = current.decision_id
             WHERE decision.artist_id = $1
             ORDER BY current.observed_artist_id LIMIT $2",
        )
        .bind(merge.source_artist_id.into_uuid())
        .bind(limit_plus_one(MAX_CATALOG_CORRECTION_IDENTITY_ROWS)?)
        .fetch_all(&mut **tx)
        .await?;
        retarget_observations(
            tx,
            &observation_ids,
            &ArtistObservationRetarget {
                expected_artist_id: merge.source_artist_id,
                artist_id: merge.target_artist_id,
                actor: &merge.actor,
                reason: &merge.reason,
                now: merge.occurred_at,
                decision_authority,
            },
        )
        .await?;
        let release_artist_count = update_artist_links(
            tx,
            "release_artists",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let track_artist_count = update_artist_links(
            tx,
            "track_artists",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let contribution_count = update_artist_links(
            tx,
            "contributions",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let alias_count = update_artist_links(
            tx,
            "artist_aliases",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let external_count = update_artist_links(
            tx,
            "artist_external_identities",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let relationship_count = sqlx::query(
            "UPDATE artist_relationships
             SET source_artist_id = CASE WHEN source_artist_id = $1 THEN $2 ELSE source_artist_id END,
                 target_artist_id = CASE WHEN target_artist_id = $1 THEN $2 ELSE target_artist_id END
             WHERE source_artist_id = $1 OR target_artist_id = $1",
        )
        .bind(merge.source_artist_id.into_uuid())
        .bind(merge.target_artist_id.into_uuid())
        .execute(&mut **tx)
        .await?
        .rows_affected();
        let membership_count = sqlx::query(
            "UPDATE group_memberships
             SET member_artist_id = CASE WHEN member_artist_id = $1 THEN $2 ELSE member_artist_id END,
                 group_artist_id = CASE WHEN group_artist_id = $1 THEN $2 ELSE group_artist_id END
             WHERE member_artist_id = $1 OR group_artist_id = $1",
        )
        .bind(merge.source_artist_id.into_uuid())
        .bind(merge.target_artist_id.into_uuid())
        .execute(&mut **tx)
        .await?
        .rows_affected();
        let coalesced = coalesce_extended_artist_data(
            tx,
            merge.source_artist_id,
            merge.target_artist_id,
            merge.occurred_at,
        )
        .await?;
        sqlx::query(
            "UPDATE artists
             SET status = 'merged', merged_into_artist_id = $2,
                 revision = revision + 1, updated_at = $3
             WHERE id = $1",
        )
        .bind(merge.source_artist_id.into_uuid())
        .bind(merge.target_artist_id.into_uuid())
        .bind(merge.occurred_at)
        .execute(&mut **tx)
        .await?;
        if let Some(source_name) = source_name {
            sqlx::query(
                "INSERT INTO artist_aliases (
                    id, artist_id, alias, normalized_alias, alias_type, locale,
                    evidence_source, confidence, is_locked, created_at
                 )
                 SELECT $1, $2, $3, $4, 'search', NULL, $5, 10000, true, $6
                 WHERE NOT EXISTS (
                     SELECT 1
                     FROM artist_aliases alias
                     WHERE alias.artist_id = $2 AND alias.normalized_alias = $4
                 )",
            )
            .bind(Uuid::now_v7())
            .bind(merge.target_artist_id.into_uuid())
            .bind(&source_name)
            .bind(source_normalized_name.as_deref().unwrap_or_default())
            .bind(&merge.actor)
            .bind(merge.occurred_at)
            .execute(&mut **tx)
            .await?;
        }
        rebuild_participation_for_tracks(tx, &track_ids, merge.occurred_at).await?;
        enqueue_track_revisions(tx, &track_ids, merge.occurred_at, true).await?;
        let moved = serde_json::json!({
            "release_artists": release_artist_count,
            "track_artists": track_artist_count,
            "contributions": contribution_count,
            "aliases": alias_count,
            "external_identities": external_count,
            "relationships": relationship_count,
            "memberships": membership_count,
            "observations": observation_ids.len(),
            "coalesced": coalesced,
            "reconciliation": decision_authority.audit_payload(),
        });
        sqlx::query(
            "INSERT INTO artist_merge_events (
                id, source_artist_id, target_artist_id, actor, reason,
                moved_relationships, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(merge.id.into_uuid())
        .bind(merge.source_artist_id.into_uuid())
        .bind(merge.target_artist_id.into_uuid())
        .bind(&merge.actor)
        .bind(&merge.reason)
        .bind(&moved)
        .bind(merge.occurred_at)
        .execute(&mut **tx)
        .await?;
        for artist_id in [merge.source_artist_id, merge.target_artist_id] {
            audit_artist_change(
                tx,
                artist_id.into_uuid(),
                "artist_merged",
                &merge.actor,
                &merge.reason,
                merge.occurred_at,
                true,
            )
            .await?;
        }
        append_overlay_revisions(
            tx,
            &release_ids,
            resolver_version,
            &merge.actor,
            "artist_merged",
            merge.occurred_at,
        )
        .await?;
        enqueue_artist_identity_seeds(
            tx,
            &[
                merge.source_artist_id.into_uuid(),
                merge.target_artist_id.into_uuid(),
            ],
            merge.occurred_at,
        )
        .await?;
        Ok(())
    }

    /// Creates a new Artist and moves only the split's explicit assignments.
    pub async fn split_artist(
        &self,
        pool: &PgPool,
        split: &ArtistSplit,
        new_artist: &Artist,
        resolver_version: &str,
    ) -> Result<(), CatalogStoreError> {
        ArtistSplit::new(split.clone())?;
        ensure_correction_scope(
            split.assignments.len(),
            CatalogCorrectionDimension::SplitAssignments,
            MAX_CATALOG_SPLIT_ASSIGNMENTS,
        )?;
        if new_artist.id() != split.new_artist_id
            || matches!(
                new_artist.status(),
                ArtistStatus::MergedInto(_) | ArtistStatus::Deprecated
            )
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let mut tx = pool.begin().await?;
        let source_status =
            sqlx::query_scalar::<_, String>("SELECT status FROM artists WHERE id = $1 FOR UPDATE")
                .bind(split.source_artist_id.into_uuid())
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(CatalogStoreError::MutationConflict)?;
        if matches!(source_status.as_str(), "merged" | "deprecated") {
            return Err(CatalogStoreError::MutationConflict);
        }
        insert_artist(&mut *tx, new_artist).await?;

        let release_artist_ids = assignment_ids(&split.assignments, AssignmentKind::ReleaseArtist);
        let track_artist_ids = assignment_ids(&split.assignments, AssignmentKind::TrackArtist);
        let contribution_ids = assignment_ids(&split.assignments, AssignmentKind::Contribution);
        let alias_ids = assignment_ids(&split.assignments, AssignmentKind::Alias);
        let external_ids = assignment_ids(&split.assignments, AssignmentKind::ExternalIdentity);
        let release_ids = split_release_ids(
            &mut tx,
            &release_artist_ids,
            &track_artist_ids,
            &contribution_ids,
        )
        .await?;
        ensure_correction_scope(
            release_ids.len(),
            CatalogCorrectionDimension::Releases,
            MAX_CATALOG_CORRECTION_RELEASES,
        )?;
        sqlx::query("SELECT id FROM releases WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(&release_ids)
            .fetch_all(&mut *tx)
            .await?;
        if split_release_ids(
            &mut tx,
            &release_artist_ids,
            &track_artist_ids,
            &contribution_ids,
        )
        .await?
            != release_ids
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let track_ids = release_track_ids(&mut tx, &release_ids).await?;
        ensure_correction_scope(
            track_ids.len(),
            CatalogCorrectionDimension::Tracks,
            MAX_CATALOG_CORRECTION_TRACKS,
        )?;
        let locked_tracks = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tracks WHERE id=ANY($1) ORDER BY id FOR NO KEY UPDATE",
        )
        .bind(&track_ids)
        .fetch_all(&mut *tx)
        .await?;
        if locked_tracks != track_ids
            || release_track_ids(&mut tx, &release_ids).await? != track_ids
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        sqlx::query("SELECT entity_id FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=ANY($1) ORDER BY entity_id FOR UPDATE")
            .bind([split.source_artist_id.into_uuid(), split.new_artist_id.into_uuid()])
            .fetch_all(&mut *tx)
            .await?;
        let observation_ids = split_observation_ids(
            &mut tx,
            &release_artist_ids,
            &track_artist_ids,
            &contribution_ids,
        )
        .await?;
        retarget_observations(
            &mut tx,
            &observation_ids,
            &ArtistObservationRetarget {
                expected_artist_id: split.source_artist_id,
                artist_id: split.new_artist_id,
                actor: &split.actor,
                reason: &split.reason,
                now: split.occurred_at,
                decision_authority: ArtistMergeDecisionAuthority::Admin,
            },
        )
        .await?;

        let mut moved = 0_u64;
        moved += move_selected(
            &mut tx,
            "release_artists",
            &release_artist_ids,
            split.source_artist_id,
            split.new_artist_id,
        )
        .await?;
        moved += move_selected(
            &mut tx,
            "track_artists",
            &track_artist_ids,
            split.source_artist_id,
            split.new_artist_id,
        )
        .await?;
        moved += move_selected(
            &mut tx,
            "contributions",
            &contribution_ids,
            split.source_artist_id,
            split.new_artist_id,
        )
        .await?;
        moved += move_selected(
            &mut tx,
            "artist_aliases",
            &alias_ids,
            split.source_artist_id,
            split.new_artist_id,
        )
        .await?;
        moved += move_selected(
            &mut tx,
            "artist_external_identities",
            &external_ids,
            split.source_artist_id,
            split.new_artist_id,
        )
        .await?;
        if usize::try_from(moved).map_err(|_| CatalogStoreError::NumericOverflow)?
            != split.assignments.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        rebuild_participation_for_tracks(&mut tx, &track_ids, split.occurred_at).await?;
        enqueue_track_revisions(&mut tx, &track_ids, split.occurred_at, true).await?;
        sqlx::query(
            "INSERT INTO artist_split_events (
                id, source_artist_id, new_artist_id, actor, reason,
                selected_assignments, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(split.id.into_uuid())
        .bind(split.source_artist_id.into_uuid())
        .bind(split.new_artist_id.into_uuid())
        .bind(&split.actor)
        .bind(&split.reason)
        .bind(serde_json::to_value(&split.assignments)?)
        .bind(split.occurred_at)
        .execute(&mut *tx)
        .await?;
        for (artist_id, bump) in [(split.source_artist_id, true), (split.new_artist_id, true)] {
            audit_artist_change(
                &mut tx,
                artist_id.into_uuid(),
                "artist_split",
                &split.actor,
                &split.reason,
                split.occurred_at,
                bump,
            )
            .await?;
        }
        append_overlay_revisions(
            &mut tx,
            &release_ids,
            resolver_version,
            &split.actor,
            "artist_split",
            split.occurred_at,
        )
        .await?;
        enqueue_artist_identity_seeds(
            &mut tx,
            &[
                split.source_artist_id.into_uuid(),
                split.new_artist_id.into_uuid(),
            ],
            split.occurred_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn artist_merge_scope_tracks(
    tx: &mut Transaction<'_, Postgres>,
    release_ids: &[Uuid],
    artist_ids: &[Uuid],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    let mut tracks = release_track_ids(tx, release_ids)
        .await?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    tracks.extend(sqlx::query_scalar::<_, Uuid>("SELECT DISTINCT track_id FROM artist_track_associations WHERE artist_id=ANY($1) OR source_artist_id=ANY($1) ORDER BY track_id LIMIT $2")
        .bind(artist_ids).bind(limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?).fetch_all(&mut **tx).await?);
    ensure_correction_scope(
        tracks.len(),
        CatalogCorrectionDimension::Tracks,
        MAX_CATALOG_CORRECTION_TRACKS,
    )?;
    Ok(tracks.into_iter().collect())
}

/// Corrections expose new identity relationships. Record both endpoints in the
/// same transaction so closure expansion survives a crash and active drain.
pub(crate) async fn enqueue_artist_identity_seeds(
    tx: &mut Transaction<'_, Postgres>,
    artist_ids: &[Uuid],
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    let has_seed_table: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.catalog_identity_reconciliation_seed') IS NOT NULL",
    )
    .fetch_one(&mut **tx)
    .await?;
    if has_seed_table {
        sqlx::query(
            "INSERT INTO catalog_identity_reconciliation_seed (artist_id, normalized_name, created_at)
             SELECT id, normalized_name, $2 FROM artists WHERE id = ANY($1)",
        )
        .bind(artist_ids)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum AssignmentKind {
    ReleaseArtist,
    TrackArtist,
    Contribution,
    Alias,
    ExternalIdentity,
}

fn validate_artist_merge_selection(
    source_artist_ids: &[ArtistId],
    target_artist_id: ArtistId,
) -> Result<Vec<Uuid>, CatalogStoreError> {
    if source_artist_ids.is_empty() || source_artist_ids.len() > MAX_ARTIST_MERGE_SOURCES {
        return Err(CatalogStoreError::InvalidRequest);
    }
    let source_ids = source_artist_ids
        .iter()
        .map(|id| id.into_uuid())
        .collect::<std::collections::BTreeSet<_>>();
    if source_ids.len() != source_artist_ids.len()
        || source_ids.contains(&target_artist_id.into_uuid())
    {
        return Err(CatalogStoreError::InvalidRequest);
    }
    Ok(source_ids.into_iter().collect())
}

/// Locks the complete selection and applies correction bounds to the union of
/// every source before the first merge writes anything. Per-source checks
/// remain in place as a defense against query-shape drift.
async fn ensure_artist_merge_batch_scope(
    tx: &mut Transaction<'_, Postgres>,
    source_artist_ids: &[ArtistId],
    target_artist_id: ArtistId,
) -> Result<(), CatalogStoreError> {
    let mut all_artist_ids = source_artist_ids.to_vec();
    all_artist_ids.push(target_artist_id);
    let all_ids = all_artist_ids
        .iter()
        .map(|id| id.into_uuid())
        .collect::<Vec<_>>();
    let locked = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,status FROM artists
          WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&all_ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked.len() != all_ids.len()
        || locked
            .iter()
            .any(|(_, status)| matches!(status.as_str(), "merged" | "deprecated"))
    {
        return Err(CatalogStoreError::MutationConflict);
    }

    let identity_rows = affected_artist_identity_row_count(tx, &all_artist_ids).await?;
    ensure_correction_scope(
        identity_rows,
        CatalogCorrectionDimension::IdentityRows,
        MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
    )?;
    let release_ids = affected_artist_release_ids(tx, source_artist_ids).await?;
    ensure_correction_scope(
        release_ids.len(),
        CatalogCorrectionDimension::Releases,
        MAX_CATALOG_CORRECTION_RELEASES,
    )?;
    // Fence Track membership before acquiring the transaction-wide Track set.
    // The Release rows also prevent a concurrent move from adding a Track
    // between our membership read and the later per-source writes.
    sqlx::query("SELECT id FROM releases WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&release_ids)
        .fetch_all(&mut **tx)
        .await?;
    if affected_artist_release_ids(tx, source_artist_ids).await? != release_ids {
        return Err(CatalogStoreError::MutationConflict);
    }
    let mut track_ids = release_track_ids(tx, &release_ids)
        .await?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    track_ids.extend(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT track_id FROM artist_track_associations
              WHERE artist_id=ANY($1) OR source_artist_id=ANY($1)
              ORDER BY track_id LIMIT $2",
        )
        .bind(&all_ids)
        .bind(limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
        .fetch_all(&mut **tx)
        .await?,
    );
    ensure_correction_scope(
        track_ids.len(),
        CatalogCorrectionDimension::Tracks,
        MAX_CATALOG_CORRECTION_TRACKS,
    )?;
    let ordered_tracks = track_ids.into_iter().collect::<Vec<_>>();
    let locked_tracks = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tracks WHERE id=ANY($1) ORDER BY id FOR NO KEY UPDATE",
    )
    .bind(&ordered_tracks)
    .fetch_all(&mut **tx)
    .await?;
    if locked_tracks != ordered_tracks {
        return Err(CatalogStoreError::MutationConflict);
    }
    let mut current_tracks = release_track_ids(tx, &release_ids)
        .await?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    current_tracks.extend(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT track_id FROM artist_track_associations
              WHERE artist_id=ANY($1) OR source_artist_id=ANY($1)
              ORDER BY track_id LIMIT $2",
        )
        .bind(&all_ids)
        .bind(limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
        .fetch_all(&mut **tx)
        .await?,
    );
    if !current_tracks
        .into_iter()
        .eq(ordered_tracks.iter().copied())
    {
        return Err(CatalogStoreError::MutationConflict);
    }
    // Each per-source credit update invokes a statement trigger. Lock the
    // union before the first update so those statements cannot acquire the
    // same candidate rows in opposing source order.
    sqlx::query("SELECT entity_id FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=ANY($1) ORDER BY entity_id FOR UPDATE")
        .bind(&all_ids)
        .fetch_all(&mut **tx)
        .await?;
    Ok(())
}

/// Coalesces Artist-owned data introduced after the original Phase 4 merge.
/// Historical audit/evidence rows stay on the tombstone; mutable serving and
/// projection rows move or fold onto the selected target.
async fn coalesce_extended_artist_data(
    tx: &mut Transaction<'_, Postgres>,
    source: ArtistId,
    target: ArtistId,
    now: OffsetDateTime,
) -> Result<serde_json::Value, CatalogStoreError> {
    let source_id = source.into_uuid();
    let target_id = target.into_uuid();

    let user_states = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_artist_user_states WHERE artist_id=$1",
    )
    .bind(source_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO native_artist_user_states (
             user_id, artist_id, starred, hated, rating, pinned, play_count,
             last_played_at, created_at, updated_at
         )
         SELECT user_id, $2, starred, hated, rating, pinned, play_count,
                last_played_at, created_at, GREATEST(updated_at, $3)
           FROM native_artist_user_states WHERE artist_id=$1
         ON CONFLICT (user_id, artist_id) DO UPDATE SET
             starred=native_artist_user_states.starred OR EXCLUDED.starred,
             hated=native_artist_user_states.hated OR EXCLUDED.hated,
             rating=GREATEST(native_artist_user_states.rating, EXCLUDED.rating),
             pinned=native_artist_user_states.pinned OR EXCLUDED.pinned,
             play_count=native_artist_user_states.play_count + EXCLUDED.play_count,
             last_played_at=GREATEST(
                 native_artist_user_states.last_played_at, EXCLUDED.last_played_at),
             created_at=LEAST(native_artist_user_states.created_at, EXCLUDED.created_at),
             updated_at=GREATEST(
                 native_artist_user_states.updated_at, EXCLUDED.updated_at, $3)",
    )
    .bind(source_id)
    .bind(target_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM native_artist_user_states WHERE artist_id=$1")
        .bind(source_id)
        .execute(&mut **tx)
        .await?;

    let chart_links =
        sqlx::query("UPDATE native_chart_items SET linked_artist_id=$2 WHERE linked_artist_id=$1")
            .bind(source_id)
            .bind(target_id)
            .execute(&mut **tx)
            .await?
            .rows_affected();

    let operator_image_adopted = sqlx::query(
        "UPDATE catalog_entity_images source
            SET artist_id=$2, updated_at=GREATEST(source.updated_at, $3)
          WHERE source.entity_type='artist' AND source.artist_id=$1
            AND NOT EXISTS (
                SELECT 1 FROM catalog_entity_images target
                 WHERE target.entity_type='artist' AND target.artist_id=$2)",
    )
    .bind(source_id)
    .bind(target_id)
    .bind(now)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let operator_image_conflicts_preserved = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_entity_images
          WHERE entity_type='artist' AND artist_id=$1",
    )
    .bind(source_id)
    .fetch_one(&mut **tx)
    .await?;

    let provider_images = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_provider_images
          WHERE entity_type='artist' AND entity_id=$1",
    )
    .bind(source_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE catalog_provider_images target
            SET source_url=source.source_url,
                image_mime_type=source.image_mime_type,
                asset_key=source.asset_key,
                confidence=source.confidence,
                observed_at=source.observed_at
           FROM catalog_provider_images source
          WHERE target.entity_type='artist' AND target.entity_id=$2
            AND source.entity_type='artist' AND source.entity_id=$1
            AND source.provider=target.provider
            AND (source.confidence > target.confidence
                 OR (source.confidence=target.confidence
                     AND source.observed_at > target.observed_at))",
    )
    .bind(source_id)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM catalog_provider_images source
          USING catalog_provider_images target
          WHERE source.entity_type='artist' AND source.entity_id=$1
            AND target.entity_type='artist' AND target.entity_id=$2
            AND source.provider=target.provider",
    )
    .bind(source_id)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE catalog_provider_images
            SET entity_id=$2
          WHERE entity_type='artist' AND entity_id=$1",
    )
    .bind(source_id)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;

    let provider_candidates = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM provider_identity_candidates WHERE artist_id=$1",
    )
    .bind(source_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM provider_identity_candidates source
          USING provider_identity_candidates target
          WHERE source.artist_id=$1 AND target.artist_id=$2
            AND source.provider=target.provider
            AND source.external_id=target.external_id
            AND source.query=target.query",
    )
    .bind(source_id)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE provider_identity_candidates SET artist_id=$2 WHERE artist_id=$1")
        .bind(source_id)
        .bind(target_id)
        .execute(&mut **tx)
        .await?;

    let provider_tasks = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM provider_enrichment_tasks
          WHERE entity_type='artist' AND entity_id=$1",
    )
    .bind(source_id)
    .fetch_one(&mut **tx)
    .await?;
    let provider_tasks_retargeted = sqlx::query(
        "UPDATE provider_enrichment_tasks source
            SET entity_id=$2, updated_at=GREATEST(source.updated_at, $3)
          WHERE source.entity_type='artist' AND source.entity_id=$1
            AND NOT EXISTS (
                SELECT 1 FROM provider_enrichment_tasks target
                 WHERE target.entity_type='artist' AND target.entity_id=$2
                   AND target.provider=source.provider
                   AND target.purpose=source.purpose
                   AND target.request_key=source.request_key)",
    )
    .bind(source_id)
    .bind(target_id)
    .bind(now)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE provider_enrichment_tasks
            SET status='failed', lease_owner=NULL, lease_acquired_at=NULL,
                lease_expires_at=NULL, dormant_at=NULL,
                last_error_category='artist_merged',
                last_error_message='duplicate provider task retained on merged Artist tombstone',
                updated_at=$2, completed_at=$2
          WHERE entity_type='artist' AND entity_id=$1
            AND status IN ('queued','running','retry','dormant')",
    )
    .bind(source_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    Ok(serde_json::json!({
        "native_user_states": user_states,
        "native_chart_links": chart_links,
        "operator_image_adopted": operator_image_adopted,
        "operator_image_conflicts_preserved": operator_image_conflicts_preserved,
        "provider_images": provider_images,
        "provider_identity_candidates": provider_candidates,
        "provider_tasks": provider_tasks,
        "provider_tasks_retargeted": provider_tasks_retargeted,
        "provider_tasks_retained_as_history": provider_tasks
            - i64::try_from(provider_tasks_retargeted)
                .map_err(|_| CatalogStoreError::NumericOverflow)?,
    }))
}

fn assignment_ids(assignments: &[SplitAssignment], kind: AssignmentKind) -> Vec<Uuid> {
    assignments
        .iter()
        .filter_map(|assignment| match (kind, assignment) {
            (AssignmentKind::ReleaseArtist, SplitAssignment::ReleaseArtist(id)) => {
                Some(id.into_uuid())
            }
            (AssignmentKind::TrackArtist, SplitAssignment::TrackArtist(id)) => Some(id.into_uuid()),
            (AssignmentKind::Contribution, SplitAssignment::Contribution(id)) => {
                Some(id.into_uuid())
            }
            (AssignmentKind::Alias, SplitAssignment::Alias(id)) => Some(id.into_uuid()),
            (AssignmentKind::ExternalIdentity, SplitAssignment::ExternalIdentity(id)) => {
                Some(id.into_uuid())
            }
            _ => None,
        })
        .collect()
}

async fn affected_artist_release_ids(
    tx: &mut Transaction<'_, Postgres>,
    artist_ids: &[ArtistId],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    let artist_ids = artist_ids
        .iter()
        .map(|id| id.into_uuid())
        .collect::<Vec<_>>();
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT release_id FROM (
             SELECT release_id FROM release_artists WHERE artist_id = ANY($1)
             UNION SELECT track.release_id FROM track_artists artist
                   JOIN tracks track ON track.id = artist.track_id
                   WHERE artist.artist_id = ANY($1)
             UNION SELECT release_id FROM contributions WHERE artist_id = ANY($1)
             UNION SELECT observed.release_id
                   FROM artist_match_current current
                   JOIN artist_match_decisions decision ON decision.id = current.decision_id
                   JOIN observed_artists observed ON observed.id = current.observed_artist_id
                   WHERE decision.artist_id = ANY($1)
         ) affected ORDER BY release_id LIMIT $2",
    )
    .bind(&artist_ids)
    .bind(limit_plus_one(MAX_CATALOG_CORRECTION_RELEASES)?)
    .fetch_all(&mut **tx)
    .await?)
}

async fn affected_artist_identity_row_count(
    tx: &mut Transaction<'_, Postgres>,
    artist_ids: &[ArtistId],
) -> Result<usize, CatalogStoreError> {
    let artist_ids = artist_ids
        .iter()
        .map(|id| id.into_uuid())
        .collect::<Vec<_>>();
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint FROM (
             SELECT 1 FROM release_artists WHERE artist_id = ANY($1)
             UNION ALL SELECT 1 FROM track_artists WHERE artist_id = ANY($1)
             UNION ALL SELECT 1 FROM contributions WHERE artist_id = ANY($1)
             UNION ALL SELECT 1 FROM artist_aliases WHERE artist_id = ANY($1)
             UNION ALL SELECT 1 FROM artist_external_identities WHERE artist_id = ANY($1)
             UNION ALL SELECT 1 FROM artist_relationships
                       WHERE source_artist_id = ANY($1) OR target_artist_id = ANY($1)
             UNION ALL SELECT 1 FROM group_memberships
                       WHERE member_artist_id = ANY($1) OR group_artist_id = ANY($1)
             UNION ALL SELECT 1
                       FROM artist_match_current current
                       JOIN artist_match_decisions decision ON decision.id = current.decision_id
                       WHERE decision.artist_id = ANY($1)
             UNION ALL SELECT 1 FROM native_artist_user_states WHERE artist_id = ANY($1)
             UNION ALL SELECT 1 FROM native_chart_items WHERE linked_artist_id = ANY($1)
             UNION ALL SELECT 1 FROM catalog_entity_images
                       WHERE entity_type='artist' AND artist_id = ANY($1)
             UNION ALL SELECT 1 FROM catalog_provider_images
                       WHERE entity_type='artist' AND entity_id = ANY($1)
             UNION ALL SELECT 1 FROM provider_identity_candidates WHERE artist_id = ANY($1)
             UNION ALL SELECT 1 FROM provider_enrichment_tasks
                       WHERE entity_type='artist' AND entity_id = ANY($1)
             LIMIT $2
         ) bounded",
    )
    .bind(&artist_ids)
    .bind(limit_plus_one(MAX_CATALOG_CORRECTION_IDENTITY_ROWS)?)
    .fetch_one(&mut **tx)
    .await?;
    usize::try_from(count).map_err(|_| CatalogStoreError::NumericOverflow)
}

async fn release_track_ids(
    tx: &mut Transaction<'_, Postgres>,
    release_ids: &[Uuid],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    if release_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tracks WHERE release_id = ANY($1) ORDER BY id LIMIT $2",
    )
    .bind(release_ids)
    .bind(limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
    .fetch_all(&mut **tx)
    .await?)
}

async fn update_artist_links(
    tx: &mut Transaction<'_, Postgres>,
    table: &'static str,
    source: ArtistId,
    target: ArtistId,
) -> Result<u64, CatalogStoreError> {
    let sql = match table {
        "release_artists" => "UPDATE release_artists SET artist_id = $2 WHERE artist_id = $1",
        "track_artists" => "UPDATE track_artists SET artist_id = $2 WHERE artist_id = $1",
        "contributions" => "UPDATE contributions SET artist_id = $2 WHERE artist_id = $1",
        "artist_aliases" => "UPDATE artist_aliases SET artist_id = $2 WHERE artist_id = $1",
        "artist_external_identities" => {
            "UPDATE artist_external_identities SET artist_id = $2 WHERE artist_id = $1"
        }
        _ => return Err(CatalogStoreError::InvalidRequest),
    };
    Ok(sqlx::query(sql)
        .bind(source.into_uuid())
        .bind(target.into_uuid())
        .execute(&mut **tx)
        .await?
        .rows_affected())
}

async fn move_selected(
    tx: &mut Transaction<'_, Postgres>,
    table: &'static str,
    ids: &[Uuid],
    source: ArtistId,
    target: ArtistId,
) -> Result<u64, CatalogStoreError> {
    if ids.is_empty() {
        return Ok(0);
    }
    let sql = match table {
        "release_artists" => {
            "UPDATE release_artists SET artist_id = $3 WHERE id = ANY($1) AND artist_id = $2"
        }
        "track_artists" => {
            "UPDATE track_artists SET artist_id = $3 WHERE id = ANY($1) AND artist_id = $2"
        }
        "contributions" => {
            "UPDATE contributions SET artist_id = $3 WHERE id = ANY($1) AND artist_id = $2"
        }
        "artist_aliases" => {
            "UPDATE artist_aliases SET artist_id = $3 WHERE id = ANY($1) AND artist_id = $2"
        }
        "artist_external_identities" => {
            "UPDATE artist_external_identities SET artist_id = $3 WHERE id = ANY($1) AND artist_id = $2"
        }
        _ => return Err(CatalogStoreError::InvalidRequest),
    };
    Ok(sqlx::query(sql)
        .bind(ids)
        .bind(source.into_uuid())
        .bind(target.into_uuid())
        .execute(&mut **tx)
        .await?
        .rows_affected())
}

async fn split_release_ids(
    tx: &mut Transaction<'_, Postgres>,
    release_artist_ids: &[Uuid],
    track_artist_ids: &[Uuid],
    contribution_ids: &[Uuid],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT release_id FROM (
             SELECT release_id FROM release_artists WHERE id = ANY($1)
             UNION SELECT track.release_id FROM track_artists artist
                   JOIN tracks track ON track.id = artist.track_id WHERE artist.id = ANY($2)
             UNION SELECT release_id FROM contributions WHERE id = ANY($3)
         ) affected ORDER BY release_id LIMIT $4",
    )
    .bind(release_artist_ids)
    .bind(track_artist_ids)
    .bind(contribution_ids)
    .bind(limit_plus_one(MAX_CATALOG_CORRECTION_RELEASES)?)
    .fetch_all(&mut **tx)
    .await?)
}

async fn split_observation_ids(
    tx: &mut Transaction<'_, Postgres>,
    release_artist_ids: &[Uuid],
    track_artist_ids: &[Uuid],
    contribution_ids: &[Uuid],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT observed_artist_id FROM (
             SELECT observed_artist_id FROM release_artists
             WHERE id = ANY($1) AND observed_artist_id IS NOT NULL
             UNION SELECT observed_artist_id FROM track_artists
                   WHERE id = ANY($2) AND observed_artist_id IS NOT NULL
             UNION SELECT observed_artist_id FROM contributions
                   WHERE id = ANY($3) AND observed_artist_id IS NOT NULL
         ) affected ORDER BY observed_artist_id LIMIT $4",
    )
    .bind(release_artist_ids)
    .bind(track_artist_ids)
    .bind(contribution_ids)
    .bind(limit_plus_one(MAX_CATALOG_CORRECTION_IDENTITY_ROWS)?)
    .fetch_all(&mut **tx)
    .await?)
}

fn limit_plus_one(limit: usize) -> Result<i64, CatalogStoreError> {
    i64::try_from(
        limit
            .checked_add(1)
            .ok_or(CatalogStoreError::NumericOverflow)?,
    )
    .map_err(|_| CatalogStoreError::NumericOverflow)
}

async fn retarget_observations(
    tx: &mut Transaction<'_, Postgres>,
    observation_ids: &[Uuid],
    retarget: &ArtistObservationRetarget<'_>,
) -> Result<(), CatalogStoreError> {
    if observation_ids.is_empty() {
        return Ok(());
    }
    ensure_correction_scope(
        observation_ids.len(),
        CatalogCorrectionDimension::IdentityRows,
        MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
    )?;
    let locked_observations = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM observed_artists WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(observation_ids)
    .fetch_all(&mut **tx)
    .await?;
    if locked_observations.len() != observation_ids.len() {
        return Err(CatalogStoreError::MutationConflict);
    }
    let predecessors = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT current.observed_artist_id, head.id
         FROM artist_match_current current
         JOIN artist_match_decisions authoritative ON authoritative.id = current.decision_id
         JOIN LATERAL (
             SELECT decision.id FROM artist_match_decisions decision
             WHERE decision.observed_artist_id = current.observed_artist_id
               AND NOT EXISTS (
                   SELECT 1 FROM artist_match_decisions successor
                   WHERE successor.predecessor_decision_id = decision.id)
             LIMIT 1
         ) head ON true
         WHERE current.observed_artist_id = ANY($1)
           AND authoritative.artist_id = $2
         ORDER BY current.observed_artist_id
         FOR UPDATE OF current, authoritative",
    )
    .bind(observation_ids)
    .bind(retarget.expected_artist_id.into_uuid())
    .fetch_all(&mut **tx)
    .await?;
    if predecessors.len() != observation_ids.len() {
        return Err(CatalogStoreError::MutationConflict);
    }
    let decision_ids = predecessors
        .iter()
        .map(|_| Uuid::now_v7())
        .collect::<Vec<_>>();
    let persisted_observation_ids = predecessors
        .iter()
        .map(|(observation_id, _)| *observation_id)
        .collect::<Vec<_>>();
    let predecessor_ids = predecessors
        .iter()
        .map(|(_, predecessor_id)| *predecessor_id)
        .collect::<Vec<_>>();
    let reconciliation_evidence_ids = match retarget.decision_authority.automated_evidence() {
        Some(evidence) => {
            insert_automated_reconciliation_evidence(
                tx,
                &persisted_observation_ids,
                retarget.artist_id,
                retarget.actor,
                retarget.now,
                evidence,
            )
            .await?
        }
        None => Vec::new(),
    };
    let (authority, confidence, is_locked) = retarget.decision_authority.database_values();
    sqlx::query(
        "INSERT INTO artist_match_decisions (
            id, observed_artist_id, artist_id, outcome, authority,
            confidence, reason, is_locked, predecessor_decision_id,
            payload, created_at
         )
         SELECT input.decision_id, input.observed_artist_id, $4,
                'confirmed', $8, $9, $5, $10,
                input.predecessor_decision_id,
                jsonb_build_object(
                    'actor', $6::text,
                    'correction', true,
                    'reconciliation_predicate', $11::text,
                    'evidence_ids', COALESCE((
                        SELECT jsonb_agg(evidence.id ORDER BY evidence.id)
                        FROM artist_match_evidence evidence
                        WHERE evidence.id=ANY($12::uuid[])
                          AND evidence.observed_artist_id=input.observed_artist_id
                    ), '[]'::jsonb)
                ), $7
         FROM unnest($1::uuid[], $2::uuid[], $3::uuid[])
              AS input(decision_id, observed_artist_id, predecessor_decision_id)",
    )
    .bind(&decision_ids)
    .bind(&persisted_observation_ids)
    .bind(&predecessor_ids)
    .bind(retarget.artist_id.into_uuid())
    .bind(retarget.reason)
    .bind(retarget.actor)
    .bind(retarget.now)
    .bind(authority)
    .bind(confidence)
    .bind(is_locked)
    .bind(retarget.decision_authority.predicate())
    .bind(&reconciliation_evidence_ids)
    .execute(&mut **tx)
    .await?;
    let updated = sqlx::query(
        "UPDATE artist_match_current current
         SET decision_id = input.decision_id, updated_at = $3
         FROM unnest($1::uuid[], $2::uuid[]) AS input(observed_artist_id, decision_id)
         WHERE current.observed_artist_id = input.observed_artist_id",
    )
    .bind(&persisted_observation_ids)
    .bind(&decision_ids)
    .bind(retarget.now)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if usize::try_from(updated).map_err(|_| CatalogStoreError::NumericOverflow)?
        != observation_ids.len()
    {
        return Err(CatalogStoreError::MutationConflict);
    }
    Ok(())
}

async fn insert_automated_reconciliation_evidence(
    tx: &mut Transaction<'_, Postgres>,
    observation_ids: &[Uuid],
    candidate_artist_id: ArtistId,
    actor: &str,
    now: OffsetDateTime,
    evidence: &ArtistReconciliationEvidence,
) -> Result<Vec<Uuid>, CatalogStoreError> {
    let facts: &[(&str, i32)] = match evidence {
        ArtistReconciliationEvidence::EmbeddedExternalId { .. } => {
            &[("embedded_external_id", 9_500)]
        }
        ArtistReconciliationEvidence::SameReleaseTrackCredit { .. } => {
            &[("name_similarity", 9_000), ("release_context", 9_500)]
        }
        ArtistReconciliationEvidence::RepeatedPrimaryReleaseCredit { .. } => {
            &[("name_similarity", 9_000), ("release_context", 9_000)]
        }
        // A repeated credit is the same evidence either way; a Track credit is
        // slightly weaker context than a primary Release credit.
        ArtistReconciliationEvidence::RepeatedCredit { .. } => {
            &[("name_similarity", 9_000), ("track_participation", 8_500)]
        }
    };
    let mut ids = Vec::with_capacity(observation_ids.len().saturating_mul(facts.len()));
    let mut observations = Vec::with_capacity(ids.capacity());
    let mut kinds = Vec::with_capacity(ids.capacity());
    let mut confidences = Vec::with_capacity(ids.capacity());
    for observation_id in observation_ids {
        for (kind, confidence) in facts {
            ids.push(Uuid::now_v7());
            observations.push(*observation_id);
            kinds.push(*kind);
            confidences.push(*confidence);
        }
    }
    if ids.is_empty() {
        return Ok(ids);
    }
    sqlx::query(
        "INSERT INTO artist_match_evidence (
            id, observed_artist_id, candidate_artist_id, evidence_kind,
            polarity, confidence, source, payload, created_at
         )
         SELECT input.id, input.observed_artist_id, $5, input.evidence_kind,
                'supports', input.confidence, $6, $7, $8
         FROM unnest($1::uuid[], $2::uuid[], $3::text[], $4::integer[])
              AS input(id, observed_artist_id, evidence_kind, confidence)",
    )
    .bind(&ids)
    .bind(&observations)
    .bind(&kinds)
    .bind(&confidences)
    .bind(candidate_artist_id.into_uuid())
    .bind(actor)
    .bind(evidence.payload())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(ids)
}

async fn audio_release_ids(
    tx: &mut Transaction<'_, Postgres>,
    track_ids: &[Uuid],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT release_id FROM tracks WHERE id = ANY($1)
         ORDER BY release_id LIMIT $2",
    )
    .bind(track_ids)
    .bind(limit_plus_one(MAX_CATALOG_CORRECTION_RELEASES)?)
    .fetch_all(&mut **tx)
    .await?)
}

#[allow(clippy::too_many_arguments)]
async fn audit_audio_identity_change(
    tx: &mut Transaction<'_, Postgres>,
    audio_identity_id: Uuid,
    event_type: &str,
    actor: &str,
    reason: &str,
    now: OffsetDateTime,
    bump_revision: bool,
) -> Result<(), CatalogStoreError> {
    let revision = if bump_revision {
        sqlx::query_scalar::<_, i64>(
            "UPDATE audio_identities SET revision = revision + 1, updated_at = $2
             WHERE id = $1 RETURNING revision",
        )
        .bind(audio_identity_id)
        .bind(now)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>("SELECT revision FROM audio_identities WHERE id = $1")
            .bind(audio_identity_id)
            .fetch_one(&mut **tx)
            .await?
    };
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog','audio_identity',$1,'upsert',$2,$3,0,$3)",
    )
    .bind(audio_identity_id)
    .bind(revision)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    audit_event(
        tx,
        "audio_identity",
        audio_identity_id,
        event_type,
        actor,
        reason,
        serde_json::json!({ "revision": revision }),
        now,
    )
    .await
}

/// Lock a bounded correction set before a multi-Track UPDATE so it cannot
/// acquire tuple locks in executor-dependent order against another writer.
async fn lock_correction_tracks(
    tx: &mut Transaction<'_, Postgres>,
    track_ids: &[Uuid],
) -> Result<(), CatalogStoreError> {
    let mut ordered = track_ids.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tracks WHERE id=ANY($1) ORDER BY id FOR NO KEY UPDATE",
    )
    .bind(&ordered)
    .fetch_all(&mut **tx)
    .await?;
    if locked != ordered {
        return Err(CatalogStoreError::MutationConflict);
    }
    Ok(())
}

async fn enqueue_track_revisions(
    tx: &mut Transaction<'_, Postgres>,
    track_ids: &[Uuid],
    now: OffsetDateTime,
    bump_revision: bool,
) -> Result<(), CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let inserted = if bump_revision {
        lock_correction_tracks(tx, track_ids).await?;
        sqlx::query(
            "WITH updated AS (
                UPDATE tracks SET revision = revision + 1, updated_at = $2
                WHERE id = ANY($1) RETURNING id, revision
             )
             INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, attempt_count, created_at
             )
             SELECT 'tantivy.catalog', 'track', id, 'upsert', revision, $2, 0, $2
             FROM updated",
        )
        .bind(track_ids)
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, attempt_count, created_at
             )
             SELECT 'tantivy.catalog', 'track', id, 'upsert', revision, $2, 0, $2
             FROM tracks WHERE id = ANY($1)",
        )
        .bind(track_ids)
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected()
    };
    if usize::try_from(inserted).map_err(|_| CatalogStoreError::NumericOverflow)? != track_ids.len()
    {
        return Err(CatalogStoreError::MutationConflict);
    }
    Ok(())
}

fn audio_evidence_kind_name(value: AudioIdentityEvidenceKind) -> &'static str {
    match value {
        AudioIdentityEvidenceKind::ByteHash => "byte_hash",
        AudioIdentityEvidenceKind::AcousticFingerprint => "acoustic_fingerprint",
        AudioIdentityEvidenceKind::Duration => "duration",
        AudioIdentityEvidenceKind::Metadata => "metadata",
        AudioIdentityEvidenceKind::ExternalIdentifier => "external_identifier",
        AudioIdentityEvidenceKind::CatalogContext => "catalog_context",
        AudioIdentityEvidenceKind::Manual => "manual",
    }
}

fn polarity_name(value: EvidencePolarity) -> &'static str {
    match value {
        EvidencePolarity::Supports => "supports",
        EvidencePolarity::Conflicts => "conflicts",
    }
}

fn authority_parts(value: &DecisionAuthority) -> (&'static str, String) {
    match value {
        DecisionAuthority::Automated { resolver_version } => {
            ("automated", resolver_version.clone())
        }
        DecisionAuthority::Administrator { actor } => ("admin", actor.clone()),
    }
}

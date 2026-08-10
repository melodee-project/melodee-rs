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
    CatalogCorrectionDimension, CatalogStore, CatalogStoreError,
    MAX_CATALOG_CORRECTION_IDENTITY_ROWS, MAX_CATALOG_CORRECTION_RELEASES,
    MAX_CATALOG_CORRECTION_TRACKS, MAX_CATALOG_SPLIT_ASSIGNMENTS,
};

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

    /// Merges an Artist into a durable redirect and retargets semantic catalog links.
    pub async fn merge_artist(
        &self,
        pool: &PgPool,
        merge: &ArtistMerge,
        resolver_version: &str,
    ) -> Result<(), CatalogStoreError> {
        ArtistMerge::new(merge.clone())?;
        let mut tx = pool.begin().await?;
        let artists = sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "SELECT id, public_api_id, status FROM artists
             WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind([
            merge.source_artist_id.into_uuid(),
            merge.target_artist_id.into_uuid(),
        ])
        .fetch_all(&mut *tx)
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
        let identity_row_count = affected_artist_identity_row_count(
            &mut tx,
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
        .fetch_one(&mut *tx)
        .await?;
        if self_link {
            return Err(CatalogStoreError::MutationConflict);
        }
        let release_ids = affected_artist_release_ids(&mut tx, merge.source_artist_id).await?;
        ensure_correction_scope(
            release_ids.len(),
            CatalogCorrectionDimension::Releases,
            MAX_CATALOG_CORRECTION_RELEASES,
        )?;
        let mut track_ids = release_track_ids(&mut tx, &release_ids)
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
            .fetch_all(&mut *tx)
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
        .fetch_all(&mut *tx)
        .await?;
        retarget_observations(
            &mut tx,
            &observation_ids,
            merge.source_artist_id,
            merge.target_artist_id,
            &merge.actor,
            &merge.reason,
            merge.occurred_at,
        )
        .await?;
        let release_artist_count = update_artist_links(
            &mut tx,
            "release_artists",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let track_artist_count = update_artist_links(
            &mut tx,
            "track_artists",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let contribution_count = update_artist_links(
            &mut tx,
            "contributions",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let alias_count = update_artist_links(
            &mut tx,
            "artist_aliases",
            merge.source_artist_id,
            merge.target_artist_id,
        )
        .await?;
        let external_count = update_artist_links(
            &mut tx,
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
        .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "UPDATE artists
             SET status = 'merged', merged_into_artist_id = $2,
                 revision = revision + 1, updated_at = $3
             WHERE id = $1",
        )
        .bind(merge.source_artist_id.into_uuid())
        .bind(merge.target_artist_id.into_uuid())
        .bind(merge.occurred_at)
        .execute(&mut *tx)
        .await?;
        rebuild_participation_for_tracks(&mut tx, &track_ids, merge.occurred_at).await?;
        enqueue_track_revisions(&mut tx, &track_ids, merge.occurred_at, true).await?;
        let moved = serde_json::json!({
            "release_artists": release_artist_count,
            "track_artists": track_artist_count,
            "contributions": contribution_count,
            "aliases": alias_count,
            "external_identities": external_count,
            "relationships": relationship_count,
            "memberships": membership_count,
            "observations": observation_ids.len(),
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
        .execute(&mut *tx)
        .await?;
        for artist_id in [merge.source_artist_id, merge.target_artist_id] {
            audit_artist_change(
                &mut tx,
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
            &mut tx,
            &release_ids,
            resolver_version,
            &merge.actor,
            "artist_merged",
            merge.occurred_at,
        )
        .await?;
        tx.commit().await?;
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
        let track_ids = release_track_ids(&mut tx, &release_ids).await?;
        ensure_correction_scope(
            track_ids.len(),
            CatalogCorrectionDimension::Tracks,
            MAX_CATALOG_CORRECTION_TRACKS,
        )?;
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
            split.source_artist_id,
            split.new_artist_id,
            &split.actor,
            &split.reason,
            split.occurred_at,
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
        tx.commit().await?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum AssignmentKind {
    ReleaseArtist,
    TrackArtist,
    Contribution,
    Alias,
    ExternalIdentity,
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
    artist_id: ArtistId,
) -> Result<Vec<Uuid>, CatalogStoreError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT release_id FROM (
             SELECT release_id FROM release_artists WHERE artist_id = $1
             UNION SELECT track.release_id FROM track_artists artist
                   JOIN tracks track ON track.id = artist.track_id
                   WHERE artist.artist_id = $1
             UNION SELECT release_id FROM contributions WHERE artist_id = $1
             UNION SELECT observed.release_id
                   FROM artist_match_current current
                   JOIN artist_match_decisions decision ON decision.id = current.decision_id
                   JOIN observed_artists observed ON observed.id = current.observed_artist_id
                   WHERE decision.artist_id = $1
         ) affected ORDER BY release_id LIMIT $2",
    )
    .bind(artist_id.into_uuid())
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
    expected_artist_id: ArtistId,
    artist_id: ArtistId,
    actor: &str,
    reason: &str,
    now: OffsetDateTime,
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
    .bind(expected_artist_id.into_uuid())
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
    sqlx::query(
        "INSERT INTO artist_match_decisions (
            id, observed_artist_id, artist_id, outcome, authority,
            confidence, reason, is_locked, predecessor_decision_id,
            payload, created_at
         )
         SELECT input.decision_id, input.observed_artist_id, $4,
                'confirmed', 'admin', 10000, $5, true,
                input.predecessor_decision_id,
                jsonb_build_object('actor', $6::text, 'correction', true), $7
         FROM unnest($1::uuid[], $2::uuid[], $3::uuid[])
              AS input(decision_id, observed_artist_id, predecessor_decision_id)",
    )
    .bind(&decision_ids)
    .bind(&persisted_observation_ids)
    .bind(&predecessor_ids)
    .bind(artist_id.into_uuid())
    .bind(reason)
    .bind(actor)
    .bind(now)
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
    .bind(now)
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

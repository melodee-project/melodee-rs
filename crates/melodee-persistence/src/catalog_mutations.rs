//! Audited catalog mutations outside the manifest import hot path.

use melodee_domain::{
    Artist, ArtistAlias, ArtistAliasType, ArtistId, ArtistMatchDecision, ArtistMatchEvidence,
    ArtistMatchOutcome, ArtistNegativeAssertion, ArtistRelationship, ArtistStatus, ArtistType,
    CatalogImportId, CatalogSemanticOverlay, CatalogSemanticOverlayId, Confidence, Contribution,
    ContributionRole, ContributionTarget, DecisionAuthority, DisplayArtist, DisplayArtistOwner,
    EvidenceKind, EvidencePolarity, ExternalIdentity, ExternalIdentityOwner, GroupMembership,
    NewCatalogSemanticOverlay, ObservedArtistId, OverlayArtistResolution, OverlayAudioIdentityLink,
    OverlayDisplayArtist, OverlayReferences, ReleaseArtistId, ReleaseId, TrackArtistId, TrackId,
};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    CatalogCorrectionDimension, CatalogStore, CatalogStoreError, LegacyCatalogEntityType,
    LegacyCatalogIdMapping, MAX_CATALOG_CORRECTION_OVERLAY_REFERENCES,
    MAX_CATALOG_CORRECTION_RELEASES, MAX_CATALOG_CORRECTION_TRACKS, ProviderNegativeAssertion,
    StoredArtistCandidate,
};

impl CatalogStore {
    /// Looks up canonical/alias candidates without turning a name into identity.
    pub async fn find_artist_candidates_by_name(
        &self,
        pool: &PgPool,
        observed_artist_id: melodee_domain::ObservedArtistId,
        normalized_name: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtistCandidate>, CatalogStoreError> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, String, i32)>(
            "WITH seeds AS (
                (SELECT observed.id AS owner_id,
                        'canonical_name'::text AS evidence_kind,
                        observed.canonical_name AS evidence_value,
                        10000 AS confidence
                 FROM artists observed
                 WHERE observed.normalized_name = $2
                 ORDER BY observed.id LIMIT $3)
                UNION ALL
                (SELECT alias.artist_id, 'alias'::text, alias.alias, alias.confidence
                FROM artist_aliases alias
                WHERE alias.normalized_alias = $2
                ORDER BY alias.id LIMIT $3)
             ), candidates AS (
                SELECT resolved.id, resolved.public_api_id, resolved.canonical_name,
                       seed.evidence_kind, seed.evidence_value, seed.confidence
                FROM seeds seed
                JOIN artists resolved
                  ON resolved.id = resolve_artist_redirect(seed.owner_id)
                WHERE resolved.status IN ('provisional','verified')
             )
             SELECT DISTINCT ON (candidate.id) candidate.*
             FROM candidates candidate
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM artist_negative_assertions negative
                 WHERE negative.observed_artist_id = $1
                   AND negative.candidate_artist_id IS NOT NULL
                   AND resolve_artist_redirect(negative.candidate_artist_id) = candidate.id
                   AND negative.assertion_kind = 'do_not_match'
                   AND NOT EXISTS (
                       SELECT 1 FROM artist_negative_assertions successor
                       WHERE successor.predecessor_assertion_id = negative.id)
             )
             ORDER BY candidate.id, candidate.confidence DESC, candidate.evidence_kind
             LIMIT $3",
        )
        .bind(observed_artist_id.into_uuid())
        .bind(normalized_name)
        .bind(i64::from(limit.clamp(1, 1_000)))
        .fetch_all(pool)
        .await?;
        candidate_rows(rows)
    }

    /// Looks up exact provider evidence, excluding only its exact active negative key.
    pub async fn find_artist_candidates_by_external_id(
        &self,
        pool: &PgPool,
        observed_artist_id: melodee_domain::ObservedArtistId,
        provider: &str,
        external_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtistCandidate>, CatalogStoreError> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, String, i32)>(
            "WITH seeds AS (
                SELECT external.id, external.artist_id, external.external_id,
                       external.confidence
                FROM artist_external_identities external
                WHERE external.provider = $2 AND external.external_id = $3
                ORDER BY external.confidence DESC, external.id
                LIMIT $4
             )
             SELECT resolved.id, resolved.public_api_id, resolved.canonical_name,
                    'external_identity'::text, external.external_id, external.confidence
             FROM seeds external
             JOIN artists resolved
               ON resolved.id = resolve_artist_redirect(external.artist_id)
             WHERE resolved.status IN ('provisional','verified')
               AND NOT EXISTS (
                   SELECT 1
                   FROM artist_negative_assertions negative
                   WHERE negative.observed_artist_id = $1
                     AND negative.candidate_provider = $2
                     AND negative.candidate_external_id = $3
                     AND negative.assertion_kind = 'do_not_match'
                     AND NOT EXISTS (
                         SELECT 1 FROM artist_negative_assertions successor
                         WHERE successor.predecessor_assertion_id = negative.id)
               )
             ORDER BY external.confidence DESC, external.id
             LIMIT $4",
        )
        .bind(observed_artist_id.into_uuid())
        .bind(provider)
        .bind(external_id)
        .bind(i64::from(limit.clamp(1, 1_000)))
        .fetch_all(pool)
        .await?;
        candidate_rows(rows)
    }

    /// Inserts a Melodee-owned Artist without treating its name as a key.
    pub async fn create_artist(
        &self,
        pool: &PgPool,
        artist: &Artist,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;
        insert_artist(&mut *tx, artist).await?;
        audit_artist_change(
            &mut tx,
            artist.id().into_uuid(),
            "artist_created",
            "catalog-store",
            "Artist identity created",
            artist.created_at(),
            false,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Persists one evidence-backed alias for an existing Artist.
    pub async fn add_artist_alias(
        &self,
        pool: &PgPool,
        alias: &ArtistAlias,
        created_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        ArtistAlias::new(alias.clone())?;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO artist_aliases (
                id, artist_id, alias, normalized_alias, alias_type, locale,
                evidence_source, confidence, is_locked, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(alias.id.into_uuid())
        .bind(alias.artist_id.into_uuid())
        .bind(&alias.value)
        .bind(&alias.normalized_value)
        .bind(alias_type_name(alias.alias_type))
        .bind(&alias.locale)
        .bind(&alias.evidence_source)
        .bind(i32::from(alias.confidence.basis_points()))
        .bind(alias.locked)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        audit_artist_change(
            &mut tx,
            alias.artist_id.into_uuid(),
            "artist_alias_added",
            &alias.evidence_source,
            "evidence-backed alias added",
            created_at,
            true,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Persists normalized Artist provider evidence with declared uniqueness scope.
    pub async fn add_artist_external_identity(
        &self,
        pool: &PgPool,
        external: &ExternalIdentity,
        created_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        ExternalIdentity::new(external.clone())?;
        let ExternalIdentityOwner::Artist(artist_id) = external.owner else {
            return Err(CatalogStoreError::InvalidRequest);
        };
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO artist_external_identities (
                id, artist_id, entity_type, provider, external_id,
                provider_unique_scope, confidence, evidence_source,
                verified_at, is_locked, created_at
             ) VALUES ($1,$2,'artist',$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(external.id.into_uuid())
        .bind(artist_id.into_uuid())
        .bind(&external.provider)
        .bind(&external.external_id)
        .bind(external.provider_guarantees_uniqueness)
        .bind(i32::from(external.confidence.basis_points()))
        .bind(&external.evidence_source)
        .bind(external.verified_at)
        .bind(external.locked)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        audit_artist_change(
            &mut tx,
            artist_id.into_uuid(),
            "artist_external_identity_added",
            &external.evidence_source,
            "external identity evidence added",
            created_at,
            true,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Persists a relationship without implying identity equality.
    pub async fn add_artist_relationship(
        &self,
        pool: &PgPool,
        relationship: &ArtistRelationship,
        created_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        ArtistRelationship::new(relationship.clone())?;
        let dates = relationship.active_dates;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO artist_relationships (
                id, source_artist_id, target_artist_id, relationship_type,
                role, start_date, end_date, confidence, evidence_source,
                is_locked, created_at
             ) VALUES ($1,$2,$3,$4,NULL,$5,$6,$7,$8,false,$9)",
        )
        .bind(relationship.id.into_uuid())
        .bind(relationship.subject_artist_id.into_uuid())
        .bind(relationship.object_artist_id.into_uuid())
        .bind(relationship.kind.as_str())
        .bind(dates.and_then(melodee_domain::DateRange::start))
        .bind(dates.and_then(melodee_domain::DateRange::end))
        .bind(i32::from(relationship.confidence.basis_points()))
        .bind(&relationship.evidence_source)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        for artist_id in [
            relationship.subject_artist_id.into_uuid(),
            relationship.object_artist_id.into_uuid(),
        ] {
            audit_artist_change(
                &mut tx,
                artist_id,
                "artist_relationship_added",
                &relationship.evidence_source,
                "evidence-backed Artist relationship added",
                created_at,
                true,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Appends immutable match evidence; `payload` retains resolver detail.
    pub async fn record_artist_match_evidence(
        &self,
        pool: &PgPool,
        evidence: &ArtistMatchEvidence,
        payload: serde_json::Value,
        created_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        ArtistMatchEvidence::new(evidence.clone())?;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO artist_match_evidence (
                id, observed_artist_id, candidate_artist_id, evidence_kind,
                polarity, confidence, source, payload, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(evidence.id.into_uuid())
        .bind(evidence.observed_artist_id.into_uuid())
        .bind(evidence.candidate_artist_id.into_uuid())
        .bind(evidence_kind_name(evidence.kind))
        .bind(polarity_name(evidence.polarity))
        .bind(i32::from(evidence.confidence.basis_points()))
        .bind(&evidence.source)
        .bind(payload)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        audit_event(
            &mut tx,
            "observed_artist",
            evidence.observed_artist_id.into_uuid(),
            "artist_match_evidence_recorded",
            &evidence.source,
            "immutable match evidence recorded",
            serde_json::json!({
                "evidence_id": evidence.id,
                "candidate_artist_id": evidence.candidate_artist_id,
            }),
            created_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Appends and advances a match decision while honoring manual locks and negatives.
    pub async fn record_artist_match_decision(
        &self,
        pool: &PgPool,
        decision: &ArtistMatchDecision,
        confidence: Confidence,
        payload: serde_json::Value,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;
        // The observation row exists before any decision and therefore also
        // serializes concurrent first decisions whose current row is absent.
        let release_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT release_id FROM observed_artists WHERE id = $1 FOR UPDATE",
        )
        .bind(decision.observed_artist_id().into_uuid())
        .fetch_one(&mut *tx)
        .await?;
        let evidence_ids = decision
            .evidence_ids()
            .iter()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        let evidence_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artist_match_evidence
             WHERE id = ANY($1) AND observed_artist_id = $2
               AND candidate_artist_id = $3",
        )
        .bind(&evidence_ids)
        .bind(decision.observed_artist_id().into_uuid())
        .bind(decision.artist_id().into_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(evidence_count).map_err(|_| CatalogStoreError::NumericOverflow)?
            != evidence_ids.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let current = sqlx::query_as::<_, (bool, String)>(
            "SELECT d.is_locked, d.authority
             FROM artist_match_current current
             JOIN artist_match_decisions d ON d.id = current.decision_id
             WHERE current.observed_artist_id = $1
             FOR UPDATE OF current, d",
        )
        .bind(decision.observed_artist_id().into_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let predecessor = sqlx::query_scalar::<_, Uuid>(
            "SELECT decision.id FROM artist_match_decisions decision
             WHERE decision.observed_artist_id = $1
               AND NOT EXISTS (
                   SELECT 1 FROM artist_match_decisions successor
                   WHERE successor.predecessor_decision_id = decision.id)
             LIMIT 1",
        )
        .bind(decision.observed_artist_id().into_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let automated = matches!(decision.authority(), DecisionAuthority::Automated { .. });
        if current
            .as_ref()
            .is_some_and(|(locked, authority)| *locked && authority == "admin" && automated)
        {
            return Err(CatalogStoreError::LockedDecision);
        }
        if automated
            && local_candidate_suppressed(
                &mut tx,
                decision.observed_artist_id().into_uuid(),
                decision.artist_id().into_uuid(),
            )
            .await?
        {
            return Err(CatalogStoreError::NegativeAssertion);
        }
        let (authority, authority_detail) = authority_parts(decision.authority());
        let mut persisted_payload = if payload.is_object() {
            payload
        } else {
            serde_json::json!({ "resolver_payload": payload })
        };
        persisted_payload["authority_detail"] = serde_json::Value::String(authority_detail);
        persisted_payload["evidence_ids"] = serde_json::to_value(decision.evidence_ids())?;
        sqlx::query(
            "INSERT INTO artist_match_decisions (
                id, observed_artist_id, artist_id, outcome, authority,
                confidence, reason, is_locked, predecessor_decision_id,
                payload, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(decision.id().into_uuid())
        .bind(decision.observed_artist_id().into_uuid())
        .bind(decision.artist_id().into_uuid())
        .bind(match decision.outcome() {
            ArtistMatchOutcome::Proposed => "proposed",
            ArtistMatchOutcome::Confirmed => "confirmed",
        })
        .bind(authority)
        .bind(i32::from(confidence.basis_points()))
        .bind(decision.reason())
        .bind(decision.locked())
        .bind(predecessor)
        .bind(persisted_payload)
        .bind(decision.decided_at())
        .execute(&mut *tx)
        .await?;
        let resolver_version = match decision.authority() {
            DecisionAuthority::Automated { resolver_version } => resolver_version.as_str(),
            DecisionAuthority::Administrator { .. } => "manual-artist-resolution-v1",
        };
        let actor = match decision.authority() {
            DecisionAuthority::Automated { resolver_version } => resolver_version.as_str(),
            DecisionAuthority::Administrator { actor } => actor.as_str(),
        };
        if decision.outcome() == ArtistMatchOutcome::Proposed {
            audit_event(
                &mut tx,
                "observed_artist",
                decision.observed_artist_id().into_uuid(),
                "artist_match_decision_proposed",
                actor,
                decision.reason(),
                serde_json::json!({
                    "decision_id": decision.id(),
                    "artist_id": decision.artist_id(),
                    "outcome": decision.outcome(),
                }),
                decision.decided_at(),
            )
            .await?;
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO artist_match_current (observed_artist_id, decision_id, updated_at)
             VALUES ($1,$2,$3)
             ON CONFLICT (observed_artist_id) DO UPDATE
             SET decision_id = EXCLUDED.decision_id, updated_at = EXCLUDED.updated_at",
        )
        .bind(decision.observed_artist_id().into_uuid())
        .bind(decision.id().into_uuid())
        .bind(decision.decided_at())
        .execute(&mut *tx)
        .await?;
        let affected_tracks = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tracks WHERE release_id = $1 ORDER BY id LIMIT $2",
        )
        .bind(release_id)
        .bind(correction_limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
        .fetch_all(&mut *tx)
        .await?;
        ensure_correction_scope(
            affected_tracks.len(),
            CatalogCorrectionDimension::Tracks,
            MAX_CATALOG_CORRECTION_TRACKS,
        )?;
        for statement in [
            "UPDATE release_artists SET artist_id = $2 WHERE observed_artist_id = $1",
            "UPDATE track_artists SET artist_id = $2 WHERE observed_artist_id = $1",
            "UPDATE contributions SET artist_id = $2 WHERE observed_artist_id = $1",
        ] {
            sqlx::query(statement)
                .bind(decision.observed_artist_id().into_uuid())
                .bind(decision.artist_id().into_uuid())
                .execute(&mut *tx)
                .await?;
        }
        rebuild_participation_for_tracks(&mut tx, &affected_tracks, decision.decided_at()).await?;
        audit_event(
            &mut tx,
            "observed_artist",
            decision.observed_artist_id().into_uuid(),
            "artist_match_decision_recorded",
            actor,
            decision.reason(),
            serde_json::json!({
                "decision_id": decision.id(),
                "artist_id": decision.artist_id(),
                "outcome": decision.outcome(),
                "locked": decision.locked(),
            }),
            decision.decided_at(),
        )
        .await?;
        append_overlay_revisions(
            &mut tx,
            &[release_id],
            resolver_version,
            actor,
            "artist_match_decision_changed",
            decision.decided_at(),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Appends a local-Artist do-not-match assertion.
    pub async fn record_artist_negative_assertion(
        &self,
        pool: &PgPool,
        assertion: &ArtistNegativeAssertion,
    ) -> Result<(), CatalogStoreError> {
        ArtistNegativeAssertion::new(assertion.clone())?;
        let (authority, detail) = authority_parts(&assertion.authority);
        let mut tx = pool.begin().await?;
        lock_observed_artist(&mut tx, assertion.observed_artist_id.into_uuid()).await?;
        ensure_negative_root_absent(
            &mut tx,
            assertion.observed_artist_id.into_uuid(),
            Some(assertion.rejected_artist_id.into_uuid()),
            None,
            None,
        )
        .await?;
        sqlx::query(
            "INSERT INTO artist_negative_assertions (
                id, observed_artist_id, candidate_artist_id, candidate_provider,
                candidate_external_id, assertion_kind, predecessor_assertion_id,
                authority, reason, is_locked, created_at
             ) VALUES ($1,$2,$3,NULL,NULL,'do_not_match',NULL,$4,$5,$6,$7)",
        )
        .bind(assertion.id.into_uuid())
        .bind(assertion.observed_artist_id.into_uuid())
        .bind(assertion.rejected_artist_id.into_uuid())
        .bind(authority)
        .bind(&assertion.reason)
        .bind(assertion.locked)
        .bind(assertion.decided_at)
        .execute(&mut *tx)
        .await?;
        audit_event(
            &mut tx,
            "observed_artist",
            assertion.observed_artist_id.into_uuid(),
            "artist_negative_assertion_recorded",
            &detail,
            &assertion.reason,
            serde_json::json!({
                "assertion_id": assertion.id,
                "candidate_artist_id": assertion.rejected_artist_id,
                "locked": assertion.locked,
            }),
            assertion.decided_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Appends a provider-keyed do-not-match assertion before a local Artist exists.
    pub async fn record_provider_negative_assertion(
        &self,
        pool: &PgPool,
        assertion: &ProviderNegativeAssertion,
    ) -> Result<(), CatalogStoreError> {
        if assertion.id.get_version_num() != 7
            || assertion.provider.trim().is_empty()
            || assertion.external_id.trim().is_empty()
            || assertion.reason.trim().is_empty()
            || (assertion.locked && !assertion.authority.is_administrator())
        {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let (authority, detail) = authority_parts(&assertion.authority);
        let mut tx = pool.begin().await?;
        lock_observed_artist(&mut tx, assertion.observed_artist_id.into_uuid()).await?;
        ensure_negative_root_absent(
            &mut tx,
            assertion.observed_artist_id.into_uuid(),
            None,
            Some(&assertion.provider),
            Some(&assertion.external_id),
        )
        .await?;
        sqlx::query(
            "INSERT INTO artist_negative_assertions (
                id, observed_artist_id, candidate_artist_id, candidate_provider,
                candidate_external_id, assertion_kind, predecessor_assertion_id,
                authority, reason, is_locked, created_at
             ) VALUES ($1,$2,NULL,$3,$4,'do_not_match',NULL,$5,$6,$7,$8)",
        )
        .bind(assertion.id)
        .bind(assertion.observed_artist_id.into_uuid())
        .bind(&assertion.provider)
        .bind(&assertion.external_id)
        .bind(authority)
        .bind(&assertion.reason)
        .bind(assertion.locked)
        .bind(assertion.decided_at)
        .execute(&mut *tx)
        .await?;
        audit_event(
            &mut tx,
            "observed_artist",
            assertion.observed_artist_id.into_uuid(),
            "provider_negative_assertion_recorded",
            &detail,
            &assertion.reason,
            serde_json::json!({
                "assertion_id": assertion.id,
                "provider": assertion.provider,
                "external_id": assertion.external_id,
                "locked": assertion.locked,
            }),
            assertion.decided_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Appends an administrator revocation of the exact predecessor candidate.
    pub async fn revoke_artist_negative_assertion(
        &self,
        pool: &PgPool,
        predecessor_assertion_id: Uuid,
        authority: &DecisionAuthority,
        reason: &str,
        decided_at: OffsetDateTime,
    ) -> Result<Uuid, CatalogStoreError> {
        let DecisionAuthority::Administrator { actor } = authority else {
            return Err(CatalogStoreError::InvalidRequest);
        };
        if reason.trim().is_empty() {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mut tx = pool.begin().await?;
        let observed_artist_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT observed_artist_id FROM artist_negative_assertions WHERE id = $1",
        )
        .bind(predecessor_assertion_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CatalogStoreError::MutationConflict)?;
        lock_observed_artist(&mut tx, observed_artist_id).await?;
        let predecessor =
            sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<String>, Option<String>)>(
                "SELECT observed_artist_id, candidate_artist_id,
                    candidate_provider, candidate_external_id
             FROM artist_negative_assertions
             WHERE id = $1 AND assertion_kind = 'do_not_match'
               AND NOT EXISTS (
                   SELECT 1 FROM artist_negative_assertions later
                   WHERE later.predecessor_assertion_id = artist_negative_assertions.id)
             FOR UPDATE",
            )
            .bind(predecessor_assertion_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(CatalogStoreError::MutationConflict)?;
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO artist_negative_assertions (
                id, observed_artist_id, candidate_artist_id, candidate_provider,
                candidate_external_id, assertion_kind, predecessor_assertion_id,
                authority, reason, is_locked, created_at
             ) VALUES ($1,$2,$3,$4,$5,'revoke',$6,'admin',$7,true,$8)",
        )
        .bind(id)
        .bind(predecessor.0)
        .bind(predecessor.1)
        .bind(&predecessor.2)
        .bind(&predecessor.3)
        .bind(predecessor_assertion_id)
        .bind(reason)
        .bind(decided_at)
        .execute(&mut *tx)
        .await?;
        audit_event(
            &mut tx,
            "observed_artist",
            predecessor.0,
            "artist_negative_assertion_revoked",
            actor,
            reason,
            serde_json::json!({
                "assertion_id": id,
                "predecessor_assertion_id": predecessor_assertion_id,
            }),
            decided_at,
        )
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Checks only the exact observation/candidate pair, never a normalized name.
    pub async fn artist_candidate_is_suppressed(
        &self,
        pool: &PgPool,
        observed_artist_id: melodee_domain::ObservedArtistId,
        artist_id: ArtistId,
    ) -> Result<bool, CatalogStoreError> {
        let mut tx = pool.begin().await?;
        let value = local_candidate_suppressed(
            &mut tx,
            observed_artist_id.into_uuid(),
            artist_id.into_uuid(),
        )
        .await?;
        tx.commit().await?;
        Ok(value)
    }

    /// Checks an exact provider/external-ID negative candidate key.
    pub async fn provider_candidate_is_suppressed(
        &self,
        pool: &PgPool,
        observed_artist_id: melodee_domain::ObservedArtistId,
        provider: &str,
        external_id: &str,
    ) -> Result<bool, CatalogStoreError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1
                FROM artist_negative_assertions negative
                WHERE negative.observed_artist_id = $1
                  AND negative.candidate_provider = $2
                  AND negative.candidate_external_id = $3
                  AND negative.assertion_kind = 'do_not_match'
                  AND NOT EXISTS (
                      SELECT 1 FROM artist_negative_assertions successor
                      WHERE successor.predecessor_assertion_id = negative.id)
             )",
        )
        .bind(observed_artist_id.into_uuid())
        .bind(provider)
        .bind(external_id)
        .fetch_one(pool)
        .await?)
    }

    /// Inserts a date-bounded membership and rebuilds direct/inferred participation set-wise.
    pub async fn add_group_membership(
        &self,
        pool: &PgPool,
        membership: &GroupMembership,
        created_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        GroupMembership::new(membership.clone())?;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO group_memberships (
                id, member_artist_id, group_artist_id, role, start_date,
                end_date, confidence, evidence_source, is_locked, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,false,$9)",
        )
        .bind(membership.id.into_uuid())
        .bind(membership.member_artist_id.into_uuid())
        .bind(membership.group_artist_id.into_uuid())
        .bind(membership.role.as_str())
        .bind(membership.active_dates.start())
        .bind(membership.active_dates.end())
        .bind(i32::from(membership.confidence.basis_points()))
        .bind(&membership.evidence_source)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        let affected_tracks = sqlx::query_scalar::<_, Uuid>(
            "SELECT track_id FROM artist_track_associations
             WHERE (artist_id = $1 AND participation_kind = 'direct' AND is_performance)
                OR (participation_kind = 'inferred_group' AND source_key = $2)
             ORDER BY track_id LIMIT $3",
        )
        .bind(membership.group_artist_id.into_uuid())
        .bind(membership.id.into_uuid())
        .bind(correction_limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
        .fetch_all(&mut *tx)
        .await?;
        ensure_correction_scope(
            membership.excluded_tracks.len(),
            CatalogCorrectionDimension::Tracks,
            MAX_CATALOG_CORRECTION_TRACKS,
        )?;
        let excluded_track_ids = membership
            .excluded_tracks
            .iter()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        if !excluded_track_ids.is_empty() {
            sqlx::query(
                "INSERT INTO group_membership_track_exclusions (
                    membership_id, track_id, reason, created_at
                 )
                 SELECT $1, track_id, 'explicit membership exclusion', $3
                 FROM unnest($2::uuid[]) AS track_id",
            )
            .bind(membership.id.into_uuid())
            .bind(&excluded_track_ids)
            .bind(created_at)
            .execute(&mut *tx)
            .await?;
        }
        rebuild_participation_for_tracks(&mut tx, &affected_tracks, created_at).await?;
        for artist_id in [
            membership.member_artist_id.into_uuid(),
            membership.group_artist_id.into_uuid(),
        ] {
            audit_artist_change(
                &mut tx,
                artist_id,
                "group_membership_added",
                &membership.evidence_source,
                "date-bounded group membership added",
                created_at,
                true,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Persists a direct credit and rebuilds the participation projection set-wise.
    pub async fn add_contribution(
        &self,
        pool: &PgPool,
        contribution: &Contribution,
        created_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        Contribution::new(contribution.clone())?;
        let mut tx = pool.begin().await?;
        let (release_id, track_id) = match contribution.target {
            ContributionTarget::Release(release_id) => (release_id.into_uuid(), None),
            ContributionTarget::Track(track_id) => {
                let release_id =
                    sqlx::query_scalar::<_, Uuid>("SELECT release_id FROM tracks WHERE id = $1")
                        .bind(track_id.into_uuid())
                        .fetch_optional(&mut *tx)
                        .await?
                        .ok_or(CatalogStoreError::MutationConflict)?;
                (release_id, Some(track_id.into_uuid()))
            }
        };
        sqlx::query(
            "INSERT INTO contributions (
                id, release_id, track_id, artist_id, observed_artist_id,
                role, role_detail, evidence_source, display_value,
                presentation_order, is_performance, created_at
             ) VALUES ($1,$2,$3,$4,NULL,$5,NULL,$6,$7,0,$8,$9)",
        )
        .bind(contribution.id.into_uuid())
        .bind(release_id)
        .bind(track_id)
        .bind(contribution.artist_id.into_uuid())
        .bind(contribution_role_name(contribution.role))
        .bind(&contribution.evidence_source)
        .bind(&contribution.credit_text)
        .bind(contribution.role.is_performance_role())
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        let affected_tracks = if let Some(track_id) = track_id {
            vec![track_id]
        } else {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM tracks WHERE release_id = $1 ORDER BY id LIMIT $2",
            )
            .bind(release_id)
            .bind(correction_limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
            .fetch_all(&mut *tx)
            .await?
        };
        rebuild_participation_for_tracks(&mut tx, &affected_tracks, created_at).await?;
        append_overlay_revisions(
            &mut tx,
            &[release_id],
            "manual-contribution-v1",
            &contribution.evidence_source,
            "contribution_added",
            created_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Rebuilds direct and inferred participation only for explicitly affected Tracks.
    pub async fn rebuild_artist_track_associations(
        &self,
        pool: &PgPool,
        track_ids: &[melodee_domain::TrackId],
        rebuilt_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;
        let track_ids = track_ids
            .iter()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        rebuild_participation_for_tracks(&mut tx, &track_ids, rebuilt_at).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Explicit maintenance-only whole-projection rebuild; never used by normal mutations.
    pub async fn rebuild_all_artist_track_associations_for_maintenance(
        &self,
        pool: &PgPool,
        rebuilt_at: OffsetDateTime,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;
        let track_ids = sqlx::query_scalar::<_, Uuid>("SELECT id FROM tracks ORDER BY id LIMIT $1")
            .bind(correction_limit_plus_one(MAX_CATALOG_CORRECTION_TRACKS)?)
            .fetch_all(&mut *tx)
            .await?;
        rebuild_participation_for_tracks(&mut tx, &track_ids, rebuilt_at).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Updates editable Release metadata fields (title, sort_title, display_artist,
    /// release_date, release_date_text). All fields are optional — only `Some`
    /// values are written. Recomputes normalized/sort columns when title changes.
    /// Records an audit event and bumps the revision.
    pub async fn update_release_metadata(
        &self,
        pool: &PgPool,
        release_id: Uuid,
        title: Option<&str>,
        sort_title: Option<&str>,
        display_artist: Option<&str>,
        release_date: Option<time::Date>,
        release_date_text: Option<&str>,
        actor: &str,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;

        // Verify the release exists first.
        let (current_sort_title, old_title) = sqlx::query_as::<_, (String, String)>(
            "SELECT sort_title, COALESCE(title, '') FROM releases WHERE id = $1",
        )
        .bind(release_id)
        .fetch_one(&mut *tx)
        .await?;

        let title_changed = title.is_some() && title.map_or(false, |t| t != old_title);

        // Build the dynamic UPDATE with only non-null fields.
        let mut builder = QueryBuilder::new(
            "UPDATE releases SET updated_at = CURRENT_TIMESTAMP",
        );
        builder.push(" ,revision = revision + 1");

        if title.is_some() {
            let new_title = title.unwrap();
            let computed_sort_title = if title_changed {
                compute_sort_key(new_title)
            } else {
                current_sort_title.clone()
            };
            builder.push(" ,title = ");
            builder.push_bind(new_title);
            let sort_to_set = match sort_title {
                Some(s) => s,
                None => &computed_sort_title,
            };
            builder.push(" ,sort_title = ");
            builder.push_bind(sort_to_set);
        } else if sort_title.is_some() {
            builder.push(" ,sort_title = ");
            builder.push_bind(sort_title.unwrap());
        }

        if display_artist.is_some() {
            builder.push(" ,display_artist = ");
            builder.push_bind(display_artist.unwrap());
        }

        if release_date.is_some() {
            builder.push(" ,release_date = ");
            builder.push_bind(release_date.unwrap());
        }

        if release_date_text.is_some() {
            builder.push(" ,release_date_text = ");
            builder.push_bind(release_date_text.unwrap());
        }

        builder.push(" WHERE id = ");
        builder.push_bind(release_id);

        builder
            .build()
            .execute(&mut *tx)
            .await?;

        audit_event(
            &mut tx,
            "release",
            release_id,
            "release_metadata_updated",
            actor,
            "Release metadata updated",
            serde_json::json!({
                "title": title,
                "sort_title": sort_title,
                "display_artist": display_artist,
                "release_date": release_date.as_ref().map(|d| format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())),
                "release_date_text": release_date_text,
            }),
            OffsetDateTime::now_utc(),
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Updates editable Artist metadata fields (canonical_name, sort_name, artist_type).
    /// All fields are optional — only `Some` values are written. Recomputes
    /// normalized_name and sort_name when canonical_name changes.
    pub async fn update_artist_metadata(
        &self,
        pool: &PgPool,
        artist_id: Uuid,
        canonical_name: Option<&str>,
        sort_name: Option<&str>,
        artist_type: Option<&str>,
        actor: &str,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;

        // Verify the artist exists first.
        let (current_canonical_name,) = sqlx::query_as::<_, (String,)>(
            "SELECT COALESCE(canonical_name, '') FROM artists WHERE id = $1",
        )
        .bind(artist_id)
        .fetch_one(&mut *tx)
        .await?;

        let name_changed = canonical_name.is_some()
            && canonical_name.map_or(false, |n| n != current_canonical_name);

        // Build the dynamic UPDATE with only non-null fields.
        let mut builder = QueryBuilder::new(
            "UPDATE artists SET updated_at = CURRENT_TIMESTAMP",
        );
        builder.push(" ,revision = revision + 1");

        if canonical_name.is_some() {
            let new_name = canonical_name.unwrap();
            let computed_normalized = compute_normalized(new_name);
            // Compute sort from current name when unchanged, else from new name.
            let computed_sort = if name_changed {
                compute_sort_key(new_name)
            } else {
                compute_sort_key(&current_canonical_name)
            };
            builder.push(" ,canonical_name = ");
            builder.push_bind(new_name);
            // Use computed values only when the field wasn't explicitly set.
            let sort_to_set = match sort_name {
                Some(s) => s,
                None => &computed_sort,
            };
            builder.push(" ,sort_name = ");
            builder.push_bind(sort_to_set);
            builder.push(" ,normalized_name = ");
            builder.push_bind(computed_normalized);
        } else if sort_name.is_some() {
            builder.push(" ,sort_name = ");
            builder.push_bind(sort_name.unwrap());
        }

        if artist_type.is_some() {
            builder.push(" ,artist_type = ");
            builder.push_bind(artist_type.unwrap());
        }

        builder.push(" WHERE id = ");
        builder.push_bind(artist_id);

        builder
            .build()
            .execute(&mut *tx)
            .await?;

        // Insert into catalog_projection_outbox so the search projection refreshes.
        let now = OffsetDateTime::now_utc();
        sqlx::query(
            "INSERT INTO catalog_projection_outbox (\n                projection_name, entity_type, entity_id, operation,\n                entity_revision, available_at, attempt_count, created_at\n             ) VALUES ('tantivy.catalog','artist',$1,$4,$2,$3,0,$3)",
        )
        .bind(artist_id)
        .bind(1_i64) // revision placeholder; consumer reads latest.
        .bind(now)
        .bind("upsert")
        .execute(&mut *tx)
        .await?;

        audit_event(
            &mut tx,
            "artist",
            artist_id,
            "artist_metadata_updated",
            actor,
            "Artist metadata updated",
            serde_json::json!({
                "canonical_name": canonical_name,
                "sort_name": sort_name,
                "artist_type": artist_type,
            }),
            now,
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Updates editable Track metadata fields (title, sort_title, display_artist, ordinal).
    pub async fn update_track_metadata(
        &self,
        pool: &PgPool,
        track_id: Uuid,
        title: Option<&str>,
        sort_title: Option<&str>,
        display_artist: Option<&str>,
        ordinal: Option<i32>,
        actor: &str,
    ) -> Result<(), CatalogStoreError> {
        let mut tx = pool.begin().await?;

        // Verify the track exists first.
        sqlx::query("SELECT id FROM tracks WHERE id = $1")
            .bind(track_id)
            .fetch_one(&mut *tx)
            .await?;

        // Build the dynamic UPDATE with only non-null fields.
        let mut builder = QueryBuilder::new(
            "UPDATE tracks SET updated_at = CURRENT_TIMESTAMP",
        );
        builder.push(" ,revision = revision + 1");

        if title.is_some() {
            builder.push(" ,title = ");
            builder.push_bind(title.unwrap());
        }
        if sort_title.is_some() {
            builder.push(" ,sort_title = ");
            builder.push_bind(sort_title.unwrap());
        }
        if display_artist.is_some() {
            builder.push(" ,display_artist = ");
            builder.push_bind(display_artist.unwrap());
        }
        if ordinal.is_some() {
            builder.push(" ,ordinal = ");
            builder.push_bind(ordinal.unwrap());
        }

        builder.push(" WHERE id = ");
        builder.push_bind(track_id);

        builder
            .build()
            .execute(&mut *tx)
            .await?;

        audit_event(
            &mut tx,
            "track",
            track_id,
            "track_metadata_updated",
            actor,
            "Track metadata updated",
            serde_json::json!({
                "title": title,
                "sort_title": sort_title,
                "display_artist": display_artist,
                "ordinal": ordinal,
            }),
            OffsetDateTime::now_utc(),
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Validates and inserts a bounded legacy mapping batch in constant statements.
    pub async fn record_legacy_catalog_id_mappings(
        &self,
        pool: &PgPool,
        mappings: &[LegacyCatalogIdMapping],
    ) -> Result<(), CatalogStoreError> {
        if mappings.is_empty() {
            return Ok(());
        }
        if mappings.iter().any(|mapping| {
            mapping.legacy_numeric_id < 0 || mapping.source_snapshot.trim().is_empty()
        }) {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mut tx = pool.begin().await?;
        let mut validate: QueryBuilder<Postgres> =
            QueryBuilder::new("WITH supplied(entity_type, internal_id, public_api_id) AS (");
        validate.push_values(mappings, |mut row, mapping| {
            row.push_bind(legacy_entity_name(mapping.entity_type))
                .push_bind(mapping.internal_id)
                .push_bind(mapping.public_api_id);
        });
        validate.push(
            ") SELECT count(*) FROM supplied supplied
             WHERE (supplied.entity_type = 'artist' AND EXISTS (
                       SELECT 1 FROM artists entity WHERE entity.id = supplied.internal_id
                         AND entity.public_api_id = supplied.public_api_id))
                OR (supplied.entity_type = 'release' AND EXISTS (
                       SELECT 1 FROM releases entity WHERE entity.id = supplied.internal_id
                         AND entity.public_api_id = supplied.public_api_id))
                OR (supplied.entity_type = 'track' AND EXISTS (
                       SELECT 1 FROM tracks entity WHERE entity.id = supplied.internal_id
                         AND entity.public_api_id = supplied.public_api_id))",
        );
        let valid_count: i64 = validate.build_query_scalar().fetch_one(&mut *tx).await?;
        if usize::try_from(valid_count).map_err(|_| CatalogStoreError::NumericOverflow)?
            != mappings.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let mut insert: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO legacy_catalog_id_mappings (
                id, migration_run_id, entity_type, legacy_numeric_id,
                internal_id, public_api_id, source_snapshot, imported_at
             ) ",
        );
        insert.push_values(mappings, |mut row, mapping| {
            row.push_bind(Uuid::now_v7())
                .push_bind(mapping.migration_run_id)
                .push_bind(legacy_entity_name(mapping.entity_type))
                .push_bind(mapping.legacy_numeric_id)
                .push_bind(mapping.internal_id)
                .push_bind(mapping.public_api_id)
                .push_bind(&mapping.source_snapshot)
                .push_bind(mapping.imported_at);
        });
        insert.push(" ON CONFLICT DO NOTHING");
        insert.build().execute(&mut *tx).await?;

        let mut compare: QueryBuilder<Postgres> = QueryBuilder::new(
            "WITH supplied(
                migration_run_id, entity_type, legacy_numeric_id, internal_id,
                public_api_id, source_snapshot, imported_at
             ) AS (",
        );
        compare.push_values(mappings, |mut row, mapping| {
            row.push_bind(mapping.migration_run_id)
                .push_bind(legacy_entity_name(mapping.entity_type))
                .push_bind(mapping.legacy_numeric_id)
                .push_bind(mapping.internal_id)
                .push_bind(mapping.public_api_id)
                .push_bind(&mapping.source_snapshot)
                .push_bind(mapping.imported_at);
        });
        compare.push(
            ") SELECT count(*) FROM supplied supplied
             JOIN legacy_catalog_id_mappings persisted
               ON persisted.migration_run_id = supplied.migration_run_id
              AND persisted.entity_type = supplied.entity_type
              AND persisted.legacy_numeric_id = supplied.legacy_numeric_id
              AND persisted.internal_id = supplied.internal_id
              AND persisted.public_api_id = supplied.public_api_id
              AND persisted.source_snapshot = supplied.source_snapshot
              AND persisted.imported_at = supplied.imported_at",
        );
        let exact_count: i64 = compare.build_query_scalar().fetch_one(&mut *tx).await?;
        if usize::try_from(exact_count).map_err(|_| CatalogStoreError::NumericOverflow)?
            != mappings.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        tx.commit().await?;
        Ok(())
    }
}

pub(crate) async fn insert_artist<'e, E>(
    executor: E,
    artist: &Artist,
) -> Result<(), CatalogStoreError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let (status, merged_into) = artist_status_parts(artist.status());
    let dates = artist.active_dates();
    sqlx::query(
        "INSERT INTO artists (
            id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, disambiguation, begin_date, end_date, status,
            merged_into_artist_id, revision, created_at, updated_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,1,$12,$13)",
    )
    .bind(artist.id().into_uuid())
    .bind(artist.public_api_id().into_uuid())
    .bind(artist.canonical_name())
    .bind(artist.normalized_name())
    .bind(artist.sort_name())
    .bind(artist_type_name(artist.artist_type()))
    .bind(artist.disambiguation())
    .bind(dates.and_then(melodee_domain::DateRange::start))
    .bind(dates.and_then(melodee_domain::DateRange::end))
    .bind(status)
    .bind(merged_into)
    .bind(artist.created_at())
    .bind(artist.updated_at())
    .execute(executor)
    .await?;
    Ok(())
}

pub(crate) fn ensure_correction_scope(
    actual: usize,
    dimension: CatalogCorrectionDimension,
    limit: usize,
) -> Result<(), CatalogStoreError> {
    if actual > limit {
        Err(CatalogStoreError::CorrectionScopeExceeded { dimension, limit })
    } else {
        Ok(())
    }
}

fn correction_limit_plus_one(limit: usize) -> Result<i64, CatalogStoreError> {
    i64::try_from(
        limit
            .checked_add(1)
            .ok_or(CatalogStoreError::NumericOverflow)?,
    )
    .map_err(|_| CatalogStoreError::NumericOverflow)
}

async fn lock_observed_artist(
    tx: &mut Transaction<'_, Postgres>,
    observed_artist_id: Uuid,
) -> Result<(), CatalogStoreError> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM observed_artists WHERE id = $1 FOR UPDATE")
        .bind(observed_artist_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(CatalogStoreError::MutationConflict)?;
    Ok(())
}

async fn ensure_negative_root_absent(
    tx: &mut Transaction<'_, Postgres>,
    observed_artist_id: Uuid,
    candidate_artist_id: Option<Uuid>,
    candidate_provider: Option<&str>,
    candidate_external_id: Option<&str>,
) -> Result<(), CatalogStoreError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM artist_negative_assertions
            WHERE observed_artist_id = $1
              AND predecessor_assertion_id IS NULL
              AND candidate_artist_id IS NOT DISTINCT FROM $2
              AND candidate_provider IS NOT DISTINCT FROM $3
              AND candidate_external_id IS NOT DISTINCT FROM $4
         )",
    )
    .bind(observed_artist_id)
    .bind(candidate_artist_id)
    .bind(candidate_provider)
    .bind(candidate_external_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Err(CatalogStoreError::MutationConflict)
    } else {
        Ok(())
    }
}

async fn local_candidate_suppressed(
    tx: &mut Transaction<'_, Postgres>,
    observed_artist_id: Uuid,
    artist_id: Uuid,
) -> Result<bool, CatalogStoreError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM artist_negative_assertions negative
            WHERE negative.observed_artist_id = $1
              AND negative.candidate_artist_id IS NOT NULL
              AND resolve_artist_redirect(negative.candidate_artist_id)
                    = resolve_artist_redirect($2)
              AND negative.assertion_kind = 'do_not_match'
              AND NOT EXISTS (
                  SELECT 1 FROM artist_negative_assertions successor
                  WHERE successor.predecessor_assertion_id = negative.id)
         )",
    )
    .bind(observed_artist_id)
    .bind(artist_id)
    .fetch_one(&mut **tx)
    .await?)
}

pub(crate) async fn rebuild_participation_for_tracks(
    tx: &mut Transaction<'_, Postgres>,
    track_ids: &[Uuid],
    rebuilt_at: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    ensure_correction_scope(
        track_ids.len(),
        CatalogCorrectionDimension::Tracks,
        MAX_CATALOG_CORRECTION_TRACKS,
    )?;
    let prospective_artist_count = sqlx::query_scalar::<_, i64>(
        "WITH direct AS MATERIALIZED (
            SELECT * FROM (
                SELECT artist_id, track_id,
                       role IN ('primary','featured','guest') AS is_performance
                FROM track_artists WHERE track_id = ANY($1)
                UNION ALL
                SELECT billed.artist_id, track.id,
                       billed.role IN ('primary','featured','guest')
                FROM release_artists billed
                JOIN tracks track ON track.release_id = billed.release_id
                WHERE track.id = ANY($1)
                UNION ALL
                SELECT artist_id, track_id, is_performance
                FROM contributions WHERE track_id = ANY($1)
                UNION ALL
                SELECT credit.artist_id, track.id, credit.is_performance
                FROM contributions credit
                JOIN tracks track ON track.release_id = credit.release_id
                WHERE credit.track_id IS NULL AND track.id = ANY($1)
            ) direct_source LIMIT $2
         ), bounded AS (
            SELECT 1 FROM artist_track_associations WHERE track_id = ANY($1)
            UNION ALL SELECT 1 FROM direct
            UNION ALL
            SELECT 1
            FROM group_memberships membership
            JOIN direct ON direct.artist_id = membership.group_artist_id
                       AND direct.is_performance
            JOIN tracks track ON track.id = direct.track_id
            JOIN releases release ON release.id = track.release_id
            LEFT JOIN group_membership_track_exclusions exclusion
              ON exclusion.membership_id = membership.id
             AND exclusion.track_id = track.id
            WHERE exclusion.track_id IS NULL
              AND release.release_date IS NOT NULL
              AND (membership.start_date IS NULL
                   OR release.release_date >= membership.start_date)
              AND (membership.end_date IS NULL
                   OR release.release_date <= membership.end_date)
            LIMIT $2
         )
         SELECT count(*)::bigint FROM bounded",
    )
    .bind(track_ids)
    .bind(correction_limit_plus_one(
        crate::MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
    )?)
    .fetch_one(&mut **tx)
    .await?;
    ensure_correction_scope(
        usize::try_from(prospective_artist_count)
            .map_err(|_| CatalogStoreError::NumericOverflow)?,
        CatalogCorrectionDimension::IdentityRows,
        crate::MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
    )?;
    let mut affected_artists = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT artist_id FROM artist_track_associations
         WHERE track_id = ANY($1)",
    )
    .bind(track_ids)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    sqlx::query("DELETE FROM artist_track_associations WHERE track_id = ANY($1)")
        .bind(track_ids)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO artist_track_associations (
            artist_id, track_id, association_type, participation_kind,
            source_key, source_artist_id, confidence, is_performance, created_at
         )
         SELECT artist_id, track_id,
                CASE role
                    WHEN 'primary' THEN 'primary_artist'
                    WHEN 'featured' THEN 'featured_artist'
                    WHEN 'guest' THEN 'guest_artist'
                    ELSE 'other'
                END,
                'direct', id, NULL::uuid, 10000,
                role IN ('primary','featured','guest'), $1
         FROM track_artists WHERE track_id = ANY($2)
         UNION ALL
         SELECT billed.artist_id, track.id,
                CASE billed.role
                    WHEN 'primary' THEN 'primary_artist'
                    WHEN 'featured' THEN 'featured_artist'
                    WHEN 'guest' THEN 'guest_artist'
                    ELSE 'other'
                END,
                'direct', billed.id, NULL::uuid, 10000,
                billed.role IN ('primary','featured','guest'), $1
         FROM release_artists billed
         JOIN tracks track ON track.release_id = billed.release_id
         WHERE track.id = ANY($2)
         UNION ALL
         SELECT artist_id, track_id, role, 'direct', id, NULL::uuid, 10000,
                is_performance, $1
         FROM contributions WHERE track_id = ANY($2)
         UNION ALL
         SELECT credit.artist_id, track.id, credit.role, 'direct', credit.id,
                NULL::uuid, 10000, credit.is_performance, $1
         FROM contributions credit
         JOIN tracks track ON track.release_id = credit.release_id
         WHERE credit.track_id IS NULL AND track.id = ANY($2)",
    )
    .bind(rebuilt_at)
    .bind(track_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO artist_track_associations (
            artist_id, track_id, association_type, participation_kind,
            source_key, source_artist_id, confidence, is_performance, created_at
         )
         SELECT membership.member_artist_id, direct.track_id,
                'band_member', 'inferred_group', membership.id,
                membership.group_artist_id, membership.confidence, true, $1
         FROM group_memberships membership
         JOIN artist_track_associations direct
           ON direct.artist_id = membership.group_artist_id
          AND direct.participation_kind = 'direct' AND direct.is_performance
         JOIN tracks track ON track.id = direct.track_id
         JOIN releases release ON release.id = track.release_id
         LEFT JOIN group_membership_track_exclusions exclusion
           ON exclusion.membership_id = membership.id AND exclusion.track_id = track.id
         WHERE exclusion.track_id IS NULL
           AND direct.track_id = ANY($2)
           AND release.release_date IS NOT NULL
           AND (membership.start_date IS NULL OR release.release_date >= membership.start_date)
           AND (membership.end_date IS NULL OR release.release_date <= membership.end_date)
         ON CONFLICT DO NOTHING",
    )
    .bind(rebuilt_at)
    .bind(track_ids)
    .execute(&mut **tx)
    .await?;
    affected_artists.extend(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT artist_id FROM artist_track_associations
             WHERE track_id = ANY($1)",
        )
        .bind(track_ids)
        .fetch_all(&mut **tx)
        .await?,
    );
    let affected_artists = affected_artists.into_iter().collect::<Vec<_>>();
    if affected_artists.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO artist_catalog_aggregates (
            artist_id, release_count, billed_track_count,
            performance_track_count, all_credit_track_count, revision, updated_at
         )
         SELECT artist.id,
                count(DISTINCT track.release_id) FILTER (WHERE association.track_id IS NOT NULL),
                count(DISTINCT association.track_id) FILTER (
                    WHERE association.association_type IN (
                        'primary_artist','featured_artist','guest_artist')),
                count(DISTINCT association.track_id) FILTER (WHERE association.is_performance),
                count(DISTINCT association.track_id), 1, $2
         FROM artists artist
         LEFT JOIN artist_track_associations association ON association.artist_id = artist.id
         LEFT JOIN tracks track ON track.id = association.track_id
         WHERE artist.id = ANY($1)
         GROUP BY artist.id
         ON CONFLICT (artist_id) DO UPDATE
         SET release_count = EXCLUDED.release_count,
             billed_track_count = EXCLUDED.billed_track_count,
             performance_track_count = EXCLUDED.performance_track_count,
             all_credit_track_count = EXCLUDED.all_credit_track_count,
             revision = artist_catalog_aggregates.revision + 1,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(&affected_artists)
    .bind(rebuilt_at)
    .execute(&mut **tx)
    .await?;
    let projected = sqlx::query(
        "WITH updated AS (
            UPDATE artists SET revision = revision + 1, updated_at = $2
            WHERE id = ANY($1) RETURNING id, revision
         )
         INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, attempt_count, created_at
         )
         SELECT 'tantivy.catalog', 'artist', id, 'upsert', revision, $2, 0, $2
         FROM updated",
    )
    .bind(&affected_artists)
    .bind(rebuilt_at)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if usize::try_from(projected).map_err(|_| CatalogStoreError::NumericOverflow)?
        != affected_artists.len()
    {
        return Err(CatalogStoreError::MutationConflict);
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct OverlayReleaseSource {
    release_id: Uuid,
    predecessor_overlay_id: Option<Uuid>,
    display_artist: Option<String>,
    catalog_import_id: Option<Uuid>,
    source_manifest_id: Option<Uuid>,
    source_semantic_manifest_hash: Option<String>,
    publication_plan_hash: Option<String>,
    has_publication_binding: bool,
}

struct OverlayAssembly {
    source: OverlayReleaseSource,
    artist_resolutions: Vec<OverlayArtistResolution>,
    release_artists: Vec<ReleaseArtistId>,
    track_artists: Vec<TrackArtistId>,
    contributions: Vec<melodee_domain::ContributionId>,
    audio_identities: Vec<OverlayAudioIdentityLink>,
    display_artists: Vec<OverlayDisplayArtist>,
}

/// Appends immutable semantic snapshots and advances Release pointers using a
/// fixed query set. Hash construction is a bounded in-memory pass because each
/// Release owns a distinct canonical overlay payload.
pub(crate) async fn append_overlay_revisions(
    tx: &mut Transaction<'_, Postgres>,
    release_ids: &[Uuid],
    resolver_version: &str,
    actor: &str,
    event_type: &str,
    now: OffsetDateTime,
) -> Result<Vec<CatalogSemanticOverlayId>, CatalogStoreError> {
    if resolver_version.trim().is_empty() || actor.trim().is_empty() || event_type.trim().is_empty()
    {
        return Err(CatalogStoreError::InvalidRequest);
    }
    let release_ids = release_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    ensure_correction_scope(
        release_ids.len(),
        CatalogCorrectionDimension::Releases,
        MAX_CATALOG_CORRECTION_RELEASES,
    )?;
    if release_ids.is_empty() {
        return Ok(Vec::new());
    }

    let sources = sqlx::query_as::<_, OverlayReleaseSource>(
        "SELECT release.id AS release_id,
                release.current_overlay_id AS predecessor_overlay_id,
                release.display_artist, import.id AS catalog_import_id,
                import.manifest_id AS source_manifest_id,
                import.semantic_manifest_hash AS source_semantic_manifest_hash,
                import.publication_plan_hash,
                EXISTS (
                    SELECT 1 FROM release_publication_receipts receipt
                    WHERE receipt.release_id = release.id
                ) AS has_publication_binding
         FROM releases release
         LEFT JOIN catalog_imports import ON import.release_id = release.id
         WHERE release.id = ANY($1)
         ORDER BY release.id
         FOR UPDATE OF release",
    )
    .bind(&release_ids)
    .fetch_all(&mut **tx)
    .await?;
    if sources.len() != release_ids.len() {
        return Err(CatalogStoreError::MutationConflict);
    }

    let mut legacy_release_ids = Vec::new();
    let mut assemblies = std::collections::BTreeMap::new();
    for source in sources {
        if source.catalog_import_id.is_none() {
            if source.has_publication_binding || source.predecessor_overlay_id.is_some() {
                return Err(CatalogStoreError::MutationConflict);
            }
            legacy_release_ids.push(source.release_id);
        } else {
            let mut display_artists = Vec::new();
            if let Some(display) = source.display_artist.as_ref() {
                display_artists.push(OverlayDisplayArtist {
                    owner: DisplayArtistOwner::Release(ReleaseId::from_uuid(source.release_id)),
                    value: DisplayArtist::new(display.clone())?,
                });
            }
            assemblies.insert(
                source.release_id,
                OverlayAssembly {
                    source,
                    artist_resolutions: Vec::new(),
                    release_artists: Vec::new(),
                    track_artists: Vec::new(),
                    contributions: Vec::new(),
                    audio_identities: Vec::new(),
                    display_artists,
                },
            );
        }
    }
    let manifest_release_ids = assemblies.keys().copied().collect::<Vec<_>>();

    if !manifest_release_ids.is_empty() {
        let reference_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*)::bigint FROM (
                SELECT 1 FROM observed_artists observed
                JOIN artist_match_current current ON current.observed_artist_id = observed.id
                WHERE observed.release_id = ANY($1)
                UNION ALL SELECT 1 FROM release_artists WHERE release_id = ANY($1)
                UNION ALL SELECT 1 FROM track_artists artist
                    JOIN tracks track ON track.id = artist.track_id
                    WHERE track.release_id = ANY($1)
                UNION ALL SELECT 1 FROM contributions WHERE release_id = ANY($1)
                UNION ALL SELECT 1 FROM tracks WHERE release_id = ANY($1)
                UNION ALL SELECT 1 FROM tracks
                    WHERE release_id = ANY($1) AND display_artist IS NOT NULL
                LIMIT $2
             ) bounded",
        )
        .bind(&manifest_release_ids)
        .bind(correction_limit_plus_one(
            MAX_CATALOG_CORRECTION_OVERLAY_REFERENCES,
        )?)
        .fetch_one(&mut **tx)
        .await?;
        ensure_correction_scope(
            usize::try_from(reference_count).map_err(|_| CatalogStoreError::NumericOverflow)?,
            CatalogCorrectionDimension::OverlayReferences,
            MAX_CATALOG_CORRECTION_OVERLAY_REFERENCES,
        )?;

        for (release_id, observed_id, artist_id, decision_id) in
            sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid)>(
                "SELECT observed.release_id, observed.id, decision.artist_id,
                        current.decision_id
                 FROM observed_artists observed
                 JOIN artist_match_current current ON current.observed_artist_id = observed.id
                 JOIN artist_match_decisions decision ON decision.id = current.decision_id
                 WHERE observed.release_id = ANY($1)
                 ORDER BY observed.release_id, observed.id",
            )
            .bind(&manifest_release_ids)
            .fetch_all(&mut **tx)
            .await?
        {
            assemblies
                .get_mut(&release_id)
                .ok_or(CatalogStoreError::MutationConflict)?
                .artist_resolutions
                .push(OverlayArtistResolution {
                    observed_artist_id: ObservedArtistId::from_uuid(observed_id),
                    artist_id: ArtistId::from_uuid(artist_id),
                    decision_id: Some(melodee_domain::ArtistMatchDecisionId::from_uuid(
                        decision_id,
                    )),
                });
        }
        for (release_id, id) in sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT release_id, id FROM release_artists
             WHERE release_id = ANY($1) ORDER BY release_id, id",
        )
        .bind(&manifest_release_ids)
        .fetch_all(&mut **tx)
        .await?
        {
            assemblies
                .get_mut(&release_id)
                .ok_or(CatalogStoreError::MutationConflict)?
                .release_artists
                .push(ReleaseArtistId::from_uuid(id));
        }
        for (release_id, id) in sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT track.release_id, artist.id FROM track_artists artist
             JOIN tracks track ON track.id = artist.track_id
             WHERE track.release_id = ANY($1) ORDER BY track.release_id, artist.id",
        )
        .bind(&manifest_release_ids)
        .fetch_all(&mut **tx)
        .await?
        {
            assemblies
                .get_mut(&release_id)
                .ok_or(CatalogStoreError::MutationConflict)?
                .track_artists
                .push(TrackArtistId::from_uuid(id));
        }
        for (release_id, id) in sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT release_id, id FROM contributions
             WHERE release_id = ANY($1) ORDER BY release_id, id",
        )
        .bind(&manifest_release_ids)
        .fetch_all(&mut **tx)
        .await?
        {
            assemblies
                .get_mut(&release_id)
                .ok_or(CatalogStoreError::MutationConflict)?
                .contributions
                .push(melodee_domain::ContributionId::from_uuid(id));
        }
        for (release_id, track_id, audio_identity_id) in sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
            "SELECT release_id, id, audio_identity_id FROM tracks
                 WHERE release_id = ANY($1) ORDER BY release_id, id",
        )
        .bind(&manifest_release_ids)
        .fetch_all(&mut **tx)
        .await?
        {
            assemblies
                .get_mut(&release_id)
                .ok_or(CatalogStoreError::MutationConflict)?
                .audio_identities
                .push(OverlayAudioIdentityLink {
                    track_id: TrackId::from_uuid(track_id),
                    audio_identity_id: melodee_domain::AudioIdentityId::from_uuid(
                        audio_identity_id,
                    ),
                });
        }
        for (release_id, track_id, display) in sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "SELECT release_id, id, display_artist FROM tracks
             WHERE release_id = ANY($1) AND display_artist IS NOT NULL
             ORDER BY release_id, id",
        )
        .bind(&manifest_release_ids)
        .fetch_all(&mut **tx)
        .await?
        {
            assemblies
                .get_mut(&release_id)
                .ok_or(CatalogStoreError::MutationConflict)?
                .display_artists
                .push(OverlayDisplayArtist {
                    owner: DisplayArtistOwner::Track(TrackId::from_uuid(track_id)),
                    value: DisplayArtist::new(display)?,
                });
        }
    }

    let mut overlays = Vec::with_capacity(assemblies.len());
    for assembly in assemblies.into_values() {
        let overlay = CatalogSemanticOverlay::new(NewCatalogSemanticOverlay {
            id: CatalogSemanticOverlayId::new(),
            release_id: ReleaseId::from_uuid(assembly.source.release_id),
            source_manifest_id: assembly
                .source
                .source_manifest_id
                .ok_or(CatalogStoreError::MutationConflict)?,
            source_semantic_manifest_hash: assembly
                .source
                .source_semantic_manifest_hash
                .ok_or(CatalogStoreError::MutationConflict)?,
            publication_plan_hash: assembly
                .source
                .publication_plan_hash
                .ok_or(CatalogStoreError::MutationConflict)?,
            catalog_import_id: CatalogImportId::from_uuid(
                assembly
                    .source
                    .catalog_import_id
                    .ok_or(CatalogStoreError::MutationConflict)?,
            ),
            resolver_version: resolver_version.to_owned(),
            predecessor_id: assembly
                .source
                .predecessor_overlay_id
                .map(CatalogSemanticOverlayId::from_uuid),
            references: OverlayReferences {
                artist_resolutions: assembly.artist_resolutions,
                release_artists: assembly.release_artists,
                track_artists: assembly.track_artists,
                contributions: assembly.contributions,
                audio_identities: assembly.audio_identities,
                display_artists: assembly.display_artists,
            },
            created_at: now,
            created_by: actor.to_owned(),
        })?;
        let payload = serde_json::to_value(&overlay)?;
        let overlay_hash = overlay.overlay_hash()?;
        overlays.push((overlay, payload, overlay_hash));
    }

    if !overlays.is_empty() {
        let overlay_ids = overlays
            .iter()
            .map(|(overlay, _, _)| overlay.id().into_uuid())
            .collect::<Vec<_>>();
        let overlay_release_ids = overlays
            .iter()
            .map(|(overlay, _, _)| overlay.release_id().into_uuid())
            .collect::<Vec<_>>();
        let import_ids = overlays
            .iter()
            .map(|(overlay, _, _)| overlay.catalog_import_id().into_uuid())
            .collect::<Vec<_>>();
        let predecessor_ids = overlays
            .iter()
            .map(|(overlay, _, _)| overlay.predecessor_id().map(|id| id.into_uuid()))
            .collect::<Vec<_>>();
        let manifest_ids = overlays
            .iter()
            .map(|(overlay, _, _)| overlay.source_manifest_id())
            .collect::<Vec<_>>();
        let source_hashes = overlays
            .iter()
            .map(|(overlay, _, _)| overlay.source_semantic_manifest_hash().to_owned())
            .collect::<Vec<_>>();
        let plan_hashes = overlays
            .iter()
            .map(|(overlay, _, _)| overlay.publication_plan_hash().to_owned())
            .collect::<Vec<_>>();
        let payloads = overlays
            .iter()
            .map(|(_, payload, _)| payload.clone())
            .collect::<Vec<_>>();
        let overlay_hashes = overlays
            .iter()
            .map(|(_, _, hash)| hash.clone())
            .collect::<Vec<_>>();
        sqlx::query(
            "INSERT INTO catalog_semantic_overlays (
                id, release_id, catalog_import_id, predecessor_overlay_id,
                schema_name, schema_version, source_manifest_id,
                source_semantic_manifest_hash, publication_plan_hash,
                resolver_version, payload, overlay_hash, created_at
             )
             SELECT input.overlay_id, input.release_id, input.catalog_import_id,
                    input.predecessor_overlay_id, 'catalog.semantic', 1,
                    input.source_manifest_id, input.source_semantic_manifest_hash,
                    input.publication_plan_hash, $10, input.payload,
                    input.overlay_hash, $11
             FROM unnest(
                 $1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::uuid[],
                 $6::text[], $7::text[], $8::jsonb[], $9::text[]
             ) AS input(
                 overlay_id, release_id, catalog_import_id, predecessor_overlay_id,
                 source_manifest_id, source_semantic_manifest_hash,
                 publication_plan_hash, payload, overlay_hash
             )",
        )
        .bind(&overlay_ids)
        .bind(&overlay_release_ids)
        .bind(&import_ids)
        .bind(&predecessor_ids)
        .bind(&manifest_ids)
        .bind(&source_hashes)
        .bind(&plan_hashes)
        .bind(&payloads)
        .bind(&overlay_hashes)
        .bind(resolver_version)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        let advanced = sqlx::query(
            "WITH updated AS (
                UPDATE releases release
                SET current_overlay_id = input.overlay_id,
                    revision = release.revision + 1, updated_at = $3
                FROM unnest($1::uuid[], $2::uuid[])
                     AS input(release_id, overlay_id)
                WHERE release.id = input.release_id
                RETURNING release.id, release.revision
             )
             INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, attempt_count, created_at
             )
             SELECT 'tantivy.catalog', 'release', id, 'upsert', revision, $3, 0, $3
             FROM updated",
        )
        .bind(&overlay_release_ids)
        .bind(&overlay_ids)
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if usize::try_from(advanced).map_err(|_| CatalogStoreError::NumericOverflow)?
            != overlays.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        sqlx::query(
            "INSERT INTO catalog_audit_events (
                event_id, entity_type, entity_id, event_type, actor,
                reason, payload, created_at
             )
             SELECT uuidv7(), 'release', input.release_id, $4, $5, $4,
                    jsonb_build_object(
                        'overlay_id', input.overlay_id,
                        'predecessor_overlay_id', input.predecessor_overlay_id,
                        'resolver_version', $6::text
                    ), $7
             FROM unnest($1::uuid[], $2::uuid[], $3::uuid[])
                  AS input(release_id, overlay_id, predecessor_overlay_id)",
        )
        .bind(&overlay_release_ids)
        .bind(&overlay_ids)
        .bind(&predecessor_ids)
        .bind(event_type)
        .bind(actor)
        .bind(resolver_version)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }

    if !legacy_release_ids.is_empty() {
        let advanced = sqlx::query(
            "WITH updated AS (
                UPDATE releases SET revision = revision + 1, updated_at = $2
                WHERE id = ANY($1) RETURNING id, revision
             )
             INSERT INTO catalog_projection_outbox (
                projection_name, entity_type, entity_id, operation,
                entity_revision, available_at, attempt_count, created_at
             )
             SELECT 'tantivy.catalog', 'release', id, 'upsert', revision, $2, 0, $2
             FROM updated",
        )
        .bind(&legacy_release_ids)
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if usize::try_from(advanced).map_err(|_| CatalogStoreError::NumericOverflow)?
            != legacy_release_ids.len()
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        sqlx::query(
            "INSERT INTO catalog_audit_events (
                event_id, entity_type, entity_id, event_type, actor,
                reason, payload, created_at
             )
             SELECT uuidv7(), 'release', release_id, $2, $3, $2,
                    jsonb_build_object(
                        'legacy_only', true, 'overlay_appended', false,
                        'resolver_version', $4::text
                    ), $5
             FROM unnest($1::uuid[]) AS release_id",
        )
        .bind(&legacy_release_ids)
        .bind(event_type)
        .bind(actor)
        .bind(resolver_version)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }

    Ok(overlays
        .into_iter()
        .map(|(overlay, _, _)| overlay.id())
        .collect())
}

pub(crate) async fn audit_artist_change(
    tx: &mut Transaction<'_, Postgres>,
    artist_id: Uuid,
    event_type: &str,
    actor: &str,
    reason: &str,
    now: OffsetDateTime,
    bump_revision: bool,
) -> Result<(), CatalogStoreError> {
    let (revision, status) = if bump_revision {
        sqlx::query_as::<_, (i64, String)>(
            "UPDATE artists SET revision = revision + 1, updated_at = $2
             WHERE id = $1 RETURNING revision, status",
        )
        .bind(artist_id)
        .bind(now)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_as::<_, (i64, String)>("SELECT revision, status FROM artists WHERE id = $1")
            .bind(artist_id)
            .fetch_one(&mut **tx)
            .await?
    };
    let operation = if matches!(status.as_str(), "merged" | "deprecated") {
        "delete"
    } else {
        "upsert"
    };
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog','artist',$1,$4,$2,$3,0,$3)",
    )
    .bind(artist_id)
    .bind(revision)
    .bind(now)
    .bind(operation)
    .execute(&mut **tx)
    .await?;
    audit_event(
        tx,
        "artist",
        artist_id,
        event_type,
        actor,
        reason,
        serde_json::json!({ "revision": revision }),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn audit_event(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: &str,
    entity_id: Uuid,
    event_type: &str,
    actor: &str,
    reason: &str,
    payload: serde_json::Value,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "INSERT INTO catalog_audit_events (
            event_id, entity_type, entity_id, event_type, actor,
            reason, payload, created_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(Uuid::now_v7())
    .bind(entity_type)
    .bind(entity_id)
    .bind(event_type)
    .bind(actor)
    .bind(reason)
    .bind(payload)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn artist_status_parts(status: ArtistStatus) -> (&'static str, Option<Uuid>) {
    match status {
        ArtistStatus::Provisional => ("provisional", None),
        ArtistStatus::Verified => ("verified", None),
        ArtistStatus::MergedInto(target) => ("merged", Some(target.into_uuid())),
        ArtistStatus::Deprecated => ("deprecated", None),
    }
}

fn artist_type_name(value: ArtistType) -> &'static str {
    match value {
        ArtistType::Person => "person",
        ArtistType::Group => "group",
        ArtistType::Orchestra => "orchestra",
        ArtistType::Ensemble => "ensemble",
        ArtistType::Choir => "choir",
        ArtistType::Character => "character",
        ArtistType::Other => "other",
        ArtistType::Unknown => "unknown",
    }
}

fn alias_type_name(value: ArtistAliasType) -> &'static str {
    match value {
        ArtistAliasType::Search => "search",
        ArtistAliasType::LegalName => "legal_name",
        ArtistAliasType::FormerName => "former_name",
        ArtistAliasType::Translation => "translation",
        ArtistAliasType::Transliteration => "transliteration",
        ArtistAliasType::Other => "other",
    }
}

fn evidence_kind_name(value: EvidenceKind) -> &'static str {
    match value {
        EvidenceKind::NameSimilarity => "name_similarity",
        EvidenceKind::Alias => "alias",
        EvidenceKind::EmbeddedExternalId => "embedded_external_id",
        EvidenceKind::ReleaseContext => "release_context",
        EvidenceKind::TrackParticipation => "track_participation",
        EvidenceKind::ArtistType => "artist_type",
        EvidenceKind::ActiveDate => "active_date",
        EvidenceKind::Area => "area",
        EvidenceKind::GroupMembership => "group_membership",
        EvidenceKind::ExternalProviderAgreement => "external_provider_agreement",
        EvidenceKind::Manual => "manual",
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

fn contribution_role_name(value: ContributionRole) -> &'static str {
    match value {
        ContributionRole::PrimaryArtist => "primary_artist",
        ContributionRole::FeaturedArtist => "featured_artist",
        ContributionRole::GuestArtist => "guest_artist",
        ContributionRole::Performer => "performer",
        ContributionRole::BandMember => "band_member",
        ContributionRole::Vocalist => "vocalist",
        ContributionRole::Instrumentalist => "instrumentalist",
        ContributionRole::Composer => "composer",
        ContributionRole::Lyricist => "lyricist",
        ContributionRole::Producer => "producer",
        ContributionRole::Remixer => "remixer",
        ContributionRole::Engineer => "engineer",
        ContributionRole::Other => "other",
    }
}

fn legacy_entity_name(value: LegacyCatalogEntityType) -> &'static str {
    match value {
        LegacyCatalogEntityType::Artist => "artist",
        LegacyCatalogEntityType::Release => "release",
        LegacyCatalogEntityType::Track => "track",
    }
}

fn compute_sort_key(display: &str) -> String {
    let lower = display.to_lowercase();
    for prefix in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return rest.trim_start().to_string();
        }
    }
    lower
}

fn compute_normalized(value: &str) -> String {
    value.to_lowercase().trim().to_string()
}

fn candidate_rows(
    rows: Vec<(Uuid, Uuid, String, String, String, i32)>,
) -> Result<Vec<StoredArtistCandidate>, CatalogStoreError> {
    rows.into_iter()
        .map(|row| {
            Ok(StoredArtistCandidate {
                artist_id: ArtistId::from_uuid(row.0),
                public_api_id: row.1,
                canonical_name: row.2,
                evidence_kind: row.3,
                evidence_value: row.4,
                confidence_basis_points: u16::try_from(row.5)
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
            })
        })
        .collect()
}

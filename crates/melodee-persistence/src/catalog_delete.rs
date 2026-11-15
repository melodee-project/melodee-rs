//! Administrator Artist deletion (ADR-0199).
//!
//! Artists are the most connected catalog entities, so deletion is a durable
//! tombstone rather than a row delete: identity evidence, credits, and audit
//! history stay attributable to the deleted public id, exactly like a merged
//! Artist or a deleted Release (ADR-0152).
//!
//! Two operator-selected scopes exist:
//!
//! * [`ArtistDeleteMode::CatalogOnly`] retires the Artist identity and keeps
//!   every Release, credit, and canonical/staged media file untouched.
//! * [`ArtistDeleteMode::ReleasesAndFiles`] additionally tombstones every
//!   Release this Artist owns alone (primary Release billing with no other
//!   primary Artist) after the caller removed that Release's canonical and
//!   staged storage. Releases that another Artist also bills as primary are
//!   retained, and the count is reported so the operator is never surprised.
//!
//! The filesystem step is deliberately *not* part of this module: persistence
//! never touches media. The Admin surface resolves the durable publication and
//! staging evidence, removes the owned trees with the shared no-follow
//! deletion helper, and only then calls [`CatalogStore::delete_artists`]. A
//! failure before that call leaves the catalog untouched.

use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use melodee_domain::ArtistId;

use crate::catalog::{
    CatalogCorrectionDimension, CatalogStore, CatalogStoreError, MAX_CATALOG_CORRECTION_RELEASES,
    MAX_CATALOG_CORRECTION_TRACKS,
};
use crate::catalog_mutations::ensure_correction_scope;

/// Maximum Artists accepted by one administrative deletion.
pub const MAX_ARTIST_DELETE_SOURCES: usize = 100;

/// What an administrative Artist deletion also removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistDeleteMode {
    /// Retire the Artist identity and keep every Release and media file.
    CatalogOnly,
    /// Also retire every Release the Artist owns alone, after its files were
    /// removed by the caller.
    ReleasesAndFiles,
}

impl ArtistDeleteMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CatalogOnly => "catalogOnly",
            Self::ReleasesAndFiles => "releasesAndFiles",
        }
    }

    /// Parses the wire value used by the Admin API.
    ///
    /// # Errors
    /// Returns `InvalidRequest` for any other value.
    pub fn parse(value: &str) -> Result<Self, CatalogStoreError> {
        match value {
            "catalogOnly" => Ok(Self::CatalogOnly),
            "releasesAndFiles" => Ok(Self::ReleasesAndFiles),
            _ => Err(CatalogStoreError::InvalidRequest),
        }
    }
}

/// One selected Artist as rendered in the deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistDeletePreviewArtist {
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub status: String,
    /// Releases retired with this Artist under `ReleasesAndFiles`.
    pub owned_releases: i64,
}

/// Exact bounded impact of the requested Artist deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtistDeleteImpact {
    pub releases: i64,
    pub tracks: i64,
    pub media_files: i64,
    pub media_bytes: i64,
    /// Releases also billed as primary by another Artist; these are retained.
    pub retained_shared_releases: i64,
    pub release_credits: i64,
    pub track_credits: i64,
    pub contributions: i64,
    pub user_states: i64,
}

/// Read-only deletion preview: what is retired and what is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistDeletePreview {
    pub artists: Vec<ArtistDeletePreviewArtist>,
    pub impact: ArtistDeleteImpact,
    pub warnings: Vec<String>,
}

/// One owned Release whose storage the caller must remove first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistDeleteReleaseStorage {
    pub release_public_id: Uuid,
    pub work_item_id: Uuid,
    pub canonical_relative_path: String,
    pub staging_relative_path: Option<String>,
}

/// One administrative Artist deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistDelete {
    pub artist_id: ArtistId,
    pub public_api_id: Uuid,
    pub mode: ArtistDeleteMode,
    pub actor: String,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

/// Committed result of one Artist deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtistDeleteOutcome {
    pub deleted_artists: Vec<Uuid>,
    pub deleted_releases: Vec<Uuid>,
    pub retained_shared_releases: i64,
}

fn validate_selection(public_ids: &[Uuid]) -> Result<Vec<Uuid>, CatalogStoreError> {
    if public_ids.is_empty() || public_ids.len() > MAX_ARTIST_DELETE_SOURCES {
        return Err(CatalogStoreError::InvalidRequest);
    }
    let distinct = public_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if distinct.len() != public_ids.len() {
        return Err(CatalogStoreError::InvalidRequest);
    }
    Ok(distinct.into_iter().collect())
}

impl CatalogStore {
    /// Returns the exact bounded impact of deleting the selected Artists
    /// without mutating catalog state.
    ///
    /// # Errors
    /// `InvalidRequest` for an empty, duplicated, or oversized selection;
    /// `MutationConflict` when an Artist is missing or not deletable.
    pub async fn preview_artist_delete(
        &self,
        pool: &PgPool,
        artist_public_ids: &[Uuid],
    ) -> Result<ArtistDeletePreview, CatalogStoreError> {
        let selected = validate_selection(artist_public_ids)?;
        let artists = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            "SELECT id, public_api_id, canonical_name, status
               FROM artists
              WHERE public_api_id = ANY($1)
              ORDER BY public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?;
        if artists.len() != selected.len() {
            return Err(CatalogStoreError::MutationConflict);
        }
        if artists
            .iter()
            .any(|(_, _, _, status)| matches!(status.as_str(), "merged" | "deprecated" | "deleted"))
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let artist_ids = artists.iter().map(|(id, _, _, _)| *id).collect::<Vec<_>>();
        let retained_shared_releases = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM (
                 SELECT billing.release_id
                   FROM release_artists billing
                   JOIN releases release ON release.id = billing.release_id
                  WHERE billing.artist_id = ANY($1)
                    AND billing.role = 'primary'
                    AND release.status = 'active'
                 EXCEPT
                 SELECT release_id FROM (
                     SELECT billing.release_id
                       FROM release_artists billing
                       JOIN releases release ON release.id = billing.release_id
                      WHERE billing.artist_id = ANY($1)
                        AND billing.role = 'primary'
                        AND release.status = 'active'
                        AND NOT EXISTS (
                            SELECT 1 FROM release_artists other
                             WHERE other.release_id = billing.release_id
                               AND other.role = 'primary'
                               AND other.artist_id <> ALL($1))
                 ) owned
             ) shared",
        )
        .bind(&artist_ids)
        .fetch_one(pool)
        .await?;
        let counts = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64, i64)>(
            "WITH owned AS (
                 SELECT billing.release_id
                   FROM release_artists billing
                   JOIN releases release ON release.id = billing.release_id
                  WHERE billing.artist_id = ANY($1)
                    AND billing.role = 'primary'
                    AND release.status = 'active'
                    AND NOT EXISTS (
                        SELECT 1 FROM release_artists other
                         WHERE other.release_id = billing.release_id
                           AND other.role = 'primary'
                           AND other.artist_id <> ALL($1))
             ), owned_tracks AS (
                 SELECT track.id, track.release_id FROM tracks track
                  WHERE track.release_id IN (SELECT release_id FROM owned)
             ), owned_media AS (
                 SELECT DISTINCT media.id, media.byte_size
                   FROM track_media_assets link
                   JOIN media_assets media ON media.id = link.media_asset_id
                  WHERE link.track_id IN (SELECT id FROM owned_tracks)
             )
             SELECT
                 (SELECT count(*) FROM owned),
                 (SELECT count(*) FROM owned_tracks),
                 (SELECT count(*) FROM owned_media),
                 (SELECT COALESCE(sum(byte_size), 0)::bigint FROM owned_media),
                 (SELECT count(*) FROM release_artists WHERE artist_id = ANY($1)),
                 (SELECT count(*) FROM track_artists WHERE artist_id = ANY($1)),
                 (SELECT count(*) FROM contributions WHERE artist_id = ANY($1)),
                 (SELECT count(*) FROM native_artist_user_states WHERE artist_id = ANY($1))",
        )
        .bind(&artist_ids)
        .fetch_one(pool)
        .await?;
        let owned_counts = sqlx::query_as::<_, (Uuid, i64)>(
            "WITH owned AS (
                 SELECT billing.release_id, billing.artist_id
                   FROM release_artists billing
                   JOIN releases release ON release.id = billing.release_id
                  WHERE billing.artist_id = ANY($1)
                    AND billing.role = 'primary'
                    AND release.status = 'active'
                    AND NOT EXISTS (
                        SELECT 1 FROM release_artists other
                         WHERE other.release_id = billing.release_id
                           AND other.role = 'primary'
                           AND other.artist_id <> ALL($1))
             )
             SELECT artist_id, count(DISTINCT release_id) FROM owned GROUP BY artist_id",
        )
        .bind(&artist_ids)
        .fetch_all(pool)
        .await?;
        let mut warnings = Vec::new();
        if counts.0 == 0 {
            warnings.push(
                "no Release is owned by this selection; only the Artist identities are retired"
                    .to_owned(),
            );
        }
        if retained_shared_releases > 0 {
            warnings.push(format!(
                "{retained_shared_releases} Release(s) are also billed as primary by another Artist and are kept"
            ));
        }
        Ok(ArtistDeletePreview {
            artists: artists
                .into_iter()
                .map(
                    |(id, public_api_id, canonical_name, status)| ArtistDeletePreviewArtist {
                        public_api_id,
                        canonical_name,
                        status,
                        owned_releases: owned_counts
                            .iter()
                            .find(|(artist_id, _)| *artist_id == id)
                            .map_or(0, |(_, count)| *count),
                    },
                )
                .collect(),
            impact: ArtistDeleteImpact {
                releases: counts.0,
                tracks: counts.1,
                media_files: counts.2,
                media_bytes: counts.3,
                retained_shared_releases,
                release_credits: counts.4,
                track_credits: counts.5,
                contributions: counts.6,
                user_states: counts.7,
            },
            warnings,
        })
    }

    /// Resolves the durable storage coordinates of every Release the selection
    /// owns alone, so the caller can remove those trees before the tombstone is
    /// written. Releases without publication evidence are returned with an
    /// empty path set by omitting them; callers compare the length against
    /// [`ArtistDeletePreview::impact`] and fail closed.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; database errors otherwise.
    pub async fn artist_delete_release_storage(
        &self,
        pool: &PgPool,
        artist_public_ids: &[Uuid],
    ) -> Result<Vec<ArtistDeleteReleaseStorage>, CatalogStoreError> {
        let selected = validate_selection(artist_public_ids)?;
        Ok(sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>)>(
            "WITH owned AS (
                 SELECT billing.release_id
                   FROM release_artists billing
                   JOIN artists artist ON artist.id = billing.artist_id
                   JOIN releases release ON release.id = billing.release_id
                  WHERE artist.public_api_id = ANY($1)
                    AND billing.role = 'primary'
                    AND release.status = 'active'
                    AND NOT EXISTS (
                        SELECT 1 FROM release_artists other
                         WHERE other.release_id = billing.release_id
                           AND other.role = 'primary'
                           AND other.artist_id <> ALL(
                               SELECT id FROM artists WHERE public_api_id = ANY($1)))
             )
             SELECT owned.release_id, plan.work_item_id, plan.target_relative_path,
                    handoff.staging_relative_path
               FROM owned
               JOIN release_publication_plans plan ON plan.release_id = owned.release_id
               LEFT JOIN release_work_item_staging_handoffs handoff ON handoff.work_item_id = plan.work_item_id
              ORDER BY owned.release_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(release_public_id, work_item_id, canonical_relative_path, staging_relative_path)| {
                ArtistDeleteReleaseStorage {
                    release_public_id,
                    work_item_id,
                    canonical_relative_path,
                    staging_relative_path,
                }
            },
        )
        .collect())
    }

    /// Commits the Artist deletion tombstone and, for
    /// [`ArtistDeleteMode::ReleasesAndFiles`], the owned Release tombstones.
    ///
    /// The caller must already have removed the owned Release storage; this
    /// method never touches the filesystem and never deletes catalog evidence
    /// rows.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid batch; `MutationConflict` when an Artist
    /// is missing, not deletable, or the batch mixes deletion modes; scope
    /// errors when the owned Release/Track set exceeds the correction bounds.
    pub async fn delete_artists(
        &self,
        pool: &PgPool,
        deletes: &[ArtistDelete],
    ) -> Result<ArtistDeleteOutcome, CatalogStoreError> {
        if deletes.is_empty() || deletes.len() > MAX_ARTIST_DELETE_SOURCES {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mode = deletes[0].mode;
        let actor = deletes[0].actor.trim();
        let reason = deletes[0].reason.trim();
        if actor.is_empty() || reason.is_empty() {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mut artist_ids = Vec::with_capacity(deletes.len());
        for delete in deletes {
            if delete.mode != mode
                || delete.actor.trim() != actor
                || delete.reason.trim() != reason
                || artist_ids.contains(&delete.artist_id.into_uuid())
            {
                return Err(CatalogStoreError::InvalidRequest);
            }
            artist_ids.push(delete.artist_id.into_uuid());
        }
        let now = deletes[0].occurred_at;
        let mut tx = pool.begin().await?;
        let locked = sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "SELECT id, public_api_id, status FROM artists
              WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(&artist_ids)
        .fetch_all(&mut *tx)
        .await?;
        if locked.len() != artist_ids.len()
            || locked.iter().any(|(id, public_api_id, status)| {
                matches!(status.as_str(), "merged" | "deprecated" | "deleted")
                    || deletes
                        .iter()
                        .find(|delete| delete.artist_id.into_uuid() == *id)
                        .is_none_or(|delete| delete.public_api_id != *public_api_id)
            })
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let mut outcome = ArtistDeleteOutcome::default();
        if mode == ArtistDeleteMode::ReleasesAndFiles {
            let owned = sqlx::query_scalar::<_, Uuid>(
                "WITH owned AS (
                     SELECT billing.release_id
                       FROM release_artists billing
                       JOIN releases release ON release.id = billing.release_id
                      WHERE billing.artist_id = ANY($1)
                        AND billing.role = 'primary'
                        AND release.status = 'active'
                        AND NOT EXISTS (
                            SELECT 1 FROM release_artists other
                             WHERE other.release_id = billing.release_id
                               AND other.role = 'primary'
                               AND other.artist_id <> ALL($1))
                 )
                 SELECT release_id FROM owned ORDER BY release_id FOR UPDATE",
            )
            .bind(&artist_ids)
            .fetch_all(&mut *tx)
            .await?;
            ensure_correction_scope(
                owned.len(),
                CatalogCorrectionDimension::Releases,
                MAX_CATALOG_CORRECTION_RELEASES,
            )?;
            let tracked = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM tracks WHERE release_id = ANY($1)",
            )
            .bind(&owned)
            .fetch_one(&mut *tx)
            .await?;
            ensure_correction_scope(
                usize::try_from(tracked).map_err(|_| CatalogStoreError::NumericOverflow)?,
                CatalogCorrectionDimension::Tracks,
                MAX_CATALOG_CORRECTION_TRACKS,
            )?;
            for release_id in &owned {
                retire_release(
                    &mut tx,
                    *release_id,
                    actor,
                    reason,
                    serde_json::json!({
                        "artist_delete": true,
                        "artist_public_ids": deletes
                            .iter()
                            .map(|delete| delete.public_api_id.to_string())
                            .collect::<Vec<_>>(),
                        "retained_catalog_tombstone": true,
                        "storage_removed": true,
                    }),
                    now,
                )
                .await?;
                outcome.deleted_releases.push(*release_id);
            }
        }
        for (artist_id, public_api_id, _) in &locked {
            let revision = sqlx::query_scalar::<_, i64>(
                "UPDATE artists SET status='deleted', revision=revision+1, updated_at=$2
                  WHERE id=$1 RETURNING revision",
            )
            .bind(artist_id)
            .bind(now)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO catalog_projection_outbox (
                    projection_name, entity_type, entity_id, operation,
                    entity_revision, available_at, attempt_count, created_at
                 ) VALUES ('tantivy.catalog','artist',$1,'delete',$2,$3,0,$3)
                 ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
                 DO NOTHING",
            )
            .bind(artist_id)
            .bind(revision)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            audit_event(
                &mut tx,
                "artist",
                *artist_id,
                "artist_deleted",
                actor,
                reason,
                serde_json::json!({
                    "mode": mode.as_str(),
                    "public_api_id": public_api_id,
                    "deleted_release_count": outcome.deleted_releases.len(),
                    "retained_catalog_tombstone": true,
                }),
                now,
            )
            .await?;
            outcome.deleted_artists.push(*public_api_id);
        }
        outcome.retained_shared_releases = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM (
                 SELECT billing.release_id
                   FROM release_artists billing
                   JOIN releases release ON release.id = billing.release_id
                  WHERE billing.artist_id = ANY($1)
                    AND billing.role = 'primary'
                    AND release.status = 'active'
                    AND EXISTS (
                        SELECT 1 FROM release_artists other
                         WHERE other.release_id = billing.release_id
                           AND other.role = 'primary'
                           AND other.artist_id <> ALL($1))
             ) retained",
        )
        .bind(&artist_ids)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(outcome)
    }
}

#[allow(clippy::too_many_arguments)]
async fn audit_event(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: &'static str,
    entity_id: Uuid,
    event_type: &'static str,
    actor: &str,
    reason: &str,
    payload: serde_json::Value,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "INSERT INTO catalog_audit_events (
            event_id, entity_type, entity_id, event_type, actor, reason, payload, created_at
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

/// Recomputes Artist catalog aggregates after one Release was retired.
async fn recompute_artist_aggregates(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "WITH affected AS (
            SELECT artist_id FROM release_artists WHERE release_id = $1
            UNION
            SELECT billing.artist_id FROM track_artists billing
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

/// Recomputes the retired Release's own catalog aggregate (Track count zero).
async fn refresh_release_aggregate(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "INSERT INTO release_catalog_aggregates (
            release_id, track_count, duration_ms, media_bytes, revision, updated_at
         )
         SELECT $1, count(DISTINCT track.id), COALESCE(sum(track.duration_ms), 0), 0,
                (SELECT revision FROM releases WHERE id=$1), $2
           FROM tracks track WHERE track.release_id = $1
         ON CONFLICT (release_id) DO UPDATE SET
            track_count = EXCLUDED.track_count,
            duration_ms = EXCLUDED.duration_ms,
            media_bytes = EXCLUDED.media_bytes,
            revision = EXCLUDED.revision,
            updated_at = EXCLUDED.updated_at",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Maximum Releases accepted by one administrative deletion.
pub const MAX_RELEASE_DELETE_SOURCES: usize = 100;

/// What an administrative Release deletion also removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDeleteMode {
    /// Retire the Release catalog rows and keep every media file.
    CatalogOnly,
    /// Also remove canonical and staged storage, which the caller already
    /// deleted using the durable publication/handoff evidence.
    ReleasesAndFiles,
}

impl ReleaseDeleteMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CatalogOnly => "catalogOnly",
            Self::ReleasesAndFiles => "releasesAndFiles",
        }
    }

    /// Parses the wire value used by the Admin API.
    ///
    /// # Errors
    /// Returns `InvalidRequest` for any other value.
    pub fn parse(value: &str) -> Result<Self, CatalogStoreError> {
        match value {
            "catalogOnly" => Ok(Self::CatalogOnly),
            "releasesAndFiles" => Ok(Self::ReleasesAndFiles),
            _ => Err(CatalogStoreError::InvalidRequest),
        }
    }
}

/// One selected Release as rendered in the deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseDeletePreviewRelease {
    pub public_api_id: Uuid,
    pub title: String,
    pub display_artist: Option<String>,
    pub status: String,
    pub track_count: i64,
}

/// Exact bounded impact of the requested Release deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseDeleteImpact {
    pub releases: i64,
    pub tracks: i64,
    pub media_files: i64,
    pub media_bytes: i64,
    pub release_credits: i64,
    pub track_credits: i64,
    pub contributions: i64,
    pub user_states: i64,
}

/// Read-only Release deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseDeletePreview {
    pub releases: Vec<ReleaseDeletePreviewRelease>,
    pub impact: ReleaseDeleteImpact,
    pub warnings: Vec<String>,
}

/// One selected Release whose storage the caller must remove first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseDeleteStorage {
    pub release_public_id: Uuid,
    pub work_item_id: Uuid,
    pub canonical_relative_path: String,
    pub staging_relative_path: Option<String>,
}

/// One administrative Release deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseDelete {
    pub release_id: Uuid,
    pub public_api_id: Uuid,
    pub mode: ReleaseDeleteMode,
    pub actor: String,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

/// Committed result of one Release deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseDeleteOutcome {
    pub deleted_releases: Vec<Uuid>,
}

impl CatalogStore {
    /// Returns the exact bounded impact of deleting the selected Releases
    /// without mutating catalog state.
    ///
    /// # Errors
    /// `InvalidRequest` for an empty, duplicated, or oversized selection;
    /// `MutationConflict` when a Release is missing, merged, or already deleted.
    pub async fn preview_release_delete(
        &self,
        pool: &PgPool,
        release_public_ids: &[Uuid],
    ) -> Result<ReleaseDeletePreview, CatalogStoreError> {
        let selected = validate_release_selection(release_public_ids)?;
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>, String, i64)>(
            "SELECT release.id, release.public_api_id, release.title, release.display_artist,
                    release.status, COALESCE(aggregate.track_count, 0)
               FROM releases release
               LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id
              WHERE release.public_api_id = ANY($1)
              ORDER BY release.public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?;
        if rows.len() != selected.len()
            || rows
                .iter()
                .any(|(_, _, _, _, status, _)| matches!(status.as_str(), "merged" | "deleted"))
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let release_ids = rows
            .iter()
            .map(|(id, _, _, _, _, _)| *id)
            .collect::<Vec<_>>();
        let counts = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
            "WITH selected_tracks AS (
                 SELECT id FROM tracks WHERE release_id = ANY($1)
             ), selected_media AS (
                 SELECT DISTINCT media.id, media.byte_size
                   FROM track_media_assets link
                   JOIN media_assets media ON media.id = link.media_asset_id
                  WHERE link.track_id IN (SELECT id FROM selected_tracks)
             )
             SELECT
                 (SELECT count(*) FROM selected_tracks),
                 (SELECT count(*) FROM selected_media),
                 (SELECT COALESCE(sum(byte_size), 0)::bigint FROM selected_media),
                 (SELECT count(*) FROM release_artists WHERE release_id = ANY($1)),
                 (SELECT count(*) FROM track_artists WHERE track_id IN (SELECT id FROM selected_tracks)),
                 (SELECT count(*) FROM contributions WHERE release_id = ANY($1)),
                 (SELECT count(*) FROM native_release_user_states WHERE release_id = ANY($1))",
        )
        .bind(&release_ids)
        .fetch_one(pool)
        .await?;
        let warnings = if counts.0 == 0 {
            vec!["the selection has no Tracks; only the Release records are retired".to_owned()]
        } else {
            Vec::new()
        };
        Ok(ReleaseDeletePreview {
            releases: rows
                .into_iter()
                .map(
                    |(_, public_api_id, title, display_artist, status, track_count)| {
                        ReleaseDeletePreviewRelease {
                            public_api_id,
                            title,
                            display_artist,
                            status,
                            track_count,
                        }
                    },
                )
                .collect(),
            impact: ReleaseDeleteImpact {
                releases: i64::try_from(release_ids.len())
                    .map_err(|_| CatalogStoreError::NumericOverflow)?,
                tracks: counts.0,
                media_files: counts.1,
                media_bytes: counts.2,
                release_credits: counts.3,
                track_credits: counts.4,
                contributions: counts.5,
                user_states: counts.6,
            },
            warnings,
        })
    }

    /// Resolves durable storage coordinates for the selected Releases.
    /// Releases without publication evidence are omitted, so callers compare
    /// the length against [`ReleaseDeletePreview::impact`] and fail closed.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; database errors otherwise.
    pub async fn release_delete_storage(
        &self,
        pool: &PgPool,
        release_public_ids: &[Uuid],
    ) -> Result<Vec<ReleaseDeleteStorage>, CatalogStoreError> {
        let selected = validate_release_selection(release_public_ids)?;
        Ok(sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>)>(
            "SELECT release.public_api_id, plan.work_item_id, plan.target_relative_path,
                    handoff.staging_relative_path
               FROM releases release
               JOIN release_publication_plans plan ON plan.release_id = release.id
               LEFT JOIN release_work_item_staging_handoffs handoff
                      ON handoff.work_item_id = plan.work_item_id
              WHERE release.public_api_id = ANY($1)
              ORDER BY release.public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(
            |(release_public_id, work_item_id, canonical_relative_path, staging_relative_path)| {
                ReleaseDeleteStorage {
                    release_public_id,
                    work_item_id,
                    canonical_relative_path,
                    staging_relative_path,
                }
            },
        )
        .collect())
    }

    /// Resolves selected Release public ids to durable rows for the commit
    /// step.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; database errors otherwise.
    pub async fn release_delete_refs(
        &self,
        pool: &PgPool,
        release_public_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Uuid)>, CatalogStoreError> {
        let selected = validate_release_selection(release_public_ids)?;
        Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, public_api_id FROM releases
              WHERE public_api_id = ANY($1) ORDER BY public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?)
    }

    /// Commits the Release deletion tombstones. The caller must already have
    /// removed storage for [`ReleaseDeleteMode::ReleasesAndFiles`].
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid batch; `MutationConflict` when a Release
    /// is missing, merged, deleted, or the batch mixes modes.
    pub async fn delete_releases(
        &self,
        pool: &PgPool,
        deletes: &[ReleaseDelete],
    ) -> Result<ReleaseDeleteOutcome, CatalogStoreError> {
        if deletes.is_empty() || deletes.len() > MAX_RELEASE_DELETE_SOURCES {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mode = deletes[0].mode;
        let actor = deletes[0].actor.trim();
        let reason = deletes[0].reason.trim();
        if actor.is_empty() || reason.is_empty() {
            return Err(CatalogStoreError::InvalidRequest);
        }
        let mut release_ids = Vec::with_capacity(deletes.len());
        for delete in deletes {
            if delete.mode != mode
                || delete.actor.trim() != actor
                || delete.reason.trim() != reason
                || release_ids.contains(&delete.release_id)
            {
                return Err(CatalogStoreError::InvalidRequest);
            }
            release_ids.push(delete.release_id);
        }
        let now = deletes[0].occurred_at;
        let mut tx = pool.begin().await?;
        let locked = sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "SELECT id, public_api_id, status FROM releases
              WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(&release_ids)
        .fetch_all(&mut *tx)
        .await?;
        if locked.len() != release_ids.len()
            || locked.iter().any(|(id, public_api_id, status)| {
                matches!(status.as_str(), "merged" | "deleted")
                    || deletes
                        .iter()
                        .find(|delete| delete.release_id == *id)
                        .is_none_or(|delete| delete.public_api_id != *public_api_id)
            })
        {
            return Err(CatalogStoreError::MutationConflict);
        }
        let mut outcome = ReleaseDeleteOutcome::default();
        for (release_id, public_api_id, _) in &locked {
            retire_release(
                &mut tx,
                *release_id,
                actor,
                reason,
                serde_json::json!({
                    "mode": mode.as_str(),
                    "public_api_id": public_api_id,
                    "storage_removed": mode == ReleaseDeleteMode::ReleasesAndFiles,
                    "retained_catalog_tombstone": true,
                }),
                now,
            )
            .await?;
            outcome.deleted_releases.push(*public_api_id);
        }
        tx.commit().await?;
        Ok(outcome)
    }
}

fn validate_release_selection(public_ids: &[Uuid]) -> Result<Vec<Uuid>, CatalogStoreError> {
    if public_ids.is_empty() || public_ids.len() > MAX_RELEASE_DELETE_SOURCES {
        return Err(CatalogStoreError::InvalidRequest);
    }
    let distinct = public_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if distinct.len() != public_ids.len() {
        return Err(CatalogStoreError::InvalidRequest);
    }
    Ok(distinct.into_iter().collect())
}

/// Retires one Release: tombstone, projection deletes, audit, and refreshed
/// aggregates. Evidence rows (Tracks, credits, observations, user state) stay.
async fn retire_release(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    actor: &str,
    reason: &str,
    payload: serde_json::Value,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    let revision = sqlx::query_scalar::<_, i64>(
        "UPDATE releases SET status='deleted', revision=revision+1, updated_at=$2
          WHERE id=$1 AND status <> 'deleted' RETURNING revision",
    )
    .bind(release_id)
    .bind(now)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(CatalogStoreError::MutationConflict)?;
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog','release',$1,'delete',$2,$3,0,$3)
         ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
         DO NOTHING",
    )
    .bind(release_id)
    .bind(revision)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    audit_event(
        tx,
        "release",
        release_id,
        "release_deleted",
        actor,
        reason,
        payload,
        now,
    )
    .await?;
    recompute_artist_aggregates(tx, release_id, now).await?;
    refresh_release_aggregate(tx, release_id, now).await?;
    sqlx::query(
        "WITH updated AS (
            UPDATE tracks SET revision = revision + 1, updated_at = $2
            WHERE release_id = $1 RETURNING id, revision
         )
         INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, attempt_count, created_at
         )
         SELECT 'tantivy.catalog','track',id,'delete',revision,$2,0,$2 FROM updated
         ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
         DO NOTHING",
    )
    .bind(release_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

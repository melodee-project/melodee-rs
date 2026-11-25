//! Hard deletion of catalog Artists, Releases, and Tracks (ADR-0211).
//!
//! The operator asked for a destructive action that leaves nothing behind: the
//! selected catalog rows, their dependent evidence, their processing work item,
//! and every media/artwork file they own are removed. This module owns the
//! database half. Files are removed by the caller first, using only durable
//! paths below configured roots; the commit then runs in one transaction.
//!
//! This deliberately replaces the tombstone semantics of ADR-0199. Public ids
//! are not preserved for deleted entities, and the originating work item is
//! deleted with the Release it produced. One `catalog_audit_events` row records
//! who confirmed the deletion, so accountability survives the removed rows.

use std::collections::BTreeSet;

use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use melodee_domain::ArtistId;

use crate::catalog::{CatalogStore, CatalogStoreError};

/// Maximum entities accepted by one administrative hard deletion.
pub const MAX_HARD_DELETE_SOURCES: usize = 100;

/// Exact impact shown in the confirmation dialog before a hard deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HardDeleteImpact {
    pub artists: i64,
    pub releases: i64,
    pub tracks: i64,
    pub media_files: i64,
    pub media_bytes: i64,
    /// Releases billed as primary by the selected Artist and another Artist;
    /// those Releases are kept and only the selected Artist's rows are removed.
    pub retained_shared_releases: i64,
    pub release_credits: i64,
    pub track_credits: i64,
    pub contributions: i64,
    pub user_states: i64,
    pub images: i64,
}

/// One Artist row in a hard-deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardDeleteArtistPreviewArtist {
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub status: String,
    pub owned_releases: i64,
}

/// Read-only Artist hard-deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardDeleteArtistPreview {
    pub artists: Vec<HardDeleteArtistPreviewArtist>,
    pub impact: HardDeleteImpact,
    pub warnings: Vec<String>,
}

/// One Release row in a hard-deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardDeleteReleasePreviewRelease {
    pub public_api_id: Uuid,
    pub title: String,
    pub display_artist: Option<String>,
    pub status: String,
    pub track_count: i64,
}

/// Read-only Release hard-deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardDeleteReleasePreview {
    pub releases: Vec<HardDeleteReleasePreviewRelease>,
    pub impact: HardDeleteImpact,
    pub warnings: Vec<String>,
}

/// One Track row in a hard-deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardDeleteTrackPreviewTrack {
    pub public_api_id: Uuid,
    pub title: String,
    pub release_public_id: Uuid,
    pub release_title: String,
}

/// Read-only Track hard-deletion preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardDeleteTrackPreview {
    pub tracks: Vec<HardDeleteTrackPreviewTrack>,
    pub impact: HardDeleteImpact,
    pub warnings: Vec<String>,
}

/// Filesystem coordinates for one Release being hard-deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseHardDeleteStorage {
    pub public_api_id: Uuid,
    pub internal_id: Uuid,
    /// Publication work item, when durable publication evidence exists.
    pub work_item_id: Option<Uuid>,
    /// Published Release bundle below the canonical root.
    pub canonical_relative_path: Option<String>,
    /// Retained staging tree below the staging root.
    pub staging_relative_path: Option<String>,
    /// Individual media files when no bounded publication bundle exists.
    pub media_relative_paths: Vec<String>,
    /// Content-addressed artwork keys owned by the Release.
    pub image_asset_keys: Vec<String>,
}

/// Filesystem coordinates for one Track being hard-deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackHardDeleteStorage {
    pub public_api_id: Uuid,
    pub internal_id: Uuid,
    /// Media files no longer referenced by another Track.
    pub media_relative_paths: Vec<String>,
}

/// Filesystem coordinates for one Artist being hard-deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistHardDeleteStorage {
    pub public_api_id: Uuid,
    pub internal_id: ArtistId,
    pub image_asset_keys: Vec<String>,
    /// Releases this Artist owns alone; they are hard-deleted with it.
    pub releases: Vec<ReleaseHardDeleteStorage>,
}

/// Committed hard deletion result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HardDeleteOutcome {
    pub deleted_artists: Vec<Uuid>,
    pub deleted_releases: Vec<Uuid>,
    pub deleted_tracks: Vec<Uuid>,
    /// Artwork asset keys no longer referenced after the commit. The caller
    /// unlinks these from the content-addressed store.
    pub released_asset_keys: Vec<String>,
}

/// Marks this transaction as the one authorized evidence-removal path.
async fn enable_hard_delete(tx: &mut Transaction<'_, Postgres>) -> Result<(), CatalogStoreError> {
    sqlx::query("SELECT set_config('melodee.hard_delete', 'on', true)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn enqueue_projection_delete(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: &str,
    entity_id: Uuid,
    revision: i64,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog',$1,$2,'delete',$3,$4,0,$4)
         ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
         DO NOTHING",
    )
    .bind(entity_type)
    .bind(entity_id)
    .bind(revision)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn enqueue_projection_upsert(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: &str,
    entity_id: Uuid,
    revision: i64,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "INSERT INTO catalog_projection_outbox (
            projection_name, entity_type, entity_id, operation,
            entity_revision, available_at, attempt_count, created_at
         ) VALUES ('tantivy.catalog',$1,$2,'upsert',$3,$4,0,$4)
         ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
         DO NOTHING",
    )
    .bind(entity_type)
    .bind(entity_id)
    .bind(revision)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_selection(public_ids: &[Uuid]) -> Result<Vec<Uuid>, CatalogStoreError> {
    if public_ids.is_empty() || public_ids.len() > MAX_HARD_DELETE_SOURCES {
        return Err(CatalogStoreError::InvalidRequest);
    }
    let distinct = public_ids.iter().copied().collect::<BTreeSet<_>>();
    if distinct.len() != public_ids.len() {
        return Err(CatalogStoreError::InvalidRequest);
    }
    Ok(distinct.into_iter().collect())
}

fn normalized_audit(actor: &str, reason: &str) -> Result<(String, String), CatalogStoreError> {
    let actor = actor.trim();
    let reason = reason.trim();
    if actor.is_empty() || reason.is_empty() {
        return Err(CatalogStoreError::InvalidRequest);
    }
    Ok((actor.to_owned(), reason.to_owned()))
}

impl CatalogStore {
    /// Counts exactly what a hard Release deletion removes. Any status is
    /// accepted: an operator may delete a merged, deprecated, or already
    /// tombstoned Release at any time.
    ///
    /// # Errors
    /// `InvalidRequest` for an empty, duplicated, or oversized selection;
    /// `MutationConflict` when a selected Release does not exist.
    pub async fn preview_hard_delete_releases(
        &self,
        pool: &PgPool,
        release_public_ids: &[Uuid],
    ) -> Result<HardDeleteReleasePreview, CatalogStoreError> {
        let selected = validate_selection(release_public_ids)?;
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
        if rows.len() != selected.len() {
            return Err(CatalogStoreError::MutationConflict);
        }
        let release_ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let impact = release_impact(pool, &release_ids).await?;
        let warnings = if impact.tracks == 0 {
            vec!["the selection has no Tracks".to_owned()]
        } else {
            Vec::new()
        };
        Ok(HardDeleteReleasePreview {
            releases: rows
                .into_iter()
                .map(
                    |(_, public_api_id, title, display_artist, status, track_count)| {
                        HardDeleteReleasePreviewRelease {
                            public_api_id,
                            title,
                            display_artist,
                            status,
                            track_count,
                        }
                    },
                )
                .collect(),
            impact,
            warnings,
        })
    }

    /// Resolves the durable storage and media paths for the selected Releases.
    /// Missing rows are tolerated here; the commit step verifies the selection
    /// again under lock.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; database errors otherwise.
    pub async fn release_hard_delete_storage(
        &self,
        pool: &PgPool,
        release_public_ids: &[Uuid],
    ) -> Result<Vec<ReleaseHardDeleteStorage>, CatalogStoreError> {
        let selected = validate_selection(release_public_ids)?;
        release_storage(pool, &selected).await
    }

    /// Hard-deletes Releases, their Tracks, credits, observations, evidence,
    /// processing work items, projection entries, and owned image references.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid actor/reason or batch;
    /// `MutationConflict` when a selected Release vanished.
    pub async fn hard_delete_releases(
        &self,
        pool: &PgPool,
        release_public_ids: &[Uuid],
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<HardDeleteOutcome, CatalogStoreError> {
        let selected = validate_selection(release_public_ids)?;
        let (actor, reason) = normalized_audit(actor, reason)?;
        let mut tx = pool.begin().await?;
        enable_hard_delete(&mut tx).await?;
        let locked = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, public_api_id FROM releases WHERE public_api_id = ANY($1)
              ORDER BY id FOR UPDATE",
        )
        .bind(&selected)
        .fetch_all(&mut *tx)
        .await?;
        if locked.len() != selected.len() {
            return Err(CatalogStoreError::MutationConflict);
        }
        let mut outcome = HardDeleteOutcome {
            deleted_releases: locked.iter().map(|(_, public)| *public).collect(),
            ..HardDeleteOutcome::default()
        };
        delete_releases_tx(
            &mut tx,
            &locked.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            &actor,
            &reason,
            now,
            &mut outcome,
        )
        .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Counts exactly what a hard Track deletion removes.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; `MutationConflict` when a
    /// selected Track does not exist.
    pub async fn preview_hard_delete_tracks(
        &self,
        pool: &PgPool,
        track_public_ids: &[Uuid],
    ) -> Result<HardDeleteTrackPreview, CatalogStoreError> {
        let selected = validate_selection(track_public_ids)?;
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, Uuid, String)>(
            "SELECT track.id, track.public_api_id, track.title,
                    release.id, release.title
               FROM tracks track
               JOIN releases release ON release.id = track.release_id
              WHERE track.public_api_id = ANY($1)
              ORDER BY track.public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?;
        if rows.len() != selected.len() {
            return Err(CatalogStoreError::MutationConflict);
        }
        let track_ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let impact = track_impact(pool, &track_ids).await?;
        Ok(HardDeleteTrackPreview {
            tracks: rows
                .into_iter()
                .map(
                    |(_, public_api_id, title, release_public_id, release_title)| {
                        HardDeleteTrackPreviewTrack {
                            public_api_id,
                            title,
                            release_public_id,
                            release_title,
                        }
                    },
                )
                .collect(),
            impact,
            warnings: if track_ids.is_empty() {
                vec!["the selection has no Tracks".to_owned()]
            } else {
                Vec::new()
            },
        })
    }

    /// Resolves media paths that become unreferenced when the selected Tracks
    /// are removed. A file another Track still uses is never listed.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; database errors otherwise.
    pub async fn track_hard_delete_storage(
        &self,
        pool: &PgPool,
        track_public_ids: &[Uuid],
    ) -> Result<Vec<TrackHardDeleteStorage>, CatalogStoreError> {
        let selected = validate_selection(track_public_ids)?;
        let rows = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, public_api_id FROM tracks WHERE public_api_id = ANY($1)
              ORDER BY public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?;
        let track_ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let by_track = media_by_track(pool, &track_ids).await?;
        rows.into_iter()
            .map(|(internal_id, public_api_id)| {
                let media_relative_paths = by_track
                    .iter()
                    .filter(|(track_id, _)| *track_id == internal_id)
                    .map(|(_, path)| path.clone())
                    .collect();
                Ok(TrackHardDeleteStorage {
                    public_api_id,
                    internal_id,
                    media_relative_paths,
                })
            })
            .collect::<Result<Vec<_>, CatalogStoreError>>()
    }

    /// Hard-deletes Tracks and every row that exists only because of them,
    /// recomputes the surviving Release aggregate, and removes orphaned media
    /// asset rows.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid actor/reason or batch;
    /// `MutationConflict` when a selected Track vanished.
    pub async fn hard_delete_tracks(
        &self,
        pool: &PgPool,
        track_public_ids: &[Uuid],
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<HardDeleteOutcome, CatalogStoreError> {
        let selected = validate_selection(track_public_ids)?;
        let (actor, reason) = normalized_audit(actor, reason)?;
        let mut tx = pool.begin().await?;
        enable_hard_delete(&mut tx).await?;
        let locked = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, public_api_id FROM tracks WHERE public_api_id = ANY($1)
              ORDER BY id FOR UPDATE",
        )
        .bind(&selected)
        .fetch_all(&mut *tx)
        .await?;
        if locked.len() != selected.len() {
            return Err(CatalogStoreError::MutationConflict);
        }
        let mut outcome = HardDeleteOutcome {
            deleted_tracks: locked.iter().map(|(_, public)| *public).collect(),
            ..HardDeleteOutcome::default()
        };
        delete_tracks_tx(
            &mut tx,
            &locked.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            &actor,
            &reason,
            now,
            &mut outcome,
        )
        .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Counts exactly what a hard Artist deletion removes.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; `MutationConflict` when a
    /// selected Artist does not exist.
    pub async fn preview_hard_delete_artists(
        &self,
        pool: &PgPool,
        artist_public_ids: &[Uuid],
    ) -> Result<HardDeleteArtistPreview, CatalogStoreError> {
        let selected = validate_selection(artist_public_ids)?;
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            "SELECT id, public_api_id, canonical_name, status FROM artists
              WHERE public_api_id = ANY($1) ORDER BY public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?;
        if rows.len() != selected.len() {
            return Err(CatalogStoreError::MutationConflict);
        }
        let artist_ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let owned = owned_release_ids(pool, &artist_ids).await?;
        let retained_shared_releases = shared_release_count(pool, &artist_ids).await?;
        let mut impact = release_impact(pool, &owned).await?;
        // Owned release counting by artist is only used for display. A
        // Release owned jointly by several *selected* Artists is still owned.
        let owned_counts = sqlx::query_as::<_, (Uuid, i64)>(
            "WITH owned AS (
                 SELECT billing.release_id, billing.artist_id
                   FROM release_artists billing
                  WHERE billing.artist_id = ANY($1)
                    AND billing.role = 'primary'
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
        let direct = sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT
                (SELECT count(*) FROM release_artists WHERE artist_id = ANY($1)),
                (SELECT count(*) FROM track_artists WHERE artist_id = ANY($1)),
                (SELECT count(*) FROM contributions WHERE artist_id = ANY($1)),
                (SELECT count(*) FROM native_artist_user_states WHERE artist_id = ANY($1))",
        )
        .bind(&artist_ids)
        .fetch_one(pool)
        .await?;
        impact.artists = i64::try_from(artist_ids.len()).unwrap_or(i64::MAX);
        impact.release_credits = direct.0;
        impact.track_credits = direct.1;
        impact.contributions = direct.2;
        impact.user_states = direct.3;
        impact.retained_shared_releases = retained_shared_releases;
        impact.images += count_artist_images(pool, &artist_ids).await?;
        let mut warnings = Vec::new();
        if impact.releases == 0 {
            warnings
                .push("the selection owns no Release; only the Artist rows are removed".to_owned());
        }
        if retained_shared_releases > 0 {
            warnings.push(format!(
                "{retained_shared_releases} Release(s) are also billed as primary by another Artist and are kept"
            ));
        }
        Ok(HardDeleteArtistPreview {
            artists: rows
                .into_iter()
                .map(
                    |(id, public_api_id, canonical_name, status)| HardDeleteArtistPreviewArtist {
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
            impact,
            warnings,
        })
    }

    /// Resolves the storage coordinates of every Release the selected Artists
    /// own alone, plus each Artist's own artwork keys.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid selection; database errors otherwise.
    pub async fn artist_hard_delete_storage(
        &self,
        pool: &PgPool,
        artist_public_ids: &[Uuid],
    ) -> Result<Vec<ArtistHardDeleteStorage>, CatalogStoreError> {
        let selected = validate_selection(artist_public_ids)?;
        let rows = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, public_api_id FROM artists WHERE public_api_id = ANY($1)
              ORDER BY public_api_id",
        )
        .bind(&selected)
        .fetch_all(pool)
        .await?;
        let owned =
            owned_release_ids(pool, &rows.iter().map(|(id, _)| *id).collect::<Vec<_>>()).await?;
        let releases = release_storage_by_internal(pool, &owned).await?;
        let artist_keys = image_keys_for_entities(
            pool,
            "artist",
            &rows.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        )
        .await?;
        rows.into_iter()
            .map(|(internal_id, public_api_id)| {
                let artist_id = ArtistId::from_uuid(internal_id);
                let image_asset_keys = artist_keys.get(&internal_id).cloned().unwrap_or_default();
                Ok(ArtistHardDeleteStorage {
                    public_api_id,
                    internal_id: artist_id,
                    image_asset_keys,
                    releases: releases.clone(),
                })
            })
            .collect()
    }

    /// Hard-deletes Artists, their credits, evidence, relationships, and every
    /// Release they own alone. Releases shared with another primary Artist are
    /// kept; the selected Artist's rows are removed from them.
    ///
    /// # Errors
    /// `InvalidRequest` for an invalid actor/reason or batch;
    /// `MutationConflict` when a selected Artist vanished.
    pub async fn hard_delete_artists(
        &self,
        pool: &PgPool,
        artist_public_ids: &[Uuid],
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<HardDeleteOutcome, CatalogStoreError> {
        let selected = validate_selection(artist_public_ids)?;
        let (actor, reason) = normalized_audit(actor, reason)?;
        let mut tx = pool.begin().await?;
        enable_hard_delete(&mut tx).await?;
        let locked = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, public_api_id FROM artists WHERE public_api_id = ANY($1)
              ORDER BY id FOR UPDATE",
        )
        .bind(&selected)
        .fetch_all(&mut *tx)
        .await?;
        if locked.len() != selected.len() {
            return Err(CatalogStoreError::MutationConflict);
        }
        let artist_ids = locked.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let owned = owned_release_ids_tx(&mut tx, &artist_ids).await?;
        if !owned.is_empty() {
            sqlx::query("SELECT id FROM releases WHERE id = ANY($1) ORDER BY id FOR UPDATE")
                .bind(&owned)
                .fetch_all(&mut *tx)
                .await?;
        }
        let mut outcome = HardDeleteOutcome {
            deleted_artists: locked.iter().map(|(_, public)| *public).collect(),
            deleted_releases: sqlx::query_scalar::<_, Uuid>(
                "SELECT public_api_id FROM releases WHERE id = ANY($1) ORDER BY public_api_id",
            )
            .bind(&owned)
            .fetch_all(&mut *tx)
            .await?,
            ..HardDeleteOutcome::default()
        };
        // Release cleanup first: it removes the billing rows that would
        // otherwise keep the Artist rows alive.
        delete_releases_tx(&mut tx, &owned, &actor, &reason, now, &mut outcome).await?;
        delete_artists_tx(&mut tx, &artist_ids, &actor, &reason, now, &mut outcome).await?;
        tx.commit().await?;
        Ok(outcome)
    }
}

async fn release_impact(
    pool: &PgPool,
    release_ids: &[Uuid],
) -> Result<HardDeleteImpact, CatalogStoreError> {
    if release_ids.is_empty() {
        return Ok(HardDeleteImpact::default());
    }
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
    .bind(release_ids)
    .fetch_one(pool)
    .await?;
    Ok(HardDeleteImpact {
        artists: 0,
        releases: i64::try_from(release_ids.len()).unwrap_or(i64::MAX),
        tracks: counts.0,
        media_files: counts.1,
        media_bytes: counts.2,
        retained_shared_releases: 0,
        release_credits: counts.3,
        track_credits: counts.4,
        contributions: counts.5,
        user_states: counts.6,
        images: count_release_images(pool, release_ids).await?,
    })
}

async fn track_impact(
    pool: &PgPool,
    track_ids: &[Uuid],
) -> Result<HardDeleteImpact, CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(HardDeleteImpact::default());
    }
    let counts = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64)>(
        "WITH selected_media AS (
             SELECT DISTINCT media.id, media.byte_size
               FROM track_media_assets link
               JOIN media_assets media ON media.id = link.media_asset_id
              WHERE link.track_id = ANY($1)
         )
         SELECT
             (SELECT count(*) FROM selected_media),
             (SELECT COALESCE(sum(byte_size), 0)::bigint FROM selected_media),
             (SELECT count(*) FROM track_artists WHERE track_id = ANY($1)),
             (SELECT count(*) FROM contributions WHERE track_id = ANY($1)),
             (SELECT count(*) FROM native_track_user_states WHERE track_id = ANY($1)),
             (SELECT count(*) FROM native_track_plays WHERE track_id = ANY($1))",
    )
    .bind(track_ids)
    .fetch_one(pool)
    .await?;
    Ok(HardDeleteImpact {
        tracks: i64::try_from(track_ids.len()).unwrap_or(i64::MAX),
        media_files: counts.0,
        media_bytes: counts.1,
        track_credits: counts.2,
        contributions: counts.3,
        user_states: counts.4 + counts.5,
        ..HardDeleteImpact::default()
    })
}

async fn owned_release_ids(
    pool: &PgPool,
    artist_ids: &[Uuid],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    if artist_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT billing.release_id
           FROM release_artists billing
          WHERE billing.artist_id = ANY($1)
            AND billing.role = 'primary'
            AND NOT EXISTS (
                SELECT 1 FROM release_artists other
                 WHERE other.release_id = billing.release_id
                   AND other.role = 'primary'
                   AND other.artist_id <> ALL($1))
          ORDER BY billing.release_id",
    )
    .bind(artist_ids)
    .fetch_all(pool)
    .await?)
}

async fn owned_release_ids_tx(
    tx: &mut Transaction<'_, Postgres>,
    artist_ids: &[Uuid],
) -> Result<Vec<Uuid>, CatalogStoreError> {
    if artist_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT billing.release_id
           FROM release_artists billing
          WHERE billing.artist_id = ANY($1)
            AND billing.role = 'primary'
            AND NOT EXISTS (
                SELECT 1 FROM release_artists other
                 WHERE other.release_id = billing.release_id
                   AND other.role = 'primary'
                   AND other.artist_id <> ALL($1))
          ORDER BY billing.release_id",
    )
    .bind(artist_ids)
    .fetch_all(&mut **tx)
    .await?)
}

async fn shared_release_count(
    pool: &PgPool,
    artist_ids: &[Uuid],
) -> Result<i64, CatalogStoreError> {
    if artist_ids.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(DISTINCT billing.release_id)
           FROM release_artists billing
          WHERE billing.artist_id = ANY($1)
            AND billing.role = 'primary'
            AND EXISTS (
                SELECT 1 FROM release_artists other
                 WHERE other.release_id = billing.release_id
                   AND other.role = 'primary'
                   AND other.artist_id <> ALL($1))",
    )
    .bind(artist_ids)
    .fetch_one(pool)
    .await?)
}

async fn count_release_images(
    pool: &PgPool,
    release_ids: &[Uuid],
) -> Result<i64, CatalogStoreError> {
    if release_ids.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_entity_images WHERE release_id = ANY($1)",
    )
    .bind(release_ids)
    .fetch_one(pool)
    .await?)
}

async fn count_artist_images(pool: &PgPool, artist_ids: &[Uuid]) -> Result<i64, CatalogStoreError> {
    if artist_ids.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_entity_images WHERE artist_id = ANY($1)",
    )
    .bind(artist_ids)
    .fetch_one(pool)
    .await?)
}

async fn release_storage(
    pool: &PgPool,
    release_public_ids: &[Uuid],
) -> Result<Vec<ReleaseHardDeleteStorage>, CatalogStoreError> {
    let rows = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT id, public_api_id FROM releases WHERE public_api_id = ANY($1)
          ORDER BY public_api_id",
    )
    .bind(release_public_ids)
    .fetch_all(pool)
    .await?;
    release_storage_by_internal(pool, &rows.iter().map(|row| row.0).collect::<Vec<_>>()).await
}

async fn release_storage_by_internal(
    pool: &PgPool,
    release_ids: &[Uuid],
) -> Result<Vec<ReleaseHardDeleteStorage>, CatalogStoreError> {
    if release_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>, Option<String>, Option<String>)>(
        "SELECT release.id, release.public_api_id, plan.work_item_id,
                plan.target_relative_path, handoff.staging_relative_path
           FROM releases release
           LEFT JOIN release_publication_plans plan ON plan.release_id = release.id
           LEFT JOIN release_work_item_staging_handoffs handoff
                  ON handoff.work_item_id = plan.work_item_id
          WHERE release.id = ANY($1)
          ORDER BY release.id",
    )
    .bind(release_ids)
    .fetch_all(pool)
    .await?;
    let track_ids = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT id, release_id FROM tracks WHERE release_id = ANY($1)",
    )
    .bind(release_ids)
    .fetch_all(pool)
    .await?;
    let all_track_ids = track_ids.iter().map(|row| row.0).collect::<Vec<_>>();
    let media_by_track = media_by_track(pool, &all_track_ids).await?;
    let image_keys = image_keys_for_entities(pool, "release", release_ids).await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                internal_id,
                public_api_id,
                work_item_id,
                canonical_relative_path,
                staging_relative_path,
            )| {
                let media_relative_paths = track_ids
                    .iter()
                    .filter(|(_, release_id)| *release_id == internal_id)
                    .flat_map(|(track_id, _)| {
                        media_by_track
                            .iter()
                            .filter(move |(candidate, _)| candidate == track_id)
                            .map(|(_, path)| path.clone())
                    })
                    .collect();
                ReleaseHardDeleteStorage {
                    public_api_id,
                    internal_id,
                    work_item_id,
                    canonical_relative_path,
                    staging_relative_path,
                    media_relative_paths,
                    image_asset_keys: image_keys.get(&internal_id).cloned().unwrap_or_default(),
                }
            },
        )
        .collect())
}

async fn media_by_track(
    pool: &PgPool,
    track_ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        "SELECT DISTINCT link.track_id, media.canonical_relative_path
           FROM track_media_assets link
           JOIN media_assets media ON media.id = link.media_asset_id
          WHERE link.track_id = ANY($1)
            AND NOT EXISTS (
                SELECT 1 FROM track_media_assets other
                 WHERE other.media_asset_id = media.id
                   AND other.track_id <> ALL($1))",
    )
    .bind(track_ids)
    .fetch_all(pool)
    .await?)
}

async fn image_keys_for_entities(
    pool: &PgPool,
    entity_type: &str,
    entity_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, CatalogStoreError> {
    if entity_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT artist_id, asset_key FROM catalog_entity_images
          WHERE $1 = 'artist' AND artist_id = ANY($2)
         UNION ALL
         SELECT release_id, asset_key FROM catalog_entity_images
          WHERE $1 = 'release' AND release_id = ANY($2)",
    )
    .bind(entity_type)
    .bind(entity_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .fold(std::collections::HashMap::new(), |mut acc, (id, key)| {
            acc.entry(id).or_insert_with(Vec::new).push(key);
            acc
        }))
}

async fn delete_releases_tx(
    tx: &mut Transaction<'_, Postgres>,
    release_ids: &[Uuid],
    actor: &str,
    reason: &str,
    now: OffsetDateTime,
    outcome: &mut HardDeleteOutcome,
) -> Result<(), CatalogStoreError> {
    if release_ids.is_empty() {
        return Ok(());
    }
    let track_revisions = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, revision FROM tracks WHERE release_id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(release_ids)
    .fetch_all(&mut **tx)
    .await?;
    let track_ids = track_revisions
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    let release_revisions = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, revision FROM releases WHERE id = ANY($1) ORDER BY id",
    )
    .bind(release_ids)
    .fetch_all(&mut **tx)
    .await?;
    let observed_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM observed_artists WHERE release_id = ANY($1) OR track_id = ANY($2)",
    )
    .bind(release_ids)
    .bind(&track_ids)
    .fetch_all(&mut **tx)
    .await?;
    let work_items = sqlx::query_scalar::<_, Uuid>(
        "SELECT work_item_id FROM catalog_imports WHERE release_id = ANY($1)
         UNION
         SELECT work_item_id FROM release_publication_plans WHERE release_id = ANY($1)",
    )
    .bind(release_ids)
    .fetch_all(&mut **tx)
    .await?;
    let media_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT media_asset_id FROM track_media_assets
          WHERE track_id = ANY($1)
            AND NOT EXISTS (
                SELECT 1 FROM track_media_assets other
                 WHERE other.media_asset_id = track_media_assets.media_asset_id
                   AND other.track_id <> ALL($1))",
    )
    .bind(&track_ids)
    .fetch_all(&mut **tx)
    .await?;
    let overlay_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM catalog_semantic_overlays WHERE release_id = ANY($1)",
    )
    .bind(release_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut asset_keys = provider_image_keys_for_entities(tx, "release", release_ids).await?;
    let release_keys = image_keys_tx(tx, "release", release_ids).await?;
    asset_keys.extend(release_keys);

    // Break self-references and references from surviving rows first.
    sqlx::query(
        "UPDATE artist_match_decisions SET predecessor_decision_id = NULL
          WHERE predecessor_decision_id IN (
              SELECT id FROM artist_match_decisions
               WHERE observed_artist_id = ANY($1)
          )",
    )
    .bind(&observed_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE artist_negative_assertions SET predecessor_assertion_id = NULL
          WHERE predecessor_assertion_id IN (
              SELECT id FROM artist_negative_assertions
               WHERE observed_artist_id = ANY($1)
          )",
    )
    .bind(&observed_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE releases SET current_overlay_id = NULL WHERE id = ANY($1)")
        .bind(release_ids)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE catalog_semantic_overlays SET predecessor_overlay_id = NULL
          WHERE predecessor_overlay_id = ANY($1)",
    )
    .bind(&overlay_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE releases SET status = 'deprecated', merged_into_release_id = NULL, updated_at = $2
          WHERE merged_into_release_id = ANY($1) AND id <> ALL($1)",
    )
    .bind(release_ids)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    for statement in [
        "DELETE FROM policy_decision_audit WHERE release_id = ANY($1) OR work_item_id = ANY($2)",
        "DELETE FROM policy_decisions WHERE release_id = ANY($1) OR work_item_id = ANY($2)",
        "DELETE FROM track_novelty WHERE release_id = ANY($1) OR track_id = ANY($3)",
        "DELETE FROM release_novelty_evaluations WHERE release_id = ANY($1)",
        "DELETE FROM release_classifications WHERE release_id = ANY($1)",
        "DELETE FROM release_catalog_trait_assertions WHERE release_id = ANY($1)",
        "DELETE FROM catalog_overlay_repair_queue WHERE release_id = ANY($1)",
        "DELETE FROM catalog_semantic_overlays WHERE release_id = ANY($1)",
        "DELETE FROM metadata_edit_audits WHERE release_id = ANY($1)",
        "DELETE FROM native_release_user_states WHERE release_id = ANY($1)",
        "DELETE FROM release_external_identities WHERE release_id = ANY($1)",
        "DELETE FROM release_split_events WHERE source_release_id = ANY($1) OR target_release_id = ANY($1)
             OR merge_event_id IN (SELECT id FROM release_merge_events WHERE source_release_id = ANY($1) OR target_release_id = ANY($1))",
        "DELETE FROM release_merge_events WHERE source_release_id = ANY($1) OR target_release_id = ANY($1)",
        "DELETE FROM catalog_imports WHERE release_id = ANY($1)",
        "DELETE FROM release_publication_journal WHERE work_item_id = ANY($2)",
        "DELETE FROM release_publication_receipts WHERE release_id = ANY($1)",
        "DELETE FROM release_publication_plans WHERE release_id = ANY($1)",
        "DELETE FROM release_publication_authorizations WHERE release_id = ANY($1) OR work_item_id = ANY($2)",
        "DELETE FROM artist_match_current WHERE observed_artist_id = ANY($4)",
        "DELETE FROM artist_match_decisions WHERE observed_artist_id = ANY($4)",
        "DELETE FROM artist_match_evidence WHERE observed_artist_id = ANY($4)",
        "DELETE FROM artist_negative_assertions WHERE observed_artist_id = ANY($4)",
        "DELETE FROM contributions WHERE release_id = ANY($1) OR track_id = ANY($3)",
        "DELETE FROM release_artists WHERE release_id = ANY($1)",
        "DELETE FROM track_artists WHERE track_id = ANY($3)",
        "DELETE FROM artist_track_associations WHERE track_id = ANY($3)",
        "DELETE FROM observed_artists WHERE id = ANY($4)",
        "DELETE FROM group_membership_track_exclusions WHERE track_id = ANY($3)",
        "DELETE FROM native_now_playing WHERE track_id = ANY($3)",
        "DELETE FROM native_playlist_items WHERE track_id = ANY($3)",
        "DELETE FROM native_track_plays WHERE track_id = ANY($3)",
        "DELETE FROM native_track_user_states WHERE track_id = ANY($3)",
        "DELETE FROM audio_identity_evidence WHERE track_id = ANY($3)",
        "DELETE FROM release_catalog_aggregates WHERE release_id = ANY($1)",
        "DELETE FROM catalog_entity_images WHERE release_id = ANY($1)",
        "DELETE FROM catalog_provider_images WHERE entity_type = 'release' AND entity_id = ANY($1)",
        "DELETE FROM catalog_projection_outbox WHERE entity_type = 'release' AND entity_id = ANY($1)",
        "DELETE FROM legacy_catalog_id_mappings WHERE entity_type = 'release' AND internal_id = ANY($1)",
        "DELETE FROM review_deletion_authorizations WHERE work_item_id = ANY($2)",
        "DELETE FROM release_work_items WHERE id = ANY($2)",
    ] {
        sqlx::query(statement)
            .bind(release_ids)
            .bind(&work_items)
            .bind(&track_ids)
            .bind(&observed_ids)
            .bind(&media_ids)
            .execute(&mut **tx)
            .await?;
    }
    for (release_id, revision) in &release_revisions {
        enqueue_projection_delete(tx, "release", *release_id, revision + 1, now).await?;
    }
    for (track_id, revision) in &track_revisions {
        enqueue_projection_delete(tx, "track", *track_id, revision + 1, now).await?;
    }
    // The mandatory playable-asset link forms a cycle between `tracks` and
    // `track_media_assets`; remove both in one statement so the deferred FK
    // sees neither side by statement end.
    sqlx::query(
        "WITH deleted_tracks AS (
             DELETE FROM tracks WHERE id = ANY($1) RETURNING id
         )
         DELETE FROM track_media_assets WHERE track_id = ANY($1)",
    )
    .bind(&track_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM media_assets WHERE id = ANY($1)
          AND NOT EXISTS (SELECT 1 FROM track_media_assets WHERE media_asset_id = media_assets.id)
          AND NOT EXISTS (SELECT 1 FROM track_novelty WHERE media_asset_id = media_assets.id)",
    )
    .bind(&media_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM releases WHERE id = ANY($1)")
        .bind(release_ids)
        .execute(&mut **tx)
        .await?;
    for release_id in release_ids {
        audit_event(
            tx,
            "release",
            *release_id,
            "release_hard_deleted",
            actor,
            reason,
            serde_json::json!({"hard_delete": true, "deleted_tracks": track_ids.len()}),
            now,
        )
        .await?;
    }
    purge_asset_files(tx, &asset_keys).await?;
    outcome.released_asset_keys.extend(asset_keys);
    Ok(())
}

async fn delete_tracks_tx(
    tx: &mut Transaction<'_, Postgres>,
    track_ids: &[Uuid],
    actor: &str,
    reason: &str,
    now: OffsetDateTime,
    _outcome: &mut HardDeleteOutcome,
) -> Result<(), CatalogStoreError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let track_revisions = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, revision FROM tracks WHERE id = ANY($1) ORDER BY id",
    )
    .bind(track_ids)
    .fetch_all(&mut **tx)
    .await?;
    let release_ids =
        sqlx::query_scalar::<_, Uuid>("SELECT DISTINCT release_id FROM tracks WHERE id = ANY($1)")
            .bind(track_ids)
            .fetch_all(&mut **tx)
            .await?;
    let release_revisions = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, revision FROM releases WHERE id = ANY($1) ORDER BY id",
    )
    .bind(&release_ids)
    .fetch_all(&mut **tx)
    .await?;
    let observed_ids =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM observed_artists WHERE track_id = ANY($1)")
            .bind(track_ids)
            .fetch_all(&mut **tx)
            .await?;
    let media_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT media_asset_id FROM track_media_assets
          WHERE track_id = ANY($1)
            AND NOT EXISTS (
                SELECT 1 FROM track_media_assets other
                 WHERE other.media_asset_id = track_media_assets.media_asset_id
                   AND other.track_id <> ALL($1))",
    )
    .bind(track_ids)
    .fetch_all(&mut **tx)
    .await?;
    for statement in [
        "DELETE FROM track_novelty WHERE track_id = ANY($1)",
        "DELETE FROM artist_match_current WHERE observed_artist_id = ANY($2)",
        "DELETE FROM artist_match_decisions WHERE observed_artist_id = ANY($2)",
        "DELETE FROM artist_match_evidence WHERE observed_artist_id = ANY($2)",
        "DELETE FROM artist_negative_assertions WHERE observed_artist_id = ANY($2)",
        "DELETE FROM contributions WHERE track_id = ANY($1)",
        "DELETE FROM track_artists WHERE track_id = ANY($1)",
        "DELETE FROM artist_track_associations WHERE track_id = ANY($1)",
        "DELETE FROM observed_artists WHERE id = ANY($2)",
        "DELETE FROM group_membership_track_exclusions WHERE track_id = ANY($1)",
        "DELETE FROM native_now_playing WHERE track_id = ANY($1)",
        "DELETE FROM native_playlist_items WHERE track_id = ANY($1)",
        "DELETE FROM native_track_plays WHERE track_id = ANY($1)",
        "DELETE FROM native_track_user_states WHERE track_id = ANY($1)",
        "DELETE FROM audio_identity_evidence WHERE track_id = ANY($1)",
        "DELETE FROM catalog_projection_outbox WHERE entity_type = 'track' AND entity_id = ANY($1)",
        "DELETE FROM legacy_catalog_id_mappings WHERE entity_type = 'track' AND internal_id = ANY($1)",
    ] {
        sqlx::query(statement)
            .bind(track_ids)
            .bind(&observed_ids)
            .bind(&media_ids)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query(
        "WITH deleted_tracks AS (
             DELETE FROM tracks WHERE id = ANY($1) RETURNING id
         )
         DELETE FROM track_media_assets WHERE track_id = ANY($1)",
    )
    .bind(track_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM media_assets WHERE id = ANY($1)
          AND NOT EXISTS (SELECT 1 FROM track_media_assets WHERE media_asset_id = media_assets.id)
          AND NOT EXISTS (SELECT 1 FROM track_novelty WHERE media_asset_id = media_assets.id)",
    )
    .bind(&media_ids)
    .execute(&mut **tx)
    .await?;
    for (track_id, revision) in &track_revisions {
        enqueue_projection_delete(tx, "track", *track_id, revision + 1, now).await?;
    }
    for (release_id, revision) in &release_revisions {
        let next_revision = revision + 1;
        sqlx::query("UPDATE releases SET revision=$2, updated_at=$3 WHERE id=$1")
            .bind(release_id)
            .bind(next_revision)
            .bind(now)
            .execute(&mut **tx)
            .await?;
        enqueue_projection_upsert(tx, "release", *release_id, next_revision, now).await?;
        refresh_release_aggregate(tx, *release_id, now).await?;
        recompute_artist_aggregates(tx, *release_id, now).await?;
    }
    for track_id in track_ids {
        audit_event(
            tx,
            "track",
            *track_id,
            "track_hard_deleted",
            actor,
            reason,
            serde_json::json!({"hard_delete": true}),
            now,
        )
        .await?;
    }
    Ok(())
}

async fn delete_artists_tx(
    tx: &mut Transaction<'_, Postgres>,
    artist_ids: &[Uuid],
    actor: &str,
    reason: &str,
    now: OffsetDateTime,
    outcome: &mut HardDeleteOutcome,
) -> Result<(), CatalogStoreError> {
    if artist_ids.is_empty() {
        return Ok(());
    }
    let artist_revisions = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id, revision FROM artists WHERE id = ANY($1) ORDER BY id",
    )
    .bind(artist_ids)
    .fetch_all(&mut **tx)
    .await?;
    let observed_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT observed_artist_id FROM (
             SELECT decision.observed_artist_id FROM artist_match_decisions decision
              WHERE decision.artist_id = ANY($1)
             UNION
             SELECT current.observed_artist_id FROM artist_match_current current
               JOIN artist_match_decisions decision ON decision.id = current.decision_id
              WHERE decision.artist_id = ANY($1)
         ) selected",
    )
    .bind(artist_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut asset_keys = provider_image_keys_for_entities(tx, "artist", artist_ids).await?;
    asset_keys.extend(image_keys_tx(tx, "artist", artist_ids).await?);

    sqlx::query(
        "UPDATE artist_match_decisions SET predecessor_decision_id = NULL
          WHERE predecessor_decision_id IN (
              SELECT id FROM artist_match_decisions
               WHERE artist_id = ANY($1) OR observed_artist_id = ANY($2)
          )",
    )
    .bind(artist_ids)
    .bind(&observed_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE artist_negative_assertions SET predecessor_assertion_id = NULL
          WHERE predecessor_assertion_id IN (
              SELECT id FROM artist_negative_assertions
               WHERE candidate_artist_id = ANY($1) OR observed_artist_id = ANY($2)
          )",
    )
    .bind(artist_ids)
    .bind(&observed_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE artists SET status = 'active', merged_into_artist_id = NULL, updated_at = $2
          WHERE merged_into_artist_id = ANY($1)",
    )
    .bind(artist_ids)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    for statement in [
        "DELETE FROM artist_match_current WHERE observed_artist_id = ANY($1)",
        "DELETE FROM artist_match_decisions WHERE artist_id = ANY($2) OR observed_artist_id = ANY($1)",
        "DELETE FROM artist_match_evidence WHERE candidate_artist_id = ANY($2) OR observed_artist_id = ANY($1)",
        "DELETE FROM artist_negative_assertions WHERE candidate_artist_id = ANY($2) OR observed_artist_id = ANY($1)",
        "DELETE FROM release_artists WHERE artist_id = ANY($2)",
        "DELETE FROM track_artists WHERE artist_id = ANY($2)",
        "DELETE FROM contributions WHERE artist_id = ANY($2)",
        "DELETE FROM artist_track_associations WHERE artist_id = ANY($2) OR source_artist_id = ANY($2)",
        "DELETE FROM artist_relationships WHERE source_artist_id = ANY($2) OR target_artist_id = ANY($2)",
        "DELETE FROM group_memberships WHERE member_artist_id = ANY($2) OR group_artist_id = ANY($2)",
        "DELETE FROM artist_split_events WHERE source_artist_id = ANY($2) OR new_artist_id = ANY($2)",
        "DELETE FROM artist_merge_events WHERE source_artist_id = ANY($2) OR target_artist_id = ANY($2)",
        "DELETE FROM artist_aliases WHERE artist_id = ANY($2)",
        "DELETE FROM artist_external_identities WHERE artist_id = ANY($2)",
        "DELETE FROM provider_identity_candidates WHERE artist_id = ANY($2)",
        "DELETE FROM native_artist_user_states WHERE artist_id = ANY($2)",
        "DELETE FROM artist_catalog_aggregates WHERE artist_id = ANY($2)",
        "DELETE FROM catalog_entity_images WHERE artist_id = ANY($2)",
        "DELETE FROM catalog_provider_images WHERE entity_type = 'artist' AND entity_id = ANY($2)",
        "DELETE FROM catalog_projection_outbox WHERE entity_type = 'artist' AND entity_id = ANY($2)",
        "DELETE FROM legacy_catalog_id_mappings WHERE entity_type = 'artist' AND internal_id = ANY($2)",
        "DELETE FROM artists WHERE id = ANY($2)",
    ] {
        sqlx::query(statement)
            .bind(&observed_ids)
            .bind(artist_ids)
            .execute(&mut **tx)
            .await?;
    }
    for (artist_id, revision) in &artist_revisions {
        enqueue_projection_delete(tx, "artist", *artist_id, revision + 1, now).await?;
    }
    for artist_id in artist_ids {
        audit_event(
            tx,
            "artist",
            *artist_id,
            "artist_hard_deleted",
            actor,
            reason,
            serde_json::json!({"hard_delete": true}),
            now,
        )
        .await?;
    }
    purge_asset_files(tx, &asset_keys).await?;
    outcome.released_asset_keys.extend(asset_keys);
    Ok(())
}

async fn image_keys_tx(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: &str,
    entity_ids: &[Uuid],
) -> Result<Vec<String>, CatalogStoreError> {
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT asset_key FROM catalog_entity_images
          WHERE ($1 = 'artist' AND artist_id = ANY($2))
             OR ($1 = 'release' AND release_id = ANY($2))",
    )
    .bind(entity_type)
    .bind(entity_ids)
    .fetch_all(&mut **tx)
    .await?)
}

async fn provider_image_keys_for_entities(
    tx: &mut Transaction<'_, Postgres>,
    entity_type: &str,
    entity_ids: &[Uuid],
) -> Result<Vec<String>, CatalogStoreError> {
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT asset_key FROM catalog_provider_images
          WHERE entity_type = $1 AND entity_id = ANY($2)",
    )
    .bind(entity_type)
    .bind(entity_ids)
    .fetch_all(&mut **tx)
    .await?)
}

async fn purge_asset_files(
    tx: &mut Transaction<'_, Postgres>,
    keys: &[String],
) -> Result<(), CatalogStoreError> {
    if keys.is_empty() {
        return Ok(());
    }
    for key in keys {
        sqlx::query(
            "DELETE FROM asset_files WHERE asset_key = $1
              AND NOT EXISTS (SELECT 1 FROM catalog_entity_images WHERE asset_key = $1)
              AND NOT EXISTS (SELECT 1 FROM catalog_provider_images WHERE asset_key = $1)
              AND NOT EXISTS (SELECT 1 FROM native_user_avatars WHERE asset_key = $1)
              AND NOT EXISTS (SELECT 1 FROM native_theme_assets WHERE asset_key = $1)
              AND NOT EXISTS (SELECT 1 FROM native_playlists WHERE asset_key = $1)
              AND NOT EXISTS (SELECT 1 FROM native_users WHERE asset_key = $1)",
        )
        .bind(key)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
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

async fn refresh_release_aggregate(
    tx: &mut Transaction<'_, Postgres>,
    release_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), CatalogStoreError> {
    sqlx::query(
        "INSERT INTO release_catalog_aggregates (
            release_id, track_count, duration_ms, media_bytes, revision, updated_at
         )
         SELECT $1, count(DISTINCT track.id), COALESCE(sum(track.duration_ms), 0),
                COALESCE((SELECT sum(media.byte_size)
                            FROM track_media_assets link
                            JOIN media_assets media ON media.id = link.media_asset_id
                           WHERE link.track_id IN (SELECT id FROM tracks WHERE release_id = $1)), 0),
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

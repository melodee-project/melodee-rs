//! Track and canonical media adoption importer (ADR-0148 §D.3). Legacy Song
//! rows become Rust Track + AudioIdentity + MediaAsset + playable link in one
//! fenced transaction per batch. Media is adopted in place: the stored path
//! is canonical-relative, hashes are adopted only when already valid, and the
//! hash-pending sentinel is recorded otherwise.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::LibraryPathMap;
use super::map;
use super::media::{CanonicalRootValidator, MediaCheck};
use super::runs;
use super::source::SourceDb;

pub async fn import_tracks(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
    maps: &[LibraryPathMap],
    media_validation: Option<&CanonicalRootValidator>,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "tracks").await?;
    let mut skip_note_recorded = media_validation.is_some();
    loop {
        // Ordinal is deterministic across resumes and batch sizes: the
        // window function must number the whole album, so it is computed in
        // a CTE scoped to the albums this keyset page touches — computing it
        // over the page itself would renumber when a batch splits an album
        // (UNIQUE (release_id, ordinal) collision on resume).
        let rows = sqlx::query(
            r#"WITH batch AS (
                 SELECT s."Id", s."AlbumId" FROM "Songs" s
                 WHERE s."Id" > $1 ORDER BY s."Id" LIMIT $2
               ),
               numbered AS (
                 SELECT s."Id",
                        ROW_NUMBER() OVER (
                          PARTITION BY s."AlbumId"
                          ORDER BY s."SongNumber", s."Id"
                        ) AS ordinal
                 FROM "Songs" s
                 WHERE s."AlbumId" IN (SELECT "AlbumId" FROM batch)
               )
               SELECT s."Id", s."ApiKey", s."Title", s."TitleNormalized",
                      s."TitleSort", s."SongNumber", s."Duration", s."FileName",
                      s."FileSize", s."FileHash", s."ContentType", s."BitRate",
                      s."SamplingRate", s."ChannelCount", s."AlbumId",
                      al."Directory" AS album_directory,
                      ar."Directory" AS artist_directory,
                      ar."Name" AS artist_name,
                      s."CreatedAt", s."LastUpdatedAt",
                      n.ordinal
               FROM batch
               JOIN "Songs" s ON s."Id" = batch."Id"
               JOIN numbered n ON n."Id" = s."Id"
               LEFT JOIN "Albums" al ON al."Id" = s."AlbumId"
               LEFT JOIN "Artists" ar ON ar."Id" = al."ArtistId"
               ORDER BY s."Id""#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("tracks source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows.last().map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        // The track/media-asset circular FK pair is validated at commit.
        sqlx::query("SET CONSTRAINTS tracks_exactly_one_playable_asset_fk DEFERRED")
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("cannot defer playable FK: {e}"))?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during tracks import"));
        }
        if !skip_note_recorded && !rows.is_empty() {
            // ADR-0148 §D.3 audit: without --canonical-root the adoption is
            // metadata-only; record that once per run so the issue trail
            // shows existence/size were not verified.
            skip_note_recorded = true;
            let already: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM legacy_migration.issues
                  WHERE run_id = $1 AND code = 'media.verification_skipped')",
            )
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| format!("skip-note check failed: {e}"))?;
            if !already {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "tracks",
                    None,
                    "info",
                    "media.verification_skipped",
                    "no --canonical-root supplied; media existence and size were not verified",
                    None,
                )
                .await?;
            }
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::catalog_mapping(&mut tx, run_id, "track", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let album_pk = i64::from(row.get::<i32, _>("AlbumId"));
            let release_target =
                match map::catalog_mapping(&mut tx, run_id, "release", album_pk).await? {
                    Some(id) => id,
                    None => {
                        runs::record_issue(
                            &mut tx,
                            run_id,
                            "tracks",
                            Some(source_pk),
                            "error",
                            "catalog.broken_song_album_fk",
                            "song references an album with no target mapping; track skipped",
                            None,
                        )
                        .await?;
                        issues += 1;
                        continue;
                    }
                };
            let file_name: Option<String> = row.get("FileName");
            let artist_directory: Option<String> = row.get("artist_directory");
            let album_directory: Option<String> = row.get("album_directory");
            let file_size: Option<i64> = row.get("FileSize");
            let Some(relative_path) = map::resolve_canonical_path(
                artist_directory.as_deref(),
                album_directory.as_deref(),
                file_name.as_deref(),
                maps,
            ) else {
                runs::record_issue(
                    &mut tx, run_id, "tracks", Some(source_pk), "error",
                    "media.unadoptable",
                    "song has no adoptable canonical path (missing file metadata or unsafe components)",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };

            // ADR-0148 §D.3: when the canonical root is mounted, validate the
            // adopted file read-only. An escape is unsafe — the track is
            // skipped; missing/mismatch keep catalog identity with a warning.
            if let Some(validator) = media_validation {
                match validator.check(&relative_path, file_size) {
                    MediaCheck::Ok => {}
                    MediaCheck::Missing => {
                        runs::record_issue(
                            &mut tx,
                            run_id,
                            "tracks",
                            Some(source_pk),
                            "warning",
                            "media.missing_file",
                            "adopted canonical file is not present under the supplied root",
                            Some(serde_json::json!({ "path": relative_path })),
                        )
                        .await?;
                        issues += 1;
                    }
                    MediaCheck::SizeMismatch => {
                        runs::record_issue(
                            &mut tx,
                            run_id,
                            "tracks",
                            Some(source_pk),
                            "warning",
                            "media.size_mismatch",
                            "adopted canonical file size differs from legacy FileSize",
                            Some(serde_json::json!({ "path": relative_path })),
                        )
                        .await?;
                        issues += 1;
                    }
                    MediaCheck::Escape => {
                        runs::record_issue(
                            &mut tx,
                            run_id,
                            "tracks",
                            Some(source_pk),
                            "error",
                            "media.unadoptable",
                            "adopted path resolves outside the canonical root through a symlink",
                            Some(serde_json::json!({ "path": relative_path })),
                        )
                        .await?;
                        issues += 1;
                        continue;
                    }
                }
            }

            let public_id: Uuid = row.get("ApiKey");
            let title: String = row.get("Title");
            let normalized = map::normalized_or(row.get("TitleNormalized"), &title);
            let sort = row
                .get::<Option<String>, _>("TitleSort")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| title.clone());
            let track_number = Some(row.get::<i32, _>("SongNumber"));
            let ordinal: i64 = row.get("ordinal");
            let duration_secs: Option<f64> = row.get("Duration");
            let duration_ms = duration_secs
                .filter(|d| *d > 0.0)
                .map(|d| (d * 1000.0) as i64);
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));

            let byte_hash = map::adopt_or_sentinel_hash(row.get("FileHash"));
            let media_id_new = Uuid::now_v7();
            let mime = row
                .get::<Option<String>, _>("ContentType")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let asset_written = sqlx::query(
                "INSERT INTO media_assets (
                    id, canonical_relative_path, byte_hash, byte_size, mime_type,
                    sample_rate_hz, channels, bitrate_bps, duration_ms, created_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                 ON CONFLICT (canonical_relative_path) DO NOTHING",
            )
            .bind(media_id_new)
            .bind(&relative_path)
            .bind(&byte_hash)
            .bind(file_size.unwrap_or(0))
            .bind(&mime)
            .bind(row.get::<Option<i32>, _>("SamplingRate"))
            .bind(row.get::<Option<i32>, _>("ChannelCount"))
            .bind(row.get::<Option<i32>, _>("BitRate").map(i64::from))
            .bind(duration_ms)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("media asset insert failed for song {source_pk}: {e}"))?
            .rows_affected();
            let media_id = if asset_written == 0 {
                // Shared media (multiple songs, one file): adopt the existing
                // asset row for this canonical path.
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM media_assets WHERE canonical_relative_path = $1",
                )
                .bind(&relative_path)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| format!("media asset lookup failed: {e}"))?
            } else {
                media_id_new
            };

            let audio_identity_id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO audio_identities (
                    id, status, revision, created_at, updated_at
                 ) VALUES ($1,'verified',1,$2,$2)",
            )
            .bind(audio_identity_id)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("audio identity insert failed: {e}"))?;

            let track_id = Uuid::now_v7();
            // Insertion order is fixed by the schema: media_assets and
            // audio_identities first, then the track (its deferred circular
            // FK validates at commit), then the track_media_assets link —
            // whose own FK to tracks is not deferrable.
            let written = sqlx::query(
                "INSERT INTO tracks (
                    id, public_api_id, release_id, audio_identity_id,
                    playable_media_asset_id, ordinal, title, normalized_title,
                    sort_title, display_artist, disc_number, track_number,
                    duration_ms, source_path, revision, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,NULL,$11,$12,$13,1,$14,$15)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(track_id)
            .bind(public_id)
            .bind(release_target)
            .bind(audio_identity_id)
            .bind(media_id)
            .bind(i32::try_from(ordinal).unwrap_or(i32::MAX))
            .bind(&title)
            .bind(&normalized)
            .bind(&sort)
            .bind(row.get::<Option<String>, _>("artist_name"))
            .bind(track_number)
            .bind(duration_ms)
            .bind(&relative_path)
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("track {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "tracks",
                    Some(source_pk),
                    "error",
                    "target.public_id_conflict",
                    "public ApiKey already exists in the target outside this run",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            sqlx::query(
                "INSERT INTO track_media_assets (
                    track_id, media_asset_id, purpose, ordinal, created_at
                 ) VALUES ($1,$2,'playable',1,$3)",
            )
            .bind(track_id)
            .bind(media_id)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("track media link failed: {e}"))?;
            map::insert_catalog_mapping(&mut tx, run_id, "track", source_pk, track_id, public_id)
                .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx, run_id, "tracks", last_pk, count, inserted, 0, issues, 0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

//! Track and canonical media adoption importer (ADR-0148 §D.3). Legacy Song
//! rows become Rust Track + AudioIdentity + MediaAsset + playable link in one
//! fenced transaction per batch. Media is adopted in place: the stored path
//! is canonical-relative, hashes are adopted only when already valid, and the
//! hash-pending sentinel is recorded otherwise.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
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
                      s."SamplingRate", s."ChannelCount", s."BPM", s."AlbumId",
                      COALESCE(cardinality(s."Genres"), 0) AS genre_count,
                      COALESCE(cardinality(s."Moods"), 0) AS mood_count,
                      s."Comment", s."Lyrics", s."PartTitles", s."Tags", s."Notes",
                      s."Description", s."AlternateNames", s."BitDepth", s."IsVbr",
                      s."PlayedCount", s."LastPlayedAt", s."LastMetaDataUpdatedAt",
                      al."Directory" AS album_directory,
                      ar."Id" AS artist_id,
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
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
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
        let track_keys: Vec<i64> = rows
            .iter()
            .map(|row| i64::from(row.get::<i32, _>("Id")))
            .collect();
        let release_keys: Vec<i64> = rows
            .iter()
            .map(|row| i64::from(row.get::<i32, _>("AlbumId")))
            .collect();
        let artist_keys: Vec<i64> = rows
            .iter()
            .filter_map(|row| row.get::<Option<i32>, _>("artist_id").map(i64::from))
            .collect();
        let existing_tracks = map::catalog_mappings(&mut tx, run_id, "track", &track_keys).await?;
        let release_targets =
            map::catalog_mappings(&mut tx, run_id, "release", &release_keys).await?;
        let artist_targets = map::catalog_mappings(&mut tx, run_id, "artist", &artist_keys).await?;
        let mut pending = Vec::with_capacity(rows.len());
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if existing_tracks.contains_key(&source_pk) {
                continue;
            }
            let album_pk = i64::from(row.get::<i32, _>("AlbumId"));
            let release_target = match release_targets.get(&album_pk).copied() {
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
                    MediaCheck::Unsafe => {
                        runs::record_issue(
                            &mut tx,
                            run_id,
                            "tracks",
                            Some(source_pk),
                            "error",
                            "media.unadoptable",
                            "adopted path is unsafe or resolves outside the canonical root",
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
            // Frozen legacy data stores the tag reader's seconds value in the
            // millisecond-declared Songs.Duration field; normalize to target
            // milliseconds through the shared bounded heuristic.
            let duration_ms = map::legacy_duration_ms(row.get("Duration"));
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));

            let byte_hash = map::adopt_or_sentinel_hash(row.get("FileHash"));
            let media_id_new = Uuid::now_v7();
            let mime = row
                .get::<Option<String>, _>("ContentType")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let container = map::legacy_container(
                Some(mime.as_str()),
                row.get::<Option<String>, _>("FileName").as_deref(),
            );
            // ADR-0148 §D.8 requires every source value without an
            // authoritative Rust representation to remain visible in the
            // migration audit. Do not turn legacy presentation metadata into
            // ReleaseTrait values, fabricate play events, or add schema here;
            // those choices require a separate accepted contract.
            let mut reference_fields = Vec::new();
            if row.get::<i32, _>("genre_count") > 0 {
                reference_fields.push("Songs.Genres");
            }
            if row.get::<i32, _>("mood_count") > 0 {
                reference_fields.push("Songs.Moods");
            }
            for (field, value) in [
                ("Songs.Comment", row.get::<Option<String>, _>("Comment")),
                ("Songs.Lyrics", row.get::<Option<String>, _>("Lyrics")),
                (
                    "Songs.PartTitles",
                    row.get::<Option<String>, _>("PartTitles"),
                ),
                ("Songs.Tags", row.get::<Option<String>, _>("Tags")),
                ("Songs.Notes", row.get::<Option<String>, _>("Notes")),
                (
                    "Songs.Description",
                    row.get::<Option<String>, _>("Description"),
                ),
                (
                    "Songs.AlternateNames",
                    row.get::<Option<String>, _>("AlternateNames"),
                ),
            ] {
                if value.is_some_and(|value| !value.trim().is_empty()) {
                    reference_fields.push(field);
                }
            }
            if row.get::<i32, _>("BitDepth") > 0 {
                reference_fields.push("Songs.BitDepth");
            }
            if row.get::<bool, _>("IsVbr") {
                reference_fields.push("Songs.IsVbr");
            }
            if row.get::<i32, _>("PlayedCount") > 0 {
                reference_fields.push("Songs.PlayedCount");
            }
            if row
                .get::<Option<time::OffsetDateTime>, _>("LastPlayedAt")
                .is_some()
            {
                reference_fields.push("Songs.LastPlayedAt");
            }
            if row
                .get::<Option<time::OffsetDateTime>, _>("LastMetaDataUpdatedAt")
                .is_some()
            {
                reference_fields.push("Songs.LastMetaDataUpdatedAt");
            }
            let artist_id = row
                .get::<Option<i32>, _>("artist_id")
                .and_then(|key| artist_targets.get(&i64::from(key)).copied());
            pending.push(PendingTrack {
                source_pk,
                public_id,
                track_id: Uuid::now_v7(),
                audio_id: Uuid::now_v7(),
                media_id: media_id_new,
                release_id: release_target,
                path: relative_path,
                hash: byte_hash,
                byte_size: file_size.unwrap_or(0),
                mime,
                container,
                sample_rate: row.get("SamplingRate"),
                channels: row.get("ChannelCount"),
                bitrate: map::legacy_bitrate_bps(row.get("BitRate")),
                duration_ms,
                title,
                normalized,
                sort,
                display_artist: row.get("artist_name"),
                artist_id,
                ordinal: i32::try_from(ordinal).unwrap_or(i32::MAX),
                track_number,
                tempo: row
                    .get::<Option<i32>, _>("BPM")
                    .map(|value| f64::from(value.max(0))),
                created,
                updated,
                reference_fields,
            });
        }
        for page in pending.chunks_mut(500) {
            let (written, reported) = insert_track_page(&mut tx, run_id, page).await?;
            inserted += written;
            issues += reported;
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

/// Prepared source rows retain bounded per-page memory and the original public IDs.
struct PendingTrack {
    source_pk: i64,
    public_id: Uuid,
    track_id: Uuid,
    audio_id: Uuid,
    media_id: Uuid,
    release_id: Uuid,
    path: String,
    hash: String,
    byte_size: i64,
    mime: String,
    container: Option<String>,
    sample_rate: Option<i32>,
    channels: Option<i32>,
    bitrate: Option<i64>,
    duration_ms: Option<i64>,
    title: String,
    normalized: String,
    sort: String,
    display_artist: Option<String>,
    artist_id: Option<Uuid>,
    ordinal: i32,
    track_number: Option<i32>,
    tempo: Option<f64>,
    created: time::OffsetDateTime,
    updated: time::OffsetDateTime,
    reference_fields: Vec<&'static str>,
}

async fn insert_track_page(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    page: &mut [PendingTrack],
) -> Result<(i64, i64), String> {
    let mut media = QueryBuilder::<Postgres>::new(
        "INSERT INTO media_assets (id,canonical_relative_path,byte_hash,byte_size,mime_type,container,sample_rate_hz,channels,bitrate_bps,duration_ms,created_at) ",
    );
    media.push_values(page.iter(), |mut values, row| {
        values
            .push_bind(row.media_id)
            .push_bind(&row.path)
            .push_bind(&row.hash)
            .push_bind(row.byte_size)
            .push_bind(&row.mime)
            .push_bind(&row.container)
            .push_bind(row.sample_rate)
            .push_bind(row.channels)
            .push_bind(row.bitrate)
            .push_bind(row.duration_ms)
            .push_bind(row.created);
    });
    media.push(" ON CONFLICT (canonical_relative_path) DO NOTHING");
    media
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|e| format!("media asset batch failed: {e}"))?;
    let paths: Vec<&str> = page.iter().map(|row| row.path.as_str()).collect();
    let assets: BTreeMap<String, Uuid> = sqlx::query_as::<_, (String, Uuid)>(
        "SELECT canonical_relative_path,id FROM media_assets WHERE canonical_relative_path=ANY($1)",
    )
    .bind(paths)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| format!("media asset batch lookup failed: {e}"))?
    .into_iter()
    .collect();
    for row in page.iter_mut() {
        row.media_id = *assets
            .get(&row.path)
            .ok_or_else(|| "adopted asset missing in transaction".to_string())?;
    }
    let mut identities = QueryBuilder::<Postgres>::new(
        "INSERT INTO audio_identities (id,status,revision,created_at,updated_at) ",
    );
    identities.push_values(page.iter(), |mut values, row| {
        values
            .push_bind(row.audio_id)
            .push_bind("verified")
            .push_bind(1_i64)
            .push_bind(row.created)
            .push_bind(row.created);
    });
    identities
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|e| format!("audio identity batch failed: {e}"))?;
    // The deferred playable FK and nondeferred link FK require this ordering.
    let mut tracks = QueryBuilder::<Postgres>::new(
        "INSERT INTO tracks (id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,ordinal,title,normalized_title,sort_title,display_artist,track_number,duration_ms,source_path,revision,created_at,updated_at) ",
    );
    tracks.push_values(page.iter(), |mut values, row| {
        values
            .push_bind(row.track_id)
            .push_bind(row.public_id)
            .push_bind(row.release_id)
            .push_bind(row.audio_id)
            .push_bind(row.media_id)
            .push_bind(row.ordinal)
            .push_bind(&row.title)
            .push_bind(&row.normalized)
            .push_bind(&row.sort)
            .push_bind(&row.display_artist)
            .push_bind(row.track_number)
            .push_bind(row.duration_ms)
            .push_bind(&row.path)
            .push_bind(1_i64)
            .push_bind(row.created)
            .push_bind(row.updated);
    });
    tracks.push(" ON CONFLICT (public_api_id) DO NOTHING RETURNING id");
    let written: BTreeSet<Uuid> = tracks
        .build_query_scalar()
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| format!("track batch failed: {e}"))?
        .into_iter()
        .collect();
    let mut issues = Vec::new();
    let mut unused_audio = Vec::new();
    for row in page.iter() {
        if !written.contains(&row.track_id) {
            unused_audio.push(row.audio_id);
            issues.push((
                row.source_pk,
                "error",
                "target.public_id_conflict",
                "public ApiKey already exists in the target outside this run",
                None,
            ));
        } else {
            if !row.reference_fields.is_empty() {
                issues.push((row.source_pk,"warning","metadata.reference_only","legacy track metadata has no authoritative Rust representation; source values remain reference-only",Some(serde_json::json!({"fields":row.reference_fields}))));
            }
            if row.artist_id.is_none() {
                issues.push((row.source_pk,"error","catalog.broken_song_artist_fk","song album artist has no target mapping; track remains without a billed Artist link",None));
            }
        }
    }
    if !unused_audio.is_empty() {
        sqlx::query("DELETE FROM audio_identities WHERE id=ANY($1)")
            .bind(unused_audio)
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("unused audio identity cleanup failed: {e}"))?;
    }
    let imported: Vec<&PendingTrack> = page
        .iter()
        .filter(|row| written.contains(&row.track_id))
        .collect();
    if !imported.is_empty() {
        let mut links = QueryBuilder::<Postgres>::new(
            "INSERT INTO track_media_assets (track_id,media_asset_id,purpose,ordinal,created_at) ",
        );
        links.push_values(imported.iter(), |mut values, row| {
            values
                .push_bind(row.track_id)
                .push_bind(row.media_id)
                .push_bind("playable")
                .push_bind(1_i32)
                .push_bind(row.created);
        });
        links
            .build()
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("track media batch failed: {e}"))?;
        let features: Vec<_> = imported.iter().filter(|row| row.tempo.is_some()).collect();
        if !features.is_empty() {
            let mut query = QueryBuilder::<Postgres>::new(
                "INSERT INTO native_track_audio_features (track_id,tempo,updated_at) ",
            );
            query.push_values(features, |mut values, row| {
                values
                    .push_bind(row.track_id)
                    .push_bind(row.tempo)
                    .push_bind(row.updated);
            });
            query
                .build()
                .execute(&mut **tx)
                .await
                .map_err(|e| format!("track features batch failed: {e}"))?;
        }
        let artists: Vec<_> = imported
            .iter()
            .filter(|row| row.artist_id.is_some())
            .collect();
        if !artists.is_empty() {
            let mut query = QueryBuilder::<Postgres>::new(
                "INSERT INTO track_artists (id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) ",
            );
            query.push_values(artists, |mut values, row| {
                values
                    .push_bind(Uuid::now_v7())
                    .push_bind(row.track_id)
                    .push_bind(row.artist_id)
                    .push_bind("primary")
                    .push_bind(1_i32)
                    .push_bind(row.display_artist.as_ref().unwrap_or(&row.title))
                    .push_bind("")
                    .push_bind(row.created);
            });
            query
                .build()
                .execute(&mut **tx)
                .await
                .map_err(|e| format!("track artist batch failed: {e}"))?;
        }
        let mut mappings = QueryBuilder::<Postgres>::new(
            "INSERT INTO legacy_catalog_id_mappings (id,migration_run_id,entity_type,legacy_numeric_id,internal_id,public_api_id,source_snapshot,imported_at) ",
        );
        mappings.push_values(imported, |mut values, row| {
            values
                .push_bind(Uuid::now_v7())
                .push_bind(run_id)
                .push_bind("track")
                .push_bind(row.source_pk)
                .push_bind(row.track_id)
                .push_bind(row.public_id)
                .push_bind(super::source::FROZEN_SCHEMA_FINAL_MIGRATION)
                .push_bind(time::OffsetDateTime::now_utc());
        });
        mappings
            .build()
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("track mapping batch failed: {e}"))?;
    }
    if !issues.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO legacy_migration.issues (id,run_id,domain,source_pk,severity,code,message,detail,created_at) ",
        );
        query.push_values(
            issues.iter(),
            |mut values, (pk, severity, code, message, detail)| {
                values
                    .push_bind(Uuid::now_v7())
                    .push_bind(run_id)
                    .push_bind("tracks")
                    .push_bind(pk)
                    .push_bind(severity)
                    .push_bind(code)
                    .push_bind(message)
                    .push_bind(detail)
                    .push_bind(time::OffsetDateTime::now_utc());
            },
        );
        query
            .build()
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("track issue batch failed: {e}"))?;
    }
    Ok((
        i64::try_from(written.len()).unwrap_or(0),
        i64::try_from(issues.len()).unwrap_or(0),
    ))
}

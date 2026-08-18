//! Remaining user-state importers: starred/rating state, play history,
//! podcasts, radio, settings (ADR-0148 §D.8).

use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::map;
use super::runs;
use super::source::SourceDb;

async fn user_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    user_pk: i64,
) -> Result<Option<Uuid>, String> {
    map::mapped_target(tx, run_id, "users", user_pk).await
}

pub async fn import_user_states(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    // (domain, source table, target table, entity type for mapping, has play stats)
    // Legacy UserArtists has no PlayedCount/LastPlayedAt columns (frozen EF
    // model); constants keep the row shape uniform without inventing data.
    let tables: &[(&str, &str, &str, &str, &str, bool)] = &[
        (
            "user_track_states",
            "UserSongs",
            "native_track_user_states",
            "track",
            "SongId",
            true,
        ),
        (
            "user_release_states",
            "UserAlbums",
            "native_release_user_states",
            "release",
            "AlbumId",
            true,
        ),
        (
            "user_artist_states",
            "UserArtists",
            "native_artist_user_states",
            "artist",
            "ArtistId",
            false,
        ),
    ];
    for (domain, source_table, target_table, entity, entity_col, has_play_stats) in tables {
        let mut key = runs::checkpoint_key(target, run_id, domain).await?;
        let stats_cols = if *has_play_stats {
            r#""PlayedCount", "LastPlayedAt","#
        } else {
            r#"0::integer AS "PlayedCount", NULL::timestamptz AS "LastPlayedAt","#
        };
        let select = format!(
            r#"SELECT "Id", "UserId", "{entity_col}" AS entity_pk, "IsStarred",
                      "IsHated", "Rating", {stats_cols}
                      "CreatedAt", "LastUpdatedAt"
               FROM "{source_table}" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#
        );
        loop {
            // Table/column names come from the hardcoded state-table
            // allowlist below; only key/batch values are bound parameters.
            let rows = sqlx::query(sqlx::AssertSqlSafe(select.clone()))
                .bind(key)
                .bind(batch_size)
                .fetch_all(source.pool())
                .await
                .map_err(|e| format!("{domain} source read failed: {e}"))?;
            if rows.is_empty() {
                break;
            }
            let last_pk = rows
                .last()
                .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
            let count = i64::try_from(rows.len()).unwrap_or(0);

            let mut tx = target.begin().await.map_err(|e| e.to_string())?;
            if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
                return Err(format!("run {run_id}: lost lease during {domain} import"));
            }
            let mut inserted = 0_i64;
            let mut issues = 0_i64;
            let entity_fk = match *entity {
                "track" => "track_id",
                "release" => "release_id",
                _ => "artist_id",
            };
            let insert = format!(
                "INSERT INTO {target_table} (
                    user_id, {entity_fk}, starred, hated, rating, play_count,
                    last_played_at, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
                 ON CONFLICT (user_id, {entity_fk}) DO NOTHING"
            );
            for row in &rows {
                let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
                if map::mapped_target(&mut tx, run_id, domain, source_pk)
                    .await?
                    .is_some()
                {
                    continue;
                }
                let user_pk = i64::from(row.get::<i32, _>("UserId"));
                let entity_pk = i64::from(row.get::<i32, _>("entity_pk"));
                let user_target = user_target(&mut tx, run_id, user_pk).await?;
                let entity_target =
                    map::catalog_mapping(&mut tx, run_id, entity, entity_pk).await?;
                let (Some(user_target), Some(entity_target)) = (user_target, entity_target) else {
                    runs::record_issue(
                        &mut tx,
                        run_id,
                        domain,
                        Some(source_pk),
                        "error",
                        "ownership.unmapped_reference",
                        "user state references an unmapped user or entity; skipped",
                        None,
                    )
                    .await?;
                    issues += 1;
                    continue;
                };
                let created: time::OffsetDateTime = row.get("CreatedAt");
                let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
                sqlx::query(sqlx::AssertSqlSafe(insert.clone()))
                    .bind(user_target)
                    .bind(entity_target)
                    .bind(row.get::<bool, _>("IsStarred"))
                    .bind(row.get::<bool, _>("IsHated"))
                    .bind(row.get::<i32, _>("Rating").clamp(0, 5) as i16)
                    .bind(i64::from(row.get::<i32, _>("PlayedCount").max(0)))
                    .bind(row.get::<Option<time::OffsetDateTime>, _>("LastPlayedAt"))
                    .bind(created)
                    .bind(updated)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| format!("{domain} row {source_pk} insert failed: {e}"))?;
                map::insert_mapping(
                    &mut tx,
                    run_id,
                    domain,
                    source_pk,
                    None,
                    entity_target,
                    updated,
                )
                .await?;
                inserted += 1;
            }
            runs::advance_checkpoint(
                &mut tx, run_id, domain, last_pk, count, inserted, 0, issues, 0,
            )
            .await?;
            tx.commit().await.map_err(|e| e.to_string())?;
            key = last_pk;
            if count < batch_size {
                break;
            }
        }
    }
    Ok(())
}

pub async fn import_track_plays(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "track_plays").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserId", "SongId", "PlayedAt", "SecondsPlayed",
                      "Client", "IsNowPlaying"
               FROM "UserSongPlayHistories"
               WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("track plays source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during track plays import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "track_plays", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let song_pk = i64::from(row.get::<i32, _>("SongId"));
            let user_target = user_target(&mut tx, run_id, user_pk).await?;
            let track_target = map::catalog_mapping(&mut tx, run_id, "track", song_pk).await?;
            let (Some(user_target), Some(track_target)) = (user_target, track_target) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "track_plays",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "play history references an unmapped user or track; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let played_at: time::OffsetDateTime = row.get("PlayedAt");
            // client_event_id is the idempotency anchor in the target schema;
            // the legacy row's source identity makes replays convergent.
            let client_event_id = format!("legacy-play-{source_pk}");
            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO native_track_plays (
                    id, user_id, track_id, client_event_id, played_at,
                    submitted_at, duration_seconds, completed
                 ) VALUES ($1,$2,$3,$4,$5,$5,$6,true)
                 ON CONFLICT (user_id, client_event_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(user_target)
            .bind(track_target)
            .bind(&client_event_id)
            .bind(played_at)
            .bind(row.get::<Option<i32>, _>("SecondsPlayed"))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("track play {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written > 0 {
                map::insert_mapping(
                    &mut tx,
                    run_id,
                    "track_plays",
                    source_pk,
                    None,
                    target_id,
                    played_at,
                )
                .await?;
                inserted += 1;
            }
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "track_plays",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
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

pub async fn import_podcasts(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "podcast_channels").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "UserId", "FeedUrl", "Title", "Description",
                      "ImageUrl", "SiteUrl", "AutoDownloadEnabled",
                      "RefreshIntervalHours", "MaxDownloadedEpisodes",
                      "MaxStorageBytes", "LastSyncAt", "IsDeleted",
                      "CreatedAt", "LastUpdatedAt"
               FROM "PodcastChannels" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("podcast channels source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during podcast import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "podcast_channels", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let Some(owner_target) = user_target(&mut tx, run_id, user_pk).await? else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_channels",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "podcast channel references an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let public_id: Uuid = row.get("ApiKey");
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let feed_url: String = row.get("FeedUrl");
            if feed_url.trim().is_empty() || feed_url.len() > 4096 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_channels",
                    Some(source_pk),
                    "error",
                    "podcast.malformed_feed_url",
                    "feed URL is empty or over 4096 characters; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            let written = sqlx::query(
                "INSERT INTO native_podcast_channels (
                    public_api_id, owner_user_id, feed_url, title, description,
                    image_url, website, auto_download_enabled,
                    refresh_interval_hours, max_downloaded_episodes,
                    max_storage_bytes, last_sync_at, soft_deleted_at,
                    created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
                 ON CONFLICT (owner_user_id, lower(feed_url)) DO NOTHING
                 RETURNING id",
            )
            .bind(public_id)
            .bind(owner_target)
            .bind(&feed_url)
            .bind(row.get::<String, _>("Title"))
            .bind(
                row.get::<Option<String>, _>("Description")
                    .unwrap_or_default(),
            )
            .bind(row.get::<Option<String>, _>("ImageUrl").unwrap_or_default())
            .bind(row.get::<Option<String>, _>("SiteUrl").unwrap_or_default())
            .bind(row.get::<bool, _>("AutoDownloadEnabled"))
            .bind(row.get::<Option<i32>, _>("RefreshIntervalHours"))
            .bind(row.get::<Option<i32>, _>("MaxDownloadedEpisodes"))
            .bind(row.get::<Option<i64>, _>("MaxStorageBytes"))
            .bind(row.get::<Option<time::OffsetDateTime>, _>("LastSyncAt"))
            .bind(
                row.get::<bool, _>("IsDeleted")
                    .then_some(time::OffsetDateTime::now_utc()),
            )
            .bind(created)
            .bind(updated)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| format!("podcast channel {source_pk} insert failed: {e}"))?;
            let Some(channel_id) = written.map(|r| r.get::<i64, _>("id")) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_channels",
                    Some(source_pk),
                    "error",
                    "target.feed_conflict",
                    "feed URL already subscribed by this user in the target",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            map::insert_mapping(
                &mut tx,
                run_id,
                "podcast_channels",
                source_pk,
                Some(public_id),
                // entity_map.target_id is uuid; the bigint channel id is
                // recoverable through public_api_id, so map to the public id.
                public_id,
                updated,
            )
            .await?;
            let _ = channel_id;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "podcast_channels",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
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

pub async fn import_radio(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "radio_stations").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "Name", "StreamUrl", "HomePageUrl",
                      "CreatedAt", "LastUpdatedAt"
               FROM "RadioStations" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("radio stations source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during radio import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "radio_stations", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let public_id: Uuid = row.get("ApiKey");
            let name: String = row.get("Name");
            let stream_url: String = row.get("StreamUrl");
            if name.trim().is_empty() || stream_url.trim().is_empty() {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "radio_stations",
                    Some(source_pk),
                    "error",
                    "radio.malformed_station",
                    "radio station has an empty name or stream URL; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO opensubsonic_radio_stations (
                    id, public_api_id, name, stream_url, home_page_url,
                    created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(public_id)
            .bind(&name)
            .bind(&stream_url)
            .bind(row.get::<Option<String>, _>("HomePageUrl"))
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("radio station {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "radio_stations",
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
            map::insert_mapping(
                &mut tx,
                run_id,
                "radio_stations",
                source_pk,
                Some(public_id),
                target_id,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "radio_stations",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
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

/// Settings are reference-only: the Rust runtime owns its configuration. The
/// import records each legacy key as an informational issue so the operator
/// review is complete without inventing a dynamic settings system (ADR-0148).
pub async fn import_settings(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "settings").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "Key", "Category", "CreatedAt"
               FROM "Settings" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("settings source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during settings import"));
        }
        let mut processed = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            let setting_key: String = row.get("Key");
            runs::record_issue(
                &mut tx,
                run_id,
                "settings",
                Some(source_pk),
                "info",
                "settings.reference_only",
                "legacy setting recorded for operator review; Rust configuration is authoritative",
                Some(serde_json::json!({ "key": setting_key })),
            )
            .await?;
            processed += 1;
        }
        runs::advance_checkpoint(
            &mut tx, run_id, "settings", last_pk, count, 0, 0, processed, 0,
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

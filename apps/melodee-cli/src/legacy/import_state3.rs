//! Extended user/operational-state importers (ADR-0148 §D.8 amendment,
//!): user groups and memberships, smart playlists, and podcast episodes
//! with their bookmarks and play history.

use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::map;
use super::runs;
use super::source::SourceDb;

pub async fn user_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    user_pk: i64,
) -> Result<Option<Uuid>, String> {
    map::mapped_target(tx, run_id, "users", user_pk).await
}

/// User groups: legacy `UserGroups`/`UserGroupMembers` become Rust library
/// access groups and memberships. Legacy `LibraryAccessControls` bind groups
/// to legacy Libraries, which have no Rust entity (media is adopted in
/// place), so those bindings become reference-only issues.
pub async fn import_user_groups(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "user_groups").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "Name", "CreatedAt", "LastUpdatedAt"
               FROM "UserGroups" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("user groups source read failed: {e}"))?;
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
                "run {run_id}: lost lease during user groups import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "user_groups", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let name: Option<String> = row.get("Name");
            let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "user_groups",
                    Some(source_pk),
                    "error",
                    "group.empty_name",
                    "user group has an empty name; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let public_id: Uuid = row.get("ApiKey");
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO native_library_access_groups (
                    id, public_api_id, name, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(public_id)
            .bind(&name)
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("user group {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "user_groups",
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
                "user_groups",
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
            "user_groups",
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

    let mut key = runs::checkpoint_key(target, run_id, "user_group_members").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserGroupId", "UserId", "CreatedAt"
               FROM "UserGroupMembers" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("user group members source read failed: {e}"))?;
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
                "run {run_id}: lost lease during user group members import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "user_group_members", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let group_pk = i64::from(row.get::<i32, _>("UserGroupId"));
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let group_target = map::mapped_target(&mut tx, run_id, "user_groups", group_pk).await?;
            let user_target = user_target(&mut tx, run_id, user_pk).await?;
            let (Some(group_target), Some(user_target)) = (group_target, user_target) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "user_group_members",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "group membership references an unmapped group or user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let created: OffsetDateTime = row.get("CreatedAt");
            sqlx::query(
                "INSERT INTO native_user_library_groups (user_id, group_id, created_at)
                 VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
            )
            .bind(user_target)
            .bind(group_target)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("group membership {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "user_group_members",
                source_pk,
                None,
                group_target,
                created,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "user_group_members",
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

/// Smart playlists: the legacy entity vocabulary (`Song`/`Album`/`Artist`)
/// maps to the target `songs`/`albums`/`artists`; anything else is an issue,
/// never a guess.
pub async fn import_smart_playlists(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "smart_playlists").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "Name", "MqlQuery", "NormalizedQuery",
                      "EntityType", "IsPublic", "LastResultCount",
                      "LastEvaluatedAt", "UserId", "CreatedAt", "LastUpdatedAt"
               FROM "SmartPlaylists" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("smart playlists source read failed: {e}"))?;
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
                "run {run_id}: lost lease during smart playlists import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "smart_playlists", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let owner_pk = i64::from(row.get::<i32, _>("UserId"));
            let Some(owner_target) = user_target(&mut tx, run_id, owner_pk).await? else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "smart_playlists",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "smart playlist references an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let query: Option<String> = row.get("MqlQuery");
            let Some(query) = query.filter(|q| !q.trim().is_empty() && q.len() <= 4000) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "smart_playlists",
                    Some(source_pk),
                    "error",
                    "smart_playlist.empty_query",
                    "smart playlist query is empty or over 4000 characters; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let entity_raw: Option<String> = row.get("EntityType");
            let entity =
                match entity_raw
                    .as_deref()
                    .map(str::trim)
                    .map(str::to_lowercase)
                    .as_deref()
                {
                    Some("song" | "songs") => "songs",
                    Some("album" | "albums") => "albums",
                    Some("artist" | "artists") => "artists",
                    other => {
                        runs::record_issue(
                        &mut tx, run_id, "smart_playlists", Some(source_pk), "error",
                        "smart_playlist.unknown_entity",
                        "smart playlist entity type has no songs/albums/artists mapping; skipped",
                        Some(serde_json::json!({ "entity_type": other })),
                    )
                    .await?;
                        issues += 1;
                        continue;
                    }
                };
            let public_id: Uuid = row.get("ApiKey");
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO native_smart_playlists (
                    id, public_api_id, owner_user_id, name, mql_query, entity_type,
                    last_result_count, last_evaluated_at, is_public, normalized_query,
                    revision, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,1,$11,$12)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(public_id)
            .bind(owner_target)
            .bind(
                row.get::<Option<String>, _>("Name")
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "Migrated smart playlist".to_string()),
            )
            .bind(&query)
            .bind(entity)
            .bind(row.get::<i32, _>("LastResultCount").max(0))
            .bind(row.get::<Option<OffsetDateTime>, _>("LastEvaluatedAt"))
            .bind(row.get::<bool, _>("IsPublic"))
            .bind(
                row.get::<Option<String>, _>("NormalizedQuery")
                    .filter(|q| q.len() <= 4000),
            )
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("smart playlist {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "smart_playlists",
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
                "smart_playlists",
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
            "smart_playlists",
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

/// Podcast episodes plus their per-user bookmarks and play history. A legacy
/// `Downloading` row has no lease in the target contract, so it resumes as
/// `Queued` with an informational issue. A legacy `Downloaded` row whose
/// local path is absolute (host-specific) is requeued as `NotDownloaded`
/// rather than pointing at a file the Rust storage root cannot see.
pub async fn import_podcast_episodes(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "podcast_episodes").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "PodcastChannelId", "Guid", "EpisodeKey",
                      "Title", "Description", "PublishDate", "EnclosureUrl",
                      "EnclosureLength", "LocalPath", "LocalFileSize",
                      "DownloadStatus", "DownloadError", "Duration",
                      "CreatedAt", "LastUpdatedAt"
               FROM "PodcastEpisodes" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("podcast episodes source read failed: {e}"))?;
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
                "run {run_id}: lost lease during podcast episodes import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "podcast_episodes", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let channel_pk = i64::from(row.get::<i32, _>("PodcastChannelId"));
            let channel_public =
                map::mapped_target(&mut tx, run_id, "podcast_channels", channel_pk).await?;
            let Some(channel_public) = channel_public else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_episodes",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "episode references an unmapped channel; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            // The channel mapping target is the channel public id (the target
            // PK is bigserial); resolve the numeric id for the FK.
            let channel_id: i64 = sqlx::query_scalar(
                "SELECT id FROM native_podcast_channels WHERE public_api_id = $1",
            )
            .bind(channel_public)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| format!("channel resolve failed: {e}"))?;

            let feed_item_id = row
                .get::<Option<String>, _>("Guid")
                .or_else(|| row.get::<Option<String>, _>("EpisodeKey"))
                .filter(|v| !v.trim().is_empty() && v.len() <= 4096);
            let Some(feed_item_id) = feed_item_id else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_episodes",
                    Some(source_pk),
                    "error",
                    "podcast.episode_missing_key",
                    "episode has neither Guid nor EpisodeKey; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let status_raw = row.get::<i32, _>("DownloadStatus");
            let local_path: Option<String> = row.get("LocalPath");
            let mut status_note: Option<(&str, &str)> = None;
            let (status, adopt_path) = match status_raw {
                0 => ("NotDownloaded", false),
                1 => ("Queued", false),
                2 => {
                    status_note = Some((
                        "podcast.episode_downloading_reset",
                        "legacy episode was mid-download; requeued as Queued",
                    ));
                    ("Queued", false)
                }
                3 => match local_path.as_deref() {
                    Some(path) if !path.starts_with('/') && !path.trim().is_empty() => {
                        ("Downloaded", true)
                    }
                    _ => {
                        status_note = Some((
                            "podcast.episode_local_path_rebased",
                            "legacy downloaded episode path is host-specific; episode requeues as NotDownloaded",
                        ));
                        ("NotDownloaded", false)
                    }
                },
                4 => ("Failed", false),
                other => {
                    runs::record_issue(
                        &mut tx,
                        run_id,
                        "podcast_episodes",
                        Some(source_pk),
                        "error",
                        "podcast.episode_unknown_status",
                        "episode download status has no target mapping; skipped",
                        Some(serde_json::json!({ "download_status": other })),
                    )
                    .await?;
                    issues += 1;
                    continue;
                }
            };
            let duration_ms = row
                .get::<Option<sqlx::postgres::types::PgInterval>, _>("Duration")
                .map(|i| i.microseconds / 1000)
                .filter(|ms| *ms >= 0);
            let public_id: Uuid = row.get("ApiKey");
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let written: Option<Uuid> = sqlx::query_scalar(
                "INSERT INTO native_podcast_episodes (
                    public_api_id, channel_id, title, description, published_at,
                    feed_item_id, duration_ms, enclosure_url, local_path,
                    download_status, download_error, downloaded_bytes,
                    downloaded_at, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
                 ON CONFLICT (channel_id, feed_item_id) DO NOTHING
                 RETURNING public_api_id",
            )
            .bind(public_id)
            .bind(channel_id)
            .bind(
                row.get::<Option<String>, _>("Title")
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| feed_item_id.clone()),
            )
            .bind(
                row.get::<Option<String>, _>("Description")
                    .unwrap_or_default(),
            )
            .bind(row.get::<Option<OffsetDateTime>, _>("PublishDate"))
            .bind(&feed_item_id)
            .bind(duration_ms)
            .bind(
                row.get::<Option<String>, _>("EnclosureUrl")
                    .filter(|u| u.len() <= 4096),
            )
            .bind(if adopt_path {
                local_path.as_deref()
            } else {
                None
            })
            .bind(status)
            .bind(row.get::<Option<String>, _>("DownloadError"))
            .bind(if adopt_path {
                row.get::<Option<i64>, _>("LocalFileSize")
            } else {
                None
            })
            .bind(if adopt_path {
                Some(created)
            } else {
                None::<OffsetDateTime>
            })
            .bind(created)
            .bind(updated)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| format!("podcast episode {source_pk} insert failed: {e}"))?;
            let Some(episode_public) = written else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_episodes",
                    Some(source_pk),
                    "error",
                    "target.feed_item_conflict",
                    "channel already has this feed item in the target; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            if let Some((code, message)) = status_note {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_episodes",
                    Some(source_pk),
                    "info",
                    code,
                    message,
                    None,
                )
                .await?;
            }
            map::insert_mapping(
                &mut tx,
                run_id,
                "podcast_episodes",
                source_pk,
                Some(public_id),
                episode_public,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "podcast_episodes",
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

    import_podcast_episode_bookmarks(source, target, run_id, owner, batch_size).await?;
    import_podcast_play_history(source, target, run_id, owner, batch_size).await?;
    Ok(())
}

async fn resolve_episode_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    episode_pk: i64,
) -> Result<Option<i64>, String> {
    let Some(public) = map::mapped_target(tx, run_id, "podcast_episodes", episode_pk).await? else {
        return Ok(None);
    };
    sqlx::query_scalar::<_, i64>("SELECT id FROM native_podcast_episodes WHERE public_api_id = $1")
        .bind(public)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| format!("episode resolve failed: {e}"))
}

async fn import_podcast_episode_bookmarks(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "podcast_episode_bookmarks").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserId", "PodcastEpisodeId", "PositionSeconds",
                      "Comment", "CreatedAt", "UpdatedAt"
               FROM "PodcastEpisodeBookmarks" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("podcast bookmarks source read failed: {e}"))?;
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
                "run {run_id}: lost lease during podcast bookmarks import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "podcast_episode_bookmarks", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let episode_pk = i64::from(row.get::<i32, _>("PodcastEpisodeId"));
            let user_target = user_target(&mut tx, run_id, user_pk).await?;
            let episode_id = resolve_episode_id(&mut tx, run_id, episode_pk).await?;
            let (Some(user_target), Some(episode_id)) = (user_target, episode_id) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_episode_bookmarks",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "episode bookmark references an unmapped user or episode; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated =
                map::clamp_updated(created, row.get::<Option<OffsetDateTime>, _>("UpdatedAt"));
            sqlx::query(
                "INSERT INTO native_podcast_bookmarks (
                    user_id, episode_id, position_seconds, comment, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6)
                 ON CONFLICT (user_id, episode_id) DO NOTHING",
            )
            .bind(user_target)
            .bind(episode_id)
            .bind(row.get::<i32, _>("PositionSeconds").max(0))
            .bind(
                row.get::<Option<String>, _>("Comment")
                    .filter(|c| c.len() <= 10000),
            )
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("podcast bookmark {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "podcast_episode_bookmarks",
                source_pk,
                None,
                user_target,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "podcast_episode_bookmarks",
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

async fn import_podcast_play_history(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "podcast_play_history").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserId", "PodcastEpisodeId", "PlayedAt",
                      "SecondsPlayed", "Client", "IsNowPlaying", "Source"
               FROM "UserPodcastEpisodePlayHistories"
               WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("podcast play history source read failed: {e}"))?;
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
                "run {run_id}: lost lease during podcast play history import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "podcast_play_history", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let episode_pk = i64::from(row.get::<i32, _>("PodcastEpisodeId"));
            let user_target = user_target(&mut tx, run_id, user_pk).await?;
            let episode_id = resolve_episode_id(&mut tx, run_id, episode_pk).await?;
            let (Some(user_target), Some(episode_id)) = (user_target, episode_id) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "podcast_play_history",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "episode play history references an unmapped user or episode; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let client: Option<String> = row.get("Client");
            let is_now_playing = row.get::<bool, _>("IsNowPlaying");
            // The target enforces one now-playing row per user; stale legacy
            // flags beyond the first are dropped with an informational issue
            // rather than violating the partial unique index.
            let allow_now_playing = if is_now_playing {
                let already: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM native_podcast_play_history
                      WHERE user_id = $1 AND is_now_playing)",
                )
                .bind(user_target)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| format!("now-playing check failed: {e}"))?;
                if already {
                    runs::record_issue(
                        &mut tx,
                        run_id,
                        "podcast_play_history",
                        Some(source_pk),
                        "info",
                        "podcast.now_playing_deduped",
                        "legacy row carried a stale now-playing flag; dropped",
                        None,
                    )
                    .await?;
                }
                !already
            } else {
                false
            };
            sqlx::query(
                "INSERT INTO native_podcast_play_history (
                    user_id, episode_id, played_at, client, seconds_played,
                    source, is_now_playing
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7)",
            )
            .bind(user_target)
            .bind(episode_id)
            .bind(row.get::<OffsetDateTime, _>("PlayedAt"))
            .bind(
                client
                    .filter(|c| !c.trim().is_empty() && c.len() <= 500)
                    .unwrap_or_else(|| "legacy".to_string()),
            )
            .bind(
                row.get::<Option<i32>, _>("SecondsPlayed")
                    .filter(|s| *s >= 0),
            )
            .bind(row.get::<i16, _>("Source"))
            .bind(allow_now_playing)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("podcast play history {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "podcast_play_history",
                source_pk,
                None,
                user_target,
                row.get::<OffsetDateTime, _>("PlayedAt"),
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "podcast_play_history",
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

//! User-state importers: playlists + ordering, shares, bookmarks (ADR-0148).
//! Ownership is resolved through the durable users mapping; legacy PlayQue
//! (jukebox queue) is retired by ADR-0133/0140/0141 and is not imported.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use melodee_persistence::{NativeShareImport, NativeShareStore};

use super::map;
use super::runs;
use super::source::SourceDb;

type ExistingShareRow = (
    Uuid,
    String,
    Uuid,
    i16,
    Uuid,
    Option<String>,
    bool,
    Option<time::OffsetDateTime>,
    time::OffsetDateTime,
    time::OffsetDateTime,
);

async fn user_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Uuid,
    user_pk: i64,
) -> Result<Option<Uuid>, String> {
    map::mapped_target(tx, run_id, "users", user_pk).await
}

pub async fn import_playlists(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "playlists").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "Name", "Description", "IsPublic",
                      "UserId", "CreatedAt", "LastUpdatedAt"
               FROM "Playlists" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("playlists source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during playlists import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "playlists", source_pk)
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
                    "playlists",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "playlist references an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let public_id: Uuid = row.get("ApiKey");
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO native_playlists (
                    id, public_api_id, owner_user_id, name, description,
                    is_public, revision, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,1,$7,$8)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(public_id)
            .bind(owner_target)
            .bind(row.get::<String, _>("Name"))
            .bind(row.get::<Option<String>, _>("Description"))
            .bind(row.get::<bool, _>("IsPublic"))
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("playlist {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "playlists",
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
                "playlists",
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
            "playlists",
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

pub async fn import_playlist_items(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "playlist_items").await?;
    loop {
        // PlaylistSong has no identity PK; keyset paginates on the composite
        // (PlaylistId, PlaylistOrder) via a synthetic ordering key. Legacy
        // PlaylistOrder starts at 1 per playlist, so the pair is unique.
        let rows = sqlx::query(
            r#"SELECT ps."PlaylistId", ps."SongId", ps."PlaylistOrder", ps."SongApiKey",
                      p."UserId" AS owner_user_id
               FROM "PlaylistSong" ps
               JOIN "Playlists" p ON p."Id" = ps."PlaylistId"
               WHERE (ps."PlaylistId", ps."PlaylistOrder") > ($1, $2)
               ORDER BY ps."PlaylistId", ps."PlaylistOrder"
               LIMIT $3"#,
        )
        .bind(i32::try_from(key / 1_000_000).unwrap_or(i32::MAX))
        .bind(i32::try_from(key % 1_000_000).unwrap_or(i32::MAX))
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("playlist items source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last = rows.last();
        let last_pk = last.map_or(key, |r| {
            i64::from(r.get::<i32, _>("PlaylistId")) * 1_000_000
                + i64::from(r.get::<i32, _>("PlaylistOrder"))
        });
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during playlist items import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let playlist_pk = i64::from(row.get::<i32, _>("PlaylistId"));
            let song_pk = i64::from(row.get::<i32, _>("SongId"));
            let position: i32 = row.get("PlaylistOrder");
            let row_key = playlist_pk * 1_000_000 + i64::from(position);
            if map::mapped_target(&mut tx, run_id, "playlist_items", row_key)
                .await?
                .is_some()
            {
                continue;
            }
            let playlist_target =
                map::mapped_target(&mut tx, run_id, "playlists", playlist_pk).await?;
            let track_target = map::catalog_mapping(&mut tx, run_id, "track", song_pk).await?;
            let owner_pk = i64::from(row.get::<i32, _>("owner_user_id"));
            let owner_target = user_target(&mut tx, run_id, owner_pk).await?;
            let (Some(playlist_target), Some(track_target), Some(owner_target)) =
                (playlist_target, track_target, owner_target)
            else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "playlist_items",
                    Some(row_key),
                    "error",
                    "ownership.unmapped_reference",
                    "playlist item references an unmapped playlist, track, or user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let target_id = Uuid::now_v7();
            // PostgreSQL rejects ON CONFLICT on tables whose unique
            // constraint is DEFERRABLE. The (run, domain, source_pk) mapping
            // row checked above is the idempotency anchor, and playlist_id is
            // minted per run, so a plain insert cannot conflict across
            // resumes; a violation here means corrupt source ordering data.
            sqlx::query(
                "INSERT INTO native_playlist_items (
                    id, playlist_id, track_id, position, added_by_user_id, created_at
                 ) VALUES ($1,$2,$3,$4,$5,now())",
            )
            .bind(target_id)
            .bind(playlist_target)
            .bind(track_target)
            .bind(position)
            .bind(owner_target)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("playlist item insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "playlist_items",
                row_key,
                None,
                target_id,
                time::OffsetDateTime::now_utc(),
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "playlist_items",
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

pub async fn import_shares(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "shares").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "ShareId", "ShareUniqueId", "ShareType", "UserId",
                      "Description", "IsDownloadable", "VisitCount", "ExpiresAt",
                      "LastVisitedAt", "CreatedAt", "LastUpdatedAt"
               FROM "Shares" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("shares source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during shares import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "shares", source_pk)
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
                    "shares",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "share references an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let share_unique: Option<String> = row.get("ShareUniqueId");
            let legacy_token = share_unique.as_deref().unwrap_or_default();
            let legacy_token_is_secure = secure_share_token(legacy_token);
            let public_id: Uuid = row.get("ApiKey");
            // Legacy ShareId is the shared entity's numeric PK; resolve its
            // preserved public ApiKey through the catalog mappings (track
            // first, then release) so the share keeps pointing at the same
            // externally visible resource. An unmapped resource is an issue,
            // never a fabricated identity.
            let share_entity_pk = i64::from(row.get::<i32, _>("ShareId"));
            let resource_public =
                match map::catalog_mapping(&mut tx, run_id, "track", share_entity_pk).await? {
                    Some(_) => sqlx::query_scalar::<_, Uuid>(
                        "SELECT public_api_id FROM legacy_catalog_id_mappings
                      WHERE migration_run_id = $1 AND entity_type = 'track'
                        AND legacy_numeric_id = $2",
                    )
                    .bind(run_id)
                    .bind(share_entity_pk)
                    .fetch_one(&mut *tx)
                    .await
                    .map(Some)
                    .map_err(|e| format!("share resource lookup failed: {e}"))?,
                    None => match map::catalog_mapping(&mut tx, run_id, "release", share_entity_pk)
                        .await?
                    {
                        Some(_) => sqlx::query_scalar::<_, Uuid>(
                            "SELECT public_api_id FROM legacy_catalog_id_mappings
                          WHERE migration_run_id = $1 AND entity_type = 'release'
                            AND legacy_numeric_id = $2",
                        )
                        .bind(run_id)
                        .bind(share_entity_pk)
                        .fetch_one(&mut *tx)
                        .await
                        .map(Some)
                        .map_err(|e| format!("share resource lookup failed: {e}"))?,
                        None => None,
                    },
                };
            let Some(resource_public) = resource_public else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "shares",
                    Some(source_pk),
                    "error",
                    "share.unmapped_resource",
                    "share references a song/album with no target mapping; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            // Legacy ShareType (NotSet=0, Song=1, Album=2, Playlist=3,
            // Artist=4) shares the target vocabulary minus NotSet; a NotSet or
            // out-of-range share is unrepresentable and becomes an issue
            // rather than a constraint-violation abort.
            let share_type = row.get::<i32, _>("ShareType");
            if !(1..=4).contains(&share_type) {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "shares",
                    Some(source_pk),
                    "error",
                    "share.malformed_type",
                    "ShareType is NotSet or outside the 1..=4 share vocabulary; skipped",
                    Some(serde_json::json!({ "share_type": share_type })),
                )
                .await?;
                issues += 1;
                continue;
            }
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let description: Option<String> = row.get("Description");
            let is_downloadable = row.get::<bool, _>("IsDownloadable");
            let expires_at = row.get::<Option<time::OffsetDateTime>, _>("ExpiresAt");

            // A later migration run may encounter a share created by an
            // earlier run. Reuse it after checking all source-owned facts;
            // asking NativeShareStore to insert again would correctly report
            // an identity conflict, but the migration adapter needs to add a
            // new run-local mapping to the already authoritative row.
            let existing: Option<ExistingShareRow> = sqlx::query_as(
                "SELECT id, share_unique_id, owner_user_id, share_type,
                        resource_public_api_id, description, is_downloadable,
                        expires_at, created_at, updated_at
                   FROM native_shares WHERE public_api_id = $1",
            )
            .bind(public_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| format!("share {source_pk} existing-row lookup failed: {e}"))?;
            if let Some((
                existing_id,
                active_token,
                existing_owner,
                existing_type,
                existing_resource,
                existing_description,
                existing_downloadable,
                existing_expires,
                existing_created,
                existing_updated,
            )) = existing
            {
                let facts_match = existing_owner == owner_target
                    && existing_type == share_type as i16
                    && existing_resource == resource_public
                    && existing_description == description
                    && existing_downloadable == is_downloadable
                    && existing_expires == expires_at
                    && existing_created == created
                    && existing_updated == updated;
                let token_match = !legacy_token_is_secure || active_token == legacy_token;
                if !facts_match || !token_match {
                    runs::record_issue(
                        &mut tx,
                        run_id,
                        "shares",
                        Some(source_pk),
                        "error",
                        "target.public_id_conflict",
                        "share ApiKey already exists with different source-owned facts",
                        None,
                    )
                    .await?;
                    issues += 1;
                    continue;
                }
                map::insert_mapping(
                    &mut tx,
                    run_id,
                    "shares",
                    source_pk,
                    Some(public_id),
                    existing_id,
                    updated,
                )
                .await?;
                inserted += 1;
                continue;
            }

            let target_id = Uuid::now_v7();
            let source_key = format!("legacy-share:{source_pk}");
            let request = NativeShareImport {
                source_key: &source_key,
                internal_id: target_id,
                public_api_id: public_id,
                legacy_token,
                legacy_token_is_secure,
                owner_user_id: owner_target,
                share_type: share_type as i16,
                resource_public_api_id: resource_public,
                description: description.as_deref(),
                is_downloadable,
                expires_at,
                created_at: created,
                updated_at: updated,
            };
            let result = NativeShareStore::new()
                .import_share_in_transaction(&mut tx, &request)
                .await
                .map_err(|e| format!("share {source_pk} import failed: {e}"))?;
            sqlx::query(
                "UPDATE native_shares
                    SET visit_count = $2, last_visited_at = $3
                  WHERE id = $1",
            )
            .bind(target_id)
            .bind(row.get::<i32, _>("VisitCount"))
            .bind(row.get::<Option<time::OffsetDateTime>, _>("LastVisitedAt"))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("share {source_pk} visit state update failed: {e}"))?;
            if result.rotated_weak_token {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "shares",
                    Some(source_pk),
                    "warning",
                    "share.weak_token_rotated",
                    "legacy public-share token did not meet the secure bearer floor and was rotated",
                    Some(serde_json::json!({ "legacy_token_length": legacy_token.len() })),
                )
                .await?;
                issues += 1;
            }
            map::insert_mapping(
                &mut tx,
                run_id,
                "shares",
                source_pk,
                Some(public_id),
                target_id,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx, run_id, "shares", last_pk, count, inserted, 0, issues, 0,
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

fn secure_share_token(token: &str) -> bool {
    token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub async fn import_bookmarks(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "bookmarks").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserId", "SongId", "Position", "Comment",
                      "CreatedAt", "LastUpdatedAt"
               FROM "Bookmarks" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("bookmarks source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during bookmarks import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "bookmarks", source_pk)
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
                    "bookmarks",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "bookmark references an unmapped user or track; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            sqlx::query(
                "INSERT INTO opensubsonic_bookmarks (
                    user_id, track_id, position, comment, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6)
                 ON CONFLICT (user_id, track_id) DO NOTHING",
            )
            .bind(user_target)
            .bind(track_target)
            .bind(i64::from(row.get::<i32, _>("Position")))
            .bind(row.get::<Option<String>, _>("Comment"))
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("bookmark {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "bookmarks",
                source_pk,
                None,
                // Bookmarks are keyed by (user, track); the mapping target is
                // the track id, and conflict above keeps them idempotent.
                track_target,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "bookmarks",
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

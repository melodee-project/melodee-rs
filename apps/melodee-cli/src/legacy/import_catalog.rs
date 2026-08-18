//! Catalog importers: artists and releases (ADR-0148 §D.2). Track/media
//! adoption lives in `import_tracks.rs`; contributions in `import_contrib.rs`.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::map;
use super::runs;
use super::source::SourceDb;

pub async fn import_artists(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "artists").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "Name", "NameNormalized", "SortName",
                      "CreatedAt", "LastUpdatedAt"
               FROM "Artists" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("artists source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows.last().map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during artists import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::catalog_mapping(&mut tx, run_id, "artist", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let public_id: Uuid = row.get("ApiKey");
            let name: String = row.get("Name");
            let normalized = map::normalized_or(row.get("NameNormalized"), &name);
            let sort = row
                .get::<Option<String>, _>("SortName")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| name.clone());
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));

            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO artists (
                    id, public_api_id, canonical_name, normalized_name, sort_name,
                    artist_type, status, revision, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,'unknown','verified',1,$6,$7)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(public_id)
            .bind(&name)
            .bind(&normalized)
            .bind(&sort)
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("artist {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "artists",
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
            map::insert_catalog_mapping(&mut tx, run_id, "artist", source_pk, target_id, public_id)
                .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx, run_id, "artists", last_pk, count, inserted, 0, issues, 0,
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

pub async fn import_releases(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "releases").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT al."Id", al."ApiKey", al."Name", al."NameNormalized",
                      al."SortName", al."ReleaseDate", al."OriginalReleaseDate",
                      al."ArtistId", ar."Name" AS artist_name,
                      al."CreatedAt", al."LastUpdatedAt"
               FROM "Albums" al
               LEFT JOIN "Artists" ar ON ar."Id" = al."ArtistId"
               WHERE al."Id" > $1 ORDER BY al."Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("releases source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows.last().map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during releases import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::catalog_mapping(&mut tx, run_id, "release", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let legacy_artist_pk = i64::from(row.get::<i32, _>("ArtistId"));
            let artist_target =
                map::catalog_mapping(&mut tx, run_id, "artist", legacy_artist_pk).await?;
            let Some(artist_target) = artist_target else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "releases",
                    Some(source_pk),
                    "error",
                    "catalog.broken_album_artist_fk",
                    "album references an artist with no target mapping; release skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let public_id: Uuid = row.get("ApiKey");
            let title: String = row.get("Name");
            let normalized = map::normalized_or(row.get("NameNormalized"), &title);
            let sort = row
                .get::<Option<String>, _>("SortName")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| title.clone());
            let display_artist: Option<String> = row.get("artist_name");
            let release_date: Option<time::Date> = row.get("ReleaseDate");
            let original_release_date: Option<time::Date> = row.get("OriginalReleaseDate");
            let created: time::OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));

            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO releases (
                    id, public_api_id, title, normalized_title, sort_title,
                    display_artist, release_date, release_date_text,
                    original_release_date, status, revision, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,NULL,$8,'active',1,$9,$10)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(public_id)
            .bind(&title)
            .bind(&normalized)
            .bind(&sort)
            .bind(&display_artist)
            .bind(release_date)
            .bind(original_release_date)
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("release {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "releases",
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
            map::insert_catalog_mapping(
                &mut tx, run_id, "release", source_pk, target_id, public_id,
            )
            .await?;
            let display = display_artist.clone().unwrap_or_else(|| title.clone());
            sqlx::query(
                "INSERT INTO release_artists (
                    id, release_id, artist_id, observed_artist_id, role,
                    presentation_order, display_value, join_phrase, created_at
                 ) VALUES ($1,$2,$3,NULL,'primary',1,$4,'',$5)",
            )
            .bind(Uuid::now_v7())
            .bind(target_id)
            .bind(artist_target)
            .bind(&display)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("release_artist link failed for {source_pk}: {e}"))?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx, run_id, "releases", last_pk, count, inserted, 0, issues, 0,
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

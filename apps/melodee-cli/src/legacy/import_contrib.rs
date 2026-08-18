//! Contributors importer: legacy `Contributors` rows become Rust
//! `Contribution` records scoped to their Release (and Track when present).

use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::map;
use super::runs;
use super::source::SourceDb;

pub async fn import_contributions(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "contributions").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT c."Id", c."ApiKey", c."AlbumId", c."SongId", c."ArtistId",
                      c."ContributorName", c."ContributorType", c."Role",
                      c."SubRole", c."CreatedAt", c."LastUpdatedAt"
               FROM "Contributors" c
               WHERE c."Id" > $1 ORDER BY c."Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("contributors source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows.last().map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during contributions import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "contributions", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let album_pk = i64::from(row.get::<i32, _>("AlbumId"));
            let song_pk = row.get::<Option<i32>, _>("SongId").map(i64::from);
            let artist_pk = row.get::<Option<i32>, _>("ArtistId").map(i64::from);
            let release_target = map::catalog_mapping(&mut tx, run_id, "release", album_pk).await?;
            let artist_target = match artist_pk {
                Some(pk) => map::catalog_mapping(&mut tx, run_id, "artist", pk).await?,
                None => None,
            };
            let (Some(release_target), Some(artist_target)) = (release_target, artist_target)
            else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "contributions",
                    Some(source_pk),
                    "error",
                    "catalog.orphan_contribution",
                    "contributor references an unmapped release or artist; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let track_target = match song_pk {
                Some(pk) => map::catalog_mapping(&mut tx, run_id, "track", pk).await?,
                None => None,
            };
            let role = row
                .get::<Option<String>, _>("Role")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "other".to_string())
                .to_lowercase();
            let role_detail: Option<String> = row.get("SubRole");
            let display_value: Option<String> = row.get("ContributorName");
            let created: time::OffsetDateTime = row.get("CreatedAt");

            let target_id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO contributions (
                    id, release_id, track_id, artist_id, observed_artist_id,
                    role, role_detail, evidence_source, display_value,
                    presentation_order, is_performance, created_at
                 ) VALUES ($1,$2,$3,$4,NULL,$5,$6,'legacy-import',$7,0,false,$8)",
            )
            .bind(target_id)
            .bind(release_target)
            .bind(track_target)
            .bind(artist_target)
            .bind(&role)
            .bind(&role_detail)
            .bind(&display_value)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("contribution {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "contributions",
                source_pk,
                row.get::<Option<Uuid>, _>("ApiKey"),
                target_id,
                map::clamp_updated(created, row.get("LastUpdatedAt")),
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "contributions",
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

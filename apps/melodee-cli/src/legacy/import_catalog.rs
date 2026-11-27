//! Catalog importers: artists and releases (ADR-0148 §D.2). Track/media
//! adoption lives in `import_tracks.rs`; contributions in `import_contrib.rs`.

use std::collections::BTreeSet;

use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
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
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during artists import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        let keys: Vec<i64> = rows
            .iter()
            .map(|row| i64::from(row.get::<i32, _>("Id")))
            .collect();
        let existing = map::catalog_mappings(&mut tx, run_id, "artist", &keys).await?;
        let mut pending = Vec::with_capacity(rows.len());
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if existing.contains_key(&source_pk) {
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

            pending.push(PendingCatalog {
                source_pk,
                public_id,
                id: Uuid::now_v7(),
                name,
                normalized,
                sort,
                created,
                updated,
                display_artist: None,
                artist_id: None,
                release_date: None,
                original_release_date: None,
            });
        }
        // Acquire the whole transaction's name set before inserting any page;
        // otherwise another importer can see a partial same-name group.
        let names = pending
            .iter()
            .map(|row| row.normalized.clone())
            .collect::<Vec<_>>();
        super::identity_name_lock::lock_names(&mut tx, &names).await?;
        for page in pending.chunks(500) {
            let (written, reported) = insert_catalog_page(&mut tx, run_id, "artist", page).await?;
            inserted += written;
            issues += reported;
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
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during releases import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        let keys: Vec<i64> = rows
            .iter()
            .map(|row| i64::from(row.get::<i32, _>("Id")))
            .collect();
        let artist_keys: Vec<i64> = rows
            .iter()
            .map(|row| i64::from(row.get::<i32, _>("ArtistId")))
            .collect();
        let existing = map::catalog_mappings(&mut tx, run_id, "release", &keys).await?;
        let artists = map::catalog_mappings(&mut tx, run_id, "artist", &artist_keys).await?;
        let mut pending = Vec::with_capacity(rows.len());
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if existing.contains_key(&source_pk) {
                continue;
            }
            let legacy_artist_pk = i64::from(row.get::<i32, _>("ArtistId"));
            let artist_target = artists.get(&legacy_artist_pk).copied();
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

            pending.push(PendingCatalog {
                source_pk,
                public_id,
                id: Uuid::now_v7(),
                name: title,
                normalized,
                sort,
                created,
                updated,
                display_artist,
                artist_id: Some(artist_target),
                release_date,
                original_release_date,
            });
        }
        for page in pending.chunks(500) {
            let (written, reported) = insert_catalog_page(&mut tx, run_id, "release", page).await?;
            inserted += written;
            issues += reported;
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

struct PendingCatalog {
    source_pk: i64,
    public_id: Uuid,
    id: Uuid,
    name: String,
    normalized: String,
    sort: String,
    created: time::OffsetDateTime,
    updated: time::OffsetDateTime,
    display_artist: Option<String>,
    artist_id: Option<Uuid>,
    release_date: Option<time::Date>,
    original_release_date: Option<time::Date>,
}

/// Bounded inserts preserve public-ID conflict reporting and atomically anchor
/// the successful rows in the same run's mapping table.
async fn insert_catalog_page(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    kind: &str,
    page: &[PendingCatalog],
) -> Result<(i64, i64), String> {
    let mut query = if kind == "artist" {
        QueryBuilder::<Postgres>::new(
            "INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,revision,created_at,updated_at) ",
        )
    } else {
        QueryBuilder::<Postgres>::new(
            "INSERT INTO releases (id,public_api_id,title,normalized_title,sort_title,display_artist,release_date,original_release_date,status,revision,created_at,updated_at) ",
        )
    };
    query.push_values(page, |mut values, row| {
        values
            .push_bind(row.id)
            .push_bind(row.public_id)
            .push_bind(&row.name)
            .push_bind(&row.normalized)
            .push_bind(&row.sort);
        if kind == "artist" {
            values.push_bind("unknown").push_bind("verified");
        } else {
            values
                .push_bind(&row.display_artist)
                .push_bind(row.release_date)
                .push_bind(row.original_release_date)
                .push_bind("active");
        }
        values
            .push_bind(1_i64)
            .push_bind(row.created)
            .push_bind(row.updated);
    });
    query.push(" ON CONFLICT (public_api_id) DO NOTHING RETURNING id");
    let written: BTreeSet<Uuid> = query
        .build_query_scalar()
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| format!("{kind} batch insert failed: {e}"))?
        .into_iter()
        .collect();
    let imported: Vec<_> = page
        .iter()
        .filter(|row| written.contains(&row.id))
        .collect();
    if !imported.is_empty() {
        let mut mappings = QueryBuilder::<Postgres>::new(
            "INSERT INTO legacy_catalog_id_mappings (id,migration_run_id,entity_type,legacy_numeric_id,internal_id,public_api_id,source_snapshot,imported_at) ",
        );
        mappings.push_values(imported.iter(), |mut values, row| {
            values
                .push_bind(Uuid::now_v7())
                .push_bind(run_id)
                .push_bind(kind)
                .push_bind(row.source_pk)
                .push_bind(row.id)
                .push_bind(row.public_id)
                .push_bind(super::source::FROZEN_SCHEMA_FINAL_MIGRATION)
                .push_bind(time::OffsetDateTime::now_utc());
        });
        mappings
            .build()
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("{kind} mapping batch failed: {e}"))?;
        if kind == "release" {
            let mut links = QueryBuilder::<Postgres>::new(
                "INSERT INTO release_artists (id,release_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) ",
            );
            links.push_values(imported, |mut values, row| {
                values
                    .push_bind(Uuid::now_v7())
                    .push_bind(row.id)
                    .push_bind(row.artist_id)
                    .push_bind("primary")
                    .push_bind(1_i32)
                    .push_bind(row.display_artist.as_ref().unwrap_or(&row.name))
                    .push_bind("")
                    .push_bind(row.created);
            });
            links
                .build()
                .execute(&mut **tx)
                .await
                .map_err(|e| format!("release artist batch failed: {e}"))?;
        }
    }
    let conflicts: Vec<_> = page
        .iter()
        .filter(|row| !written.contains(&row.id))
        .collect();
    if !conflicts.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO legacy_migration.issues (id,run_id,domain,source_pk,severity,code,message,created_at) ",
        );
        query.push_values(conflicts.iter(), |mut values, row| {
            values
                .push_bind(Uuid::now_v7())
                .push_bind(run_id)
                .push_bind(if kind == "artist" {
                    "artists"
                } else {
                    "releases"
                })
                .push_bind(row.source_pk)
                .push_bind("error")
                .push_bind("target.public_id_conflict")
                .push_bind("public ApiKey already exists in the target outside this run")
                .push_bind(time::OffsetDateTime::now_utc());
        });
        query
            .build()
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("{kind} conflicts batch failed: {e}"))?;
    }
    Ok((
        i64::try_from(written.len()).unwrap_or(0),
        i64::try_from(conflicts.len()).unwrap_or(0),
    ))
}

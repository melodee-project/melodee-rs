//! Users importer: identity, portable credentials, roles (ADR-0148 §D.8).

use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::map;
use super::runs;
use super::source::SourceDb;

pub async fn import_users(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "users").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "UserName", "UserNameNormalized", "Email",
                      "EmailNormalized", "PasswordHash", "PasswordHashAlgorithm",
                      "IsAdmin", "IsEditor", "IsLocked",
                      "HasStreamRole", "HasDownloadRole", "HasUploadRole",
                      "HasPlaylistRole", "HasShareRole", "HasCoverArtRole",
                      "HasCommentRole", "HasPodcastRole", "HasSettingsRole",
                      "PreferredLanguage", "PreferredTheme", "CreatedAt",
                      "LastUpdatedAt"
               FROM "Users" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("users source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows.last().map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during users import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "users", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let public_id: Uuid = row.get("ApiKey");
            let username: String = row.get("UserName");
            let normalized = map::normalized_or(row.get("UserNameNormalized"), &username);
            let (email, normalized_email) = match row
                .get::<Option<String>, _>("Email")
                .filter(|v| !v.trim().is_empty())
            {
                Some(email) => {
                    let norm = map::normalized_or(row.get("EmailNormalized"), &email);
                    (email, norm)
                }
                None => {
                    // native_users requires a unique non-empty email; a
                    // deterministic placeholder keeps identity stable across
                    // resumes without inventing a routable address.
                    let placeholder = format!("migrated-user-{source_pk}@import.local");
                    let norm = placeholder.clone();
                    (placeholder, norm)
                }
            };
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));

            let legacy_hash = row.get::<Option<String>, _>("PasswordHash");
            let legacy_algo = row.get::<Option<String>, _>("PasswordHashAlgorithm");
            let portable = legacy_algo.as_deref() == Some("bcrypt")
                && legacy_hash.as_deref().is_some_and(|h| h.starts_with("$2"));
            let password_hash = if portable {
                legacy_hash.clone().unwrap_or_default()
            } else {
                // Unrepresentable credential: substitute a bcrypt hash of an
                // unguessable random token so the account cannot authenticate
                // until the reset flow runs (ADR-0148 §D.8); the issue row is
                // the pre-cutover report.
                let token = Uuid::now_v7().simple().to_string();
                melodee_auth::hash_password(token.as_bytes())
                    .map_err(|e| format!("cannot hash reset placeholder: {e}"))?
            };

            let target_id = Uuid::now_v7();
            let stream_key = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
            let written = sqlx::query(
                "INSERT INTO native_users (
                    id, public_api_id, username, normalized_username, email,
                    normalized_email, password_hash, password_hash_algorithm,
                    stream_signing_key, is_admin, is_editor, enabled, locked,
                    preferred_language, preferred_theme, auth_revision,
                    created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,'bcrypt',$8,$9,$10,true,$11,$12,$13,1,$14,$15)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(public_id)
            .bind(&username)
            .bind(&normalized)
            .bind(&email)
            .bind(&normalized_email)
            .bind(&password_hash)
            .bind(&stream_key)
            .bind(row.get::<bool, _>("IsAdmin"))
            .bind(row.get::<bool, _>("IsEditor"))
            .bind(row.get::<bool, _>("IsLocked"))
            .bind(row.get::<Option<String>, _>("PreferredLanguage"))
            .bind(row.get::<Option<String>, _>("PreferredTheme"))
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("user {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "users",
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
                "users",
                source_pk,
                Some(public_id),
                target_id,
                updated,
            )
            .await?;
            if !portable {
                runs::record_issue(
                    &mut tx, run_id, "users", Some(source_pk), "warning",
                    "credential.unsupported",
                    "legacy credential is not portable bcrypt; user must reset password before cutover",
                    None,
                )
                .await?;
                issues += 1;
            }
            insert_user_roles(&mut tx, target_id, row).await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx, run_id, "users", last_pk, count, inserted, 0, issues, 0,
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

/// Maps legacy `Has*Role` flags to native role rows.
async fn insert_user_roles(
    tx: &mut Transaction<'_, Postgres>,
    target_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> Result<(), String> {
    let flags: &[(&str, bool)] = &[
        ("stream", row.get("HasStreamRole")),
        ("download", row.get("HasDownloadRole")),
        ("upload", row.get("HasUploadRole")),
        ("playlist", row.get("HasPlaylistRole")),
        ("share", row.get("HasShareRole")),
        ("cover_art", row.get("HasCoverArtRole")),
        ("comment", row.get("HasCommentRole")),
        ("podcast", row.get("HasPodcastRole")),
        ("settings", row.get("HasSettingsRole")),
    ];
    for (role, enabled) in flags {
        if *enabled {
            sqlx::query(
                "INSERT INTO native_user_roles (user_id, role, created_at)
                 VALUES ($1, $2, now()) ON CONFLICT DO NOTHING",
            )
            .bind(target_id)
            .bind(role)
            .execute(&mut **tx)
            .await
            .map_err(|e| format!("role insert failed: {e}"))?;
        }
    }
    Ok(())
}

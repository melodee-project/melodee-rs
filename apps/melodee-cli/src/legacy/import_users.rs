//! Users importer: identity, portable credentials, roles (ADR-0148 §D.8).

use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::map;
use super::runs;
use super::source::SourceDb;
use melodee_persistence::{
    MAX_DESCRIPTION_CHARS, MAX_LANGUAGE_CHARS, MAX_THEME_CHARS, MAX_TIME_ZONE_CHARS,
    valid_time_zone_id,
};

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
                      "HasJukeboxRole",
                      "PreferredLanguage", "PreferredTheme", "TimeZoneId", "Description", "CreatedAt",
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
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
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
            // Legacy normalized columns use a different comparer (the
            // frozen fixture removes punctuation). Native auth normalizes by
            // trimming and uppercasing the submitted login, so recompute the
            // target key from the displayed identity rather than preserving
            // an incompatible legacy search key.
            let normalized = username.trim().to_uppercase();
            let (email, normalized_email) = match row
                .get::<Option<String>, _>("Email")
                .filter(|v| !v.trim().is_empty())
            {
                Some(email) => {
                    let norm = email.trim().to_uppercase();
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
            let source_description = row.get::<Option<String>, _>("Description");
            let description = source_description
                .as_deref()
                .filter(|value| value.chars().count() <= MAX_DESCRIPTION_CHARS)
                .map(str::to_owned);
            let source_time_zone = row.get::<String, _>("TimeZoneId");
            let time_zone_id = if source_time_zone.is_empty() {
                "UTC".to_owned()
            } else if source_time_zone.chars().count() <= MAX_TIME_ZONE_CHARS {
                source_time_zone.clone()
            } else {
                "UTC".to_owned()
            };
            let preferred_language = row
                .get::<Option<String>, _>("PreferredLanguage")
                .filter(|value| value.chars().count() <= MAX_LANGUAGE_CHARS);
            let preferred_theme = row
                .get::<Option<String>, _>("PreferredTheme")
                .filter(|value| value.chars().count() <= MAX_THEME_CHARS);

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
                    description, time_zone_id, preferred_language, preferred_theme,
                    auth_revision, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,'bcrypt',$8,$9,$10,true,$11,$12,$13,$14,$15,1,$16,$17)
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
            // The legacy user DTO exposes administrators as effective
            // editors even when the stored IsEditor bit is false.
            .bind(row.get::<bool, _>("IsEditor") || row.get::<bool, _>("IsAdmin"))
            .bind(row.get::<bool, _>("IsLocked"))
            .bind(&description)
            .bind(&time_zone_id)
            .bind(&preferred_language)
            .bind(&preferred_theme)
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
            if source_description.is_some() && description.is_none() {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "users",
                    Some(source_pk),
                    "error",
                    "profile.description_unsupported",
                    "legacy Description exceeds the C# profile storage bound and was not imported",
                    None,
                )
                .await?;
                issues += 1;
            }
            if source_time_zone.chars().count() > MAX_TIME_ZONE_CHARS {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "users",
                    Some(source_pk),
                    "error",
                    "profile.time_zone_unsupported",
                    "legacy TimeZoneId exceeds the C# profile storage bound; target uses UTC",
                    None,
                )
                .await?;
                issues += 1;
            } else if !source_time_zone.is_empty() && !valid_time_zone_id(&source_time_zone) {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "users",
                    Some(source_pk),
                    "warning",
                    "profile.time_zone_invalid",
                    "legacy TimeZoneId was preserved but does not match the safe timezone grammar",
                    None,
                )
                .await?;
                issues += 1;
            }
            if row
                .get::<Option<String>, _>("PreferredLanguage")
                .is_some_and(|value| value.chars().count() > MAX_LANGUAGE_CHARS)
            {
                runs::record_issue(
                    &mut tx, run_id, "users", Some(source_pk), "error",
                    "profile.language_unsupported",
                    "legacy PreferredLanguage exceeds the C# profile storage bound and was not imported",
                    None,
                )
                .await?;
                issues += 1;
            }
            if row
                .get::<Option<String>, _>("PreferredTheme")
                .is_some_and(|value| value.chars().count() > MAX_THEME_CHARS)
            {
                runs::record_issue(
                    &mut tx, run_id, "users", Some(source_pk), "error",
                    "profile.theme_unsupported",
                    "legacy PreferredTheme exceeds the C# profile storage bound and was not imported",
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

/// Maps legacy `Has*Role` flags to the preserved native role vocabulary.
async fn insert_user_roles(
    tx: &mut Transaction<'_, Postgres>,
    target_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> Result<(), String> {
    let flags: &[(&str, bool)] = &[
        ("HasStreamRole", row.get("HasStreamRole")),
        ("HasDownloadRole", row.get("HasDownloadRole")),
        ("HasUploadRole", row.get("HasUploadRole")),
        ("HasPlaylistRole", row.get("HasPlaylistRole")),
        ("HasShareRole", row.get("HasShareRole")),
        ("HasCoverArtRole", row.get("HasCoverArtRole")),
        ("HasCommentRole", row.get("HasCommentRole")),
        ("HasPodcastRole", row.get("HasPodcastRole")),
        ("HasSettingsRole", row.get("HasSettingsRole")),
        ("HasJukeboxRole", row.get("HasJukeboxRole")),
    ];
    for (role, enabled) in flags
        .iter()
        .chain([("Administrator", row.get("IsAdmin"))].iter())
    {
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

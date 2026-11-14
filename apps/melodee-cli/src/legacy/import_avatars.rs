//! Read-only import of legacy User Images avatars (ADR-0178).
//!
//! The legacy numeric filename is used only to join a source file to a source
//! user. It is never copied into target identity or storage paths.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use melodee_persistence::MAX_AVATAR_BYTES;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::source::SourceDb;
use super::{map, runs};

const USER_IMAGES_LIBRARY_TYPE: i32 = 4;
const MAX_SOURCE_AVATAR_BYTES: u64 = 10 * 1024 * 1024;
const MAX_DISCOVERED_FILES: usize = 10_000;

#[derive(Debug)]
struct ScanIssue {
    source_pk: Option<i64>,
    code: &'static str,
    message: &'static str,
}

#[derive(Debug)]
struct AvatarDirectory {
    files: BTreeMap<i64, PathBuf>,
    issues: Vec<ScanIssue>,
}

pub async fn import_user_avatars(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    if runs::checkpoint_key(target, run_id, "user_avatars").await? > 0 {
        return Ok(());
    }
    let libraries =
        sqlx::query(r#"SELECT "Path" FROM "Libraries" WHERE "Type" = $1 ORDER BY "Id""#)
            .bind(USER_IMAGES_LIBRARY_TYPE)
            .fetch_all(source.pool())
            .await
            .map_err(|error| format!("user images library read failed: {error}"))?;
    let mut directory = None;
    let mut initial_issues = Vec::new();
    match libraries.as_slice() {
        [] => initial_issues.push(ScanIssue {
            source_pk: None,
            code: "avatar.library_missing",
            message: "legacy User Images library (Type 4) is not configured",
        }),
        [library] => {
            let path = PathBuf::from(library.get::<String, _>("Path"));
            let scanned = scan_directory(&path);
            match scanned {
                Ok(scanned) => directory = Some(scanned),
                Err(issue) => initial_issues.push(issue),
            }
        }
        _ => initial_issues.push(ScanIssue {
            source_pk: None,
            code: "avatar.library_ambiguous",
            message: "more than one legacy User Images library exists; import stopped safely",
        }),
    }
    let mut key = 0_i64;
    let mut first_batch = true;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "CreatedAt" FROM "Users"
               WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|error| format!("avatar user source read failed: {error}"))?;
        let last_pk = rows
            .last()
            .map_or(key, |row| i64::from(row.get::<i32, _>("Id")));
        let mut tx = target.begin().await.map_err(|error| error.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during avatar import"));
        }
        let mut inserted = 0_i64;
        let mut updated = 0_i64;
        let mut issues = 0_i64;
        if first_batch {
            for issue in &initial_issues {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "user_avatars",
                    issue.source_pk,
                    if issue.code == "avatar.library_missing" {
                        "warning"
                    } else {
                        "error"
                    },
                    issue.code,
                    issue.message,
                    None,
                )
                .await?;
                issues += 1;
            }
            if let Some(directory) = &directory {
                for issue in &directory.issues {
                    runs::record_issue(
                        &mut tx,
                        run_id,
                        "user_avatars",
                        issue.source_pk,
                        "error",
                        issue.code,
                        issue.message,
                        None,
                    )
                    .await?;
                    issues += 1;
                }
                for source_pk in directory.files.keys() {
                    if map::mapped_target(&mut tx, run_id, "users", *source_pk)
                        .await?
                        .is_none()
                    {
                        runs::record_issue(
                            &mut tx,
                            run_id,
                            "user_avatars",
                            Some(*source_pk),
                            "error",
                            "avatar.unmapped_owner",
                            "avatar filename does not map to an imported legacy user",
                            None,
                        )
                        .await?;
                        issues += 1;
                    }
                }
            }
            first_batch = false;
        }
        for row in &rows {
            let source_pk = i64::from(row.get::<i32, _>("Id"));
            let Some(directory) = &directory else {
                continue;
            };
            let Some(path) = directory.files.get(&source_pk) else {
                continue;
            };
            let Some(target_id) = map::mapped_target(&mut tx, run_id, "users", source_pk).await?
            else {
                continue;
            };
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(_) => {
                    runs::record_issue(
                        &mut tx,
                        run_id,
                        "user_avatars",
                        Some(source_pk),
                        "error",
                        "avatar.read_failed",
                        "owned avatar could not be read from the read-only source",
                        None,
                    )
                    .await?;
                    issues += 1;
                    continue;
                }
            };
            let normalized =
                match melodee_application::avatars::normalize_legacy_avatar_image(bytes).await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        runs::record_issue(
                            &mut tx,
                            run_id,
                            "user_avatars",
                            Some(source_pk),
                            "error",
                            "avatar.image_unsupported",
                            "owned avatar is not a bounded decodable image",
                            None,
                        )
                        .await?;
                        issues += 1;
                        continue;
                    }
                };
            if normalized.len() > MAX_AVATAR_BYTES {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "user_avatars",
                    Some(source_pk),
                    "error",
                    "avatar.normalized_too_large",
                    "normalized avatar exceeds the target store bound",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            let existing = sqlx::query_as::<_, (String, Vec<u8>)>(
                "SELECT content_type,image FROM native_user_avatars WHERE user_id=$1 FOR UPDATE",
            )
            .bind(target_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| format!("avatar target read failed: {error}"))?;
            if existing.as_ref().is_some_and(|(content_type, image)| {
                content_type == "image/png" && image == &normalized
            }) {
                continue;
            }
            sqlx::query(
                "INSERT INTO native_user_avatars (user_id,content_type,image,updated_at)
                 VALUES ($1,'image/png',$2,$3)
                 ON CONFLICT (user_id) DO UPDATE
                    SET content_type='image/png', image=$2, updated_at=$3",
            )
            .bind(target_id)
            .bind(&normalized)
            .bind(OffsetDateTime::now_utc())
            .execute(&mut *tx)
            .await
            .map_err(|error| format!("avatar target write failed: {error}"))?;
            let fields = vec!["avatar".to_owned()];
            sqlx::query(
                "INSERT INTO native_user_profile_audits
                    (id,user_id,actor_user_id,event,changed_fields,created_at)
                 VALUES ($1,$2,$2,'avatar_migrated',$3,$4)",
            )
            .bind(Uuid::now_v7())
            .bind(target_id)
            .bind(fields)
            .bind(OffsetDateTime::now_utc())
            .execute(&mut *tx)
            .await
            .map_err(|error| format!("avatar audit write failed: {error}"))?;
            if existing.is_some() {
                updated += 1;
            } else {
                inserted += 1;
            }
        }
        if rows.is_empty() {
            runs::advance_checkpoint(
                &mut tx,
                run_id,
                "user_avatars",
                1,
                0,
                inserted,
                updated,
                issues,
                0,
            )
            .await?;
        } else {
            runs::advance_checkpoint(
                &mut tx,
                run_id,
                "user_avatars",
                last_pk,
                rows.len() as i64,
                inserted,
                updated,
                issues,
                0,
            )
            .await?;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
        if rows.is_empty() || (rows.len() as i64) < batch_size {
            break;
        }
        key = last_pk;
    }
    Ok(())
}

fn scan_directory(path: &Path) -> Result<AvatarDirectory, ScanIssue> {
    if !path.is_absolute() {
        return Err(ScanIssue {
            source_pk: None,
            code: "avatar.library_path_relative",
            message: "legacy User Images library path must be absolute",
        });
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| ScanIssue {
        source_pk: None,
        code: "avatar.library_unavailable",
        message: "legacy User Images library path is unavailable",
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ScanIssue {
            source_pk: None,
            code: "avatar.library_unsafe",
            message: "legacy User Images library must be a direct regular directory, not a symlink",
        });
    }
    let entries = fs::read_dir(path).map_err(|_| ScanIssue {
        source_pk: None,
        code: "avatar.library_unreadable",
        message: "legacy User Images library could not be enumerated read-only",
    })?;
    let mut files = BTreeMap::new();
    let mut issues = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_DISCOVERED_FILES {
            issues.push(ScanIssue {
                source_pk: None,
                code: "avatar.directory_too_large",
                message: "legacy User Images directory exceeds the bounded discovery limit",
            });
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                issues.push(ScanIssue {
                    source_pk: None,
                    code: "avatar.entry_unreadable",
                    message: "a User Images directory entry could not be read",
                });
                continue;
            }
        };
        let entry_path = entry.path();
        let metadata = match fs::symlink_metadata(&entry_path) {
            Ok(metadata) => metadata,
            Err(_) => {
                issues.push(ScanIssue {
                    source_pk: None,
                    code: "avatar.entry_unreadable",
                    message: "a User Images entry could not be inspected safely",
                });
                continue;
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            issues.push(ScanIssue {
                source_pk: None,
                code: "avatar.unsafe_file",
                message: "a User Images entry is not a direct regular file",
            });
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(source_pk) = parse_avatar_name(&name) else {
            issues.push(ScanIssue {
                source_pk: None,
                code: "avatar.unsupported_filename",
                message: "User Images file does not use the legacy eight-digit .gif naming convention",
            });
            continue;
        };
        if metadata.len() == 0 || metadata.len() > MAX_SOURCE_AVATAR_BYTES {
            issues.push(ScanIssue {
                source_pk: Some(source_pk),
                code: "avatar.source_size_unsupported",
                message: "owned avatar is empty or exceeds the bounded source size",
            });
            continue;
        }
        files.insert(source_pk, entry_path);
    }
    Ok(AvatarDirectory { files, issues })
}

fn parse_avatar_name(name: &str) -> Option<i64> {
    let (digits, extension) = name.split_once('.')?;
    if extension != "gif" || digits.len() != 8 || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let source_pk = digits.parse::<i64>().ok()?;
    (source_pk > 0 && format!("{source_pk:08}.gif") == name).then_some(source_pk)
}

#[cfg(test)]
mod tests {
    use super::parse_avatar_name;

    #[test]
    fn only_legacy_avatar_names_are_owned() {
        assert_eq!(parse_avatar_name("00000001.gif"), Some(1));
        assert_eq!(parse_avatar_name("00000001.png"), None);
        assert_eq!(parse_avatar_name("1.gif"), None);
        assert_eq!(parse_avatar_name("00000000.gif"), None);
    }
}

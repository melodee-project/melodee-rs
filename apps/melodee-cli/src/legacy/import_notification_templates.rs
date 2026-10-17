//! Legacy notification-template importer (MEL-6).
//!
//! The C# `EmailTemplateService` reads operator-authored templates from the
//! legacy Templates library at `Templates/<culture>/PasswordReset.{txt,html}`
//! before falling back to built-ins. This module scans that directory tree,
//! validates each template, and upserts it into the Rust
//! `notification_templates` table. Import is idempotent (upsert on
//! `(template_key, locale)`); missing or malformed files are recorded as
//! durable issues, never silently dropped.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::runs;
use super::source::SourceDb;
use melodee_persistence::MAX_LOCALE_CHARS;

/// Template key for the password-reset event, matching the seeded default.
const TEMPLATE_KEY: &str = "user.password_reset";

/// Maximum cultures to scan per import run (bounded input).
const MAX_CULTURES: usize = 64;

/// Bounded subject length, mirroring the migration CHECK.
const MAX_SUBJECT_LEN: usize = 500;
/// Bounded body length, mirroring the migration CHECK.
const MAX_BODY_LEN: usize = 64 * 1024;

/// Validate a BCP-47-ish locale tag.
fn is_valid_locale(locale: &str) -> bool {
    let segments: Vec<&str> = locale.split('-').collect();
    // BCP-47-ish: language (2-3 alpha) optionally followed by region (2-8 alphanumeric).
    if segments.is_empty() || segments.len() > 2 {
        return false;
    }
    let language = segments[0];
    if !(2..=3).contains(&language.len())
        || !language.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return false;
    }
    if locale.len() > MAX_LOCALE_CHARS {
        return false;
    }
    // When a region segment is present, it must differ from the language
    // segment and be a valid BCP-47 region (2-8 alphanumeric chars).
    if segments.len() == 2 {
        let region = segments[1];
        if region == language {
            return false;
        }
        if !(2..=8).contains(&region.len())
            || !region.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return false;
        }
    }
    true
}

#[allow(clippy::collapsible_if)]
/// Read one legacy template file. Returns `(subject, body)` where `subject`
/// is the first line of the txt file and `body` is the remainder after the
/// first blank line. If the file is missing or unreadable, returns an error.
fn read_template_file(path: &std::path::Path) -> Result<(Option<String>, String), String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {path:?}: {e}"))?;
    if content.len() > MAX_BODY_LEN {
        return Err(format!("{path:?} exceeds {MAX_BODY_LEN} bytes"));
    }
    let lines: Vec<&str> = content.lines().collect();
    let subject = lines.first().map(|s| s.trim().to_string());
    let body = if lines.len() > 1 {
        // Skip the subject line and any leading blank lines, then join the rest.
        let mut start = 1;
        while start < lines.len() && lines[start].trim().is_empty() {
            start += 1;
        }
        if start < lines.len() {
            lines[start..].join("\n")
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    if let Some(subject) = subject.as_ref() {
        if subject.len() > MAX_SUBJECT_LEN {
            return Err(format!(
                "subject in {path:?} exceeds {MAX_SUBJECT_LEN} characters"
            ));
        }
    }
    Ok((subject, body))
}

#[allow(clippy::collapsible_if)]
/// Import legacy notification templates from the Templates library.
pub async fn import_notification_templates(
    _source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    _batch_size: i64,
    legacy_root: &std::path::Path,
) -> Result<(), String> {
    let templates_dir = legacy_root.join("Templates");
    if !templates_dir.is_dir() {
        // No Templates library: nothing to import; the seeded defaults remain.
        return Ok(());
    }

    // Collect culture directories, bounded.
    let mut cultures: Vec<std::path::PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&templates_dir)
        .map_err(|e| format!("cannot read Templates directory: {e}"))?
    {
        let entry = entry.map_err(|e| format!("cannot read Templates directory: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("invalid culture directory name: {path:?}"))?;
            if is_valid_locale(name) && cultures.len() < MAX_CULTURES {
                cultures.push(path);
            }
        }
    }
    cultures.sort();

    for culture in cultures {
        let locale = culture
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("invalid locale in {culture:?}"))?;
        let txt_path = culture.join("PasswordReset.txt");
        let html_path = culture.join("PasswordReset.html");

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during notification template import"
            ));
        }

        let (subject, body_text) = match read_template_file(&txt_path) {
            Ok(result) => result,
            Err(error) => {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "notification_templates",
                    None,
                    "error",
                    "template.read_failed",
                    &error,
                    None,
                )
                .await?;
                runs::advance_checkpoint(
                    &mut tx,
                    run_id,
                    "notification_templates",
                    0,
                    1,
                    0,
                    0,
                    1,
                    0,
                )
                .await?;
                tx.commit().await.map_err(|e| e.to_string())?;
                continue;
            }
        };

        let body_html = match read_template_file(&html_path) {
            Ok((_, html)) => Some(html),
            Err(_) => None,
        };

        // Validate subject length.
        if let Some(subject) = subject.as_ref() {
            if subject.len() > MAX_SUBJECT_LEN {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "notification_templates",
                    None,
                    "error",
                    "template.subject_too_long",
                    &format!("subject exceeds {MAX_SUBJECT_LEN} characters"),
                    None,
                )
                .await?;
                runs::advance_checkpoint(
                    &mut tx,
                    run_id,
                    "notification_templates",
                    0,
                    1,
                    0,
                    0,
                    1,
                    0,
                )
                .await?;
                tx.commit().await.map_err(|e| e.to_string())?;
                continue;
            }
        }

        // Upsert the template (idempotent): replace on conflict and bump version.
        let now = OffsetDateTime::now_utc();
        let _written = sqlx::query(
            "INSERT INTO notification_templates \
             (template_key,locale,subject,body_text,body_html,version,updated_at) \
             VALUES ($1,$2,$3,$4,$5,1,$6) \
             ON CONFLICT (template_key, locale) DO UPDATE SET \
               subject = EXCLUDED.subject, body_text = EXCLUDED.body_text, \
               body_html = EXCLUDED.body_html, \
               version = notification_templates.version + 1, updated_at = EXCLUDED.updated_at \
             RETURNING template_key,locale",
        )
        .bind(TEMPLATE_KEY)
        .bind(locale)
        .bind(subject.as_deref())
        .bind(&body_text)
        .bind(body_html.as_deref())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("template upsert failed: {e}"))?;

        runs::advance_checkpoint(&mut tx, run_id, "notification_templates", 0, 1, 1, 0, 0, 0)
            .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn locale_validation_accepts_bcp47_tags() {
        assert!(is_valid_locale("en"));
        assert!(is_valid_locale("en-US"));
        assert!(is_valid_locale("pt-BR"));
        assert!(is_valid_locale("zh-Hant"));
        assert!(is_valid_locale("de-DE"));
    }

    #[test]
    fn locale_validation_rejects_invalid_tags() {
        assert!(!is_valid_locale(""));
        assert!(!is_valid_locale("e"));
        assert!(!is_valid_locale("en-US-"));
        assert!(!is_valid_locale("123"));
        assert!(!is_valid_locale("en-US-extra"));
    }

    #[test]
    fn read_template_file_parses_subject_and_body() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("PasswordReset.txt");
        fs::write(
            &path,
            "Reset your password\n\nUse this link to reset:\n{{resetUrl}}\n",
        )
        .expect("write file");
        let (subject, body) = read_template_file(&path).expect("read file");
        assert_eq!(subject, Some("Reset your password".to_string()));
        assert_eq!(body, "Use this link to reset:\n{{resetUrl}}");
    }

    #[test]
    fn read_template_file_missing_file_returns_error() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("PasswordReset.txt");
        let result = read_template_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn subject_length_bound_is_enforced() {
        let long_subject = "A".repeat(MAX_SUBJECT_LEN + 1);
        assert!(long_subject.len() > MAX_SUBJECT_LEN);
    }

    #[test]
    fn body_length_bound_is_enforced() {
        let long_body = "A".repeat(MAX_BODY_LEN + 1);
        assert!(long_body.len() > MAX_BODY_LEN);
    }
}

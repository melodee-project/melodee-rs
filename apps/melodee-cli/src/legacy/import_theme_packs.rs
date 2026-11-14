//! Legacy theme-pack importer (MEL-7).
//!
//! The C# `ThemeService` discovers, imports, exports, serves, and deletes
//! packs under the C# Theme library. This module scans the legacy Theme
//! library at `<legacy-root>/Themes/` for `.zip` theme packs, validates each
//! pack using the same bounds and safety checks as the Rust theme import
//! route, and imports valid packs into the Rust `native_themes` /
//! `native_theme_assets` tables. Import is idempotent (skips themes that
//! already exist); missing or malformed packs are recorded as durable
//! issues, never silently dropped.

use std::path::Path;

use melodee_application::theme_archive::read_theme_entries;
#[cfg(test)]
use melodee_application::theme_archive::safe_theme_entry_name;
use sqlx::PgPool;
use uuid::Uuid;

use super::runs;
use super::source::SourceDb;

/// Maximum theme packs to scan per import run (bounded input).
const MAX_THEME_PACKS: usize = 64;

/// Asset list returned by theme pack parsing.
type ThemeAssetList = Vec<(String, Vec<u8>)>;

/// Validate a theme ID (mirrors `valid_theme_id` in discovery.rs).
fn valid_theme_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && id.len() <= 128
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

/// Theme metadata parsed from a legacy theme pack's `theme.json`.
#[derive(Debug, Clone)]
struct LegacyThemeMetadata {
    id: String,
    name: String,
    author: Option<String>,
    version: Option<String>,
    description: Option<String>,
    preview_image: Option<String>,
}

/// Parse a legacy theme pack ZIP archive. Mirrors the validation in
/// `discovery.rs::parse_theme_archive` but returns structured data for
/// import.
fn parse_theme_archive(bytes: &[u8]) -> Result<(LegacyThemeMetadata, ThemeAssetList), String> {
    let entries = read_theme_entries(bytes).map_err(str::to_owned)?;
    let mut assets = Vec::new();
    let mut metadata = None;
    for (name, data) in entries {
        if name == "theme.json" {
            let json: serde_json::Value =
                serde_json::from_slice(&data).map_err(|e| format!("Invalid theme.json: {e}"))?;
            metadata = Some(LegacyThemeMetadata {
                id: json
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_default(),
                name: json
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_default(),
                author: json
                    .get("author")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                version: json
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                description: json
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                preview_image: json
                    .get("preview_image")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        } else {
            assets.push((name, data));
        }
    }

    let meta = metadata.ok_or("Theme archive missing theme.json")?;
    if !valid_theme_id(&meta.id) {
        return Err(format!("Invalid theme ID: {}", meta.id));
    }
    if meta.name.is_empty() {
        return Err("Theme name is required".to_string());
    }

    Ok((meta, assets))
}

/// Import legacy theme packs from the Theme library.
pub async fn import_theme_packs(
    _source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    _batch_size: i64,
    legacy_root: &Path,
) -> Result<(), String> {
    let themes_dir = legacy_root.join("Themes");
    if !themes_dir.is_dir() {
        // No Theme library: nothing to import.
        return Ok(());
    }

    // Collect theme pack files, bounded.
    let mut packs: Vec<std::path::PathBuf> = Vec::new();
    let read_dir =
        std::fs::read_dir(&themes_dir).map_err(|e| format!("cannot read Themes directory: {e}"))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| format!("cannot read Themes directory: {e}"))?;
        let path = entry.path();
        if path.is_file()
            && path.extension().is_some_and(|ext| ext == "zip")
            && packs.len() < MAX_THEME_PACKS
        {
            packs.push(path);
        }
    }
    packs.sort();

    for pack_path in packs {
        let pack_name = pack_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let bytes = match std::fs::File::open(&pack_path)
            .and_then(melodee_application::theme_archive::read_theme_archive)
        {
            Ok(b) => b,
            Err(error) => {
                let mut tx = target.begin().await.map_err(|e| e.to_string())?;
                if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
                    return Err(format!("run {run_id}: lost lease during theme pack import"));
                }
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "theme_packs",
                    None,
                    "error",
                    "theme_pack.read_failed",
                    &format!("cannot read {pack_name}: {error}"),
                    None,
                )
                .await?;
                runs::advance_checkpoint(&mut tx, run_id, "theme_packs", 0, 1, 0, 0, 1, 0).await?;
                tx.commit().await.map_err(|e| e.to_string())?;
                continue;
            }
        };

        let parsed = match parse_theme_archive(&bytes) {
            Ok(parsed) => parsed,
            Err(error) => {
                let mut tx = target.begin().await.map_err(|e| e.to_string())?;
                if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
                    return Err(format!("run {run_id}: lost lease during theme pack import"));
                }
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "theme_packs",
                    None,
                    "error",
                    "theme_pack.invalid",
                    &format!("{pack_name}: {error}"),
                    None,
                )
                .await?;
                runs::advance_checkpoint(&mut tx, run_id, "theme_packs", 0, 1, 0, 0, 1, 0).await?;
                tx.commit().await.map_err(|e| e.to_string())?;
                continue;
            }
        };

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during theme pack import"));
        }

        // Check if theme already exists (idempotent).
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_themes WHERE id=$1)")
                .bind(&parsed.0.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| format!("cannot check theme existence: {e}"))?;

        if exists {
            runs::advance_checkpoint(&mut tx, run_id, "theme_packs", 0, 1, 0, 0, 0, 0).await?;
            tx.commit().await.map_err(|e| e.to_string())?;
            continue;
        }

        // Insert theme metadata.
        sqlx::query(
            "INSERT INTO native_themes(id,name,author,version,description,is_built_in,preview_image,\
             has_warnings,created_at,updated_at) \
             VALUES($1,$2,$3,$4,$5,false,$6,false,now(),now())",
        )
        .bind(&parsed.0.id)
        .bind(parsed.0.name.trim())
        .bind(parsed.0.author.as_deref().unwrap_or("Unknown"))
        .bind(parsed.0.version.as_deref().unwrap_or("1.0.0"))
        .bind(parsed.0.description.as_deref())
        .bind(parsed.0.preview_image.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("theme insert failed: {e}"))?;

        // Insert theme assets.
        for (path, data) in &parsed.1 {
            let mime = mime_for_asset(path);
            sqlx::query(
                "INSERT INTO native_theme_assets(theme_id,relative_path,mime_type,bytes) \
                 VALUES($1,$2,$3,$4)",
            )
            .bind(&parsed.0.id)
            .bind(path)
            .bind(mime)
            .bind(data)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("theme asset insert failed: {e}"))?;
        }

        runs::advance_checkpoint(&mut tx, run_id, "theme_packs", 0, 1, 1, 0, 0, 0).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// MIME type for a theme asset file (mirrors `theme_asset_mime` in discovery.rs).
fn mime_for_asset(path: &str) -> String {
    let extension = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("css") => "text/css".to_string(),
        Some("json") => "application/json".to_string(),
        Some("png") => "image/png".to_string(),
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("ico") => "image/x-icon".to_string(),
        Some("svg") => "image/svg+xml".to_string(),
        Some("woff") => "font/woff".to_string(),
        Some("woff2") => "font/woff2".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use zip::ZipWriter;

    #[test]
    fn theme_id_validation_accepts_valid_ids() {
        assert!(valid_theme_id("my-theme"));
        assert!(valid_theme_id("Theme.123"));
        assert!(valid_theme_id("a"));
    }

    #[test]
    fn theme_id_validation_rejects_invalid_ids() {
        assert!(!valid_theme_id(""));
        assert!(!valid_theme_id("-theme"));
        assert!(!valid_theme_id("theme with spaces"));
        assert!(!valid_theme_id("theme/with/slashes"));
        assert!(!valid_theme_id(".."));
    }

    #[test]
    fn safe_theme_entry_name_allows_safe_paths() {
        assert!(safe_theme_entry_name("theme.css"));
        assert!(safe_theme_entry_name("sub/dir/image.png"));
        assert!(safe_theme_entry_name("file.name.with.dots.css"));
    }

    #[test]
    fn safe_theme_entry_name_rejects_unsafe_paths() {
        assert!(!safe_theme_entry_name(""));
        assert!(!safe_theme_entry_name("/absolute"));
        assert!(!safe_theme_entry_name("with\\backslash"));
        assert!(!safe_theme_entry_name("."));
        assert!(!safe_theme_entry_name(".."));
        assert!(!safe_theme_entry_name("sub/../escape"));
    }

    #[test]
    fn valid_theme_pack_preserves_metadata_and_asset_bytes() {
        use std::io::{Cursor, Write};
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("theme.json", zip::write::SimpleFileOptions::default())
            .expect("metadata entry");
        writer
            .write_all(br#"{"id":"legacy-theme","name":"Legacy Theme","author":"A"}"#)
            .expect("metadata");
        writer
            .start_file("theme.css", zip::write::SimpleFileOptions::default())
            .expect("asset entry");
        writer
            .write_all(b"body { color: #123456; }")
            .expect("asset");
        let bytes = writer.finish().expect("finish").into_inner();
        let (metadata, assets) = parse_theme_archive(&bytes).expect("valid pack");
        assert_eq!(metadata.id, "legacy-theme");
        assert_eq!(metadata.name, "Legacy Theme");
        assert_eq!(metadata.author.as_deref(), Some("A"));
        assert_eq!(
            assets,
            vec![("theme.css".to_owned(), b"body { color: #123456; }".to_vec())]
        );
    }

    #[test]
    fn parse_theme_archive_rejects_non_zip() {
        let result = parse_theme_archive(b"Not a ZIP");
        assert!(result.is_err());
    }

    #[test]
    fn parse_theme_archive_rejects_missing_metadata() {
        use std::io::Write;
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("test.zip");
        let file = std::fs::File::create(&path).expect("create zip file");
        let mut zip = ZipWriter::new(file);
        zip.start_file("other.txt", zip::write::SimpleFileOptions::default())
            .expect("start file");
        zip.write_all("other.txt".as_bytes()).expect("write entry");
        drop(zip);
        let bytes = fs::read(path).expect("read zip");
        let result = parse_theme_archive(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing theme.json"));
    }

    #[test]
    fn mime_for_asset_returns_correct_types() {
        assert_eq!(mime_for_asset("theme.css"), "text/css");
        assert_eq!(mime_for_asset("image.png"), "image/png");
        assert_eq!(mime_for_asset("image.jpg"), "image/jpeg");
        assert_eq!(mime_for_asset("unknown.xyz"), "application/octet-stream");
    }
}

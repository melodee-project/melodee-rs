//! Shared mapping/path/timestamp helpers for the legacy importers (ADR-0148).

use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::LibraryPathMap;

/// ADR-0148 §D.3: 64 ASCII zeros marks "hash pending backfill" — never
/// authoritative, never dedup evidence, never deletion authority.
pub const HASH_PENDING_SENTINEL: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Existing non-catalog mapping target, when this source row already migrated.
pub async fn mapped_target(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    domain: &str,
    source_pk: i64,
) -> Result<Option<Uuid>, String> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT target_id FROM legacy_migration.entity_map
          WHERE run_id = $1 AND domain = $2 AND source_pk = $3",
    )
    .bind(run_id)
    .bind(domain)
    .bind(source_pk)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| format!("mapping lookup failed for {domain}/{source_pk}: {e}"))
}

/// Existing catalog mapping internal id (artist/release/track).
pub async fn catalog_mapping(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    entity_type: &str,
    legacy_pk: i64,
) -> Result<Option<Uuid>, String> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT internal_id FROM legacy_catalog_id_mappings
          WHERE migration_run_id = $1 AND entity_type = $2 AND legacy_numeric_id = $3",
    )
    .bind(run_id)
    .bind(entity_type)
    .bind(legacy_pk)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| format!("catalog mapping lookup failed for {entity_type}/{legacy_pk}: {e}"))
}

/// Inserts the non-catalog mapping row; a conflict means a committed earlier
/// batch already migrated this source row, which is the idempotency anchor.
pub async fn insert_mapping(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    domain: &str,
    source_pk: i64,
    source_public_id: Option<Uuid>,
    target_id: Uuid,
    source_updated_at: OffsetDateTime,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO legacy_migration.entity_map (
            run_id, domain, source_pk, source_public_id, target_id,
            source_updated_at, created_at
         ) VALUES ($1,$2,$3,$4,$5,$6,now())
         ON CONFLICT (run_id, domain, source_pk) DO NOTHING",
    )
    .bind(run_id)
    .bind(domain)
    .bind(source_pk)
    .bind(source_public_id)
    .bind(target_id)
    .bind(source_updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("mapping insert failed for {domain}/{source_pk}: {e}"))?;
    Ok(())
}

/// Inserts the catalog identity mapping (Phase-4 table, ADR-0148 §D.2).
pub async fn insert_catalog_mapping(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    entity_type: &str,
    legacy_pk: i64,
    internal_id: Uuid,
    public_api_id: Uuid,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO legacy_catalog_id_mappings (
            id, migration_run_id, entity_type, legacy_numeric_id, internal_id,
            public_api_id, source_snapshot, imported_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,now())
         ON CONFLICT (migration_run_id, entity_type, legacy_numeric_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(run_id)
    .bind(entity_type)
    .bind(legacy_pk)
    .bind(internal_id)
    .bind(public_api_id)
    .bind(super::source::FROZEN_SCHEMA_FINAL_MIGRATION)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("catalog mapping insert failed for {entity_type}/{legacy_pk}: {e}"))?;
    Ok(())
}

/// Resolves legacy path components into a canonical-relative path under the
/// mapped root prefix. The legacy media path composes
/// `Artist.Directory / Album.Directory / Song.FileName` (evidence:
/// `AlbumService.cs`/`ArtistRescanEventHandler.cs` path composition), so the
/// artist directory precedes the album directory; either may be empty.
/// Returns `None` for missing metadata or unsafe components (traversal,
/// absolute, backslash-escape) — the caller records an issue rather than
/// importing an unsafe path (ADR-0148 §D.3).
pub fn resolve_canonical_path(
    artist_directory: Option<&str>,
    album_directory: Option<&str>,
    file_name: Option<&str>,
    maps: &[LibraryPathMap],
) -> Option<String> {
    let file_name = file_name?.trim();
    if file_name.is_empty() {
        return None;
    }
    let artist_directory = artist_directory.unwrap_or("").replace('\\', "/");
    let album_directory = album_directory.unwrap_or("").replace('\\', "/");
    let directory = match (artist_directory.is_empty(), album_directory.is_empty()) {
        (true, true) => String::new(),
        (false, true) => artist_directory,
        (true, false) => album_directory,
        (false, false) => format!("{artist_directory}/{album_directory}"),
    };

    // Root mapping (ADR-0148 §D.3): an absolute legacy directory must sit
    // under one of the operator-supplied legacy roots (longest match wins);
    // a relative directory is adopted under the default mapping — the map
    // with the longest relative prefix — so a single-map deployment adopts
    // every library-relative path deterministically.
    let mut matched_prefix = "";
    let mut relative_dir = directory.as_str();
    if relative_dir.starts_with('/') {
        let mut best: Option<(&super::LibraryPathMap, &str)> = None;
        for map in maps {
            let root = map.legacy_path.as_str();
            if (directory == *root || directory.starts_with(&format!("{root}/")))
                && best.is_none_or(|(current, _)| root.len() > current.legacy_path.len())
            {
                best = Some((map, directory[root.len()..].trim_start_matches('/')));
            }
        }
        let (map, rest) = best?;
        matched_prefix = &map.relative_prefix;
        relative_dir = rest;
    } else {
        for map in maps {
            if map.relative_prefix.len() >= matched_prefix.len() {
                matched_prefix = &map.relative_prefix;
            }
        }
    }

    let mut components: Vec<String> = Vec::new();
    if !matched_prefix.is_empty() {
        components.push(matched_prefix.to_string());
    }
    for part in relative_dir.split('/') {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.starts_with('/') || part.contains('\\') {
            return None;
        }
        components.push(part.to_string());
    }
    if file_name.contains("..") || file_name.contains('/') || file_name.contains('\\') {
        return None;
    }
    components.push(file_name.to_string());
    let joined = components.join("/");
    if joined.is_empty() || joined.starts_with('/') {
        return None;
    }
    Some(joined)
}

/// Legacy timestamps occasionally predate creation; the Rust schema requires
/// `created_at <= updated_at`.
pub fn clamp_updated(created: OffsetDateTime, updated: Option<OffsetDateTime>) -> OffsetDateTime {
    updated.filter(|u| *u >= created).unwrap_or(created)
}

/// Legacy normalized fields fall back to a lowercase trim; normalized values
/// are search attributes, never identity (ADR-0148 §D.2).
pub fn normalized_or(value: Option<String>, fallback: &str) -> String {
    value
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| fallback.trim().to_lowercase())
}

/// Converts the legacy bitrate field to the Rust `bitrate_bps` unit.
///
/// Frozen legacy rows use the historical UI unit (kbps) for ordinary values
/// such as `1411`, while some imported/test rows already contain a bits-per-
/// second value such as `900000`. Keep large values intact and convert only
/// the bounded human-scale form; this avoids turning an already-bps value into
/// an invalid thousand-fold rate while preserving the captured API value.
pub fn legacy_bitrate_bps(value: Option<i32>) -> Option<i64> {
    value.and_then(|value| {
        let value = i64::from(value);
        if value <= 0 {
            None
        } else if value < 100_000 {
            value.checked_mul(1_000)
        } else {
            Some(value)
        }
    })
}

/// Normalizes the frozen legacy `Songs.Duration` value into target milliseconds.
///
/// The legacy model declares the field in milliseconds, but the tag reader
/// reports seconds and the frozen fixture data was written with that value.
/// Values below the plausibility ceiling (1,000 — no real song shorter than
/// one second is expected) are seconds and are converted to milliseconds;
/// values at or above the ceiling are already milliseconds and are preserved.
pub fn legacy_duration_ms(value: Option<f64>) -> Option<i64> {
    value.filter(|value| *value > 0.0).and_then(|value| {
        let seconds = value as i64;
        if seconds < 1 {
            // Sub-second values are truncated to zero; treat as absent.
            None
        } else if seconds < 1_000 {
            Some(seconds * 1_000)
        } else {
            Some(seconds)
        }
    })
}

/// Returns the existing media-container label that can be derived without
/// opening or rewriting a legacy media file. Unknown values remain absent;
/// migration must not pretend that an arbitrary MIME value proves a codec.
pub fn legacy_container(content_type: Option<&str>, file_name: Option<&str>) -> Option<String> {
    let content_type = content_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let from_content_type = content_type.as_deref().and_then(|value| match value {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/flac" => Some("flac"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/opus" => Some("opus"),
        "audio/mp4" | "audio/x-m4a" => Some("m4a"),
        "audio/aac" => Some("aac"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/x-ms-wma" => Some("wma"),
        _ => None,
    });
    if let Some(container) = from_content_type {
        return Some(container.to_owned());
    }

    let extension = file_name?
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| !extension.is_empty())?
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "aac" | "flac" | "m4a" | "mp3" | "oga" | "ogg" | "opus" | "wav" | "wma"
    )
    .then_some(extension)
}

/// Adopts a legacy FileHash only when it already satisfies the 64-lowercase-
/// hex target constraint; anything else becomes the hash-pending sentinel.
pub fn adopt_or_sentinel_hash(legacy: Option<String>) -> String {
    legacy
        .filter(|h| {
            h.len() == 64
                && h.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .unwrap_or_else(|| HASH_PENDING_SENTINEL.to_string())
}

#[cfg(test)]
mod tests {
    use super::{legacy_bitrate_bps, legacy_container, legacy_duration_ms};

    #[test]
    fn legacy_bitrate_conversion_preserves_captured_units() {
        assert_eq!(legacy_bitrate_bps(Some(1411)), Some(1_411_000));
        assert_eq!(legacy_bitrate_bps(Some(320)), Some(320_000));
        assert_eq!(legacy_bitrate_bps(Some(900_000)), Some(900_000));
        assert_eq!(legacy_bitrate_bps(Some(0)), None);
        assert_eq!(legacy_bitrate_bps(None), None);
    }

    #[test]
    fn legacy_container_uses_content_type_before_extension() {
        assert_eq!(
            legacy_container(Some("audio/wav"), Some("track.flac")),
            Some("wav".to_owned())
        );
        assert_eq!(
            legacy_container(Some("audio/x-m4a"), Some("track.m4a")),
            Some("m4a".to_owned())
        );
    }

    #[test]
    fn legacy_container_falls_back_only_to_known_audio_extensions() {
        assert_eq!(
            legacy_container(Some("application/octet-stream"), Some("track.FLAC")),
            Some("flac".to_owned())
        );
        assert_eq!(
            legacy_container(Some("application/octet-stream"), Some("track.bin")),
            None
        );
        assert_eq!(legacy_container(None, None), None);
    }

    #[test]
    fn legacy_duration_converts_seconds_and_preserves_milliseconds() {
        // Values below the ceiling are seconds (tag-reader unit) → ×1000.
        assert_eq!(legacy_duration_ms(Some(2.4)), Some(2400));
        assert_eq!(legacy_duration_ms(Some(545.0)), Some(545_000));
        assert_eq!(legacy_duration_ms(Some(999.0)), Some(999_000));
        // Plausibility ceiling: values >= 1000 are already milliseconds.
        assert_eq!(legacy_duration_ms(Some(1_000.0)), Some(1_000));
        assert_eq!(legacy_duration_ms(Some(1800.0)), Some(1800));
        assert_eq!(legacy_duration_ms(Some(200_000.0)), Some(200_000));
        assert_eq!(legacy_duration_ms(Some(6_500_000.0)), Some(6_500_000));
        // Boundary: just below the ceiling is still seconds.
        assert_eq!(legacy_duration_ms(Some(999.9)), Some(999_000));
        // Zero and missing are absent, not fabricated.
        assert_eq!(legacy_duration_ms(Some(0.0)), None);
        assert_eq!(legacy_duration_ms(None), None);
    }
}

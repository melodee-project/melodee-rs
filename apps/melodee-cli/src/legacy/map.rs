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
    use super::legacy_bitrate_bps;

    #[test]
    fn legacy_bitrate_conversion_preserves_captured_units() {
        assert_eq!(legacy_bitrate_bps(Some(1411)), Some(1_411_000));
        assert_eq!(legacy_bitrate_bps(Some(320)), Some(320_000));
        assert_eq!(legacy_bitrate_bps(Some(900_000)), Some(900_000));
        assert_eq!(legacy_bitrate_bps(Some(0)), None);
        assert_eq!(legacy_bitrate_bps(None), None);
    }
}

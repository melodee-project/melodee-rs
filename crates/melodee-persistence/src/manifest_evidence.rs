//! Full manifest evidence on persistent files with compact PostgreSQL facts
//! (ADR-0213, PRD-INGEST-007, PRD-FP-001..003, PRD-CATALOG-005).
//!
//! The logical manifest row keeps its identity, version, semantic and sealed
//! hashes. The payload — including Blackbeard provenance documents, raw
//! fallback text, lyrics, parsed sidecars and full Chromaprint strings — is an
//! immutable, verified content-addressed object under
//! `MELODEE__STORAGE__ASSET_ROOT`. PostgreSQL keeps a reference plus the query
//! facts that policy, browse and compatibility surfaces actually need.
//!
//! Every write path is verify-then-bind: the object is published, read back
//! from its final path, and only then referenced. The upgrade is an expand
//! (this schema) → backfill → independent verification → finalize sequence, so
//! an interrupted run leaves the inline source in place and a retry converges.

use serde_json::Value;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::asset_files::{self, StoredEvidence};

/// Version of the on-disk evidence serialization. Readers reject unknown
/// versions instead of guessing.
pub const EVIDENCE_SERIALIZATION_VERSION: i32 = 1;
/// Manifests handled per checkpoint during the upgrade.
pub const MAX_MANIFESTS_PER_CHECKPOINT: i64 = 50;

#[derive(Debug, Error)]
pub enum ManifestEvidenceError {
    #[error("manifest evidence storage failed")]
    Storage(#[from] sqlx::Error),
    #[error("manifest evidence is missing for row {0}")]
    Missing(i64),
    #[error("manifest evidence for row {0} failed verification: {1}")]
    Verification(i64, String),
    #[error("manifest evidence serialization is invalid")]
    Serialization,
}

/// Compact, indexed facts derived from one manifest payload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestFacts {
    pub release_title: Option<String>,
    pub source_path: Option<String>,
    pub has_corrupt_media: bool,
    pub structured_traits: Vec<String>,
    pub embedded_traits: Vec<String>,
    pub lyrics_present: bool,
    pub tracks: Vec<ManifestTrackFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTrackFact {
    pub relative_path: String,
    pub bit_depth: Option<i32>,
    pub sample_rate_hz: Option<i32>,
    pub channels: Option<i32>,
    pub fingerprint_digest: Option<String>,
    pub fingerprint_length: Option<i32>,
    pub corrupt: bool,
    pub genre: Option<String>,
    pub source_ordinal: i32,
}

/// Deterministic serialization used for both write and verification.
pub fn serialize_manifest(value: &Value) -> Result<Vec<u8>, ManifestEvidenceError> {
    serde_json::to_vec(value).map_err(|_| ManifestEvidenceError::Serialization)
}

fn integer(value: Option<&Value>) -> Option<i32> {
    value
        .and_then(Value::as_i64)
        .and_then(|number| i32::try_from(number).ok())
        .filter(|number| *number > 0)
}

/// Derives the query facts from a manifest payload.
///
/// Trait extraction is shared with policy so the stored facts and the
/// classification contract cannot drift.
pub fn extract_facts(manifest: &Value) -> ManifestFacts {
    let (structured, embedded) = extract_traits(manifest);
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let has_corrupt_media = files.iter().any(|file| {
        file.get("bytes_read_errors")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    let lyrics_present = files.iter().any(|file| {
        file.get("metadata")
            .and_then(|metadata| metadata.get("raw"))
            .and_then(|raw| raw.get("lyrics"))
            .and_then(Value::as_str)
            .is_some_and(|lyrics| !lyrics.trim().is_empty())
    }) || manifest
        .get("normalization")
        .and_then(|normalization| normalization.get("parsed_sidecars"))
        .and_then(Value::as_array)
        .is_some_and(|sidecars| {
            sidecars.iter().any(|sidecar| {
                sidecar
                    .get("sidecar_type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "txt" | "lyrics" | "lrc"))
            })
        });

    let mut fingerprints: Vec<(String, String, i32)> = manifest
        .get("normalization")
        .and_then(|normalization| normalization.get("fingerprints"))
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let source = row.get("source_file").and_then(Value::as_str)?;
                    let fingerprint = row.get("fingerprint").and_then(Value::as_str)?;
                    let trimmed = fingerprint.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    Some((
                        source.to_owned(),
                        melodee_domain::acoustic_fingerprint_digest(trimmed),
                        i32::try_from(trimmed.chars().count()).unwrap_or(i32::MAX),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut tracks = files
        .iter()
        .enumerate()
        .filter_map(|(ordinal, file)| {
            let relative_path = file.get("relative_path").and_then(Value::as_str)?;
            let audio = file.get("audio");
            let fingerprint = fingerprints
                .iter()
                .position(|(source, _, _)| source == relative_path)
                .map(|index| fingerprints.remove(index));
            Some(ManifestTrackFact {
                relative_path: relative_path.to_owned(),
                bit_depth: integer(audio.and_then(|audio| audio.get("bit_depth"))),
                sample_rate_hz: integer(audio.and_then(|audio| audio.get("sample_rate_hz"))),
                channels: integer(audio.and_then(|audio| audio.get("channels"))),
                fingerprint_digest: fingerprint.as_ref().map(|value| value.1.clone()),
                fingerprint_length: fingerprint.as_ref().map(|value| value.2),
                corrupt: file
                    .get("bytes_read_errors")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                genre: file
                    .get("metadata")
                    .and_then(|metadata| metadata.get("genre"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                source_ordinal: i32::try_from(ordinal + 1).unwrap_or(i32::MAX),
            })
        })
        .collect::<Vec<_>>();
    tracks.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    ManifestFacts {
        release_title: manifest
            .get("release_observation")
            .and_then(|observation| observation.get("title"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        source_path: manifest
            .get("source_path")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        has_corrupt_media,
        structured_traits: structured
            .into_iter()
            .map(|trait_| trait_.as_str().to_owned())
            .collect(),
        embedded_traits: embedded
            .into_iter()
            .map(|trait_| trait_.as_str().to_owned())
            .collect(),
        lyrics_present,
        tracks,
    }
}

/// Structured and embedded traits observed in the payload (PRD-CATALOG-005).
pub fn extract_traits(
    manifest: &Value,
) -> (
    Vec<melodee_domain::ReleaseTrait>,
    Vec<melodee_domain::ReleaseTrait>,
) {
    let mut structured = std::collections::BTreeSet::new();
    let mut embedded = std::collections::BTreeSet::new();
    let metadata = manifest
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("metadata"))
        .chain(
            manifest
                .get("tracks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|track| track.get("observed_metadata")),
        );
    for raw in metadata.filter_map(|metadata| metadata.get("raw")) {
        let Some(object) = raw.as_object() else {
            continue;
        };
        for (key, value) in object {
            let normalized_key: String = key
                .chars()
                .filter(|char_| char_.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect();
            if normalized_key == "flagcompilation"
                && (value == &Value::Bool(true)
                    || value.as_str().is_some_and(|value| {
                        matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true")
                    }))
            {
                structured.insert(melodee_domain::ReleaseTrait::Compilation);
            } else if normalized_key == "musicbrainzreleasetype" {
                for token in trait_tokens(value) {
                    embedded.insert(token);
                }
            }
        }
    }
    (
        structured.into_iter().collect(),
        embedded.into_iter().collect(),
    )
}

fn trait_tokens(value: &Value) -> Vec<melodee_domain::ReleaseTrait> {
    let values: Vec<&str> = match value {
        Value::String(value) => vec![value],
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    values
        .into_iter()
        .flat_map(|value| value.split([',', ';', '/']))
        .filter_map(|token| {
            let mut normalized = String::new();
            for char_ in token.trim().chars() {
                if char_.is_ascii_alphanumeric() {
                    normalized.extend(char_.to_lowercase());
                } else if !normalized.ends_with('_') {
                    normalized.push('_');
                }
            }
            let normalized = normalized.trim_matches('_');
            melodee_domain::ReleaseTrait::parse(normalized)
        })
        .collect()
}

/// Writes evidence for one manifest row inside the caller's transaction and
/// returns its verified receipt.
///
/// `source` distinguishes evidence produced by the writer (`written`) from
/// upgraded legacy payloads (`backfilled`).
pub async fn store_evidence(
    conn: &mut PgConnection,
    manifest_row_id: i64,
    manifest: &Value,
    source: &str,
    now: OffsetDateTime,
) -> Result<StoredEvidence, ManifestEvidenceError> {
    let bytes = serialize_manifest(manifest)?;
    let receipt = asset_files::put_evidence_on(conn, &bytes).await?;
    // Verify from the final path before any reference is bound.
    let verified = asset_files::read_evidence(
        receipt.key.clone(),
        receipt.storage_kind.to_owned(),
        receipt.sha256.clone(),
        receipt.length,
    )
    .await?;
    if verified != bytes {
        return Err(ManifestEvidenceError::Verification(
            manifest_row_id,
            "stored evidence differs from the serialized manifest".to_string(),
        ));
    }
    let facts = extract_facts(manifest);
    let revision: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(revision), 0) + 1
           FROM release_work_item_manifest_evidence WHERE manifest_row_id = $1",
    )
    .bind(manifest_row_id)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO release_work_item_manifest_evidence (
            manifest_row_id, asset_key, storage_kind, content_sha256, content_length,
            serialization_version, revision, source, created_at, verified_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)
         ON CONFLICT (manifest_row_id) DO UPDATE
         SET asset_key = EXCLUDED.asset_key,
             storage_kind = EXCLUDED.storage_kind,
             content_sha256 = EXCLUDED.content_sha256,
             content_length = EXCLUDED.content_length,
             serialization_version = EXCLUDED.serialization_version,
             revision = EXCLUDED.revision,
             source = EXCLUDED.source,
             verified_at = EXCLUDED.verified_at
         WHERE release_work_item_manifest_evidence.content_sha256
               IS DISTINCT FROM EXCLUDED.content_sha256",
    )
    .bind(manifest_row_id)
    .bind(&receipt.key)
    .bind(receipt.storage_kind)
    .bind(&receipt.sha256)
    .bind(receipt.length)
    .bind(EVIDENCE_SERIALIZATION_VERSION)
    .bind(revision)
    .bind(source)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO release_work_item_manifest_evidence_revision (
            manifest_row_id, revision, asset_key, storage_kind, content_sha256,
            content_length, source, created_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (manifest_row_id, revision) DO NOTHING",
    )
    .bind(manifest_row_id)
    .bind(revision)
    .bind(&receipt.key)
    .bind(receipt.storage_kind)
    .bind(&receipt.sha256)
    .bind(receipt.length)
    .bind(source)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    store_facts(conn, manifest_row_id, &facts, now).await?;
    // A durable per-row receipt is written by every producer, so the
    // independent check and a bare server start can prove each reference
    // without rereading history.
    sqlx::query(
        "INSERT INTO manifest_evidence_migration_receipts (
            manifest_row_id, content_sha256, content_length, derived_facts_hash, migrated_at
         ) VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (manifest_row_id) DO UPDATE
         SET content_sha256 = EXCLUDED.content_sha256,
             content_length = EXCLUDED.content_length,
             derived_facts_hash = EXCLUDED.derived_facts_hash,
             migrated_at = EXCLUDED.migrated_at",
    )
    .bind(manifest_row_id)
    .bind(&receipt.sha256)
    .bind(receipt.length)
    .bind(facts_digest(&facts))
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(receipt)
}

async fn store_facts(
    conn: &mut PgConnection,
    manifest_row_id: i64,
    facts: &ManifestFacts,
    now: OffsetDateTime,
) -> Result<(), ManifestEvidenceError> {
    sqlx::query(
        "INSERT INTO release_work_item_manifest_facts (
            manifest_row_id, release_title, source_path, has_corrupt_media,
            structured_traits, embedded_traits, lyrics_present, updated_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (manifest_row_id) DO UPDATE
         SET release_title = EXCLUDED.release_title,
             source_path = EXCLUDED.source_path,
             has_corrupt_media = EXCLUDED.has_corrupt_media,
             structured_traits = EXCLUDED.structured_traits,
             embedded_traits = EXCLUDED.embedded_traits,
             lyrics_present = EXCLUDED.lyrics_present,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(manifest_row_id)
    .bind(&facts.release_title)
    .bind(&facts.source_path)
    .bind(facts.has_corrupt_media)
    .bind(&facts.structured_traits)
    .bind(&facts.embedded_traits)
    .bind(facts.lyrics_present)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM release_work_item_manifest_track_facts WHERE manifest_row_id=$1")
        .bind(manifest_row_id)
        .execute(&mut *conn)
        .await?;
    for track in &facts.tracks {
        sqlx::query(
            "INSERT INTO release_work_item_manifest_track_facts (
                manifest_row_id, relative_path, bit_depth, sample_rate_hz, channels,
                fingerprint_digest, fingerprint_length, corrupt, genre,
                source_ordinal, observed_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(manifest_row_id)
        .bind(&track.relative_path)
        .bind(track.bit_depth)
        .bind(track.sample_rate_hz)
        .bind(track.channels)
        .bind(&track.fingerprint_digest)
        .bind(track.fingerprint_length)
        .bind(track.corrupt)
        .bind(&track.genre)
        .bind(track.source_ordinal)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Reads the full manifest for one row: the inline payload before upgrade, or
/// the verified evidence file afterwards.
///
/// Only authorized lifecycle and detail paths call this. Browse, search,
/// policy batches and Admin listings read the compact facts instead.
pub async fn load_manifest(
    pool: &PgPool,
    manifest_row_id: i64,
) -> Result<Value, ManifestEvidenceError> {
    let row: Option<(Option<Value>,)> =
        sqlx::query_as("SELECT manifest_payload FROM release_work_item_manifests WHERE id = $1")
            .bind(manifest_row_id)
            .fetch_optional(pool)
            .await?;
    let Some((payload,)) = row else {
        return Err(ManifestEvidenceError::Missing(manifest_row_id));
    };
    if let Some(payload) = payload {
        return Ok(payload);
    }
    let evidence: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT asset_key, storage_kind, content_sha256, content_length
           FROM release_work_item_manifest_evidence WHERE manifest_row_id = $1",
    )
    .bind(manifest_row_id)
    .fetch_optional(pool)
    .await?;
    let Some((key, kind, sha256, length)) = evidence else {
        return Err(ManifestEvidenceError::Missing(manifest_row_id));
    };
    let bytes = asset_files::read_evidence(key, kind, sha256, length).await?;
    serde_json::from_slice(&bytes).map_err(|_| ManifestEvidenceError::Serialization)
}

/// Migration progress for the operator interface.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceMigrationStatus {
    pub store_id: Option<Uuid>,
    pub started_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub manifests_total: i64,
    pub manifests_migrated: i64,
    pub bytes_migrated: i64,
    pub verification_errors: i64,
    pub last_manifest_row_id: i64,
    pub pending: i64,
    pub inline_remaining: i64,
    pub evidence_rows: i64,
    pub evidence_bytes: i64,
    pub filesystem_bytes: i64,
}

pub async fn status(pool: &PgPool) -> Result<EvidenceMigrationStatus, ManifestEvidenceError> {
    #[allow(clippy::type_complexity)]
    let row: (
        Option<Uuid>,
        Option<OffsetDateTime>,
        Option<OffsetDateTime>,
        Option<OffsetDateTime>,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT migration.store_id, migration.started_at, migration.updated_at,
                migration.completed_at,
                COALESCE(migration.manifests_total, 0),
                COALESCE(migration.manifests_migrated, 0),
                COALESCE(migration.bytes_migrated, 0),
                COALESCE(migration.verification_errors, 0),
                COALESCE(migration.last_manifest_row_id, 0),
                (SELECT count(*) FROM release_work_item_manifests
                  WHERE manifest_payload IS NOT NULL),
                (SELECT count(*) FROM release_work_item_manifests
                  WHERE manifest_payload IS NULL),
                (SELECT count(*) FROM release_work_item_manifest_evidence),
                (SELECT COALESCE(sum(content_length), 0)::bigint
                   FROM release_work_item_manifest_evidence),
                (SELECT COALESCE(sum(files.size_bytes), 0)::bigint
                   FROM asset_files files
                  WHERE files.asset_key IN (
                        SELECT asset_key FROM release_work_item_manifest_evidence
                  ))
           FROM (SELECT 1) singleton
           LEFT JOIN manifest_evidence_migration migration ON migration.singleton
          LIMIT 1",
    )
    .fetch_one(pool)
    .await?;
    Ok(EvidenceMigrationStatus {
        store_id: row.0,
        started_at: row.1,
        updated_at: row.2,
        completed_at: row.3,
        manifests_total: row.4,
        manifests_migrated: row.5,
        bytes_migrated: row.6,
        verification_errors: row.7,
        last_manifest_row_id: row.8,
        pending: row.9,
        inline_remaining: row.9,
        evidence_rows: row.11,
        evidence_bytes: row.12,
        filesystem_bytes: row.13,
    })
}

/// Backfills one bounded checkpoint of legacy inline payloads in stable id
/// order. Returns how many rows were converted.
pub async fn backfill_checkpoint(
    pool: &PgPool,
    max_rows: i64,
    now: OffsetDateTime,
) -> Result<i64, ManifestEvidenceError> {
    let max_rows = max_rows.clamp(1, MAX_MANIFESTS_PER_CHECKPOINT);
    let rows: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT manifest.id, manifest.manifest_payload
           FROM release_work_item_manifests manifest
           LEFT JOIN release_work_item_manifest_evidence evidence
             ON evidence.manifest_row_id = manifest.id
          WHERE manifest.manifest_payload IS NOT NULL
            AND evidence.manifest_row_id IS NULL
          ORDER BY manifest.id
          LIMIT $1",
    )
    .bind(max_rows)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let mut migrated = 0_i64;
    for (manifest_row_id, payload) in rows {
        let mut tx = pool.begin().await?;
        let receipt = store_evidence(&mut tx, manifest_row_id, &payload, "backfilled", now).await?;
        let store_id: Uuid =
            sqlx::query_scalar("SELECT store_id FROM asset_store_binding WHERE singleton")
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query(
            "INSERT INTO manifest_evidence_migration (
                singleton, store_id, started_at, updated_at
             ) VALUES (true, $1, $2, $2)
             ON CONFLICT (singleton) DO NOTHING",
        )
        .bind(store_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        // Revalidate the reconstructed source before recording progress: the
        // file must parse back to exactly the payload it was produced from.
        let reconstructed = load_manifest_on_connection(&mut tx, manifest_row_id).await?;
        if reconstructed != payload {
            return Err(ManifestEvidenceError::Verification(
                manifest_row_id,
                "reconstructed JSON differs from the inline payload".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO manifest_evidence_migration_receipts (
                manifest_row_id, content_sha256, content_length, derived_facts_hash,
                migrated_at
             ) VALUES ($1,$2,$3,$4,$5)
             ON CONFLICT (manifest_row_id) DO NOTHING",
        )
        .bind(manifest_row_id)
        .bind(&receipt.sha256)
        .bind(receipt.length)
        .bind(facts_digest(&extract_facts(&payload)))
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE manifest_evidence_migration
                SET updated_at = $1,
                    manifests_migrated = manifests_migrated + 1,
                    bytes_migrated = bytes_migrated + $2,
                    last_manifest_row_id = GREATEST(last_manifest_row_id, $3)
              WHERE singleton",
        )
        .bind(now)
        .bind(receipt.length)
        .bind(manifest_row_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        migrated += 1;
    }
    Ok(migrated)
}

pub async fn load_manifest_on_connection(
    conn: &mut PgConnection,
    manifest_row_id: i64,
) -> Result<Value, ManifestEvidenceError> {
    let row: Option<(Option<Value>,)> =
        sqlx::query_as("SELECT manifest_payload FROM release_work_item_manifests WHERE id = $1")
            .bind(manifest_row_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((payload,)) = row else {
        return Err(ManifestEvidenceError::Missing(manifest_row_id));
    };
    if let Some(payload) = payload {
        return Ok(payload);
    }
    let evidence: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT asset_key, storage_kind, content_sha256, content_length
           FROM release_work_item_manifest_evidence WHERE manifest_row_id = $1",
    )
    .bind(manifest_row_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((key, kind, sha256, length)) = evidence else {
        return Err(ManifestEvidenceError::Missing(manifest_row_id));
    };
    let bytes = asset_files::read_evidence(key, kind, sha256, length).await?;
    serde_json::from_slice(&bytes).map_err(|_| ManifestEvidenceError::Serialization)
}

fn facts_digest(facts: &ManifestFacts) -> String {
    let serialized = serde_json::to_vec(&serde_json::json!({
        "releaseTitle": facts.release_title,
        "sourcePath": facts.source_path,
        "hasCorruptMedia": facts.has_corrupt_media,
        "structuredTraits": facts.structured_traits,
        "embeddedTraits": facts.embedded_traits,
        "lyricsPresent": facts.lyrics_present,
        "tracks": facts.tracks.iter().map(|track| serde_json::json!({
            "relativePath": track.relative_path,
            "bitDepth": track.bit_depth,
            "sampleRateHz": track.sample_rate_hz,
            "channels": track.channels,
            "fingerprintDigest": track.fingerprint_digest,
            "fingerprintLength": track.fingerprint_length,
            "corrupt": track.corrupt,
            "genre": track.genre,
            "sourceOrdinal": track.source_ordinal,
        })).collect::<Vec<_>>(),
    }))
    .unwrap_or_default();
    blake3::hash(&serialized).to_hex().to_string()
}

/// Removes inline evidence only after an independent reconstruction check.
///
/// Fenced by the manifest hash and the presence of verified evidence, so a
/// concurrent writer that changed the row is left for the next pass.
pub async fn finalize(pool: &PgPool, now: OffsetDateTime) -> Result<i64, ManifestEvidenceError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT manifest.id, manifest.manifest_hash
           FROM release_work_item_manifests manifest
           JOIN release_work_item_manifest_evidence evidence
             ON evidence.manifest_row_id = manifest.id
          WHERE manifest.manifest_payload IS NOT NULL
          ORDER BY manifest.id",
    )
    .fetch_all(pool)
    .await?;
    let mut finalized = 0_i64;
    for (manifest_row_id, manifest_hash) in rows {
        let mut tx = pool.begin().await?;
        let stored: Option<(Option<Value>, String)> = sqlx::query_as(
            "SELECT manifest_payload, manifest_hash
               FROM release_work_item_manifests WHERE id = $1 FOR UPDATE",
        )
        .bind(manifest_row_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((Some(payload), current_hash)) = stored else {
            tx.rollback().await?;
            continue;
        };
        if current_hash != manifest_hash {
            tx.rollback().await?;
            continue;
        }
        let reconstructed = load_manifest_on_connection(&mut tx, manifest_row_id).await?;
        if reconstructed != payload {
            return Err(ManifestEvidenceError::Verification(
                manifest_row_id,
                "evidence differs from the inline payload; refusing to remove it".to_string(),
            ));
        }
        let cleared = sqlx::query(
            "UPDATE release_work_item_manifests
                SET manifest_payload = NULL
              WHERE id = $1 AND manifest_hash = $2 AND manifest_payload IS NOT NULL",
        )
        .bind(manifest_row_id)
        .bind(&manifest_hash)
        .execute(&mut *tx)
        .await?;
        if cleared.rows_affected() == 1 {
            finalized += 1;
        }
        tx.commit().await?;
    }
    if finalized > 0 {
        sqlx::query(
            "INSERT INTO manifest_evidence_migration (
                singleton, store_id, started_at, updated_at, completed_at
             ) VALUES (true, COALESCE((SELECT store_id FROM asset_store_binding WHERE singleton),
                                      uuidv7()), $1, $1, $1)
             ON CONFLICT (singleton) DO UPDATE
             SET updated_at = EXCLUDED.updated_at,
                 completed_at = EXCLUDED.completed_at",
        )
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(finalized)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceVerificationReport {
    pub manifests_checked: i64,
    pub files_verified: i64,
    pub bytes_verified: i64,
    pub inline_remaining: i64,
    pub discrepancies: Vec<String>,
}

impl EvidenceVerificationReport {
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.discrepancies.is_empty()
    }
}

struct EvidenceRow {
    manifest_row_id: i64,
    payload: Option<Value>,
    asset_key: String,
    storage_kind: String,
    content_sha256: String,
    content_length: i64,
    receipt_sha256: Option<String>,
    receipt_length: Option<i64>,
    has_corrupt_media: bool,
    structured_traits: Vec<String>,
    embedded_traits: Vec<String>,
    release_title: Option<String>,
    source_path: Option<String>,
}

/// Independent verification over every manifest row with evidence.
///
/// For each row: the file is read from its final path and checked against the
/// recorded SHA-256 and length; the reconstructed JSON is compared with the
/// inline payload while it still exists; and the recorded query facts are
/// compared with facts recomputed from the file. A row with an inline payload
/// but no evidence, or evidence with no receipt, is a discrepancy.
pub async fn verify_all(
    pool: &PgPool,
    max_discrepancies: usize,
) -> Result<EvidenceVerificationReport, ManifestEvidenceError> {
    let mut report = EvidenceVerificationReport::default();
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        i64,
        Option<Value>,
        String,
        String,
        String,
        i64,
        Option<String>,
        Option<i64>,
        bool,
        Vec<String>,
        Vec<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT manifest.id, manifest.manifest_payload,
                evidence.asset_key, evidence.storage_kind,
                evidence.content_sha256, evidence.content_length,
                receipt.content_sha256, receipt.content_length,
                facts.has_corrupt_media, facts.structured_traits, facts.embedded_traits,
                facts.release_title, facts.source_path
           FROM release_work_item_manifests manifest
           JOIN release_work_item_manifest_evidence evidence
             ON evidence.manifest_row_id = manifest.id
           JOIN release_work_item_manifest_facts facts
             ON facts.manifest_row_id = manifest.id
           LEFT JOIN manifest_evidence_migration_receipts receipt
             ON receipt.manifest_row_id = manifest.id
          ORDER BY manifest.id",
    )
    .fetch_all(pool)
    .await?;
    let evidence = rows
        .into_iter()
        .map(
            |(
                manifest_row_id,
                payload,
                asset_key,
                storage_kind,
                content_sha256,
                content_length,
                receipt_sha256,
                receipt_length,
                has_corrupt_media,
                structured_traits,
                embedded_traits,
                release_title,
                source_path,
            )| EvidenceRow {
                manifest_row_id,
                payload,
                asset_key,
                storage_kind,
                content_sha256,
                content_length,
                receipt_sha256,
                receipt_length,
                has_corrupt_media,
                structured_traits,
                embedded_traits,
                release_title,
                source_path,
            },
        )
        .collect::<Vec<_>>();

    let total_manifests: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_work_item_manifests")
            .fetch_one(pool)
            .await?;
    let inline_remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_manifests WHERE manifest_payload IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    report.inline_remaining = inline_remaining;
    let missing = total_manifests - i64::try_from(evidence.len()).unwrap_or(0);
    if missing > 0 {
        report.discrepancies.push(format!(
            "{missing} manifest rows have no verified evidence reference and facts"
        ));
    }

    for row in evidence {
        report.manifests_checked += 1;
        if row.receipt_sha256.as_deref() != Some(row.content_sha256.as_str())
            || row.receipt_length != Some(row.content_length)
        {
            report.discrepancies.push(format!(
                "manifest {} receipt does not match its evidence reference",
                row.manifest_row_id
            ));
            if report.discrepancies.len() >= max_discrepancies {
                break;
            }
            continue;
        }
        let reconstructed = asset_files::read_evidence(
            row.asset_key.clone(),
            row.storage_kind.clone(),
            row.content_sha256.clone(),
            row.content_length,
        )
        .await;
        let reconstructed = match reconstructed {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    report.discrepancies.push(format!(
                        "manifest {} evidence is not valid JSON",
                        row.manifest_row_id
                    ));
                    continue;
                }
            },
            Err(error) => {
                report.discrepancies.push(format!(
                    "manifest {} evidence could not be read: {error}",
                    row.manifest_row_id
                ));
                continue;
            }
        };
        report.files_verified += 1;
        report.bytes_verified += row.content_length;
        if let Some(payload) = row.payload.as_ref()
            && payload != &reconstructed
        {
            report.discrepancies.push(format!(
                "manifest {} reconstructed JSON differs from its inline payload",
                row.manifest_row_id
            ));
            continue;
        }
        let facts = extract_facts(&reconstructed);
        if facts.has_corrupt_media != row.has_corrupt_media
            || facts.structured_traits != row.structured_traits
            || facts.embedded_traits != row.embedded_traits
            || facts.release_title != row.release_title
            || facts.source_path != row.source_path
        {
            report.discrepancies.push(format!(
                "manifest {} query facts differ from its evidence file",
                row.manifest_row_id
            ));
        }
        if report.discrepancies.len() >= max_discrepancies {
            report
                .discrepancies
                .push("verification stopped after reaching the discrepancy bound".to_string());
            break;
        }
    }
    Ok(report)
}

/// Runs the complete expand → backfill → verify → finalize sequence.
///
/// Resumable: progress is checkpointed per row, the independent check runs
/// before any inline evidence is removed, and a repeat run over a finalized
/// installation performs no bulk work.
pub async fn upgrade(
    pool: &PgPool,
    now: OffsetDateTime,
) -> Result<EvidenceVerificationReport, ManifestEvidenceError> {
    loop {
        let migrated = backfill_checkpoint(pool, MAX_MANIFESTS_PER_CHECKPOINT, now).await?;
        if migrated == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    // Independent check before finalization: any discrepancy leaves the
    // recoverable inline source in place.
    let report = verify_all(pool, 100).await?;
    if !report.is_clean() {
        return Ok(report);
    }
    finalize(pool, now).await?;
    Ok(report)
}

/// Cheap gate used by a bare server start: no inline payload may remain and
/// every manifest must have a verified reference and receipt.
pub async fn require_complete(pool: &PgPool) -> Result<(), ManifestEvidenceError> {
    let inline: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_manifests WHERE manifest_payload IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    let missing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_manifests manifest
          LEFT JOIN release_work_item_manifest_evidence evidence
            ON evidence.manifest_row_id = manifest.id
          LEFT JOIN manifest_evidence_migration_receipts receipt
            ON receipt.manifest_row_id = manifest.id
         WHERE evidence.manifest_row_id IS NULL OR receipt.manifest_row_id IS NULL",
    )
    .fetch_one(pool)
    .await?;
    if inline > 0 || missing > 0 {
        return Err(ManifestEvidenceError::Verification(
            0,
            format!(
                "manifest evidence upgrade is incomplete: {inline} inline rows, \
                 {missing} rows without a verified receipt"
            ),
        ));
    }
    Ok(())
}

/// Filesystem bytes held by manifest evidence revisions that are no longer
/// the current reference. The store retains them; this is the reported cost.
pub async fn superseded_evidence_bytes(pool: &PgPool) -> Result<i64, ManifestEvidenceError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(sum(files.size_bytes), 0)::bigint
           FROM asset_files files
          WHERE files.asset_key IN (
                SELECT revision.asset_key
                  FROM release_work_item_manifest_evidence_revision revision
                 WHERE NOT EXISTS (
                       SELECT 1 FROM release_work_item_manifest_evidence current
                        WHERE current.asset_key = revision.asset_key
                 )
          )",
    )
    .fetch_one(pool)
    .await?)
}

/// Serializes a manifest for hashing without writing it (used by review
/// corrections to record the expected revision).
pub fn manifest_revision_hash(manifest: &Value) -> String {
    let bytes = serialize_manifest(manifest).unwrap_or_default();
    crate::migrate::checksum_hex(&blake3::hash(&bytes).as_bytes()[..])
}

/// Helper used by writers that already hold a transaction.
pub async fn store_evidence_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    manifest_row_id: i64,
    manifest: &Value,
    now: OffsetDateTime,
) -> Result<StoredEvidence, ManifestEvidenceError> {
    store_evidence(&mut *tx, manifest_row_id, manifest, "written", now).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn facts_extract_traits_fingerprints_and_corruption() {
        let manifest = json!({
            "source_path": "Artist/Release",
            "release_observation": { "title": "Example" },
            "files": [
                {
                    "relative_path": "Artist/Release/01.flac",
                    "bytes_read_errors": true,
                    "audio": { "bit_depth": 24, "sample_rate_hz": "44100", "channels": 2 },
                    "metadata": { "raw": { "FLAGCOMPILATION": "1" } }
                },
                {
                    "relative_path": "Artist/Release/02.flac",
                    "audio": { "bit_depth": 16 },
                    "metadata": { "raw": { "MusicBrainzReleaseType": "EP; Live" } }
                }
            ],
            "normalization": {
                "fingerprints": [
                    { "source_file": "Artist/Release/01.flac", "fingerprint": "  AQADtEmS  " }
                ],
                "parsed_sidecars": [
                    { "sidecar_type": "lrc", "relative_path": "Artist/Release/01.lrc",
                      "parsed": { "value": "[00:01]line" } }
                ]
            }
        });
        let facts = extract_facts(&manifest);
        assert_eq!(facts.release_title.as_deref(), Some("Example"));
        assert!(facts.has_corrupt_media);
        assert!(facts.lyrics_present);
        assert_eq!(facts.structured_traits, vec!["compilation"]);
        assert_eq!(facts.embedded_traits, vec!["ep", "live"]);
        assert_eq!(facts.tracks.len(), 2);
        assert_eq!(facts.tracks[0].bit_depth, Some(24));
        assert_eq!(facts.tracks[0].sample_rate_hz, None);
        assert_eq!(
            facts.tracks[0].fingerprint_digest.as_deref(),
            Some(melodee_domain::acoustic_fingerprint_digest("AQADtEmS").as_str())
        );
        assert_eq!(facts.tracks[0].fingerprint_length, Some(8));
        assert!(facts.tracks[0].corrupt);
        assert!(!facts.tracks[1].corrupt);
    }
}

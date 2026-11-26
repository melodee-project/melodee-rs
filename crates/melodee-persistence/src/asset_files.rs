//! Verified immutable files for catalog artwork, avatars, playlists and themes (ADR-0210).
use rustix::fs::{Mode, OFlags, open};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use uuid::Uuid;

pub const ENV_ASSET_ROOT: &str = "MELODEE__STORAGE__ASSET_ROOT";
/// Largest single stored object. Upload limits for artwork, avatars,
/// playlists and themes are unchanged; larger evidence is chunked (ADR-0213).
const MAX_BYTES: usize = 10 * 1024 * 1024;
/// Fixed evidence chunk size for objects above `MAX_BYTES`.
pub const EVIDENCE_CHUNK_BYTES: usize = 4 * 1024 * 1024;
/// Refuse absurd reconstructions before allocating; a manifest is metadata,
/// not media.
pub const MAX_EVIDENCE_BYTES: usize = 512 * 1024 * 1024;
const CHUNK_DESCRIPTOR_FORMAT: &str = "melodee-chunked-evidence";
const MARKER: &str = ".melodee-asset-store";

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn root() -> io::Result<PathBuf> {
    if let Some(value) = std::env::var_os(ENV_ASSET_ROOT) {
        let path = PathBuf::from(value);
        if !path.is_absolute() || !path.is_dir() || path.canonicalize()? != path {
            return Err(invalid(
                "configured asset root must be an existing absolute directory",
            ));
        }
        return Ok(path);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| invalid("configure MELODEE__STORAGE__ASSET_ROOT for this service"))?;
    let path = base.join("melodee/assets");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

// Hard-link publication is atomic and never replaces a concurrent writer.
// The temp file is on the same filesystem and drops even on a failed publish.
fn publish(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing asset directory"))?;
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    match fs::hard_link(temp.path(), path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    sync_dir(parent)
}

fn store_id(root: &Path, initialize: bool) -> io::Result<Uuid> {
    let marker = root.join(MARKER);
    if initialize && !marker.try_exists()? {
        publish(&marker, Uuid::now_v7().to_string().as_bytes())?;
    }
    let mut id = String::new();
    open_file(&marker)?.take(40).read_to_string(&mut id)?;
    Uuid::parse_str(id.trim()).map_err(|_| invalid("invalid asset store marker"))
}

fn key_for(bytes: &[u8]) -> String {
    format!(
        "{}-{}",
        crate::migrate::checksum_hex(&Sha256::digest(bytes)),
        bytes.len()
    )
}

fn object_path(root: &Path, key: &str) -> io::Result<(PathBuf, usize)> {
    let (digest, len) = key
        .split_once('-')
        .ok_or_else(|| invalid("invalid asset key"))?;
    let size = len
        .parse::<usize>()
        .map_err(|_| invalid("invalid asset length"))?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || !(1..=MAX_BYTES).contains(&size)
        || len != size.to_string()
    {
        return Err(invalid("invalid asset reference"));
    }
    Ok((root.join(&digest[..2]).join(key), size))
}

fn open_file(path: &Path) -> io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let file = File::from(open(path, flags, Mode::empty())?);
    if !file.metadata()?.is_file() {
        return Err(invalid("expected a regular asset file"));
    }
    Ok(file)
}

fn read_verified(root: &Path, key: &str) -> io::Result<Vec<u8>> {
    let (path, size) = object_path(root, key)?;
    if path.parent().is_none_or(|shard| shard.is_symlink()) {
        return Err(invalid("invalid asset shard"));
    }
    let file = open_file(&path)?;
    if file.metadata()?.len() != size as u64 {
        return Err(invalid("asset size differs from stored reference"));
    }
    let mut bytes = Vec::with_capacity(size);
    file.take(size as u64 + 1).read_to_end(&mut bytes)?;
    if key_for(&bytes) != key {
        return Err(invalid("asset checksum differs from stored reference"));
    }
    Ok(bytes)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, sqlx::Error> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(io::Error::other)?
        .map_err(Into::into)
}

/// Reads and verifies a reference selected by the caller's authorized database query.
pub async fn read(key: String) -> Result<Vec<u8>, sqlx::Error> {
    blocking(move || read_verified(&root()?, &key)).await
}

pub async fn read_optional(key: Option<String>) -> Result<Option<Vec<u8>>, sqlx::Error> {
    match key {
        Some(key) => read(key).await.map(Some),
        None => Ok(None),
    }
}

/// Writes an immutable file and registers its verified size and digest.
pub async fn put(pool: &PgPool, bytes: &[u8]) -> Result<String, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    put_on(&mut conn, bytes).await
}

/// Uses the caller's connection, including single-connection migration/import transactions.
pub async fn put_on(conn: &mut PgConnection, bytes: &[u8]) -> Result<String, sqlx::Error> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(invalid("asset exceeds file storage bounds").into());
    }
    let bound: Option<Uuid> =
        sqlx::query_scalar("SELECT store_id FROM asset_store_binding WHERE singleton")
            .fetch_optional(&mut *conn)
            .await?;
    let (root_path, marker) = blocking(move || {
        let path = root()?;
        let id = store_id(&path, bound.is_none())?;
        Ok((path, id))
    })
    .await?;
    let id = match bound {
        Some(id) => id,
        None => {
            sqlx::query(
                "INSERT INTO asset_store_binding(store_id) VALUES ($1) ON CONFLICT DO NOTHING",
            )
            .bind(marker)
            .execute(&mut *conn)
            .await?;
            sqlx::query_scalar("SELECT store_id FROM asset_store_binding WHERE singleton")
                .fetch_one(&mut *conn)
                .await?
        }
    };
    if id != marker {
        return Err(invalid("asset root does not match the database's registered store").into());
    }
    let size = bytes.len() as i32;
    let data = bytes.to_vec();
    let key = blocking(move || {
        let key = key_for(&data);
        let (path, _) = object_path(&root_path, &key)?;
        let shard = path
            .parent()
            .ok_or_else(|| invalid("missing asset shard"))?;
        if shard.is_symlink() {
            return Err(invalid("invalid asset shard"));
        }
        if !shard.is_dir() {
            fs::create_dir_all(shard)?;
            sync_dir(&root_path)?;
        }
        if !path.try_exists()? {
            publish(&path, &data)?;
        }
        read_verified(&root_path, &key)?;
        Ok(key)
    })
    .await?;
    sqlx::query(
        "INSERT INTO asset_files(asset_key,size_bytes) VALUES($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(&key)
    .bind(size)
    .execute(conn)
    .await?;
    Ok(key)
}

/// One verified evidence object: a single file, or a chunked descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvidence {
    /// Content-addressed key of the object that must be read.
    pub key: String,
    /// `object` for one file, `chunked` for a descriptor over 4 MiB parts.
    pub storage_kind: &'static str,
    /// Lowercase hex SHA-256 of the reconstructed content.
    pub sha256: String,
    /// Reconstructed content length in bytes.
    pub length: i64,
    /// Number of chunk objects (0 for a single object).
    pub chunk_count: i64,
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::migrate::checksum_hex(&Sha256::digest(bytes))
}

fn chunk_descriptor(chunks: &[String], sha256: &str, length: usize) -> Vec<u8> {
    // Versioned, deterministic serialization: readers reject unknown versions
    // and any length/chunk-count mismatch instead of guessing.
    let descriptor = serde_json::json!({
        "format": CHUNK_DESCRIPTOR_FORMAT,
        "version": 1,
        "chunkBytes": EVIDENCE_CHUNK_BYTES,
        "chunks": chunks,
        "sha256": sha256,
        "length": length,
    });
    serde_json::to_vec(&descriptor).unwrap_or_default()
}

/// Writes an immutable evidence object of any supported size.
///
/// Objects up to 10 MiB use the ordinary store path. Larger evidence is
/// streamed in fixed 4 MiB parts and represented by a versioned descriptor;
/// every part is registered in `asset_files`, so a shared part survives the
/// deletion of one owner. A retry reuses the existing immutable objects.
pub async fn put_evidence(pool: &PgPool, bytes: &[u8]) -> Result<StoredEvidence, sqlx::Error> {
    if bytes.is_empty() {
        return Err(invalid("evidence is empty").into());
    }
    if bytes.len() > MAX_EVIDENCE_BYTES {
        return Err(invalid("evidence exceeds the supported maximum").into());
    }
    let sha256 = sha256_hex(bytes);
    let length =
        i64::try_from(bytes.len()).map_err(|_| invalid("evidence length is not representable"))?;
    if bytes.len() <= MAX_BYTES {
        let key = put(pool, bytes).await?;
        return Ok(StoredEvidence {
            key,
            storage_kind: "object",
            sha256,
            length,
            chunk_count: 0,
        });
    }
    let mut chunks = Vec::with_capacity(bytes.len().div_ceil(EVIDENCE_CHUNK_BYTES));
    let mut chunk_count = 0_i64;
    for chunk in bytes.chunks(EVIDENCE_CHUNK_BYTES) {
        chunks.push(put(pool, chunk).await?);
        chunk_count += 1;
    }
    let descriptor = chunk_descriptor(&chunks, &sha256, bytes.len());
    let key = put(pool, &descriptor).await?;
    Ok(StoredEvidence {
        key,
        storage_kind: "chunked",
        sha256,
        length,
        chunk_count,
    })
}

/// Writes evidence using the caller's connection so a lifecycle transaction
/// can bind the reference atomically with its owner.
pub async fn put_evidence_on(
    conn: &mut PgConnection,
    bytes: &[u8],
) -> Result<StoredEvidence, sqlx::Error> {
    if bytes.is_empty() {
        return Err(invalid("evidence is empty").into());
    }
    if bytes.len() > MAX_EVIDENCE_BYTES {
        return Err(invalid("evidence exceeds the supported maximum").into());
    }
    let sha256 = sha256_hex(bytes);
    let length =
        i64::try_from(bytes.len()).map_err(|_| invalid("evidence length is not representable"))?;
    if bytes.len() <= MAX_BYTES {
        let key = put_on(conn, bytes).await?;
        return Ok(StoredEvidence {
            key,
            storage_kind: "object",
            sha256,
            length,
            chunk_count: 0,
        });
    }
    let mut chunks = Vec::with_capacity(bytes.len().div_ceil(EVIDENCE_CHUNK_BYTES));
    let mut chunk_count = 0_i64;
    for chunk in bytes.chunks(EVIDENCE_CHUNK_BYTES) {
        chunks.push(put_on(&mut *conn, chunk).await?);
        chunk_count += 1;
    }
    let descriptor = chunk_descriptor(&chunks, &sha256, bytes.len());
    let key = put_on(conn, &descriptor).await?;
    Ok(StoredEvidence {
        key,
        storage_kind: "chunked",
        sha256,
        length,
        chunk_count,
    })
}

/// The database's registered store identity, if it has one.
pub async fn bound_store_id(pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT store_id FROM asset_store_binding WHERE singleton")
        .fetch_optional(pool)
        .await
}

/// Filesystem bytes held by the store that no owner references.
///
/// The store never deletes superseded objects on its own (ADR-0210), so the
/// operator interface reports their cost instead of silently reclaiming it.
pub async fn unreferenced_store_bytes(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(files.size_bytes), 0)::bigint
           FROM asset_files files
          WHERE NOT EXISTS (SELECT 1 FROM catalog_provider_images WHERE asset_key = files.asset_key)
            AND NOT EXISTS (SELECT 1 FROM catalog_entity_images WHERE asset_key = files.asset_key)
            AND NOT EXISTS (SELECT 1 FROM native_user_avatars WHERE asset_key = files.asset_key)
            AND NOT EXISTS (SELECT 1 FROM native_theme_assets WHERE asset_key = files.asset_key)
            AND NOT EXISTS (SELECT 1 FROM native_playlists WHERE asset_key = files.asset_key)
            AND NOT EXISTS (SELECT 1 FROM native_users WHERE asset_key = files.asset_key)
            AND NOT EXISTS (SELECT 1 FROM release_work_item_manifest_evidence
                             WHERE asset_key = files.asset_key)
            AND NOT EXISTS (SELECT 1 FROM release_work_item_manifest_evidence_revision
                             WHERE asset_key = files.asset_key)",
    )
    .fetch_one(pool)
    .await
}

/// Reads and verifies a stored evidence object.
///
/// Reconstruction verifies the descriptor version, every part digest and
/// length, then the overall SHA-256 and length. A truncated, reordered or
/// substituted part fails closed.
pub async fn read_evidence(
    key: String,
    storage_kind: String,
    expected_sha256: String,
    expected_length: i64,
) -> Result<Vec<u8>, sqlx::Error> {
    let bytes = blocking(move || read_evidence_blocking(&key, &storage_kind)).await?;
    if bytes.len() as i64 != expected_length || sha256_hex(&bytes) != expected_sha256 {
        return Err(invalid("evidence content differs from its recorded receipt").into());
    }
    Ok(bytes)
}

fn read_evidence_blocking(key: &str, storage_kind: &str) -> io::Result<Vec<u8>> {
    let root_path = root()?;
    match storage_kind {
        "object" => read_verified(&root_path, key),
        "chunked" => {
            let descriptor = read_verified(&root_path, key)?;
            if descriptor.len() > 1024 * 1024 {
                return Err(invalid("evidence descriptor is oversized"));
            }
            let parsed: serde_json::Value = serde_json::from_slice(&descriptor)
                .map_err(|_| invalid("evidence descriptor is not valid JSON"))?;
            if parsed.get("format").and_then(serde_json::Value::as_str)
                != Some(CHUNK_DESCRIPTOR_FORMAT)
                || parsed.get("version").and_then(serde_json::Value::as_u64) != Some(1)
                || parsed.get("chunkBytes").and_then(serde_json::Value::as_u64)
                    != Some(EVIDENCE_CHUNK_BYTES as u64)
            {
                return Err(invalid("unsupported evidence descriptor"));
            }
            let length = parsed
                .get("length")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| invalid("evidence descriptor has no length"))?;
            let length = usize::try_from(length)
                .map_err(|_| invalid("evidence descriptor length is out of range"))?;
            if length == 0 || length > MAX_EVIDENCE_BYTES {
                return Err(invalid("evidence descriptor length is out of range"));
            }
            let expected = parsed
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("evidence descriptor has no digest"))?;
            let chunks = parsed
                .get("chunks")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| invalid("evidence descriptor has no chunks"))?;
            let expected_chunks = length.div_ceil(EVIDENCE_CHUNK_BYTES);
            if chunks.len() != expected_chunks {
                return Err(invalid("evidence descriptor chunk count is inconsistent"));
            }
            let mut bytes = Vec::with_capacity(length);
            for chunk in chunks {
                let key = chunk
                    .as_str()
                    .ok_or_else(|| invalid("evidence descriptor chunk is not a key"))?;
                bytes.extend_from_slice(&read_verified(&root_path, key)?);
                if bytes.len() > length {
                    return Err(invalid("evidence reconstruction exceeded its length"));
                }
            }
            if bytes.len() != length || sha256_hex(&bytes) != expected {
                return Err(invalid("evidence reconstruction failed verification"));
            }
            Ok(bytes)
        }
        _ => Err(invalid("unknown evidence storage kind")),
    }
}

/// Removes one content-addressed file whose database references have already
/// been deleted. The key is validated before any path is constructed, the
/// shard must not be a symlink, and a missing object is idempotent success.
pub async fn remove(key: String) -> Result<(), sqlx::Error> {
    blocking(move || {
        let root_path = root()?;
        let (path, _) = object_path(&root_path, &key)?;
        if path.parent().is_none_or(|shard| shard.is_symlink()) {
            return Err(invalid("invalid asset shard"));
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    })
    .await
}

/// Fail startup before allowing work when a previously bound volume is missing or replaced.
pub(crate) async fn check_binding(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    if let Some(id) =
        sqlx::query_scalar::<_, Uuid>("SELECT store_id FROM asset_store_binding WHERE singleton")
            .fetch_optional(conn)
            .await?
    {
        blocking(move || {
            if store_id(&root()?, false)? != id {
                return Err(invalid(
                    "configured asset root belongs to a different store",
                ));
            }
            Ok(())
        })
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn corrupt_file_and_unsafe_keys_are_rejected_and_existing_objects_cannot_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_for(b"first");
        let (path, _) = object_path(dir.path(), &key).unwrap();
        fs::create_dir(path.parent().unwrap()).unwrap();
        publish(&path, b"first").unwrap();
        publish(&path, b"other").unwrap();
        assert_eq!(read_verified(dir.path(), &key).unwrap(), b"first");
        fs::write(&path, b"other").unwrap();
        assert!(read_verified(dir.path(), &key).is_err());
        for invalid in [
            "../malformed",
            &key.replace("-5", "-05"),
            &key.replace("-5", "-10485761"),
        ] {
            assert!(object_path(dir.path(), invalid).is_err());
        }
    }
}

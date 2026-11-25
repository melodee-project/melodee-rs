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
const MAX_BYTES: usize = 10 * 1024 * 1024;
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

//! Durable inbound-to-staging acquisition (ADR-0145).
//!
//! Inbound bytes are never rewritten. A Release is moved atomically when
//! possible; otherwise it is copied into a hidden staging directory, verified
//! with streamed BLAKE3 tree hashing, finalized without replacement, and only
//! then removed from inbound.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use melodee_domain::ReleaseWorkItem;
use melodee_persistence::{
    StagingHandoff, StagingHandoffMethod, StagingHandoffStatus, WorkItemStore, WorkerId,
};
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;

use crate::{
    config::{InboundSource, IngestionConfig},
    secure_fs::{create_directory_tree, open_directory, rename_noreplace},
    snapshot::snapshot_release_directory,
};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct OwnedSource {
    pub root: PathBuf,
    pub tree_hash: String,
}

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error("work item source is no longer configured")]
    SourceNotConfigured,
    #[error("invalid inbound candidate path: {0}")]
    InvalidSourcePath(String),
    #[error("inbound source changed during acquisition")]
    SourceChanged,
    #[error("staging receipt is missing or incomplete: {0}")]
    IncompleteReceipt(String),
    #[error("filesystem operation failed for {path:?}: {source}")]
    Filesystem { path: PathBuf, source: io::Error },
    #[error("persistence failed: {0}")]
    Persistence(#[from] melodee_persistence::WorkStoreError),
    #[error("task join failed: {0}")]
    Task(String),
}

fn io_error(path: &Path, source: io::Error) -> AcquisitionError {
    AcquisitionError::Filesystem {
        path: path.to_path_buf(),
        source,
    }
}

/// Writes durable intent then returns the one owned source root. No media
/// inspection happens until this succeeds.
pub async fn acquire_owned_source(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: &ReleaseWorkItem,
    expected_snapshot_signature: Option<&str>,
    now: OffsetDateTime,
) -> Result<OwnedSource, AcquisitionError> {
    let source = find_source(config, item).ok_or(AcquisitionError::SourceNotConfigured)?;
    let inbound = resolve_candidate(source, &item.source_path)?;
    let staging_root = config.staging_root.as_ref().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt(
            "MELODEE__INGEST__STAGING_ROOT is not configured".to_owned(),
        )
    })?;
    let existing_handoff = store.staging_handoff(pool, item.id).await?;
    let generation = match &existing_handoff {
        Some(handoff) => handoff.source_generation.clone(),
        None => source_generation(&inbound)?,
    };
    let relative = format!("releases/{}/source", item.id);
    let handoff = store
        .begin_staging_handoff(pool, item.id, worker, &relative, &generation, now)
        .await?;
    let destination = staging_root.join(&handoff.staging_relative_path);

    if matches!(
        handoff.status,
        StagingHandoffStatus::Staged | StagingHandoffStatus::InboundRemoved
    ) && destination.is_dir()
    {
        let tree_hash = verify_existing_receipt(&destination, &handoff)?;
        finish_inbound_cleanup(
            store,
            pool,
            worker,
            item.id,
            &inbound,
            &source.root,
            &handoff.source_generation,
            &tree_hash,
            now,
        )
        .await?;
        return Ok(OwnedSource {
            root: destination,
            tree_hash,
        });
    }

    if !matches!(handoff.status, StagingHandoffStatus::Intent) {
        return Err(AcquisitionError::IncompleteReceipt(format!(
            "{} is missing for completed handoff {}",
            destination.display(),
            item.id
        )));
    }

    // A final directory can exist after a crash between finalization and the
    // durable receipt. Adopt it only when the still-present inbox occurrence
    // proves the exact same tree; otherwise a deterministic-path conflict is
    // never overwritten or misattributed.
    if destination.is_dir() && inbound.exists() {
        let inbound_hash = tree_hash(&inbound)?;
        let staged_hash = tree_hash(&destination)?;
        if inbound_hash != staged_hash {
            return Err(AcquisitionError::IncompleteReceipt(format!(
                "existing final staging tree conflicts with inbound occurrence at {}",
                destination.display()
            )));
        }
    }

    let snapshot = snapshot_release_directory(&inbound, &config.ready_marker_name)
        .map_err(|error| AcquisitionError::IncompleteReceipt(error.to_string()))?;
    if expected_snapshot_signature.is_some_and(|expected| expected != snapshot.signature) {
        return Err(AcquisitionError::SourceChanged);
    }

    let inbound_for_task = inbound.clone();
    let destination_for_task = destination.clone();
    let staging_root_for_task = staging_root.clone();
    let work_item_id = item.id;
    let staged = tokio::task::spawn_blocking(move || {
        stage_source(
            &inbound_for_task,
            &destination_for_task,
            &staging_root_for_task,
            work_item_id,
        )
    })
    .await
    .map_err(|error| AcquisitionError::Task(error.to_string()))??;

    store
        .record_staging_receipt(pool, item.id, worker, staged.method, &staged.tree_hash, now)
        .await?;
    finish_inbound_cleanup(
        store,
        pool,
        worker,
        item.id,
        &inbound,
        &source.root,
        &handoff.source_generation,
        &staged.tree_hash,
        now,
    )
    .await?;
    Ok(OwnedSource {
        root: destination,
        tree_hash: staged.tree_hash,
    })
}

/// Resolves a completed acquisition only. Downstream stages never fall back
/// to the historic inbound path.
pub async fn owned_source_root(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    item: &ReleaseWorkItem,
) -> Result<OwnedSource, AcquisitionError> {
    let handoff = store.staging_handoff(pool, item.id).await?.ok_or_else(|| {
        AcquisitionError::IncompleteReceipt("no staging handoff intent".to_owned())
    })?;
    if !matches!(
        handoff.status,
        StagingHandoffStatus::Staged | StagingHandoffStatus::InboundRemoved
    ) {
        return Err(AcquisitionError::IncompleteReceipt(format!(
            "handoff status is {:?}",
            handoff.status
        )));
    }
    let staging_root = config.staging_root.as_ref().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt(
            "MELODEE__INGEST__STAGING_ROOT is not configured".to_owned(),
        )
    })?;
    let root = staging_root.join(&handoff.staging_relative_path);
    let tree_hash = verify_existing_receipt(&root, &handoff)?;
    Ok(OwnedSource { root, tree_hash })
}

/// Detects an unexpected staged-source mutation before and after processing.
pub fn verify_owned_source(source: &OwnedSource) -> Result<(), AcquisitionError> {
    let actual = tree_hash(&source.root)?;
    if actual == source.tree_hash {
        Ok(())
    } else {
        Err(AcquisitionError::IncompleteReceipt(format!(
            "staged source integrity mismatch at {}",
            source.root.display()
        )))
    }
}

/// Generation evidence for delayed cleanup. Unix directory identity fences a
/// replaced path; all platforms additionally compare the complete tree hash.
pub fn source_generation(path: &Path) -> Result<String, AcquisitionError> {
    let metadata = fs::metadata(path).map_err(|error| io_error(path, error))?;
    if !metadata.is_dir() {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        Ok(format!(
            "unix:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ))
    }
    #[cfg(not(unix))]
    {
        let modified = metadata.modified().map_err(|error| io_error(path, error))?;
        Ok(format!("metadata:{modified:?}:{}", metadata.len()))
    }
}

struct StagedTree {
    method: StagingHandoffMethod,
    tree_hash: String,
}

fn stage_source(
    inbound: &Path,
    destination: &Path,
    staging_root: &Path,
    work_item_id: uuid::Uuid,
) -> Result<StagedTree, AcquisitionError> {
    let parent = destination.parent().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt("staging destination has no parent".to_owned())
    })?;
    create_directory_tree(parent).map_err(|error| io_error(parent, error))?;

    if destination.is_dir() {
        return Ok(StagedTree {
            method: StagingHandoffMethod::VerifiedCopy,
            tree_hash: tree_hash(destination)?,
        });
    }

    match rename_noreplace(inbound, destination) {
        Ok(()) => {
            sync_directory(parent)?;
            sync_directory(staging_root)?;
            if let Some(inbound_parent) = inbound.parent() {
                sync_directory(inbound_parent)?;
            }
            Ok(StagedTree {
                method: StagingHandoffMethod::Rename,
                tree_hash: tree_hash(destination)?,
            })
        }
        Err(error) if is_cross_device(&error) => {
            let releases = parent.parent().ok_or_else(|| {
                AcquisitionError::IncompleteReceipt("staging release parent is missing".to_owned())
            })?;
            let partial = releases.join(format!(".{work_item_id}.acquiring"));
            if partial.exists() {
                remove_partial(&partial, releases)?;
            }
            fs::create_dir(&partial).map_err(|error| io_error(&partial, error))?;
            let before_hash = tree_hash(inbound)?;
            copy_tree(inbound, &partial)?;
            let after_hash = tree_hash(inbound)?;
            let copied_hash = tree_hash(&partial)?;
            if before_hash != after_hash || before_hash != copied_hash {
                remove_partial(&partial, releases)?;
                return Err(AcquisitionError::SourceChanged);
            }
            sync_directory(&partial)?;
            rename_noreplace(&partial, destination)
                .map_err(|error| io_error(destination, error))?;
            sync_directory(parent)?;
            sync_directory(staging_root)?;
            Ok(StagedTree {
                method: StagingHandoffMethod::VerifiedCopy,
                tree_hash: copied_hash,
            })
        }
        Err(error) => Err(io_error(inbound, error)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_inbound_cleanup(
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item_id: uuid::Uuid,
    inbound: &Path,
    inbound_root: &Path,
    expected_generation: &str,
    expected_tree_hash: &str,
    now: OffsetDateTime,
) -> Result<(), AcquisitionError> {
    let inbound = inbound.to_path_buf();
    let inbound_root = inbound_root.to_path_buf();
    let expected_generation = expected_generation.to_owned();
    let expected_tree_hash = expected_tree_hash.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        cleanup_inbound(
            &inbound,
            &inbound_root,
            &expected_generation,
            &expected_tree_hash,
        )
    })
    .await
    .map_err(|error| AcquisitionError::Task(error.to_string()))??;
    match result {
        CleanupResult::Removed | CleanupResult::AlreadyAbsent => {
            store
                .record_inbound_removed(pool, item_id, worker, now)
                .await?;
        }
        CleanupResult::Replaced => {
            store
                .record_inbound_replaced(pool, item_id, worker, now)
                .await?;
        }
    }
    Ok(())
}

enum CleanupResult {
    Removed,
    AlreadyAbsent,
    Replaced,
}

fn cleanup_inbound(
    inbound: &Path,
    inbound_root: &Path,
    expected_generation: &str,
    expected_tree_hash: &str,
) -> Result<CleanupResult, AcquisitionError> {
    if !inbound.starts_with(inbound_root)
        || inbound.parent() != Some(inbound_root)
        || inbound.file_name().is_none()
    {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "refusing cleanup outside direct inbound root: {}",
            inbound.display()
        )));
    }
    if !inbound.exists() {
        return Ok(CleanupResult::AlreadyAbsent);
    }
    if source_generation(inbound)? != expected_generation
        || tree_hash(inbound)? != expected_tree_hash
    {
        return Ok(CleanupResult::Replaced);
    }
    fs::remove_dir_all(inbound).map_err(|error| io_error(inbound, error))?;
    sync_directory(inbound_root)?;
    Ok(CleanupResult::Removed)
}

fn verify_existing_receipt(
    destination: &Path,
    handoff: &StagingHandoff,
) -> Result<String, AcquisitionError> {
    let expected = handoff.tree_hash.as_deref().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt("receipt has no verified tree hash".to_owned())
    })?;
    let actual = tree_hash(destination)?;
    if actual != expected {
        return Err(AcquisitionError::IncompleteReceipt(format!(
            "final staging tree differs from durable receipt at {}",
            destination.display()
        )));
    }
    Ok(actual)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), AcquisitionError> {
    let mut queue = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((current_source, current_destination)) = queue.pop() {
        let mut entries = fs::read_dir(&current_source)
            .map_err(|error| io_error(&current_source, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(&current_source, error))?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        for entry in entries {
            let source_path = entry.path();
            let destination_path = current_destination.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_path)
                .map_err(|error| io_error(&source_path, error))?;
            if metadata.is_dir() {
                fs::create_dir(&destination_path)
                    .map_err(|error| io_error(&destination_path, error))?;
                queue.push((source_path, destination_path));
            } else if metadata.is_file() {
                copy_regular_file(&source_path, &destination_path)?;
            } else {
                // Symlinks and other special files are not durable original
                // source bytes and are never followed (a symlink could escape
                // the inbound root). Skip them here; the inspection layer
                // likewise ignores symlinks (see melodee-media lib.rs), so the
                // staged tree and the manifest stay consistent.
                continue;
            }
        }
    }
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<(), AcquisitionError> {
    let input = File::open(source).map_err(|error| io_error(source, error))?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| io_error(destination, error))?;
    let mut reader = BufReader::with_capacity(COPY_BUFFER_BYTES, input);
    let mut writer = BufWriter::with_capacity(COPY_BUFFER_BYTES, output);
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| io_error(source, error))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| io_error(destination, error))?;
    }
    let output = writer
        .into_inner()
        .map_err(|error| io_error(destination, error.into_error()))?;
    output
        .sync_all()
        .map_err(|error| io_error(destination, error))
}

fn tree_hash(root: &Path) -> Result<String, AcquisitionError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| io_error(root, error))?;
    if !metadata.is_dir() {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let mut hasher = blake3::Hasher::new();
    let mut queue = vec![(root.to_path_buf(), PathBuf::new())];
    while let Some((directory, relative)) = queue.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| io_error(&directory, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(&directory, error))?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let next_relative = relative.join(entry.file_name());
            let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
            if metadata.is_dir() {
                hash_path(&mut hasher, &next_relative, &metadata);
                queue.push((path, next_relative));
            } else if metadata.is_file() {
                hash_path(&mut hasher, &next_relative, &metadata);
                let file = File::open(&path).map_err(|error| io_error(&path, error))?;
                let mut reader = BufReader::with_capacity(COPY_BUFFER_BYTES, file);
                let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
                loop {
                    let read = reader
                        .read(&mut buffer)
                        .map_err(|error| io_error(&path, error))?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..read]);
                }
            } else {
                // Symlinks and other special files contribute no durable bytes
                // and are never followed; skipping keeps the verification hash
                // stable across same- and cross-filesystem acquisition and
                // matches the inspection layer's symlink handling.
                continue;
            }
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_path(hasher: &mut blake3::Hasher, path: &Path, metadata: &fs::Metadata) {
    hasher.update(if metadata.is_dir() { b"d" } else { b"f" });
    for component in path.components() {
        if let Component::Normal(part) = component {
            hasher.update(part.to_string_lossy().as_bytes());
            hasher.update(&[0]);
        }
    }
    hasher.update(&metadata.len().to_le_bytes());
}

fn remove_partial(partial: &Path, releases_root: &Path) -> Result<(), AcquisitionError> {
    if partial.parent() != Some(releases_root) || !partial.is_dir() {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "refusing to remove unexpected partial staging path {}",
            partial.display()
        )));
    }
    fs::remove_dir_all(partial).map_err(|error| io_error(partial, error))
}

fn sync_directory(path: &Path) -> Result<(), AcquisitionError> {
    open_directory(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

fn is_cross_device(error: &io::Error) -> bool {
    error.raw_os_error() == Some(18)
}

fn find_source<'a>(
    config: &'a IngestionConfig,
    item: &ReleaseWorkItem,
) -> Option<&'a InboundSource> {
    config.sources.iter().find(|source| {
        source.id == item.source_library_id
            || (source.id.is_none() && item.source_library_id.is_none())
    })
}

fn resolve_candidate(
    source: &InboundSource,
    source_path: &str,
) -> Result<PathBuf, AcquisitionError> {
    let relative = Path::new(source_path);
    if relative.is_absolute()
        || relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(AcquisitionError::InvalidSourcePath(source_path.to_owned()));
    }
    let path = source.root.join(relative);
    if path.parent() != Some(source.root.as_path()) {
        return Err(AcquisitionError::InvalidSourcePath(source_path.to_owned()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "melodee-acquisition-{label}-{}",
            uuid::Uuid::now_v7().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn same_filesystem_stage_moves_the_complete_tree_without_changing_bytes() {
        let root = temporary_root("rename");
        let inbound = root.join("inbound/release with spaces");
        fs::create_dir_all(inbound.join("disc-01")).unwrap();
        fs::write(inbound.join("disc-01/track-01.flac"), b"audio-a").unwrap();
        fs::write(inbound.join("cover.jpg"), b"artwork-a").unwrap();
        let expected = tree_hash(&inbound).unwrap();
        let staging = root.join("staging");
        let destination = staging.join("releases/test/source");

        let staged = stage_source(&inbound, &destination, &staging, uuid::Uuid::now_v7()).unwrap();

        assert_eq!(staged.method, StagingHandoffMethod::Rename);
        assert_eq!(staged.tree_hash, expected);
        assert!(!inbound.exists());
        assert_eq!(tree_hash(&destination).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn streamed_copy_preserves_nested_unicode_and_punctuation_paths() {
        let root = temporary_root("copy");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("disc 01/(live) & friends")).unwrap();
        fs::write(
            source.join("disc 01/(live) & friends/01 - cafe.flac"),
            vec![42_u8; COPY_BUFFER_BYTES + 17],
        )
        .unwrap();
        fs::create_dir_all(&destination).unwrap();
        let expected = tree_hash(&source).unwrap();

        copy_tree(&source, &destination).unwrap();

        assert_eq!(tree_hash(&destination).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delayed_cleanup_never_removes_a_reused_inbound_path() {
        let root = temporary_root("replacement");
        let inbound_root = root.join("inbound");
        let candidate = inbound_root.join("release");
        fs::create_dir_all(&candidate).unwrap();
        fs::write(candidate.join("track.flac"), b"occurrence-a").unwrap();
        let generation = source_generation(&candidate).unwrap();
        let hash = tree_hash(&candidate).unwrap();

        fs::remove_dir_all(&candidate).unwrap();
        fs::create_dir_all(&candidate).unwrap();
        fs::write(candidate.join("track.flac"), b"occurrence-b").unwrap();

        assert!(matches!(
            cleanup_inbound(&candidate, &inbound_root, &generation, &hash).unwrap(),
            CleanupResult::Replaced
        ));
        assert_eq!(
            fs::read(candidate.join("track.flac")).unwrap(),
            b"occurrence-b"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn tree_hash_and_copy_tree_skip_symlinks_and_special_files() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("symlinks");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("track.flac"), b"audio").unwrap();
        // An internal symlink and a dangling escape symlink must not be
        // followed, copied, or hashed.
        symlink(source.join("track.flac"), source.join("link.flac")).unwrap();
        symlink(root.join("outside"), source.join("escape")).unwrap();

        let expected = {
            let bare = root.join("bare");
            fs::create_dir_all(&bare).unwrap();
            fs::write(bare.join("track.flac"), b"audio").unwrap();
            tree_hash(&bare).unwrap()
        };

        assert_eq!(tree_hash(&source).unwrap(), expected);

        let destination = root.join("destination");
        fs::create_dir_all(&destination).unwrap();
        copy_tree(&source, &destination).unwrap();
        assert!(destination.join("track.flac").is_file());
        assert!(!destination.join("link.flac").exists());
        assert!(!destination.join("escape").exists());
        assert_eq!(tree_hash(&destination).unwrap(), expected);

        fs::remove_dir_all(root).unwrap();
    }
}

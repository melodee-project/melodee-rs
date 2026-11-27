//! Bounded reconstruction of the legacy staging-tree digest from sealed v3
//! observations. The result is provisional until the caller revalidates the
//! staging namespace and binds all observations under the work-item lease.

use std::{
    fmt::Write as _,
    io::{BufReader, Read},
    path::{Component, Path, PathBuf},
};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use melodee_persistence::{
    CustodyLegacyCursor, CustodyLegacyEntry, SealedCustodyHash, WorkItemStore, WorkerId,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use uuid::Uuid;

use crate::{
    acquisition::{AcquisitionError, hash_custody_path, source_generation_from_directory},
    snapshot::{MAX_STREAM_PATH_BYTES, open_cap_relative},
    source::{candidate_relative_path, decode_candidate_relative_path},
};

/// The fields are private so callers cannot turn an arbitrary digest into a
/// verified legacy observation. The seal still requires fenced DB validation.
#[allow(dead_code)]
pub(crate) struct VerifiedStagingLegacy {
    seal: SealedCustodyHash,
    legacy_hash: String,
}

impl VerifiedStagingLegacy {
    #[allow(dead_code)]
    pub(crate) fn seal(&self) -> &SealedCustodyHash {
        &self.seal
    }

    pub(crate) fn legacy_hash(&self) -> &str {
        &self.legacy_hash
    }
}

#[allow(dead_code)]
enum Command {
    Entry(CustodyLegacyEntry),
    Finish,
}

/// Hash metadata emitted only to a provisional baseline writer. The writer
/// must not publish it unless the enclosing verified staging pass succeeds.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum StagingBaselineEvent {
    DirectoryVerified {
        path_key: String,
        v3_digest: String,
    },
    FileStart {
        ordinal: i64,
        path_key: String,
        length: u64,
    },
    Chunk {
        ordinal: i64,
        index: u64,
        length: usize,
        digest: String,
    },
    FileVerified {
        ordinal: i64,
        v3_digest: String,
    },
    Complete,
}

#[allow(dead_code)]
fn emit_baseline(
    sender: &Option<mpsc::Sender<StagingBaselineEvent>>,
    event: StagingBaselineEvent,
) -> Result<(), AcquisitionError> {
    if let Some(sender) = sender {
        sender
            .blocking_send(event)
            .map_err(|_| AcquisitionError::Task("staging baseline consumer stopped".to_owned()))?;
    }
    Ok(())
}

pub(crate) fn staging_chunk_digest(index: u64, bytes: &[u8]) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"melodee-retained-file-chunk-v4\0");
    hash.update(&index.to_le_bytes());
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    hash.finalize().to_hex().to_string()
}

/// An admitted thread holds its permit until it actually exits. If the async
/// caller is dropped, closing the channel stops it and the bounded process-wide
/// reaper retains the thread and its admission until termination.
#[allow(dead_code)]
struct LegacyVerifier {
    sender: Option<mpsc::Sender<Command>>,
    thread: Option<std::thread::JoinHandle<Result<String, AcquisitionError>>>,
    reaper_permit: Option<OwnedSemaphorePermit>,
}

impl LegacyVerifier {
    #[allow(dead_code)]
    async fn start(
        root: cap_std::fs::Dir,
        expected_generation: String,
        baseline: Option<mpsc::Sender<StagingBaselineEvent>>,
    ) -> Result<Self, AcquisitionError> {
        let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
            .await
            .map_err(AcquisitionError::Task)?;
        let admission = crate::admission::heavy_ingestion()
            .await
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let (sender, mut receiver) = mpsc::channel(1);
        let thread = std::thread::Builder::new()
            .name("melodee-legacy-custody".to_owned())
            .spawn(move || {
                let _admission = admission;
                let root_fd = root
                    .try_clone()
                    .map_err(|error| AcquisitionError::Filesystem {
                        path: PathBuf::from("<pinned staging>"),
                        source: error,
                    })?
                    .into_std_file();
                verify_root_generation(&root_fd, &expected_generation)?;
                let mut legacy = blake3::Hasher::new();
                let mut operation = melodee_media::operation_metrics::OperationGuard::start(
                    melodee_media::operation_metrics::Operation::AcquisitionTreeHash,
                );
                loop {
                    match receiver.blocking_recv() {
                        Some(Command::Entry(entry)) => {
                            hash_entry(
                                &root,
                                &entry,
                                &mut legacy,
                                &mut operation,
                                &baseline,
                                || receiver.is_closed(),
                            )?;
                        }
                        Some(Command::Finish) => {
                            verify_root_generation(&root_fd, &expected_generation)?;
                            emit_baseline(&baseline, StagingBaselineEvent::Complete)?;
                            operation.finish();
                            return Ok(legacy.finalize().to_hex().to_string());
                        }
                        None => {
                            return Err(AcquisitionError::Task(
                                "legacy custody verifier cancelled".to_owned(),
                            ));
                        }
                    }
                }
            })
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            reaper_permit: Some(reaper_permit),
        })
    }

    async fn send(&self, command: Command) -> Result<(), AcquisitionError> {
        self.sender
            .as_ref()
            .ok_or_else(|| AcquisitionError::Task("legacy verifier stopped".to_owned()))?
            .send(command)
            .await
            .map_err(|_| AcquisitionError::Task("legacy verifier stopped".to_owned()))
    }

    async fn finish(mut self) -> Result<String, AcquisitionError> {
        // Keep the sender alive until the worker receives Finish: channel
        // closure is reserved for cancellation during a large file read.
        let _ = self.send(Command::Finish).await;
        loop {
            let thread = self
                .thread
                .as_ref()
                .ok_or_else(|| AcquisitionError::Task("legacy verifier missing".to_owned()))?;
            if thread.is_finished() {
                let thread = self
                    .thread
                    .take()
                    .ok_or_else(|| AcquisitionError::Task("legacy verifier missing".to_owned()))?;
                return thread
                    .join()
                    .map_err(|_| AcquisitionError::Task("legacy verifier panicked".to_owned()))?;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

#[allow(dead_code)]
fn verify_root_generation(root: &std::fs::File, expected: &str) -> Result<(), AcquisitionError> {
    let actual = source_generation_from_directory(root, Path::new("<pinned staging>"))?;
    if actual != expected {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok(())
}

impl Drop for LegacyVerifier {
    fn drop(&mut self) {
        self.sender.take();
        if let (Some(thread), Some(permit)) = (self.thread.take(), self.reaper_permit.take()) {
            crate::reconcile::submit_reaper(
                Box::new(move || {
                    let _ = thread.join();
                }),
                permit,
            );
        }
    }
}

#[allow(dead_code)]
fn invalid_evidence() -> AcquisitionError {
    AcquisitionError::IncompleteReceipt("invalid legacy custody observation".to_owned())
}

#[allow(dead_code)]
fn checked_path(entry: &CustodyLegacyEntry) -> Result<PathBuf, AcquisitionError> {
    let path = decode_candidate_relative_path(&entry.path_key).map_err(|_| invalid_evidence())?;
    if path.as_os_str().is_empty()
        || path.as_os_str().as_encoded_bytes().len() > MAX_STREAM_PATH_BYTES
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || candidate_relative_path(&path) != entry.path_key
    {
        return Err(invalid_evidence());
    }
    let name = path.file_name().ok_or_else(invalid_evidence)?;
    let mut name_hex = String::with_capacity(name.as_encoded_bytes().len().saturating_mul(2));
    for byte in name.as_encoded_bytes() {
        let _ = write!(name_hex, "{byte:02x}");
    }
    if name_hex != entry.name_key {
        return Err(invalid_evidence());
    }
    let mut legacy_hex = String::new();
    for part in path.components() {
        if let Component::Normal(name) = part {
            for byte in name.to_string_lossy().as_bytes() {
                let _ = write!(legacy_hex, "{byte:02x}");
            }
            legacy_hex.push_str("00");
        }
    }
    if legacy_hex != entry.legacy_path_hex || entry.legacy_length < 0 {
        return Err(invalid_evidence());
    }
    Ok(path)
}

#[allow(dead_code)]
fn hash_entry(
    root: &cap_std::fs::Dir,
    entry: &CustodyLegacyEntry,
    legacy: &mut blake3::Hasher,
    operation: &mut melodee_media::operation_metrics::OperationGuard,
    baseline: &Option<mpsc::Sender<StagingBaselineEvent>>,
    cancelled: impl Fn() -> bool,
) -> Result<(), AcquisitionError> {
    if cancelled() {
        return Err(AcquisitionError::Task(
            "legacy custody verifier cancelled".to_owned(),
        ));
    }
    let path = checked_path(entry)?;
    let parent_path = path.parent().ok_or_else(invalid_evidence)?;
    let name = path.file_name().ok_or_else(invalid_evidence)?;
    let parent = open_cap_relative(root, parent_path)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let metadata = parent
        .symlink_metadata(name)
        .map_err(|error| AcquisitionError::Filesystem {
            path: path.clone(),
            source: error,
        })?;
    let length = u64::try_from(entry.legacy_length).map_err(|_| invalid_evidence())?;
    if metadata.len() != length
        || (entry.is_directory && !metadata.is_dir())
        || (!entry.is_directory && !metadata.is_file())
    {
        return Err(AcquisitionError::SourceChanged);
    }

    let mut observed = blake3::Hasher::new();
    observed.update(b"melodee-custody-entry-v3\0");
    #[cfg(unix)]
    observed.update(b"unix\0");
    #[cfg(windows)]
    observed.update(b"windows\0");
    hash_custody_path(&mut observed, &path, entry.is_directory, length)?;

    // The legacy format hashes the lossy component text, each followed by NUL.
    // Derive those bytes from the native path, rather than trusting spool text.
    legacy.update(if entry.is_directory { b"d" } else { b"f" });
    for component in path.components() {
        if let Component::Normal(part) = component {
            legacy.update(part.to_string_lossy().as_bytes());
            legacy.update(&[0]);
        }
    }
    legacy.update(&length.to_le_bytes());

    if entry.is_directory {
        let child =
            parent
                .open_dir_nofollow(name)
                .map_err(|error| AcquisitionError::Filesystem {
                    path: path.clone(),
                    source: error,
                })?;
        let opened = child
            .dir_metadata()
            .map_err(|error| AcquisitionError::Filesystem {
                path: path.clone(),
                source: error,
            })?;
        if !opened.is_dir() || opened.len() != length {
            return Err(AcquisitionError::SourceChanged);
        }
    } else {
        if baseline.is_some() {
            emit_baseline(
                baseline,
                StagingBaselineEvent::FileStart {
                    ordinal: entry.ordinal,
                    path_key: entry.path_key.clone(),
                    length,
                },
            )?;
        }
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file =
            parent
                .open_with(name, &options)
                .map_err(|error| AcquisitionError::Filesystem {
                    path: path.clone(),
                    source: error,
                })?;
        let opened = file
            .metadata()
            .map_err(|error| AcquisitionError::Filesystem {
                path: path.clone(),
                source: error,
            })?;
        if !opened.is_file() || opened.len() != length {
            return Err(AcquisitionError::SourceChanged);
        }
        let mut reader = BufReader::new(file.into_std());
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_u64;
        let mut chunk_index = 0_u64;
        while total < length {
            if cancelled() {
                return Err(AcquisitionError::Task(
                    "legacy custody verifier cancelled".to_owned(),
                ));
            }
            let expected = (length - total).min(buffer.len() as u64) as usize;
            reader
                .read_exact(&mut buffer[..expected])
                .map_err(|error| {
                    operation.mark_bytes_incomplete();
                    if error.kind() == std::io::ErrorKind::UnexpectedEof {
                        AcquisitionError::SourceChanged
                    } else {
                        AcquisitionError::Filesystem {
                            path: path.clone(),
                            source: error,
                        }
                    }
                })?;
            let count = expected;
            total = total
                .checked_add(count as u64)
                .ok_or(AcquisitionError::SourceChanged)?;
            observed.update(&buffer[..count]);
            legacy.update(&buffer[..count]);
            operation.add_read(count as u64);
            if baseline.is_some() {
                emit_baseline(
                    baseline,
                    StagingBaselineEvent::Chunk {
                        ordinal: entry.ordinal,
                        index: chunk_index,
                        length: count,
                        digest: staging_chunk_digest(chunk_index, &buffer[..count]),
                    },
                )?;
            }
            chunk_index = chunk_index
                .checked_add(1)
                .ok_or(AcquisitionError::SourceChanged)?;
        }
        let mut extra = [0_u8; 1];
        if reader.read(&mut extra).map_err(|error| {
            operation.mark_bytes_incomplete();
            AcquisitionError::Filesystem {
                path: path.clone(),
                source: error,
            }
        })? != 0
        {
            operation.add_read(1);
            return Err(AcquisitionError::SourceChanged);
        }
        if total != length
            || reader
                .get_ref()
                .metadata()
                .map_err(|error| AcquisitionError::Filesystem {
                    path: path.clone(),
                    source: error,
                })?
                .len()
                != length
        {
            return Err(AcquisitionError::SourceChanged);
        }
    }
    if observed.finalize().to_hex().as_str() != entry.digest {
        return Err(AcquisitionError::SourceChanged);
    }
    if baseline.is_some() {
        let event = if entry.is_directory {
            StagingBaselineEvent::DirectoryVerified {
                path_key: entry.path_key.clone(),
                v3_digest: entry.digest.clone(),
            }
        } else {
            StagingBaselineEvent::FileVerified {
                ordinal: entry.ordinal,
                v3_digest: entry.digest.clone(),
            }
        };
        emit_baseline(baseline, event)?;
    }
    Ok(())
}

#[derive(Default)]
#[allow(dead_code)]
struct Frame {
    after: Option<CustodyLegacyCursor>,
    directories: bool,
}

/// Reconstruct the legacy digest in the old traversal order using only bounded
/// DB pages and bounded, admitted filesystem work. The caller is responsible
/// for lease heartbeat and namespace validation before and after this future.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(crate) async fn verify_legacy_staging(
    pool: &PgPool,
    store: &WorkItemStore,
    item: Uuid,
    worker: &WorkerId,
    _lease_duration: Duration,
    seal: SealedCustodyHash,
    pinned_root: cap_std::fs::Dir,
) -> Result<VerifiedStagingLegacy, AcquisitionError> {
    verify_legacy_staging_inner(pool, store, item, worker, seal, pinned_root, None).await
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
async fn verify_legacy_staging_inner(
    pool: &PgPool,
    store: &WorkItemStore,
    item: Uuid,
    worker: &WorkerId,
    seal: SealedCustodyHash,
    pinned_root: cap_std::fs::Dir,
    baseline: Option<mpsc::Sender<StagingBaselineEvent>>,
) -> Result<VerifiedStagingLegacy, AcquisitionError> {
    let verifier =
        LegacyVerifier::start(pinned_root, seal.generation().to_owned(), baseline).await?;
    let mut path = PathBuf::new();
    let mut frames = vec![Frame::default()];
    let mut seen = 0_i64;
    while let Some(frame) = frames.last_mut() {
        let parent_key = candidate_relative_path(&path);
        let page = store
            .custody_legacy_siblings_page(
                pool,
                seal.capture_id(),
                item,
                worker,
                &parent_key,
                frame.after.as_ref(),
                frame.directories,
                frame.directories,
                OffsetDateTime::now_utc(),
            )
            .await?;
        if page.is_empty() {
            if frame.directories {
                frames.pop();
                if !frames.is_empty() {
                    path.pop();
                }
            } else {
                frame.directories = true;
                frame.after = None;
            }
            continue;
        }
        if frame.directories {
            // Do not retain sibling pages at every level of a deep tree.
            let entry = page.into_iter().next().ok_or_else(invalid_evidence)?;
            let child = checked_path(&entry)?;
            if !entry.is_directory || child.parent() != Some(path.as_path()) {
                return Err(invalid_evidence());
            }
            frame.after = Some(CustodyLegacyCursor {
                name_key: entry.name_key,
                ordinal: entry.ordinal,
            });
            path = child;
            if frames.len() >= MAX_STREAM_PATH_BYTES {
                return Err(invalid_evidence());
            }
            frames.push(Frame::default());
        } else {
            for entry in page {
                let child = checked_path(&entry)?;
                if child.parent() != Some(path.as_path()) {
                    return Err(invalid_evidence());
                }
                frame.after = Some(CustodyLegacyCursor {
                    name_key: entry.name_key.clone(),
                    ordinal: entry.ordinal,
                });
                seen = seen.checked_add(1).ok_or_else(invalid_evidence)?;
                if seen > seal.entries() {
                    return Err(invalid_evidence());
                }
                if verifier.send(Command::Entry(entry)).await.is_err() {
                    return match verifier.finish().await {
                        Err(error) => Err(error),
                        Ok(_) => Err(AcquisitionError::Task(
                            "legacy verifier exited early".to_owned(),
                        )),
                    };
                }
            }
        }
    }
    if seen != seal.entries() {
        return Err(invalid_evidence());
    }
    let legacy_hash = verifier.finish().await?;
    Ok(VerifiedStagingLegacy { seal, legacy_hash })
}

/// Capture provisional v4 chunk metadata from the same pinned byte stream that
/// revalidates each v3 entry and computes the legacy receipt. A returned pair
/// still requires private-namespace revalidation and atomic receipt binding.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(crate) async fn verify_legacy_staging_with_baseline(
    pool: &PgPool,
    store: &WorkItemStore,
    item: Uuid,
    worker: &WorkerId,
    seal: SealedCustodyHash,
    pinned_root: cap_std::fs::Dir,
) -> Result<
    (
        VerifiedStagingLegacy,
        melodee_persistence::RetainedBaselineFrozen,
    ),
    AcquisitionError,
> {
    let baseline = store.begin_retained_baseline(pool, worker, &seal).await?;
    let (sender, receiver) = mpsc::channel(1);
    let scan = verify_legacy_staging_inner(
        pool,
        store,
        item,
        worker,
        seal.clone(),
        pinned_root,
        Some(sender),
    );
    let writer = write_provisional_baseline(pool, store, worker, baseline, &seal, receiver);
    tokio::try_join!(scan, writer)
}

#[allow(dead_code)]
async fn write_provisional_baseline(
    pool: &PgPool,
    store: &WorkItemStore,
    worker: &WorkerId,
    baseline: Uuid,
    seal: &SealedCustodyHash,
    mut receiver: mpsc::Receiver<StagingBaselineEvent>,
) -> Result<melodee_persistence::RetainedBaselineFrozen, AcquisitionError> {
    let mut current: Option<melodee_persistence::RetainedBaselineFile> = None;
    let mut chunks = Vec::with_capacity(128);
    while let Some(event) = receiver.recv().await {
        match event {
            StagingBaselineEvent::DirectoryVerified {
                path_key,
                v3_digest,
            } => {
                if current.is_some() || !chunks.is_empty() {
                    return Err(invalid_evidence());
                }
                store
                    .record_retained_baseline_directory(
                        pool, worker, baseline, seal, &path_key, &v3_digest,
                    )
                    .await?;
            }
            StagingBaselineEvent::FileStart {
                ordinal,
                path_key,
                length,
            } => {
                if current.is_some() || !chunks.is_empty() {
                    return Err(invalid_evidence());
                }
                let file = store
                    .begin_retained_baseline_file(pool, worker, baseline, seal, &path_key)
                    .await?;
                if file.ordinal != ordinal || u64::try_from(file.length).ok() != Some(length) {
                    return Err(invalid_evidence());
                }
                current = Some(file);
            }
            StagingBaselineEvent::Chunk {
                ordinal,
                index,
                length,
                digest,
            } => {
                if current.as_ref().is_none_or(|file| file.ordinal != ordinal) {
                    return Err(invalid_evidence());
                }
                chunks.push(melodee_persistence::RetainedBaselineChunk {
                    index: i64::try_from(index).map_err(|_| invalid_evidence())?,
                    length: i32::try_from(length).map_err(|_| invalid_evidence())?,
                    digest,
                });
                if chunks.len() == 128 {
                    store
                        .append_retained_baseline_chunks(
                            pool, worker, baseline, seal, ordinal, &chunks,
                        )
                        .await?;
                    chunks.clear();
                }
            }
            StagingBaselineEvent::FileVerified { ordinal, v3_digest } => {
                if current.as_ref().is_none_or(|file| file.ordinal != ordinal) {
                    return Err(invalid_evidence());
                }
                if !chunks.is_empty() {
                    store
                        .append_retained_baseline_chunks(
                            pool, worker, baseline, seal, ordinal, &chunks,
                        )
                        .await?;
                    chunks.clear();
                }
                store
                    .finish_retained_baseline_file(
                        pool, worker, baseline, seal, ordinal, &v3_digest,
                    )
                    .await?;
                current = None;
            }
            StagingBaselineEvent::Complete => {
                if current.is_some() || !chunks.is_empty() {
                    return Err(invalid_evidence());
                }
                return store
                    .freeze_retained_baseline(pool, worker, baseline, seal)
                    .await
                    .map_err(AcquisitionError::from);
            }
        }
    }
    Err(AcquisitionError::Task(
        "staging baseline stream ended before completion".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use std::{
        cell::Cell,
        collections::HashMap,
        fs,
        path::{Path, PathBuf},
    };

    use cap_std::ambient_authority;

    use super::*;

    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("alpha/nested")).unwrap();
        fs::create_dir_all(temp.path().join("zeta/deep")).unwrap();
        fs::write(temp.path().join("alpha/nested/one"), b"first").unwrap();
        fs::write(temp.path().join("zeta/deep/two"), b"second").unwrap();
        fs::write(temp.path().join("root.bin"), b"root").unwrap();
        // More than one database page of siblings; legacy reconstruction must
        // not mistake page boundaries for directory traversal boundaries.
        for index in 0..41 {
            fs::write(temp.path().join(format!("file-{index:03}")), [index as u8]).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            fs::write(
                temp.path()
                    .join(std::ffi::OsString::from_vec(b"bad-\xff".to_vec())),
                b"bytes",
            )
            .unwrap();
            fs::write(temp.path().join("bad-\u{fffd}"), b"other").unwrap();
        }
        temp
    }

    fn observations(root: &cap_std::fs::Dir) -> HashMap<String, CustodyLegacyEntry> {
        let mut entries = HashMap::new();
        crate::custody::stream_custody_hash_observations(
            root,
            || false,
            |entry| {
                let key = entry.path_key.clone();
                entries.insert(
                    key,
                    CustodyLegacyEntry {
                        ordinal: 1,
                        path_key: entry.path_key,
                        name_key: entry.name_key,
                        legacy_path_hex: entry.legacy_path_hex,
                        is_directory: entry.is_directory,
                        legacy_length: entry.legacy_length,
                        digest: entry.digest,
                    },
                );
                Ok(())
            },
        )
        .unwrap();
        entries
    }

    fn test_hash_entry(
        root: &cap_std::fs::Dir,
        entry: &CustodyLegacyEntry,
        legacy: &mut blake3::Hasher,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), AcquisitionError> {
        let mut operation = melodee_media::operation_metrics::OperationGuard::start(
            melodee_media::operation_metrics::Operation::AcquisitionTreeHash,
        );
        hash_entry(root, entry, legacy, &mut operation, &None, cancelled)
    }

    // Independent test oracle for the old traversal: hash a directory's
    // sorted children first, then visit subdirectories in reverse order.
    fn legacy_order(root: &Path) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            let mut entries = fs::read_dir(root.join(&relative))
                .unwrap()
                .map(|entry| entry.unwrap())
                .collect::<Vec<_>>();
            entries.sort_unstable_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = relative.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    pending.push(path.clone());
                }
                result.push(path);
            }
        }
        result
    }

    #[test]
    fn legacy_hash_matches_independent_tree_with_pages_and_native_names() {
        let temp = fixture();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entries = observations(&root);
        let mut actual = blake3::Hasher::new();
        for path in legacy_order(temp.path()) {
            let key = candidate_relative_path(&path);
            test_hash_entry(&root, entries.get(&key).unwrap(), &mut actual, || false).unwrap();
        }
        let descriptor = std::fs::File::open(temp.path()).unwrap();
        let (_, expected) =
            crate::acquisition::tree_hashes_from_pinned_directory(&descriptor).unwrap();
        assert_eq!(actual.finalize().to_hex().as_str(), expected);
        #[cfg(unix)]
        {
            assert!(entries.contains_key("bad-\u{fffd}"));
            assert!(
                entries
                    .keys()
                    .any(|key| key.starts_with("//melodee-unix-hex:"))
            );
        }
    }

    #[test]
    fn same_stage_stream_emits_v4_chunks_only_with_a_matching_v3_entry() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = vec![0x5a; 64 * 1024 + 17];
        fs::write(temp.path().join("track"), &bytes).unwrap();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entries = observations(&root);
        let entry = entries.get("track").unwrap();
        let (sender, mut receiver) = mpsc::channel(8);
        let mut operation = melodee_media::operation_metrics::OperationGuard::start(
            melodee_media::operation_metrics::Operation::AcquisitionTreeHash,
        );
        hash_entry(
            &root,
            entry,
            &mut blake3::Hasher::new(),
            &mut operation,
            &Some(sender),
            || false,
        )
        .unwrap();
        assert!(
            matches!(receiver.try_recv().unwrap(), StagingBaselineEvent::FileStart { ordinal, path_key, length } if ordinal == entry.ordinal && path_key == "track" && length == bytes.len() as u64)
        );
        assert!(
            matches!(receiver.try_recv().unwrap(), StagingBaselineEvent::Chunk { ordinal, index: 0, length, digest } if ordinal == entry.ordinal && length == 64 * 1024 && digest == staging_chunk_digest(0, &bytes[..64 * 1024]))
        );
        assert!(
            matches!(receiver.try_recv().unwrap(), StagingBaselineEvent::Chunk { ordinal, index: 1, length: 17, digest } if ordinal == entry.ordinal && digest == staging_chunk_digest(1, &bytes[64 * 1024..]))
        );
        assert!(
            matches!(receiver.try_recv().unwrap(), StagingBaselineEvent::FileVerified { ordinal, v3_digest } if ordinal == entry.ordinal && v3_digest == entry.digest)
        );
        assert!(receiver.try_recv().is_err());

        let (sender, mut receiver) = mpsc::channel(8);
        let checks = Cell::new(0);
        let changed = vec![0x33; bytes.len()];
        let result = hash_entry(
            &root,
            entry,
            &mut blake3::Hasher::new(),
            &mut operation,
            &Some(sender),
            || {
                checks.set(checks.get() + 1);
                if checks.get() == 3 {
                    fs::write(temp.path().join("track"), &changed).unwrap();
                }
                false
            },
        );
        assert!(matches!(result, Err(AcquisitionError::SourceChanged)));
        while let Ok(event) = receiver.try_recv() {
            assert!(
                !matches!(event, StagingBaselineEvent::FileVerified { .. }),
                "mismatched bytes cannot finish the baseline file"
            );
        }
    }

    #[test]
    fn stage_directory_event_requires_matching_v3_evidence() {
        let temp = fixture();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entries = observations(&root);
        let entry = entries.get("alpha").unwrap();
        let (sender, mut receiver) = mpsc::channel(2);
        let mut operation = melodee_media::operation_metrics::OperationGuard::start(
            melodee_media::operation_metrics::Operation::AcquisitionTreeHash,
        );
        hash_entry(
            &root,
            entry,
            &mut blake3::Hasher::new(),
            &mut operation,
            &Some(sender),
            || false,
        )
        .unwrap();
        assert!(
            matches!(receiver.try_recv().unwrap(), StagingBaselineEvent::DirectoryVerified { path_key, v3_digest } if path_key == "alpha" && v3_digest == entry.digest)
        );
        let mut forged = entry.clone();
        forged.digest = "0".repeat(64);
        let (sender, mut receiver) = mpsc::channel(2);
        assert!(matches!(
            hash_entry(
                &root,
                &forged,
                &mut blake3::Hasher::new(),
                &mut operation,
                &Some(sender),
                || false
            ),
            Err(AcquisitionError::SourceChanged)
        ));
        assert!(
            receiver.try_recv().is_err(),
            "unverified directory must not be recorded"
        );
    }

    #[tokio::test]
    async fn admitted_worker_streams_entries_and_matches_legacy_oracle() {
        let temp = fixture();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entries = observations(&root);
        let descriptor = std::fs::File::open(temp.path()).unwrap();
        let generation = source_generation_from_directory(&descriptor, temp.path()).unwrap();
        let verifier = LegacyVerifier::start(root, generation, None).await.unwrap();
        for path in legacy_order(temp.path()) {
            let key = candidate_relative_path(&path);
            verifier
                .send(Command::Entry(entries.get(&key).unwrap().clone()))
                .await
                .unwrap();
        }
        let actual = verifier.finish().await.unwrap();
        let descriptor = std::fs::File::open(temp.path()).unwrap();
        let (_, expected) =
            crate::acquisition::tree_hashes_from_pinned_directory(&descriptor).unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn dropped_baseline_consumer_reaps_blocked_verifier() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("track"), vec![0x5a; 3 * 64 * 1024]).unwrap();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entry = observations(&root).get("track").unwrap().clone();
        let descriptor = std::fs::File::open(temp.path()).unwrap();
        let generation = source_generation_from_directory(&descriptor, temp.path()).unwrap();
        let (sender, mut receiver) = mpsc::channel(1);
        let verifier = LegacyVerifier::start(root, generation, Some(sender))
            .await
            .unwrap();
        verifier.send(Command::Entry(entry)).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
                .await
                .unwrap(),
            Some(StagingBaselineEvent::FileStart { .. })
        ));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while receiver.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!verifier.thread.as_ref().unwrap().is_finished());
        drop(receiver);
        drop(verifier);

        // Both permits can be reacquired only after the dropped worker has
        // exited and the supervisor has reaped it. No Complete event or seal
        // can be emitted after the consumer has closed its channel.
        let permits = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let first = crate::reconcile::acquire_custody_reaper_permit()
                .await
                .unwrap();
            let second = crate::reconcile::acquire_custody_reaper_permit()
                .await
                .unwrap();
            (first, second)
        })
        .await
        .unwrap();
        drop(permits);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn identical_content_from_a_different_root_is_not_accepted() {
        let original = fixture();
        let replacement = fixture();
        let original_fd = std::fs::File::open(original.path()).unwrap();
        let generation = source_generation_from_directory(&original_fd, original.path()).unwrap();
        let replacement_root =
            cap_std::fs::Dir::open_ambient_dir(replacement.path(), ambient_authority()).unwrap();
        let verifier = LegacyVerifier::start(replacement_root, generation, None)
            .await
            .unwrap();
        assert!(matches!(
            verifier.finish().await,
            Err(AcquisitionError::SourceChanged)
        ));
    }

    #[test]
    fn changed_bytes_or_spool_path_cannot_authorize_a_digest() {
        let temp = fixture();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entries = observations(&root);
        let file = entries.get("root.bin").unwrap();
        fs::write(temp.path().join("root.bin"), b"ROOT").unwrap();
        assert!(matches!(
            test_hash_entry(&root, file, &mut blake3::Hasher::new(), || false),
            Err(AcquisitionError::SourceChanged)
        ));
        let mut forged = file.clone();
        forged.legacy_path_hex.push_str("00");
        assert!(checked_path(&forged).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_symlink_is_never_opened_as_the_observed_file() {
        let temp = fixture();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entries = observations(&root);
        let file = entries.get("root.bin").unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("same-bytes");
        fs::write(&target, b"root").unwrap();
        fs::remove_file(temp.path().join("root.bin")).unwrap();
        std::os::unix::fs::symlink(&target, temp.path().join("root.bin")).unwrap();
        assert!(test_hash_entry(&root, file, &mut blake3::Hasher::new(), || false).is_err());
        assert_eq!(fs::read(target).unwrap(), b"root");
    }

    #[test]
    fn cancellation_is_checked_during_file_reads() {
        let temp = fixture();
        fs::write(temp.path().join("large"), vec![7_u8; 256 * 1024]).unwrap();
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let entries = observations(&root);
        let checks = Cell::new(0);
        let outcome = test_hash_entry(
            &root,
            entries.get("large").unwrap(),
            &mut blake3::Hasher::new(),
            || {
                checks.set(checks.get() + 1);
                checks.get() >= 3
            },
        );
        assert!(outcome.is_err());
        assert!(checks.get() >= 3);
    }
}

//! Descriptor-relative quarantine primitives for a future durable handoff.
//!
//! These operations are not yet wired into production acquisition or purge.

#[cfg(all(test, target_os = "linux"))]
use crate::secure_fs::mkdir_private_child;
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
use std::io;
use std::{ffi::OsStr, fs::File};

use crate::{
    acquisition::{
        AcquisitionError, source_generation_from_directory, tree_hash_from_pinned_directory,
    },
    secure_fs::{open_child_directory, rename_child_noreplace},
    source::GuardedRoot,
};

struct CustodyProducer {
    receiver: Option<tokio::sync::mpsc::Receiver<melodee_persistence::CustodyHashObservation>>,
    thread: Option<std::thread::JoinHandle<Result<u64, AcquisitionError>>>,
    reaper_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl CustodyProducer {
    async fn finish(mut self) -> Result<u64, AcquisitionError> {
        self.receiver.take();
        loop {
            let Some(thread) = self.thread.as_ref() else {
                return Err(AcquisitionError::Task(
                    "custody producer missing".to_owned(),
                ));
            };
            if thread.is_finished() {
                let thread = self
                    .thread
                    .take()
                    .ok_or_else(|| AcquisitionError::Task("custody producer missing".to_owned()))?;
                return thread
                    .join()
                    .map_err(|_| AcquisitionError::Task("custody producer panicked".to_owned()))?;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// A bounded page reader uses the receiver only as a cancellation signal;
    /// keep it open until the producer actually terminates.
    async fn wait_for_page(mut self) -> Result<u64, AcquisitionError> {
        loop {
            let Some(thread) = self.thread.as_ref() else {
                return Err(AcquisitionError::Task(
                    "custody producer missing".to_owned(),
                ));
            };
            if thread.is_finished() {
                let thread = self
                    .thread
                    .take()
                    .ok_or_else(|| AcquisitionError::Task("custody producer missing".to_owned()))?;
                return thread
                    .join()
                    .map_err(|_| AcquisitionError::Task("custody producer panicked".to_owned()))?;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

impl Drop for CustodyProducer {
    fn drop(&mut self) {
        self.receiver.take();
        if let (Some(thread), Some(permit)) = (self.thread.take(), self.reaper_permit.take()) {
            crate::reconcile::reap_custody_producer(thread, permit);
        }
    }
}

async fn spawn_custody_producer(
    root: cap_std::fs::Dir,
    retained_mib_per_second: Option<u32>,
) -> Result<CustodyProducer, AcquisitionError> {
    let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
        .await
        .map_err(AcquisitionError::Task)?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    // Native path metadata can be large; one in-flight observation keeps the
    // producer/consumer queue bounded independently of filename lengths.
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let thread = std::thread::Builder::new()
        .name("melodee-custody-hash".to_owned())
        .spawn(move || {
            let _permit = permit;
            let cancelled = sender.clone();
            stream_custody_observations_inner(
                &root,
                None,
                retained_mib_per_second,
                || cancelled.is_closed(),
                |entry| {
                    sender.blocking_send(entry).map_err(|_| {
                        AcquisitionError::Task("custody hash consumer stopped".to_owned())
                    })
                },
            )
        })
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(CustodyProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    })
}

/// A retained-baseline page is capped by the persistence reader at sixteen
/// 64 KiB chunks. The thread owns admission until it terminates; dropping the
/// waiter closes its cancellation channel and transfers joining to the reaper.
async fn spawn_retained_page_reader(
    root: cap_std::fs::Dir,
    path_key: String,
    file_length: i64,
    chunks: Vec<melodee_persistence::RetainedBaselineChunk>,
    rate: u32,
) -> Result<CustodyProducer, AcquisitionError> {
    let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
        .await
        .map_err(AcquisitionError::Task)?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let thread = std::thread::Builder::new()
        .name("melodee-retained-page".to_owned())
        .spawn(move || {
            let _permit = permit;
            read_retained_baseline_page(&root, &path_key, file_length, &chunks, rate, || {
                sender.is_closed()
            })
            .map(u64::from)
        })
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(CustodyProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    })
}

/// Read a bounded Linux directory page. The cookie is only a traversal hint:
/// concurrent filesystem changes can invalidate it, so EOF is never evidence
/// that an occurrence is absent or that a tree is unchanged.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetainedDirectoryContinuation {
    More(u64),
    Eof,
    Invalidated,
}

#[allow(clippy::type_complexity)]
#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_retained_directory_page(
    root: &cap_std::fs::Dir,
    parent_key: &str,
    cookie: Option<u64>,
    cancelled: impl Fn() -> bool,
    after_entry: impl Fn(usize),
) -> Result<
    (
        Vec<(String, Option<bool>)>,
        RetainedDirectoryContinuation,
        String,
    ),
    AcquisitionError,
> {
    use cap_fs_ext::DirExt;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let relative = if parent_key.is_empty() {
        std::path::PathBuf::new()
    } else {
        crate::source::decode_candidate_relative_path(parent_key)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?
    };
    let mut directory = root
        .try_clone()
        .map_err(|source| AcquisitionError::Filesystem {
            path: relative.clone(),
            source,
        })?;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(AcquisitionError::InvalidSourcePath(
                "invalid directory path".to_owned(),
            ));
        };
        directory =
            directory
                .open_dir_nofollow(name)
                .map_err(|source| AcquisitionError::Filesystem {
                    path: relative.clone(),
                    source,
                })?;
    }
    // Open a new file description: dup/try_clone would share directory offset
    // with another concurrent reader and could corrupt both cursors.
    let pinned = directory.into_std_file();
    let fd = std::fs::File::from(
        rustix::fs::openat(
            &pinned,
            ".",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| AcquisitionError::Filesystem {
            path: relative.clone(),
            source: std::io::Error::from_raw_os_error(error.raw_os_error()),
        })?,
    );
    let generation = |file: &std::fs::File| -> Result<String, AcquisitionError> {
        let metadata = file
            .metadata()
            .map_err(|source| AcquisitionError::Filesystem {
                path: relative.clone(),
                source,
            })?;
        Ok(format!(
            "unix:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ))
    };
    let before = generation(&fd)?;
    if let Some(cookie) = cookie {
        rustix::fs::seek(&fd, rustix::fs::SeekFrom::Start(cookie)).map_err(|error| {
            AcquisitionError::Filesystem {
                path: relative.clone(),
                source: std::io::Error::from_raw_os_error(error.raw_os_error()),
            }
        })?;
    }
    let mut buffer = [std::mem::MaybeUninit::uninit(); 8192];
    let mut entries = rustix::fs::RawDir::new(&fd, &mut buffer);
    let mut output = Vec::with_capacity(64);
    let mut next_cookie = cookie;
    let mut path_bytes = 0_usize;
    let mut eof = false;
    for examined in 0..64 {
        if cancelled() {
            return Err(AcquisitionError::Task(
                "retained directory page cancelled".to_owned(),
            ));
        }
        let Some(entry) = entries.next() else {
            eof = true;
            break;
        };
        let entry = entry.map_err(|error| AcquisitionError::Filesystem {
            path: relative.clone(),
            source: std::io::Error::from_raw_os_error(error.raw_os_error()),
        })?;
        after_entry(examined);
        let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes());
        if name == std::ffi::OsStr::new(".") || name == std::ffi::OsStr::new("..") {
            next_cookie = Some(entry.next_entry_cookie());
            continue;
        }
        let is_directory = match entry.file_type() {
            rustix::fs::FileType::Directory => Some(true),
            rustix::fs::FileType::RegularFile => Some(false),
            // Unknown or special entries request fresh verification, but
            // still advance the hint so they cannot starve later entries.
            _ => None,
        };
        let path_key = crate::source::candidate_relative_path(&relative.join(name));
        if path_bytes.saturating_add(path_key.len()) > 1024 * 1024 {
            if output.is_empty() {
                return Err(AcquisitionError::SourceChanged);
            }
            break;
        }
        path_bytes += path_key.len();
        next_cookie = Some(entry.next_entry_cookie());
        output.push((path_key, is_directory));
    }
    let current = crate::snapshot::open_cap_relative(root, &relative)
        .map_err(|_| AcquisitionError::SourceChanged)?
        .into_std_file();
    let changed = generation(&fd)? != before
        || generation(&current)? != before
        || (!eof && next_cookie == cookie);
    // Directory metadata only invalidates a hint; it is never proof that
    // the namespace did not change during or between passes.
    let continuation = if changed {
        RetainedDirectoryContinuation::Invalidated
    } else if eof {
        RetainedDirectoryContinuation::Eof
    } else if let Some(cookie) = next_cookie {
        RetainedDirectoryContinuation::More(cookie)
    } else {
        RetainedDirectoryContinuation::Invalidated
    };
    Ok((output, continuation, before))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
type RetainedDirectoryPage = (
    Vec<(String, Option<bool>)>,
    RetainedDirectoryContinuation,
    String,
);

#[cfg(any(target_os = "linux", target_os = "android"))]
struct RetainedDirectoryProducer {
    receiver: Option<tokio::sync::mpsc::Receiver<Result<RetainedDirectoryPage, AcquisitionError>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    reaper_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl RetainedDirectoryProducer {
    async fn finish(mut self) -> Result<RetainedDirectoryPage, AcquisitionError> {
        let result = self
            .receiver
            .as_mut()
            .ok_or_else(|| AcquisitionError::Task("directory reader missing".to_owned()))?
            .recv()
            .await
            .ok_or_else(|| AcquisitionError::Task("directory reader stopped".to_owned()))?;
        loop {
            let thread = self
                .thread
                .as_ref()
                .ok_or_else(|| AcquisitionError::Task("directory reader missing".to_owned()))?;
            if thread.is_finished() {
                let thread = self
                    .thread
                    .take()
                    .ok_or_else(|| AcquisitionError::Task("directory reader missing".to_owned()))?;
                thread
                    .join()
                    .map_err(|_| AcquisitionError::Task("directory reader panicked".to_owned()))?;
                return result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl Drop for RetainedDirectoryProducer {
    fn drop(&mut self) {
        self.receiver.take();
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

#[cfg(any(target_os = "linux", target_os = "android"))]
async fn spawn_retained_directory_reader(
    root: cap_std::fs::Dir,
    parent_key: String,
    cookie: Option<u64>,
) -> Result<RetainedDirectoryProducer, AcquisitionError> {
    let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
        .await
        .map_err(AcquisitionError::Task)?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let thread = std::thread::Builder::new()
        .name("melodee-retained-directory".to_owned())
        .spawn(move || {
            let _permit = permit;
            let result = read_retained_directory_page(
                &root,
                &parent_key,
                cookie,
                || sender.is_closed(),
                |_| {},
            );
            let _ = sender.blocking_send(result);
        })
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(RetainedDirectoryProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    })
}

/// A mismatch is only a change signal; it never proves a coherent current tree
/// and cannot authorize source removal, publication, or a staging receipt.
fn read_retained_baseline_page(
    root: &cap_std::fs::Dir,
    path_key: &str,
    file_length: i64,
    chunks: &[melodee_persistence::RetainedBaselineChunk],
    rate: u32,
    cancelled: impl Fn() -> bool,
) -> Result<bool, AcquisitionError> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use std::io::{Read, Seek, SeekFrom};
    if file_length < 0 || chunks.len() > 16 {
        return Err(AcquisitionError::SourceChanged);
    }
    let path = crate::source::decode_candidate_relative_path(path_key)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let parent_path = path
        .parent()
        .ok_or_else(|| AcquisitionError::InvalidSourcePath("baseline parent missing".to_owned()))?;
    let name = path
        .file_name()
        .ok_or_else(|| AcquisitionError::InvalidSourcePath("baseline file missing".to_owned()))?;
    let mut parent = root
        .try_clone()
        .map_err(|source| AcquisitionError::Filesystem {
            path: parent_path.to_path_buf(),
            source,
        })?;
    for component in parent_path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(AcquisitionError::InvalidSourcePath(
                "invalid baseline parent".to_owned(),
            ));
        };
        parent = parent
            .open_dir_nofollow(name)
            .map_err(|source| AcquisitionError::Filesystem {
                path: parent_path.to_path_buf(),
                source,
            })?;
    }
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = parent
        .open_with(name, &options)
        .map_err(|error| AcquisitionError::Filesystem {
            path: path.clone(),
            source: error,
        })?
        .into_std();
    let metadata = file
        .metadata()
        .map_err(|error| AcquisitionError::Filesystem {
            path: path.clone(),
            source: error,
        })?;
    if !metadata.is_file() {
        return Err(AcquisitionError::SourceChanged);
    }
    if metadata.len() != file_length as u64 {
        // An observed length difference is an accelerator for requesting a
        // fresh cryptographic verification, not a content proof.
        return Ok(true);
    }
    let mut mismatch = false;
    let mut buffer = [0_u8; 64 * 1024];
    for chunk in chunks {
        if cancelled() {
            return Err(AcquisitionError::Task("retained page cancelled".to_owned()));
        }
        let index = u64::try_from(chunk.index).map_err(|_| AcquisitionError::SourceChanged)?;
        let length = usize::try_from(chunk.length).map_err(|_| AcquisitionError::SourceChanged)?;
        if length == 0 || length > buffer.len() {
            return Err(AcquisitionError::SourceChanged);
        }
        let offset = index
            .checked_mul(buffer.len() as u64)
            .ok_or(AcquisitionError::SourceChanged)?;
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > metadata.len())
        {
            return Err(AcquisitionError::SourceChanged);
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| AcquisitionError::Filesystem {
                path: path.clone(),
                source: error,
            })?;
        pace_retained_read(length, rate, &cancelled)?;
        file.read_exact(&mut buffer[..length])
            .map_err(|error| AcquisitionError::Filesystem {
                path: path.clone(),
                source: error,
            })?;
        mismatch |= crate::legacy_custody_verify::staging_chunk_digest(index, &buffer[..length])
            != chunk.digest;
    }
    if file
        .metadata()
        .map_err(|error| AcquisitionError::Filesystem {
            path,
            source: error,
        })?
        .len()
        != metadata.len()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok(mismatch)
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub async fn test_custody_producers_quiescent() -> bool {
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        crate::reconcile::acquire_custody_reaper_permit(),
    )
    .await;
    let Ok(Ok(_first)) = first else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            crate::reconcile::acquire_custody_reaper_permit()
        )
        .await,
        Ok(Ok(_))
    )
}

/// Provisional evidence only. Publishing a receipt requires an additional
/// atomic binding and a protected, independently verified staged copy.
pub(crate) struct ProvisionalCustodyDigest {
    #[allow(dead_code)]
    pub(crate) capture_id: uuid::Uuid,
    #[allow(dead_code)]
    pub(crate) digest: String,
    #[allow(dead_code)]
    pub(crate) entries: u64,
    pub(crate) seal: melodee_persistence::SealedCustodyHash,
}

#[derive(Clone)]
struct ObservationTarget {
    purpose: melodee_persistence::CustodyHashPurpose,
    generation: String,
    binding: String,
    open_verified:
        std::sync::Arc<dyn Fn() -> Result<cap_std::fs::Dir, AcquisitionError> + Send + Sync>,
}

impl ObservationTarget {
    fn source(
        root: crate::source::GuardedRoot,
        candidate: crate::source::GuardedCandidate,
    ) -> Result<Self, AcquisitionError> {
        let generation = root.generation(&candidate)?;
        let binding = crate::source::candidate_relative_path(&candidate.relative_path);
        Ok(Self {
            purpose: melodee_persistence::CustodyHashPurpose::Source,
            generation,
            binding,
            open_verified: std::sync::Arc::new(move || {
                root.verify_current()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                root.verify_candidate(&candidate)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                root.cap_directory(&candidate)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))
            }),
        })
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[allow(dead_code)]
    fn staging(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: crate::source::GuardedRoot,
        name: std::ffi::OsString,
        staging_root: &std::path::Path,
        item_id: uuid::Uuid,
    ) -> Result<Self, AcquisitionError> {
        use std::os::unix::fs::MetadataExt;
        let binding = format!("releases/{item_id}/source");
        if !staging_root.is_absolute()
            || name != std::ffi::OsStr::new("source")
            || namespace.private_path.join(&name) != staging_root.join(&binding)
        {
            return Err(AcquisitionError::IncompleteReceipt(
                "staging namespace does not match the receipt path".to_owned(),
            ));
        }
        namespace.revalidate(&root)?;
        let held = open_child_directory(&namespace.private, &name)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let generation = source_generation_from_directory(&held, std::path::Path::new(&name))?;
        let held = std::sync::Arc::new(held);
        Ok(Self {
            purpose: melodee_persistence::CustodyHashPurpose::Staging,
            generation,
            binding,
            open_verified: std::sync::Arc::new(move || {
                namespace.revalidate(&root)?;
                let current = open_child_directory(&namespace.private, &name)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                let a = current
                    .metadata()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                let b = held
                    .metadata()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
                    return Err(AcquisitionError::SourceChanged);
                }
                held.try_clone()
                    .map(cap_std::fs::Dir::from_std_file)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))
            }),
        })
    }
}

#[allow(dead_code)]
async fn with_custody_lease<T>(
    future: impl std::future::Future<Output = Result<T, AcquisitionError>>,
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
) -> Result<T, AcquisitionError> {
    let interval = crate::normalize::heartbeat_interval(lease_duration)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    tokio::pin!(future);
    loop {
        tokio::select! {
            outcome = &mut future => return outcome,
            _ = ticker.tick() => {
                let heartbeat = store.renew_custody_hash_lease(pool, item, worker, lease_duration);
                tokio::pin!(heartbeat);
                // The in-flight operation may hold the item row needed by the
                // heartbeat. Keep polling both to avoid deadlocking its commit.
                tokio::select! {
                    outcome = &mut future => return outcome,
                    result = &mut heartbeat => { result?; }
                }
            }
        }
    }
}

/// Capture a bounded, restartable content observation under an active lease.
/// All records remain provisional and can be cleaned without touching media.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
async fn capture_provisional_custody_digest(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    target: ObservationTarget,
) -> Result<ProvisionalCustodyDigest, AcquisitionError> {
    with_custody_lease(
        capture_provisional_custody_digest_inner(store, pool, item, worker, lease_duration, target),
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
async fn capture_provisional_custody_digest_inner(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    target: ObservationTarget,
) -> Result<ProvisionalCustodyDigest, AcquisitionError> {
    let prior = with_custody_lease(
        async {
            store
                .custody_hash_spool_for_item(pool, item, target.purpose)
                .await
                .map_err(AcquisitionError::from)
        },
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let capture_id = if let Some(prior) = prior {
        if prior.source_generation != target.generation
            || prior.target_binding != target.binding
            || prior.state == "discarding"
        {
            with_custody_lease(
                async {
                    store
                        .discard_custody_hash_spool_page(
                            pool,
                            prior.id,
                            item,
                            worker,
                            time::OffsetDateTime::now_utc(),
                        )
                        .await
                        .map_err(AcquisitionError::from)
                },
                store,
                pool,
                item,
                worker,
                lease_duration,
            )
            .await?;
            return Err(AcquisitionError::IncompleteReceipt(
                "obsolete custody observations are being cleaned; retry".to_owned(),
            ));
        }
        with_custody_lease(
            async {
                store
                    .revalidate_custody_hash_spool(
                        pool,
                        prior.id,
                        item,
                        worker,
                        &target.generation,
                        uuid::Uuid::now_v7(),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await
                    .map_err(AcquisitionError::from)
            },
            store,
            pool,
            item,
            worker,
            lease_duration,
        )
        .await?;
        prior.id
    } else {
        with_custody_lease(
            async {
                store
                    .begin_custody_hash_spool(
                        pool,
                        item,
                        worker,
                        target.purpose,
                        &target.generation,
                        &target.binding,
                        time::OffsetDateTime::now_utc(),
                    )
                    .await
                    .map_err(AcquisitionError::from)
            },
            store,
            pool,
            item,
            worker,
            lease_duration,
        )
        .await?
    };
    let open_verified = target.open_verified.clone();
    let directory = with_custody_lease(
        async move {
            crate::admission::run(move || open_verified())
                .await
                .map_err(AcquisitionError::Task)?
        },
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let mut producer = with_custody_lease(
        spawn_custody_producer(directory, None),
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let mut batch = Vec::with_capacity(melodee_persistence::CUSTODY_HASH_PAGE_SIZE);
    let mut batch_bytes = 0_usize;
    loop {
        let receiver = producer
            .receiver
            .as_mut()
            .ok_or_else(|| AcquisitionError::Task("custody receiver missing".to_owned()))?;
        let next = with_custody_lease(
            async { Ok(receiver.recv().await) },
            store,
            pool,
            item,
            worker,
            lease_duration,
        )
        .await?;
        let Some(entry) = next else {
            break;
        };
        let entry_bytes = entry.logical_bytes();
        if entry_bytes > melodee_persistence::CUSTODY_HASH_PAGE_BYTES {
            return Err(AcquisitionError::IncompleteReceipt(
                "custody observation exceeds page budget".to_owned(),
            ));
        }
        if !batch.is_empty()
            && batch_bytes.saturating_add(entry_bytes)
                > melodee_persistence::CUSTODY_HASH_PAGE_BYTES
        {
            with_custody_lease(
                async {
                    store
                        .append_custody_hash_spool(
                            pool,
                            capture_id,
                            item,
                            worker,
                            &batch,
                            time::OffsetDateTime::now_utc(),
                        )
                        .await
                        .map_err(AcquisitionError::from)
                },
                store,
                pool,
                item,
                worker,
                lease_duration,
            )
            .await?;
            batch.clear();
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes.saturating_add(entry_bytes);
        batch.push(entry);
        if batch.len() == melodee_persistence::CUSTODY_HASH_PAGE_SIZE {
            with_custody_lease(
                async {
                    store
                        .append_custody_hash_spool(
                            pool,
                            capture_id,
                            item,
                            worker,
                            &batch,
                            time::OffsetDateTime::now_utc(),
                        )
                        .await
                        .map_err(AcquisitionError::from)
                },
                store,
                pool,
                item,
                worker,
                lease_duration,
            )
            .await?;
            batch.clear();
            batch_bytes = 0;
        }
    }
    if !batch.is_empty() {
        with_custody_lease(
            async {
                store
                    .append_custody_hash_spool(
                        pool,
                        capture_id,
                        item,
                        worker,
                        &batch,
                        time::OffsetDateTime::now_utc(),
                    )
                    .await
                    .map_err(AcquisitionError::from)
            },
            store,
            pool,
            item,
            worker,
            lease_duration,
        )
        .await?;
    }
    let count =
        with_custody_lease(producer.finish(), store, pool, item, worker, lease_duration).await?;
    let expected = i64::try_from(count).map_err(|_| AcquisitionError::SourceChanged)?;
    let open_verified = target.open_verified.clone();
    with_custody_lease(
        async move {
            crate::admission::run(move || open_verified().map(drop))
                .await
                .map_err(AcquisitionError::Task)?
        },
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    with_custody_lease(
        async {
            store
                .freeze_custody_hash_spool(
                    pool,
                    capture_id,
                    item,
                    worker,
                    expected,
                    time::OffsetDateTime::now_utc(),
                )
                .await
                .map_err(AcquisitionError::from)
        },
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let seal = with_custody_lease(
        async {
            store
                .seal_custody_hash_spool(pool, capture_id, item, worker)
                .await
                .map_err(AcquisitionError::from)
        },
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    if u64::try_from(seal.entries()).ok() != Some(count) {
        return Err(AcquisitionError::SourceChanged);
    }
    let open_verified = target.open_verified.clone();
    with_custody_lease(
        async move {
            crate::admission::run(move || open_verified().map(drop))
                .await
                .map_err(AcquisitionError::Task)?
        },
        store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    Ok(ProvisionalCustodyDigest {
        capture_id,
        digest: seal.digest().to_owned(),
        entries: count,
        seal,
    })
}

/// Reobservation has a separate token-fenced lease and must never renew or
/// claim the processing work item. Poll the traversal during a blocked
/// heartbeat because its current page may hold the retained lease row.
async fn with_retained_lease<T>(
    future: impl std::future::Future<Output = Result<T, AcquisitionError>>,
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    lease: &melodee_persistence::RetainedSourceLease,
    duration: time::Duration,
) -> Result<T, AcquisitionError> {
    let interval = crate::normalize::heartbeat_interval(duration)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    tokio::pin!(future);
    loop {
        tokio::select! {
            outcome = &mut future => return outcome,
            _ = ticker.tick() => {
                let heartbeat = store.renew_retained_source_lease(pool, lease, duration);
                tokio::pin!(heartbeat);
                tokio::select! {
                    outcome = &mut future => return outcome,
                    result = &mut heartbeat => { result?; }
                }
            }
        }
    }
}

async fn capture_retained_digest_inner(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    worker: &melodee_persistence::WorkerId,
    lease: &melodee_persistence::RetainedSourceLease,
    target: ObservationTarget,
    retained_mib_per_second: u32,
) -> Result<ProvisionalCustodyDigest, AcquisitionError> {
    use melodee_persistence::{
        CUSTODY_HASH_PAGE_BYTES, CUSTODY_HASH_PAGE_SIZE, CustodyHashPurpose,
    };
    let item = lease.work_item_id();
    let token = lease.token();
    if target.purpose != CustodyHashPurpose::Retained || target.binding != lease.source_path() {
        return Err(AcquisitionError::SourceChanged);
    }
    let prior = store
        .custody_hash_spool_for_item(pool, item, CustodyHashPurpose::Retained)
        .await?;
    let capture = if let Some(prior) = prior {
        if prior.retained_lease_token != Some(token)
            || prior.source_generation != target.generation
            || prior.target_binding != target.binding
            || prior.state == "discarding"
        {
            store
                .discard_custody_hash_spool_page_retained(
                    token,
                    pool,
                    prior.id,
                    item,
                    worker,
                    time::OffsetDateTime::now_utc(),
                )
                .await?;
            return Err(AcquisitionError::IncompleteReceipt(
                "obsolete retained observations are being cleaned; retry".to_owned(),
            ));
        }
        store
            .revalidate_custody_hash_spool_retained(
                token,
                pool,
                prior.id,
                item,
                worker,
                &target.generation,
                uuid::Uuid::now_v7(),
                time::OffsetDateTime::now_utc(),
            )
            .await?;
        prior.id
    } else {
        store
            .begin_custody_hash_spool_retained(
                token,
                pool,
                item,
                worker,
                CustodyHashPurpose::Retained,
                &target.generation,
                &target.binding,
                time::OffsetDateTime::now_utc(),
            )
            .await?
    };
    let open = target.open_verified.clone();
    let directory = crate::admission::run(move || open())
        .await
        .map_err(AcquisitionError::Task)??;
    let mut producer = spawn_custody_producer(directory, Some(retained_mib_per_second)).await?;
    let mut batch = Vec::with_capacity(CUSTODY_HASH_PAGE_SIZE);
    let mut batch_bytes = 0_usize;
    loop {
        let receiver = producer
            .receiver
            .as_mut()
            .ok_or_else(|| AcquisitionError::Task("custody receiver missing".to_owned()))?;
        let Some(entry) = receiver.recv().await else {
            break;
        };
        let size = entry.logical_bytes();
        if size > CUSTODY_HASH_PAGE_BYTES {
            return Err(AcquisitionError::IncompleteReceipt(
                "custody observation exceeds page budget".to_owned(),
            ));
        }
        if !batch.is_empty()
            && (batch.len() == CUSTODY_HASH_PAGE_SIZE
                || batch_bytes.saturating_add(size) > CUSTODY_HASH_PAGE_BYTES)
        {
            store
                .append_custody_hash_spool_retained(
                    token,
                    pool,
                    capture,
                    item,
                    worker,
                    &batch,
                    time::OffsetDateTime::now_utc(),
                )
                .await?;
            batch.clear();
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes.saturating_add(size);
        batch.push(entry);
    }
    if !batch.is_empty() {
        store
            .append_custody_hash_spool_retained(
                token,
                pool,
                capture,
                item,
                worker,
                &batch,
                time::OffsetDateTime::now_utc(),
            )
            .await?;
    }
    let count = producer.finish().await?;
    let expected = i64::try_from(count).map_err(|_| AcquisitionError::SourceChanged)?;
    let open = target.open_verified.clone();
    crate::admission::run(move || open().map(drop))
        .await
        .map_err(AcquisitionError::Task)??;
    store
        .freeze_custody_hash_spool_retained(
            token,
            pool,
            capture,
            item,
            worker,
            expected,
            time::OffsetDateTime::now_utc(),
        )
        .await?;
    let seal = store
        .seal_custody_hash_spool_retained(token, pool, capture, item, worker)
        .await?;
    if u64::try_from(seal.entries()).ok() != Some(count) {
        return Err(AcquisitionError::SourceChanged);
    }
    let open = target.open_verified.clone();
    crate::admission::run(move || open().map(drop))
        .await
        .map_err(AcquisitionError::Task)??;
    Ok(ProvisionalCustodyDigest {
        capture_id: capture,
        digest: seal.digest().to_owned(),
        entries: count,
        seal,
    })
}

/// Reobserve a retained occurrence without touching the producer's bytes.
/// A missing or ambiguous root is a retry, never an absence declaration.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
#[derive(Debug, thiserror::Error)]
pub(crate) enum RetainedReobserveError {
    #[error("retained source root is unavailable")]
    Missing,
    #[error("retained source root is ambiguous")]
    Ambiguous,
    #[error(transparent)]
    Acquisition(#[from] AcquisitionError),
}

pub(crate) type RetainedObservationHook = dyn Fn() -> Result<(), AcquisitionError> + Send + Sync;

/// Scan one bounded directory page for changes relative to the verified
/// baseline. The resulting signal never proves absence or authorizes intake.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) async fn reobserve_retained_directory_page(
    config: &crate::config::IngestionConfig,
    pool: &sqlx::PgPool,
    store: &melodee_persistence::WorkItemStore,
    lease: &melodee_persistence::RetainedSourceLease,
    duration: time::Duration,
    next_check: time::Duration,
) -> Result<(), RetainedReobserveError> {
    let mut matching = config
        .sources
        .iter()
        .filter(|source| source.id == lease.source_library_id());
    let source = matching
        .next()
        .cloned()
        .ok_or(RetainedReobserveError::Missing)?;
    if matching.next().is_some() {
        return Err(RetainedReobserveError::Ambiguous);
    }
    let path = crate::source::decode_candidate_relative_path(lease.source_path())
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    enum DirectoryTurn {
        Read(
            melodee_persistence::RetainedBaselineDirectory,
            RetainedDirectoryContinuation,
            String,
            bool,
        ),
        Unreadable(
            melodee_persistence::RetainedBaselineDirectory,
            melodee_persistence::RetainedBaselineUnreadableReason,
        ),
    }
    let operation = async {
        let page = store.next_retained_baseline_directory(pool, lease).await?;
        let cookie = page
            .cookie()
            .map(|value| u64::from_str_radix(value, 16))
            .transpose()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let target = crate::admission::run(move || {
            let root = GuardedRoot::open(source)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let candidate = root
                .resolve_relative(&path)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?
                .ok_or(AcquisitionError::SourceNotConfigured)?;
            ObservationTarget::source(root, candidate)
        })
        .await
        .map_err(AcquisitionError::Task)??;
        if target.binding != lease.source_path() {
            return Err(AcquisitionError::SourceChanged);
        }
        let open = target.open_verified.clone();
        let directory = crate::admission::run(move || open())
            .await
            .map_err(AcquisitionError::Task)??;
        let read = spawn_retained_directory_reader(directory, page.path().to_owned(), cookie)
            .await?
            .finish()
            .await;
        let verify = target.open_verified.clone();
        crate::admission::run(move || verify())
            .await
            .map_err(AcquisitionError::Task)??;
        let (entries, continuation, generation) = match read {
            Ok(result) => result,
            Err(AcquisitionError::Filesystem { source, .. }) => {
                let reason = if source.kind() == std::io::ErrorKind::NotFound {
                    melodee_persistence::RetainedBaselineUnreadableReason::Missing
                } else {
                    melodee_persistence::RetainedBaselineUnreadableReason::Unreadable
                };
                return Ok(DirectoryTurn::Unreadable(page, reason));
            }
            Err(AcquisitionError::SourceChanged) => {
                return Ok(DirectoryTurn::Unreadable(
                    page,
                    melodee_persistence::RetainedBaselineUnreadableReason::Unreadable,
                ));
            }
            Err(error) => return Err(error),
        };
        let mut mismatch = continuation == RetainedDirectoryContinuation::Invalidated;
        mismatch |= target.generation != lease.baseline_generation();
        let mut members = Vec::with_capacity(entries.len());
        for (path_key, kind) in entries {
            if let Some(is_directory) = kind {
                members.push(melodee_persistence::RetainedDirectoryMember {
                    path_key,
                    is_directory,
                });
            } else {
                mismatch = true;
            }
        }
        let matches = store
            .retained_directory_membership(pool, lease, &page, &members)
            .await?;
        mismatch |= matches.iter().any(|matched| !matched);
        Ok(DirectoryTurn::Read(
            page,
            continuation,
            generation,
            mismatch,
        ))
    };
    match with_retained_lease(operation, store, pool, lease, duration).await? {
        DirectoryTurn::Read(page, continuation, generation, mismatch) => {
            let cookie = match continuation {
                RetainedDirectoryContinuation::More(value) => Some(format!("{value:016x}")),
                RetainedDirectoryContinuation::Eof | RetainedDirectoryContinuation::Invalidated => {
                    None
                }
            };
            let continuation = match continuation {
                RetainedDirectoryContinuation::More(_) => {
                    melodee_persistence::RetainedDirectoryContinuation::More(
                        cookie.as_deref().ok_or_else(|| {
                            AcquisitionError::Task("missing directory cookie".to_owned())
                        })?,
                    )
                }
                RetainedDirectoryContinuation::Eof => {
                    melodee_persistence::RetainedDirectoryContinuation::Eof
                }
                RetainedDirectoryContinuation::Invalidated => {
                    melodee_persistence::RetainedDirectoryContinuation::Invalidated
                }
            };
            store
                .advance_retained_baseline_directory(
                    pool,
                    lease,
                    &page,
                    melodee_persistence::RetainedDirectoryAdvance {
                        continuation,
                        generation: &generation,
                        mismatch,
                        yield_after: time::Duration::seconds(1),
                        recheck_after: next_check,
                    },
                )
                .await
                .map_err(AcquisitionError::from)?;
        }
        DirectoryTurn::Unreadable(page, reason) => {
            store
                .unreadable_retained_baseline_directory(
                    pool,
                    lease,
                    &page,
                    reason,
                    time::Duration::seconds(1),
                )
                .await
                .map_err(AcquisitionError::from)?;
        }
    }
    Ok(())
}

/// Visit one bounded baseline page. This is only a rolling change detector:
/// neither a complete pass nor a matching page proves a current snapshot.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(crate) async fn reobserve_retained_baseline_page(
    config: &crate::config::IngestionConfig,
    pool: &sqlx::PgPool,
    store: &melodee_persistence::WorkItemStore,
    lease: &melodee_persistence::RetainedSourceLease,
    duration: time::Duration,
    next_check: time::Duration,
) -> Result<(), RetainedReobserveError> {
    let mut matching = config
        .sources
        .iter()
        .filter(|source| source.id == lease.source_library_id());
    let source = matching
        .next()
        .cloned()
        .ok_or(RetainedReobserveError::Missing)?;
    if matching.next().is_some() {
        return Err(RetainedReobserveError::Ambiguous);
    }
    let path = crate::source::decode_candidate_relative_path(lease.source_path())
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    enum PageTurn {
        Read(melodee_persistence::RetainedBaselinePage, bool),
        Unreadable(
            melodee_persistence::RetainedBaselinePage,
            melodee_persistence::RetainedBaselineUnreadableReason,
        ),
        Pass(melodee_persistence::RetainedBaselinePass, bool),
    }
    let operation = async {
        let target = crate::admission::run(move || {
            let root = GuardedRoot::open(source)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let candidate = root
                .resolve_relative(&path)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?
                .ok_or(AcquisitionError::SourceNotConfigured)?;
            ObservationTarget::source(root, candidate)
        })
        .await
        .map_err(AcquisitionError::Task)??;
        if target.binding != lease.source_path() {
            return Err(AcquisitionError::SourceChanged);
        }
        match store.retained_baseline_next_page(pool, lease).await? {
            melodee_persistence::RetainedBaselineRead::Page(page) => {
                let open = target.open_verified.clone();
                let directory = crate::admission::run(move || open())
                    .await
                    .map_err(AcquisitionError::Task)??;
                let result = spawn_retained_page_reader(
                    directory,
                    page.path().to_owned(),
                    page.file_length(),
                    page.chunks().to_vec(),
                    config.retained_recheck_mib_per_second,
                )
                .await?
                .wait_for_page()
                .await;
                // The page result is a hint only, but avoid advancing a cursor
                // for a detached root or a replaced candidate.
                let verify = target.open_verified.clone();
                crate::admission::run(move || verify())
                    .await
                    .map_err(AcquisitionError::Task)??;
                match result {
                    Ok(changed) => Ok(PageTurn::Read(
                        page,
                        changed != 0 || target.generation != lease.baseline_generation(),
                    )),
                    Err(AcquisitionError::Filesystem { source, .. }) => {
                        let reason = if source.kind() == std::io::ErrorKind::NotFound {
                            melodee_persistence::RetainedBaselineUnreadableReason::Missing
                        } else {
                            melodee_persistence::RetainedBaselineUnreadableReason::Unreadable
                        };
                        Ok(PageTurn::Unreadable(page, reason))
                    }
                    Err(AcquisitionError::SourceChanged) => Ok(PageTurn::Unreadable(
                        page,
                        melodee_persistence::RetainedBaselineUnreadableReason::Unreadable,
                    )),
                    Err(error) => Err(error),
                }
            }
            melodee_persistence::RetainedBaselineRead::PassComplete(pass) => {
                let verify = target.open_verified.clone();
                crate::admission::run(move || verify())
                    .await
                    .map_err(AcquisitionError::Task)??;
                Ok(PageTurn::Pass(
                    pass,
                    target.generation != lease.baseline_generation(),
                ))
            }
        }
    };
    let turn = with_retained_lease(operation, store, pool, lease, duration).await?;
    match turn {
        PageTurn::Read(page, changed) => {
            store
                .acknowledge_retained_baseline_page(pool, lease, &page, changed)
                .await
                .map_err(AcquisitionError::from)?;
            store
                .yield_retained_baseline_source(pool, lease, time::Duration::seconds(1))
                .await
                .map_err(AcquisitionError::from)?;
        }
        PageTurn::Unreadable(page, reason) => {
            store
                .unreadable_retained_baseline_page(
                    pool,
                    lease,
                    &page,
                    reason,
                    time::Duration::seconds(1),
                )
                .await
                .map_err(AcquisitionError::from)?;
        }
        PageTurn::Pass(pass, changed) => {
            store
                .complete_retained_baseline_pass(pool, lease, &pass, changed, next_check)
                .await
                .map_err(AcquisitionError::from)?;
            store
                .yield_retained_baseline_source(pool, lease, time::Duration::seconds(1))
                .await
                .map_err(AcquisitionError::from)?;
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reobserve_retained_source(
    config: &crate::config::IngestionConfig,
    pool: &sqlx::PgPool,
    store: &melodee_persistence::WorkItemStore,
    worker: &melodee_persistence::WorkerId,
    lease: &melodee_persistence::RetainedSourceLease,
    duration: time::Duration,
    next_check: time::Duration,
    after_observation: Option<&RetainedObservationHook>,
) -> Result<melodee_persistence::RetainedRecheckOutcome, RetainedReobserveError> {
    let mut matching = config
        .sources
        .iter()
        .filter(|source| source.id == lease.source_library_id());
    let source = matching
        .next()
        .cloned()
        .ok_or(RetainedReobserveError::Missing)?;
    if matching.next().is_some() {
        return Err(RetainedReobserveError::Ambiguous);
    }
    let path = crate::source::decode_candidate_relative_path(lease.source_path())
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let operation = async {
        let resolved = crate::admission::run(move || {
            let root = GuardedRoot::open(source)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let candidate = root
                .resolve_relative(&path)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?
                .ok_or(AcquisitionError::SourceNotConfigured)?;
            root.verify_current()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let held = root
                .custody_root_handle()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let root_generation =
                source_generation_from_directory(&held, std::path::Path::new("retained-root"))?;
            let mut target = ObservationTarget::source(root.clone(), candidate)?;
            target.purpose = melodee_persistence::CustodyHashPurpose::Retained;
            Ok::<_, AcquisitionError>((root, root_generation, target))
        })
        .await
        .map_err(AcquisitionError::Task)??;
        let (root, root_generation, target) = resolved;
        let before = capture_retained_digest_inner(
            store,
            pool,
            worker,
            lease,
            target,
            config.retained_recheck_mib_per_second,
        )
        .await?;
        let session = store
            .begin_retained_recheck(pool, lease, &before.seal, &root_generation)
            .await?;
        let root_again = root.clone();
        let path_again = crate::source::decode_candidate_relative_path(lease.source_path())
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let target = crate::admission::run(move || {
            let candidate = root_again
                .resolve_relative(&path_again)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?
                .ok_or(AcquisitionError::SourceNotConfigured)?;
            let mut target = ObservationTarget::source(root_again, candidate)?;
            target.purpose = melodee_persistence::CustodyHashPurpose::Retained;
            Ok::<_, AcquisitionError>(target)
        })
        .await
        .map_err(AcquisitionError::Task)??;
        let after = capture_retained_digest_inner(
            store,
            pool,
            worker,
            lease,
            target,
            config.retained_recheck_mib_per_second,
        )
        .await?;
        let current_root = crate::admission::run(move || {
            root.verify_current()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let held = root
                .custody_root_handle()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            source_generation_from_directory(&held, std::path::Path::new("retained-root"))
        })
        .await
        .map_err(AcquisitionError::Task)??;
        if current_root != root_generation {
            return Err(AcquisitionError::SourceChanged);
        }
        if let Some(hook) = after_observation {
            hook()?;
        }
        store
            .finish_retained_recheck(
                pool,
                lease,
                &session,
                &after.seal,
                &current_root,
                next_check,
            )
            .await
            .map_err(AcquisitionError::from)
    };
    with_retained_lease(operation, store, pool, lease, duration)
        .await
        .map_err(Into::into)
}

#[allow(dead_code)]
struct RetentionProofSession {
    id: uuid::Uuid,
    before: melodee_persistence::SealedCustodyHash,
    staging: crate::legacy_custody_verify::VerifiedStagingLegacy,
}

#[allow(dead_code)]
async fn validate_frozen_seal(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    seal: &melodee_persistence::SealedCustodyHash,
) -> Result<(), AcquisitionError> {
    let purpose = match seal.purpose() {
        melodee_persistence::CustodyHashPurpose::Source => "source",
        melodee_persistence::CustodyHashPurpose::Staging => "staging",
        melodee_persistence::CustodyHashPurpose::Retained => {
            return Err(AcquisitionError::IncompleteReceipt(
                "retained reobservation cannot bind staging".to_owned(),
            ));
        }
    };
    let valid: Option<bool> = sqlx::query_scalar("SELECT true FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND purpose=$4 AND protected_claim_epoch IS NULL AND source_generation=$5 AND target_binding COLLATE \"C\"=$6 COLLATE \"C\" AND state='frozen' AND observation_epoch=$7 AND sealed_epoch=$7 AND sealed_digest=$8 AND entry_count=$9 FOR UPDATE")
        .bind(seal.capture_id()).bind(item).bind(worker.as_str()).bind(purpose).bind(seal.generation()).bind(seal.target_binding()).bind(seal.epoch()).bind(seal.digest()).bind(seal.entries())
        .fetch_optional(&mut **tx).await.map_err(|error| AcquisitionError::Persistence(error.into()))?;
    if valid.is_none() {
        return Err(AcquisitionError::IncompleteReceipt(
            "retained custody seal is stale".to_owned(),
        ));
    }
    Ok(())
}

#[allow(dead_code)]
async fn lock_live_retention_item(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
) -> Result<(String, Option<uuid::Uuid>), AcquisitionError> {
    let db_error = |error: sqlx::Error| AcquisitionError::Persistence(error.into());
    #[allow(clippy::type_complexity)]
    let row: Option<(String,Option<uuid::Uuid>,bool,Option<String>,Option<time::OffsetDateTime>)> = sqlx::query_as("SELECT source_path,source_library_id,inbound_active,lease_owner,lease_expires_at FROM release_work_items WHERE id=$1 FOR UPDATE")
        .bind(item).fetch_optional(&mut **tx).await.map_err(db_error)?;
    let Some((source_path, library_id, active, owner, expiry)) = row else {
        return Err(AcquisitionError::Persistence(
            melodee_persistence::WorkStoreError::NotFound(item),
        ));
    };
    let db_now: time::OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await
        .map_err(db_error)?;
    if owner.as_deref() != Some(worker.as_str()) || expiry.is_none_or(|end| end <= db_now) {
        return Err(AcquisitionError::Persistence(
            melodee_persistence::WorkStoreError::LeaseExpired(item),
        ));
    }
    if !active {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok((source_path, library_id))
}

#[allow(dead_code)]
async fn ensure_retention_lease_live(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
) -> Result<(), AcquisitionError> {
    let live: bool = sqlx::query_scalar("SELECT COALESCE(lease_owner=$2 AND lease_expires_at>clock_timestamp(),false) FROM release_work_items WHERE id=$1")
        .bind(item).bind(worker.as_str()).fetch_one(&mut **tx).await.map_err(|error| AcquisitionError::Persistence(error.into()))?;
    if !live {
        return Err(AcquisitionError::Persistence(
            melodee_persistence::WorkStoreError::LeaseExpired(item),
        ));
    }
    Ok(())
}

/// Persist the before seal before a staging verification can be registered.
#[allow(dead_code)]
async fn begin_retention_proof_session(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    before: &melodee_persistence::SealedCustodyHash,
) -> Result<uuid::Uuid, AcquisitionError> {
    if before.item_id() != item
        || before.owner() != worker.as_str()
        || before.purpose() != melodee_persistence::CustodyHashPurpose::Source
    {
        return Err(AcquisitionError::IncompleteReceipt(
            "invalid source-before seal".to_owned(),
        ));
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| AcquisitionError::Persistence(error.into()))?;
    let (path, _) = lock_live_retention_item(&mut tx, item, worker).await?;
    if path != before.target_binding() {
        return Err(AcquisitionError::SourceChanged);
    }
    validate_frozen_seal(&mut tx, item, worker, before).await?;
    let id = uuid::Uuid::now_v7();
    let result = sqlx::query("INSERT INTO inbound_retained_proof_sessions(work_item_id,session_id,lease_owner,source_capture_id,source_before_epoch,source_before_digest,state) VALUES($1,$2,$3,$4,$5,$6,'before') ON CONFLICT(work_item_id) DO UPDATE SET session_id=EXCLUDED.session_id,lease_owner=EXCLUDED.lease_owner,source_capture_id=EXCLUDED.source_capture_id,source_before_epoch=EXCLUDED.source_before_epoch,source_before_digest=EXCLUDED.source_before_digest,staging_capture_id=NULL,staging_epoch=NULL,staging_digest=NULL,staging_legacy_hash=NULL,state='before' WHERE inbound_retained_proof_sessions.protected_claim_epoch IS NULL")
        .bind(item).bind(id).bind(worker.as_str()).bind(before.capture_id()).bind(before.epoch()).bind(before.digest()).execute(&mut *tx).await.map_err(|error| AcquisitionError::Persistence(error.into()))?;
    if result.rows_affected() != 1 {
        return Err(AcquisitionError::SourceChanged);
    }
    ensure_retention_lease_live(&mut tx, item, worker).await?;
    tx.commit()
        .await
        .map_err(|error| AcquisitionError::Persistence(error.into()))?;
    Ok(id)
}

/// Register staging only while the original before epoch is still current.
/// Thus a source-after scan started early cannot be replayed past this step.
#[allow(dead_code)]
async fn register_retention_staging(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    id: uuid::Uuid,
    before: &melodee_persistence::SealedCustodyHash,
    staging: &crate::legacy_custody_verify::VerifiedStagingLegacy,
) -> Result<(), AcquisitionError> {
    let stage = staging.seal();
    if stage.item_id() != item
        || stage.owner() != worker.as_str()
        || stage.purpose() != melodee_persistence::CustodyHashPurpose::Staging
        || stage.capture_id() == before.capture_id()
        || stage.digest() != before.digest()
        || stage.entries() != before.entries()
    {
        return Err(AcquisitionError::IncompleteReceipt(
            "staging observation differs from source-before".to_owned(),
        ));
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| AcquisitionError::Persistence(error.into()))?;
    lock_live_retention_item(&mut tx, item, worker).await?;
    let valid: Option<bool> = sqlx::query_scalar("SELECT true FROM inbound_retained_proof_sessions WHERE work_item_id=$1 AND session_id=$2 AND lease_owner=$3 AND protected_claim_epoch IS NULL AND state='before' AND source_capture_id=$4 AND source_before_epoch=$5 AND source_before_digest=$6 FOR UPDATE")
        .bind(item).bind(id).bind(worker.as_str()).bind(before.capture_id()).bind(before.epoch()).bind(before.digest()).fetch_optional(&mut *tx).await.map_err(|error| AcquisitionError::Persistence(error.into()))?;
    if valid.is_none() {
        return Err(AcquisitionError::IncompleteReceipt(
            "retained proof session changed".to_owned(),
        ));
    }
    validate_frozen_seal(&mut tx, item, worker, before).await?;
    validate_frozen_seal(&mut tx, item, worker, stage).await?;
    sqlx::query("UPDATE inbound_retained_proof_sessions SET state='staging_verified',staging_capture_id=$2,staging_epoch=$3,staging_digest=$4,staging_legacy_hash=$5 WHERE work_item_id=$1")
        .bind(item).bind(stage.capture_id()).bind(stage.epoch()).bind(stage.digest()).bind(staging.legacy_hash()).execute(&mut *tx).await.map_err(|error| AcquisitionError::Persistence(error.into()))?;
    ensure_retention_lease_live(&mut tx, item, worker).await?;
    tx.commit()
        .await
        .map_err(|error| AcquisitionError::Persistence(error.into()))?;
    Ok(())
}

/// Bind opaque independent observations to a retained receipt. The verified
/// staging legacy digest can only be constructed by the staging verifier.
/// This remains disconnected until protected copy and reobservation are ready.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
async fn bind_retained_receipt(
    pool: &sqlx::PgPool,
    store: &melodee_persistence::WorkItemStore,
    item_id: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    source_target: &ObservationTarget,
    staging_target: &ObservationTarget,
    session: &RetentionProofSession,
    after: &melodee_persistence::SealedCustodyHash,
) -> Result<(), AcquisitionError> {
    let before = &session.before;
    let stage = session.staging.seal();
    let legacy_hash = session.staging.legacy_hash();
    let valid_hash = legacy_hash.len() == 64
        && legacy_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid_hash
        || [before, stage, after]
            .iter()
            .any(|seal| seal.item_id() != item_id || seal.owner() != worker.as_str())
        || before.purpose() != melodee_persistence::CustodyHashPurpose::Source
        || after.purpose() != melodee_persistence::CustodyHashPurpose::Source
        || stage.purpose() != melodee_persistence::CustodyHashPurpose::Staging
        || before.capture_id() != after.capture_id()
        || before.capture_id() == stage.capture_id()
        || before.epoch() == after.epoch()
        || before.generation() != after.generation()
        || before.target_binding() != after.target_binding()
        || before.digest() != stage.digest()
        || before.digest() != after.digest()
        || before.entries() != stage.entries()
        || before.entries() != after.entries()
        || source_target.purpose != after.purpose()
        || source_target.generation != after.generation()
        || source_target.binding != after.target_binding()
        || staging_target.purpose != stage.purpose()
        || staging_target.generation != stage.generation()
        || staging_target.binding != stage.target_binding()
    {
        return Err(AcquisitionError::IncompleteReceipt(
            "retained custody observations do not match".to_owned(),
        ));
    }
    for target in [staging_target, source_target] {
        let open = target.open_verified.clone();
        with_custody_lease(
            async move {
                crate::admission::run(move || open().map(drop))
                    .await
                    .map_err(AcquisitionError::Task)?
            },
            store,
            pool,
            item_id,
            worker,
            lease_duration,
        )
        .await?;
    }
    let operation = async {
        let db_error = |error: sqlx::Error| AcquisitionError::Persistence(error.into());
        let mut tx = pool.begin().await.map_err(db_error)?;
        let hint: Option<(Option<uuid::Uuid>, String)> = sqlx::query_as(
            "SELECT source_library_id,source_path FROM release_work_items WHERE id=$1",
        )
        .bind(item_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let (hint_library, hint_path) = hint.ok_or_else(|| {
            AcquisitionError::Persistence(melodee_persistence::WorkStoreError::NotFound(item_id))
        })?;
        sqlx::query("SELECT lock_inbound_occurrence($1,$2)")
            .bind(hint_library)
            .bind(&hint_path)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        #[allow(clippy::type_complexity)]
        let item: Option<(String,Option<uuid::Uuid>,bool,Option<String>,Option<time::OffsetDateTime>,i16)> = sqlx::query_as("SELECT source_path,source_library_id,inbound_active,lease_owner,lease_expires_at,discovery_path_format FROM release_work_items WHERE id=$1 FOR UPDATE")
            .bind(item_id).fetch_optional(&mut *tx).await.map_err(db_error)?;
        let Some((source_path, library_id, active, owner, expiry, path_format)) = item else {
            return Err(AcquisitionError::Persistence(
                melodee_persistence::WorkStoreError::NotFound(item_id),
            ));
        };
        let db_now: time::OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?;
        if owner.as_deref() != Some(worker.as_str()) || expiry.is_none_or(|end| end <= db_now) {
            return Err(AcquisitionError::Persistence(
                melodee_persistence::WorkStoreError::LeaseExpired(item_id),
            ));
        }
        if !active
            || library_id != hint_library
            || source_path != hint_path
            || source_path != after.target_binding()
            || (path_format == 0 && source_path.contains('\u{fffd}'))
        {
            return Err(AcquisitionError::SourceChanged);
        }
        let ordered: Option<bool> = sqlx::query_scalar("SELECT true FROM inbound_retained_proof_sessions WHERE work_item_id=$1 AND session_id=$2 AND lease_owner=$3 AND protected_claim_epoch IS NULL AND state IN ('staging_verified','bound') AND source_capture_id=$4 AND source_before_epoch=$5 AND source_before_digest=$6 AND staging_capture_id=$7 AND staging_epoch=$8 AND staging_digest=$9 AND staging_legacy_hash=$10 FOR UPDATE")
            .bind(item_id).bind(session.id).bind(worker.as_str()).bind(before.capture_id()).bind(before.epoch()).bind(before.digest())
            .bind(stage.capture_id()).bind(stage.epoch()).bind(stage.digest()).bind(legacy_hash).fetch_optional(&mut *tx).await.map_err(db_error)?;
        if ordered.is_none() {
            return Err(AcquisitionError::IncompleteReceipt(
                "ordered retained proof session changed".to_owned(),
            ));
        }
        for seal in [after, stage] {
            let purpose = match seal.purpose() {
                melodee_persistence::CustodyHashPurpose::Source => "source",
                melodee_persistence::CustodyHashPurpose::Staging => "staging",
                melodee_persistence::CustodyHashPurpose::Retained => {
                    return Err(AcquisitionError::IncompleteReceipt(
                        "retained reobservation cannot bind staging".to_owned(),
                    ));
                }
            };
            let valid: Option<bool> = sqlx::query_scalar("SELECT true FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND purpose=$4 AND source_generation=$5 AND target_binding COLLATE \"C\"=$6 COLLATE \"C\" AND state='frozen' AND observation_epoch=$7 AND sealed_epoch=$7 AND sealed_digest=$8 AND entry_count=$9 FOR UPDATE")
                .bind(seal.capture_id()).bind(item_id).bind(worker.as_str()).bind(purpose).bind(seal.generation()).bind(seal.target_binding()).bind(seal.epoch()).bind(seal.digest()).bind(seal.entries())
                .fetch_optional(&mut *tx).await.map_err(db_error)?;
            if valid.is_none() {
                return Err(AcquisitionError::IncompleteReceipt(
                    "retained custody seal is stale".to_owned(),
                ));
            }
        }
        let handoff: Option<(String,String,String,Option<String>,bool)> = sqlx::query_as("SELECT status,source_generation,staging_relative_path,tree_hash,retained_source FROM release_work_item_staging_handoffs WHERE work_item_id=$1 FOR UPDATE")
            .bind(item_id).fetch_optional(&mut *tx).await.map_err(db_error)?;
        let Some((status, generation, stage_path, old_hash, retained)) = handoff else {
            return Err(AcquisitionError::IncompleteReceipt(
                "retained custody needs a staging intent".to_owned(),
            ));
        };
        if generation != after.generation() || stage_path != stage.target_binding() {
            return Err(AcquisitionError::SourceChanged);
        }
        if status == "staged" && retained && old_hash.as_deref() == Some(legacy_hash) {
            let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_sources WHERE work_item_id=$1 AND source_library_id IS NOT DISTINCT FROM $2 AND source_path COLLATE \"C\"=$3 COLLATE \"C\" AND source_generation=$4 AND content_digest=$5 AND source_path_format=$6)")
                .bind(item_id).bind(library_id).bind(&source_path).bind(after.generation()).bind(after.digest()).bind(path_format).fetch_one(&mut *tx).await.map_err(db_error)?;
            if !matches {
                return Err(AcquisitionError::IncompleteReceipt(
                    "retained obligation differs".to_owned(),
                ));
            }
        } else if status == "intent" && !retained {
            sqlx::query("UPDATE release_work_item_staging_handoffs SET status='staged',method='verified_copy',tree_hash=$2,staged_at=$3,retained_source=true WHERE work_item_id=$1")
                .bind(item_id).bind(legacy_hash).bind(db_now).execute(&mut *tx).await.map_err(db_error)?;
            sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_library_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at,source_path_format) VALUES($1,$2,$3,$4,$5,$6,$6,$6,$7)")
                .bind(item_id).bind(library_id).bind(&source_path).bind(after.generation()).bind(after.digest()).bind(db_now).bind(path_format).execute(&mut *tx).await.map_err(|error| {
                    if matches!(&error, sqlx::Error::Database(db) if db.code().as_deref() == Some("53400")) {
                        AcquisitionError::Persistence(melodee_persistence::WorkStoreError::RetainedSourceBudgetExhausted)
                    } else {
                        db_error(error)
                    }
                })?;
        } else {
            return Err(AcquisitionError::IncompleteReceipt(
                "staging handoff is not an unclaimed intent".to_owned(),
            ));
        }
        sqlx::query("UPDATE inbound_retained_proof_sessions SET state='bound' WHERE work_item_id=$1 AND session_id=$2")
            .bind(item_id).bind(session.id).execute(&mut *tx).await.map_err(db_error)?;
        let live: bool = sqlx::query_scalar("SELECT COALESCE(lease_owner=$2 AND lease_expires_at>clock_timestamp(),false) FROM release_work_items WHERE id=$1")
            .bind(item_id).bind(worker.as_str()).fetch_one(&mut *tx).await.map_err(db_error)?;
        if !live {
            return Err(AcquisitionError::Persistence(
                melodee_persistence::WorkStoreError::LeaseExpired(item_id),
            ));
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    };
    with_custody_lease(operation, store, pool, item_id, worker, lease_duration).await
}

/// Exercise the provisional capture in integration tests. This does not
/// acquire or publish media and is absent from ordinary builds.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub async fn test_capture_provisional_custody_digest(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    source: crate::config::InboundSource,
    candidate_name: &std::ffi::OsStr,
) -> Result<(uuid::Uuid, String, u64), AcquisitionError> {
    let root = crate::source::GuardedRoot::open(source)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let candidate = root
        .resolve(candidate_name)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?
        .ok_or(AcquisitionError::SourceChanged)?;
    let target = ObservationTarget::source(root, candidate)?;
    let result = capture_provisional_custody_digest(
        &melodee_persistence::WorkItemStore,
        pool,
        item,
        worker,
        lease_duration,
        target,
    )
    .await?;
    Ok((result.capture_id, result.digest, result.entries))
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn test_capture_provisional_staging_digest(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    source: crate::config::InboundSource,
    private_path: &std::path::Path,
    stage_name: &std::ffi::OsStr,
    staging_root: &std::path::Path,
    producer_uid: u32,
) -> Result<(uuid::Uuid, String, u64), AcquisitionError> {
    let namespace = std::sync::Arc::new(validate_custody_namespace(
        private_path,
        &source.root,
        producer_uid,
    )?);
    let root = crate::source::GuardedRoot::open(source)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let target = ObservationTarget::staging(
        namespace,
        root,
        stage_name.to_os_string(),
        staging_root,
        item,
    )?;
    let result = capture_provisional_custody_digest(
        &melodee_persistence::WorkItemStore,
        pool,
        item,
        worker,
        lease_duration,
        target,
    )
    .await?;
    Ok((result.capture_id, result.digest, result.entries))
}

/// Test-only provisional baseline capture; it does not seal file contents or
/// authorize receipt publication.
#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn test_verify_staging_baseline(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    source: crate::config::InboundSource,
    private_path: &std::path::Path,
    staging_root: &std::path::Path,
    producer_uid: u32,
) -> Result<(uuid::Uuid, i64, i64), AcquisitionError> {
    let store = melodee_persistence::WorkItemStore;
    let namespace = std::sync::Arc::new(validate_custody_namespace(
        private_path,
        &source.root,
        producer_uid,
    )?);
    let root =
        GuardedRoot::open(source).map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let target = ObservationTarget::staging(
        namespace,
        root,
        std::ffi::OsString::from("source"),
        staging_root,
        item,
    )?;
    let capture = capture_provisional_custody_digest(
        &store,
        pool,
        item,
        worker,
        lease_duration,
        target.clone(),
    )
    .await?;
    let open = target.open_verified.clone();
    let directory = with_custody_lease(
        async move {
            crate::admission::run(move || open())
                .await
                .map_err(AcquisitionError::Task)?
        },
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let (_, baseline) = with_custody_lease(
        crate::legacy_custody_verify::verify_legacy_staging_with_baseline(
            pool,
            &store,
            item,
            worker,
            capture.seal,
            directory,
        ),
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let open = target.open_verified.clone();
    with_custody_lease(
        async move {
            crate::admission::run(move || open().map(drop))
                .await
                .map_err(AcquisitionError::Task)?
        },
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    Ok((baseline.id(), baseline.file_count(), baseline.chunk_count()))
}

/// Test-only orchestration over pre-created, protected staging. Production
/// still needs an occurrence-bound copy and retained-source reobservation.
#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn test_bind_retained_receipt(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    source: crate::config::InboundSource,
    candidate_name: &std::ffi::OsStr,
    private_path: &std::path::Path,
    staging_root: &std::path::Path,
    producer_uid: u32,
    early_after_mutation: Option<(std::path::PathBuf, Vec<u8>)>,
) -> Result<String, AcquisitionError> {
    let store = melodee_persistence::WorkItemStore;
    let root = GuardedRoot::open(source.clone())
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let candidate = root
        .resolve(candidate_name)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?
        .ok_or(AcquisitionError::SourceChanged)?;
    let source_target = ObservationTarget::source(root.clone(), candidate)?;
    let namespace = std::sync::Arc::new(validate_custody_namespace(
        private_path,
        &source.root,
        producer_uid,
    )?);
    let stage_target = ObservationTarget::staging(
        namespace,
        root,
        std::ffi::OsString::from("source"),
        staging_root,
        item,
    )?;
    let before = capture_provisional_custody_digest(
        &store,
        pool,
        item,
        worker,
        lease_duration,
        source_target.clone(),
    )
    .await?;
    let session_id = with_custody_lease(
        begin_retention_proof_session(pool, item, worker, &before.seal),
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    if early_after_mutation.is_some() {
        let _early = capture_provisional_custody_digest(
            &store,
            pool,
            item,
            worker,
            lease_duration,
            source_target.clone(),
        )
        .await?;
    }
    let staged = capture_provisional_custody_digest(
        &store,
        pool,
        item,
        worker,
        lease_duration,
        stage_target.clone(),
    )
    .await?;
    let open = stage_target.open_verified.clone();
    let directory = with_custody_lease(
        async move {
            crate::admission::run(move || open())
                .await
                .map_err(AcquisitionError::Task)?
        },
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let legacy = with_custody_lease(
        crate::legacy_custody_verify::verify_legacy_staging(
            pool,
            &store,
            item,
            worker,
            lease_duration,
            staged.seal.clone(),
            directory,
        ),
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    if let Some((path, bytes)) = early_after_mutation {
        std::fs::write(path, bytes).map_err(|error| AcquisitionError::Task(error.to_string()))?;
    }
    with_custody_lease(
        register_retention_staging(pool, item, worker, session_id, &before.seal, &legacy),
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await?;
    let legacy_hash = legacy.legacy_hash().to_owned();
    let session = RetentionProofSession {
        id: session_id,
        before: before.seal,
        staging: legacy,
    };
    if bind_retained_receipt(
        pool,
        &store,
        item,
        worker,
        lease_duration,
        &source_target,
        &stage_target,
        &session,
        &session.before,
    )
    .await
    .is_ok()
    {
        return Err(AcquisitionError::IncompleteReceipt(
            "duplicate source epoch was accepted".to_owned(),
        ));
    }
    let after = capture_provisional_custody_digest(
        &store,
        pool,
        item,
        worker,
        lease_duration,
        source_target.clone(),
    )
    .await?;
    bind_retained_receipt(
        pool,
        &store,
        item,
        worker,
        lease_duration,
        &source_target,
        &stage_target,
        &session,
        &after.seal,
    )
    .await?;
    bind_retained_receipt(
        pool,
        &store,
        item,
        worker,
        lease_duration,
        &source_target,
        &stage_target,
        &session,
        &after.seal,
    )
    .await?;
    Ok(legacy_hash)
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub async fn test_custody_lease_wait(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
    lock_row: bool,
) -> Result<(), AcquisitionError> {
    let store = melodee_persistence::WorkItemStore;
    with_custody_lease(
        async {
            if lock_row {
                let mut tx = pool
                    .begin()
                    .await
                    .map_err(melodee_persistence::WorkStoreError::from)?;
                sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
                    .bind(item)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(melodee_persistence::WorkStoreError::from)?;
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                tx.commit()
                    .await
                    .map_err(melodee_persistence::WorkStoreError::from)?;
            } else {
                for _ in 0..10 {
                    tokio::time::sleep(std::time::Duration::from_millis(160)).await;
                }
            }
            Ok(())
        },
        &store,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub async fn test_custody_lease_pending(
    pool: &sqlx::PgPool,
    item: uuid::Uuid,
    worker: &melodee_persistence::WorkerId,
    lease_duration: time::Duration,
) -> Result<(), AcquisitionError> {
    with_custody_lease(
        std::future::pending(),
        &melodee_persistence::WorkItemStore,
        pool,
        item,
        worker,
        lease_duration,
    )
    .await
}

/// Emit content observations without retaining a tree-wide collection. The
/// bounded consumer must persist every emitted record before accepting a
/// completed traversal, and later revalidate the source before receipt use.
#[allow(dead_code)]
pub(crate) fn stream_custody_hash_observations(
    root: &cap_std::fs::Dir,
    cancelled: impl Fn() -> bool,
    emit: impl FnMut(melodee_persistence::CustodyHashObservation) -> Result<(), AcquisitionError>,
) -> Result<u64, AcquisitionError> {
    stream_custody_observations_inner(root, None, None, cancelled, emit)
}

/// Copy into a newly created protected tree while hashing the source bytes.
/// A partial destination remains owned recovery state on any failure.
#[allow(dead_code)]
fn stream_custody_copy_observations(
    root: &cap_std::fs::Dir,
    destination: &cap_std::fs::Dir,
    cancelled: impl Fn() -> bool,
    emit: impl FnMut(melodee_persistence::CustodyHashObservation) -> Result<(), AcquisitionError>,
) -> Result<u64, AcquisitionError> {
    stream_custody_observations_inner(root, Some(destination), None, cancelled, emit)
}

/// Reserve each read on one process-wide timeline; idle periods never accrue
/// burst credit. Concurrent configurations converge to the strictest rate,
/// so raising a limit requires a process restart.
fn pace_retained_read(
    bytes: usize,
    mib_per_second: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<(), AcquisitionError> {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static PACER: OnceLock<Mutex<(Instant, u32)>> = OnceLock::new();
    if mib_per_second == 0 {
        return Err(AcquisitionError::Task(
            "retained read rate must be positive".to_owned(),
        ));
    }
    if cancelled() {
        return Err(AcquisitionError::Task(
            "custody hash capture cancelled".to_owned(),
        ));
    }
    loop {
        if cancelled() {
            return Err(AcquisitionError::Task(
                "custody hash capture cancelled".to_owned(),
            ));
        }
        let mut state = PACER
            .get_or_init(|| Mutex::new((Instant::now(), mib_per_second)))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.1 = state.1.min(mib_per_second);
        let now = Instant::now();
        let Some(wait) = state.0.checked_duration_since(now) else {
            let bytes_per_second = u64::from(state.1) * 1024 * 1024;
            let nanos = (bytes as u64)
                .saturating_mul(1_000_000_000)
                .div_ceil(bytes_per_second);
            state.0 = now
                .checked_add(Duration::from_nanos(nanos))
                .ok_or_else(|| {
                    AcquisitionError::Task("retained read pacing overflow".to_owned())
                })?;
            return Ok(());
        };
        drop(state);
        std::thread::sleep(wait.min(Duration::from_millis(50)));
    }
}

#[cfg(unix)]
fn sync_cap_directory(directory: &cap_std::fs::Dir) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags, openat};
    let handle = openat(
        directory,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    handle.sync_all()
}

#[cfg(not(unix))]
fn sync_cap_directory(directory: &cap_std::fs::Dir) -> std::io::Result<()> {
    directory
        .try_clone()
        .map(cap_std::fs::Dir::into_std_file)?
        .sync_all()
}

fn stream_custody_observations_inner(
    root: &cap_std::fs::Dir,
    destination: Option<&cap_std::fs::Dir>,
    retained_mib_per_second: Option<u32>,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(melodee_persistence::CustodyHashObservation) -> Result<(), AcquisitionError>,
) -> Result<u64, AcquisitionError> {
    use cap_fs_ext::OpenOptionsFollowExt;
    use std::io::{BufReader, BufWriter, Read, Write};

    use crate::snapshot::{
        MAX_STREAM_DIRECTORY_DEPTH, MAX_STREAM_PATH_BYTES, StreamFrame, open_cap_relative,
    };
    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::AcquisitionTreeHash,
    );
    let mut path = std::path::PathBuf::new();
    let mut frames = vec![
        StreamFrame::open(root, &path, true)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
    ];
    let mut emitted = 0_u64;
    while let Some(frame) = frames.last_mut() {
        if cancelled() {
            return Err(AcquisitionError::Task(
                "custody hash capture cancelled".to_owned(),
            ));
        }
        let Some((name, metadata)) = frame
            .next(root, &path, &cancelled)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?
        else {
            frames.pop();
            if !frames.is_empty() {
                path.pop();
            }
            continue;
        };
        let relative = path.join(&name);
        if relative.as_os_str().as_encoded_bytes().len() > MAX_STREAM_PATH_BYTES {
            return Err(AcquisitionError::InvalidSourcePath(
                "custody path exceeds platform path budget".to_owned(),
            ));
        }
        let is_dir = metadata.is_dir();
        if !is_dir && !metadata.is_file() {
            continue;
        }
        if is_dir && let Some(destination) = destination {
            let parent = open_cap_relative(destination, &path)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let mut options = cap_std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use cap_std::fs::DirBuilderExt;
                options.mode(0o700);
            }
            parent
                .create_dir_with(&name, &options)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            sync_cap_directory(&parent)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        }
        let mut digest = blake3::Hasher::new();
        digest.update(b"melodee-custody-entry-v3\0");
        #[cfg(unix)]
        digest.update(b"unix\0");
        #[cfg(windows)]
        digest.update(b"windows\0");
        crate::acquisition::hash_custody_path(&mut digest, &relative, is_dir, metadata.len())?;
        if !is_dir {
            let parent = open_cap_relative(root, &path)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).follow(cap_fs_ext::FollowSymlinks::No);
            let file = parent
                .open_with(&name, &options)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let opened = file
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            if !opened.is_file() || opened.len() != metadata.len() {
                return Err(AcquisitionError::SourceChanged);
            }
            let mut reader = BufReader::new(file.into_std());
            let mut copy_operation = destination.map(|_| {
                melodee_media::operation_metrics::OperationGuard::start(
                    melodee_media::operation_metrics::Operation::AcquisitionCopy,
                )
            });
            let mut writer = if let Some(destination) = destination {
                let parent = open_cap_relative(destination, &path)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                let mut options = cap_std::fs::OpenOptions::new();
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                options
                    .write(true)
                    .create_new(true)
                    .follow(cap_fs_ext::FollowSymlinks::No);
                let output = parent
                    .open_with(&name, &options)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                Some((BufWriter::new(output.into_std()), parent))
            } else {
                None
            };
            let mut buffer = [0_u8; 64 * 1024];
            let mut total = 0_u64;
            loop {
                if cancelled() {
                    return Err(AcquisitionError::Task(
                        "custody hash capture cancelled".to_owned(),
                    ));
                }
                if let Some(rate) = retained_mib_per_second {
                    pace_retained_read(buffer.len(), rate, &cancelled)?;
                }
                let count = reader.read(&mut buffer).map_err(|error| {
                    operation.mark_bytes_incomplete();
                    if let Some(copy) = copy_operation.as_mut() {
                        copy.mark_bytes_incomplete();
                    }
                    AcquisitionError::Task(error.to_string())
                })?;
                if count == 0 {
                    break;
                }
                total = total
                    .checked_add(count as u64)
                    .ok_or(AcquisitionError::SourceChanged)?;
                operation.add_read(count as u64);
                if let Some(copy) = copy_operation.as_mut() {
                    copy.add_read(count as u64);
                }
                digest.update(&buffer[..count]);
                if let Some((writer, _)) = writer.as_mut() {
                    writer.write_all(&buffer[..count]).map_err(|error| {
                        if let Some(copy) = copy_operation.as_mut() {
                            copy.mark_bytes_incomplete();
                        }
                        AcquisitionError::Task(error.to_string())
                    })?;
                    if let Some(copy) = copy_operation.as_mut() {
                        copy.add_written(count as u64);
                    }
                }
            }
            if total != metadata.len()
                || reader
                    .get_ref()
                    .metadata()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?
                    .len()
                    != metadata.len()
            {
                return Err(AcquisitionError::SourceChanged);
            }
            if let Some((writer, parent)) = writer {
                let output = writer.into_inner().map_err(|error| {
                    if let Some(copy) = copy_operation.as_mut() {
                        copy.mark_bytes_incomplete();
                    }
                    AcquisitionError::Task(error.to_string())
                })?;
                output
                    .sync_all()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                sync_cap_directory(&parent)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                if let Some(copy) = copy_operation {
                    copy.finish();
                }
            }
        }
        emit(melodee_persistence::CustodyHashObservation {
            path_key: crate::source::candidate_relative_path(&relative),
            parent_key: crate::source::candidate_relative_path(&path),
            name_key: {
                use std::fmt::Write;
                let mut key =
                    String::with_capacity(name.as_encoded_bytes().len().saturating_mul(2));
                for byte in name.as_encoded_bytes() {
                    let _ = write!(key, "{byte:02x}");
                }
                key
            },
            legacy_path_hex: {
                use std::fmt::Write;
                let mut key = String::new();
                for component in relative.components() {
                    if let std::path::Component::Normal(part) = component {
                        for byte in part.to_string_lossy().as_bytes() {
                            let _ = write!(key, "{byte:02x}");
                        }
                        key.push_str("00");
                    }
                }
                key
            },
            is_directory: is_dir,
            legacy_length: i64::try_from(metadata.len())
                .map_err(|_| AcquisitionError::SourceChanged)?,
            digest: digest.finalize().to_hex().to_string(),
        })?;
        emitted = emitted
            .checked_add(1)
            .ok_or(AcquisitionError::SourceChanged)?;
        if is_dir {
            path.push(&name);
            let frame = StreamFrame::open(root, &path, frames.len() < MAX_STREAM_DIRECTORY_DEPTH)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            frames.push(frame);
        }
    }
    operation.finish();
    Ok(emitted)
}

/// Provisional events from a protected-stage copy. Chunk and receipt evidence
/// are bound separately; these events alone never authorize publication.
#[cfg(all(test, target_os = "linux"))]
enum ProtectedCopyEvent {
    Observation(melodee_persistence::CustodyHashObservation),
    SealedFile {
        path_key: String,
        digest: [u8; 32],
        device: u64,
        inode: u64,
    },
}

/// Create a fresh protected stage tree with bounded traversal state. The
/// durable attempt/receipt and post-copy independent verification are not
/// provided here; interruption leaves partial bytes for fenced recovery.
#[cfg(all(test, target_os = "linux"))]
fn stream_protected_stage_copy(
    source: &cap_std::fs::Dir,
    attempt: &ProtectedStageAttempt,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(ProtectedCopyEvent) -> Result<(), AcquisitionError>,
) -> Result<u64, AcquisitionError> {
    use crate::snapshot::{
        MAX_STREAM_DIRECTORY_DEPTH, MAX_STREAM_PATH_BYTES, StreamFrame, open_cap_relative,
    };
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};

    attempt.verify()?;
    let parent = attempt.parent.clone();
    mkdir_private_child(parent.checked_file()?, std::ffi::OsStr::new("source"))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    parent
        .checked_file()?
        .sync_all()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let mut path = std::path::PathBuf::new();
    let mut frames = vec![
        StreamFrame::open(source, &path, true)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
    ];
    let mut emitted = 0_u64;
    while let Some(frame) = frames.last_mut() {
        if cancelled() {
            return Err(AcquisitionError::Task(
                "protected stage copy cancelled".to_owned(),
            ));
        }
        attempt.verify()?;
        let Some((name, metadata)) = frame
            .next(source, &path, &cancelled)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?
        else {
            frames.pop();
            if !frames.is_empty() {
                path.pop();
            }
            continue;
        };
        let relative = path.join(&name);
        if relative.as_os_str().as_encoded_bytes().len() > MAX_STREAM_PATH_BYTES {
            return Err(AcquisitionError::InvalidSourcePath(
                "custody path exceeds platform path budget".to_owned(),
            ));
        }
        let is_directory = metadata.is_dir();
        if !is_directory && !metadata.is_file() {
            continue;
        }
        let protected_parent = std::sync::Arc::new(PrivateStageParent::open(
            attempt.parent.namespace.clone(),
            attempt.parent.source_root.clone(),
            &attempt.relative.join("source").join(&path),
        )?);
        let mut digest = blake3::Hasher::new();
        digest.update(b"melodee-custody-entry-v3\0unix\0");
        crate::acquisition::hash_custody_path(
            &mut digest,
            &relative,
            is_directory,
            metadata.len(),
        )?;
        if is_directory {
            mkdir_private_child(protected_parent.checked_file()?, &name)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            protected_parent
                .checked_file()?
                .sync_all()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        } else {
            let parent = open_cap_relative(source, &path)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let mut options = cap_std::fs::OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            let file = parent
                .open_with(&name, &options)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?
                .into_std();
            let opened = file
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            if !opened.is_file() || opened.len() != metadata.len() {
                return Err(AcquisitionError::SourceChanged);
            }
            let sealed = crate::stage_verity::copy_opened_file_and_seal(
                file,
                &protected_parent,
                &name,
                &cancelled,
                |_, chunk| {
                    digest.update(chunk);
                    Ok(())
                },
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let (verity_digest, device, inode) = sealed
                .evidence()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            emit(ProtectedCopyEvent::SealedFile {
                path_key: crate::source::candidate_relative_path(&relative),
                digest: verity_digest,
                device,
                inode,
            })?;
        }
        let name_key = {
            use std::fmt::Write;
            let mut key = String::new();
            for byte in name.as_encoded_bytes() {
                let _ = write!(key, "{byte:02x}");
            }
            key
        };
        let legacy_path_hex = {
            use std::fmt::Write;
            let mut key = String::new();
            for component in relative.components() {
                if let std::path::Component::Normal(part) = component {
                    for byte in part.to_string_lossy().as_bytes() {
                        let _ = write!(key, "{byte:02x}");
                    }
                    key.push_str("00");
                }
            }
            key
        };
        emit(ProtectedCopyEvent::Observation(
            melodee_persistence::CustodyHashObservation {
                path_key: crate::source::candidate_relative_path(&relative),
                parent_key: crate::source::candidate_relative_path(&path),
                name_key,
                legacy_path_hex,
                is_directory,
                legacy_length: i64::try_from(metadata.len())
                    .map_err(|_| AcquisitionError::SourceChanged)?,
                digest: digest.finalize().to_hex().to_string(),
            },
        ))?;
        emitted = emitted
            .checked_add(1)
            .ok_or(AcquisitionError::SourceChanged)?;
        if is_directory {
            path.push(&name);
            frames.push(
                StreamFrame::open(source, &path, frames.len() < MAX_STREAM_DIRECTORY_DEPTH)
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?,
            );
        }
    }
    attempt.verify()?;
    Ok(emitted)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum QuarantineOutcome {
    Held,
    ReplacementRestored,
    ReplacementPreserved,
}

/// Pinned namespace evidence is a prerequisite, not proof that producer
/// write handles were closed. No destructive production use is authorized yet.
pub(crate) struct CustodyNamespace {
    source_path: std::path::PathBuf,
    private_path: std::path::PathBuf,
    producer_uid: u32,
    source: File,
    private: File,
    topology: Vec<u8>,
    policy: CustodyTopologyPolicy,
}

#[derive(Clone, Copy)]
enum CustodyTopologyPolicy {
    #[allow(dead_code)]
    RenameSameMount,
    #[cfg(target_os = "linux")]
    LegacyLocalMounts,
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
impl CustodyNamespace {
    fn revalidate(&self, root: &GuardedRoot) -> Result<(), AcquisitionError> {
        use std::os::unix::fs::MetadataExt;
        let current = validate_custody_namespace_with_policy(
            &self.private_path,
            &self.source_path,
            self.producer_uid,
            self.policy,
        )?;
        let root_handle = root
            .custody_root_handle()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let same_directory = |left: &File, right: &File| -> Result<bool, AcquisitionError> {
            let left = left
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let right = right
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            Ok((left.dev(), left.ino()) == (right.dev(), right.ino()))
        };
        if !same_directory(&self.source, &current.source)?
            || !same_directory(&self.source, &root_handle)?
            || !same_directory(&self.private, &current.private)?
            || self.topology != current.topology
        {
            return Err(AcquisitionError::Task(
                "custody namespace changed".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Exclusive operation capability for an existing, maintenance-provisioned
/// legacy work-item lock. It is never constructed from a path alone as stage
/// authorization: the caller must revalidate its claim or administrative
/// reservation after acquisition and before any filesystem work.
#[cfg(target_os = "linux")]
pub(crate) struct LegacyLifecycleCapability {
    inner: std::sync::Arc<std::sync::Mutex<LegacyLifecycleLock>>,
}

#[cfg(target_os = "linux")]
struct LegacyLifecycleLock {
    namespace: std::sync::Arc<CustodyNamespace>,
    root: GuardedRoot,
    held_locks: melodee_custody::HeldLifecycleLocks,
    expected_root: String,
    expected_lock: String,
    other_roots: Vec<LegacyTopologyRoot>,
}

#[cfg(target_os = "linux")]
struct LegacyTopologyRoot {
    path: std::path::PathBuf,
    generation: String,
    ancestry: Vec<(u64, u64)>,
    // A live descriptor prevents each observed inode from being recycled
    // while this topology binding is held.
    pins: Vec<std::sync::Arc<File>>,
}

#[cfg(target_os = "linux")]
const MAX_LEGACY_TOPOLOGY_ROOTS: usize = 132;
#[cfg(target_os = "linux")]
const MAX_LEGACY_ROOT_DEPTH: usize = 256;
// Bound distinct topology descriptors; common ancestors share one handle.
#[cfg(target_os = "linux")]
const MAX_LEGACY_TOPOLOGY_HANDLES: usize = 512;

#[cfg(all(test, target_os = "linux"))]
fn same_legacy_topology(
    held: &[LegacyTopologyRoot],
    current: &[LegacyTopologyRoot],
) -> Result<bool, AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    if held.len() != current.len() {
        return Ok(false);
    }
    for (old, new) in held.iter().zip(current) {
        if old.path != new.path
            || old.generation != new.generation
            || old.ancestry != new.ancestry
            || old.pins.len() != old.ancestry.len()
        {
            return Ok(false);
        }
        for (pin, identity) in old.pins.iter().zip(&old.ancestry) {
            let metadata = pin
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            if (metadata.dev(), metadata.ino()) != *identity {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

// Reopen and compare one ancestor at a time. Keeping the original pins alive
// prevents inode reuse, without allocating a second full topology of FDs.
#[cfg(target_os = "linux")]
fn revalidate_legacy_topology_with(
    namespace: &CustodyNamespace,
    held: &[LegacyTopologyRoot],
    mut observe: impl FnMut() -> Result<(), AcquisitionError>,
) -> Result<(), AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    for root in held {
        if root.pins.len() != root.ancestry.len()
            || root.path.ancestors().count() != root.pins.len()
        {
            return Err(AcquisitionError::SourceChanged);
        }
        for ((ancestor, pin), identity) in root.path.ancestors().zip(&root.pins).zip(&root.ancestry)
        {
            let current = crate::secure_fs::open_directory(ancestor)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            observe()?;
            let old = pin
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let new = current
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            if (old.dev(), old.ino()) != *identity || (new.dev(), new.ino()) != *identity {
                return Err(AcquisitionError::SourceChanged);
            }
        }
        let current = crate::secure_fs::open_directory(&root.path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        observe()?;
        if lifecycle_file_identity(&current)? != root.generation
            || legacy_local_mount_topology(
                &current,
                &namespace.private,
                &root.path,
                &namespace.private_path,
            )? != namespace.topology
        {
            return Err(AcquisitionError::SourceChanged);
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn revalidate_legacy_topology(
    namespace: &CustodyNamespace,
    held: &[LegacyTopologyRoot],
) -> Result<(), AcquisitionError> {
    revalidate_legacy_topology_with(namespace, held, || Ok(()))
}

#[allow(clippy::type_complexity)]
#[cfg(target_os = "linux")]
fn legacy_root_ancestry(
    path: &std::path::Path,
    cache: &mut std::collections::BTreeMap<std::path::PathBuf, std::sync::Arc<File>>,
) -> Result<(Vec<(u64, u64)>, Vec<std::sync::Arc<File>>), AcquisitionError> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    let raw = path.as_os_str().as_bytes();
    if !path.is_absolute()
        || path.parent().is_none()
        || raw
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let mut ancestry = Vec::new();
    let mut pins = Vec::new();
    for ancestor in path.ancestors() {
        if ancestry.len() == MAX_LEGACY_ROOT_DEPTH {
            return Err(AcquisitionError::SourceChanged);
        }
        let directory = crate::secure_fs::open_directory(ancestor)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let metadata = directory
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let identity = (metadata.dev(), metadata.ino());
        let pin = if let Some(pin) = cache.get(ancestor) {
            let held = pin
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            if (held.dev(), held.ino()) != identity {
                return Err(AcquisitionError::SourceChanged);
            }
            pin.clone()
        } else {
            if cache.len() == MAX_LEGACY_TOPOLOGY_HANDLES {
                return Err(AcquisitionError::SourceChanged);
            }
            let pin = std::sync::Arc::new(directory);
            cache.insert(ancestor.to_path_buf(), pin.clone());
            pin
        };
        ancestry.push(identity);
        pins.push(pin);
    }
    Ok((ancestry, pins))
}

#[cfg(target_os = "linux")]
fn validate_legacy_other_roots(
    namespace: &CustodyNamespace,
    roots: &[std::path::PathBuf],
) -> Result<Vec<LegacyTopologyRoot>, AcquisitionError> {
    if roots.len() > MAX_LEGACY_TOPOLOGY_ROOTS {
        return Err(AcquisitionError::SourceChanged);
    }
    let mut cache = std::collections::BTreeMap::new();
    let (private_ancestry, _private_pins) =
        legacy_root_ancestry(&namespace.private_path, &mut cache)?;
    let private_id = *private_ancestry
        .first()
        .ok_or(AcquisitionError::SourceChanged)?;
    use std::os::unix::fs::MetadataExt;
    let private_metadata = namespace
        .private
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if private_id != (private_metadata.dev(), private_metadata.ino()) {
        return Err(AcquisitionError::SourceChanged);
    }
    let mut bound = Vec::with_capacity(roots.len());
    for path in roots {
        if path.starts_with(&namespace.private_path) || namespace.private_path.starts_with(path) {
            return Err(AcquisitionError::SourceChanged);
        }
        let root = crate::secure_fs::open_directory(path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let (ancestry, pins) = legacy_root_ancestry(path, &mut cache)?;
        let root_id = *ancestry.first().ok_or(AcquisitionError::SourceChanged)?;
        let root_metadata = root
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if root_id != (root_metadata.dev(), root_metadata.ino())
            || ancestry.contains(&private_id)
            || private_ancestry.contains(&root_id)
        {
            return Err(AcquisitionError::SourceChanged);
        }
        for previous in &bound {
            let previous: &LegacyTopologyRoot = previous;
            let previous_id = *previous
                .ancestry
                .first()
                .ok_or(AcquisitionError::SourceChanged)?;
            if path.starts_with(&previous.path)
                || previous.path.starts_with(path)
                || ancestry.contains(&previous_id)
                || previous.ancestry.contains(&root_id)
            {
                return Err(AcquisitionError::SourceChanged);
            }
        }
        if legacy_local_mount_topology(&root, &namespace.private, path, &namespace.private_path)?
            != namespace.topology
        {
            return Err(AcquisitionError::SourceChanged);
        }
        bound.push(LegacyTopologyRoot {
            path: path.clone(),
            generation: lifecycle_file_identity(&root)?,
            ancestry,
            pins,
        });
    }
    Ok(bound)
}

#[cfg(target_os = "linux")]
fn lifecycle_file_identity(file: &File) -> Result<String, AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(target_os = "linux")]
impl LegacyLifecycleLock {
    fn revalidate(&self) -> Result<(), AcquisitionError> {
        self.namespace.revalidate(&self.root)?;
        self.held_locks
            .revalidate()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if lifecycle_file_identity(&self.namespace.private)? != self.expected_root {
            return Err(AcquisitionError::SourceChanged);
        }
        revalidate_legacy_topology(&self.namespace, &self.other_roots)?;
        self.namespace.revalidate(&self.root)?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl LegacyLifecycleCapability {
    /// Exclusive creation is permitted only after a durable reservation and
    /// externally quiesced maintenance fence. A pre-existing name is never
    /// adopted: it may be an unacknowledged or substituted occurrence.
    fn create_reserved(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        item: uuid::Uuid,
    ) -> Result<Self, AcquisitionError> {
        namespace.revalidate(&root)?;
        let expected_root = lifecycle_file_identity(&namespace.private)?;
        let name: std::ffi::OsString = format!("work-{}.lock", item.simple()).into();
        let file = File::from(
            rustix::fs::openat(
                &namespace.private,
                &name,
                rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
        );
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        file.sync_all()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        namespace
            .private
            .sync_all()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let expected_lock = lifecycle_file_identity(&file)?;
        let held_locks = melodee_custody::HeldLifecycleLocks::acquire_from_descriptor(
            &namespace.private_path,
            melodee_custody::LifecycleLockIdentity {
                work_item_id: item,
                lock_root_generation: expected_root.clone(),
                lock_generation: expected_lock.clone(),
            },
            file,
        )
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let lock = LegacyLifecycleLock {
            namespace,
            root,
            held_locks,
            expected_root,
            expected_lock,
            other_roots: Vec::new(),
        };
        lock.revalidate()?;
        Ok(Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(lock)),
        })
    }

    fn generations(&self) -> Result<(String, String), AcquisitionError> {
        let lock = self
            .inner
            .lock()
            .map_err(|_| AcquisitionError::SourceChanged)?;
        lock.revalidate()?;
        Ok((lock.expected_root.clone(), lock.expected_lock.clone()))
    }

    fn acquire(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        item: uuid::Uuid,
        expected_root: String,
        expected_lock: String,
    ) -> Result<Self, AcquisitionError> {
        namespace.revalidate(&root)?;
        if lifecycle_file_identity(&namespace.private)? != expected_root {
            return Err(AcquisitionError::SourceChanged);
        }
        let held_locks = melodee_custody::HeldLifecycleLocks::acquire(
            &namespace.private_path,
            &[melodee_custody::LifecycleLockIdentity {
                work_item_id: item,
                lock_root_generation: expected_root.clone(),
                lock_generation: expected_lock.clone(),
            }],
        )
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let lock = LegacyLifecycleLock {
            namespace,
            root,
            held_locks,
            expected_root,
            expected_lock,
            other_roots: Vec::new(),
        };
        lock.revalidate()?;
        Ok(Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(lock)),
        })
    }

    fn revalidate(&self) -> Result<(), AcquisitionError> {
        self.inner
            .lock()
            .map_err(|_| AcquisitionError::SourceChanged)?
            .revalidate()
    }

    /// Bind every other configured data root while still under the externally
    /// quiesced fence. Unknown/missing roots and differing mounts fail closed.
    fn bind_other_roots(&self, roots: &[std::path::PathBuf]) -> Result<(), AcquisitionError> {
        let mut lock = self
            .inner
            .lock()
            .map_err(|_| AcquisitionError::SourceChanged)?;
        lock.revalidate()?;
        lock.other_roots = validate_legacy_other_roots(&lock.namespace, roots)?;
        lock.revalidate()
    }

    #[allow(dead_code)]
    fn lifetime_hold(&self) -> crate::StageLifetimeHold {
        crate::StageLifetimeHold::from_arc(self.inner.clone())
    }
}

// This guard is deliberately test-only until ordinary workers participate in
// the same lock protocol and administrative finalization is token-fenced.
// A database reservation by itself cannot stop an already-running writer.
#[cfg(all(test, target_os = "linux"))]
struct ProvisionalAdminLifecycleScope {
    reservation_id: uuid::Uuid,
    identities: Vec<melodee_persistence::AdminReservedLifecycle>,
    canonical_root: Option<std::path::PathBuf>,
    configured_roots: Vec<melodee_custody::ConfiguredRoot>,
    inner: std::sync::Arc<std::sync::Mutex<ProvisionalAdminScopeData>>,
}

#[cfg(all(test, target_os = "linux"))]
struct ProvisionalAdminSourceBinding {
    namespace: CustodyNamespace,
    stage: CustodyNamespace,
    source: GuardedRoot,
    stage_generation: String,
}

#[cfg(all(test, target_os = "linux"))]
struct ProvisionalAdminScopeData {
    locks: melodee_custody::HeldLifecycleLocks,
    sources: Vec<ProvisionalAdminSourceBinding>,
    topology: Vec<LegacyTopologyRoot>,
    registered_roots: Option<melodee_custody::ExactConfiguredRootSet>,
}

#[cfg(all(test, target_os = "linux"))]
impl melodee_custody::ObservationScope for ProvisionalAdminLifecycleScope {
    fn revalidate_observation_scope(&self) -> std::io::Result<()> {
        self.revalidate()
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

#[cfg(all(test, target_os = "linux"))]
fn extend_admin_lifecycle_page(
    identities: &mut Vec<melodee_persistence::AdminReservedLifecycle>,
    page: Vec<melodee_persistence::AdminReservedLifecycle>,
) -> Result<(), AcquisitionError> {
    let after = identities.last().map(|item| item.work_item_id);
    if page.len() > 16
        || identities.len() + page.len() > 128
        || page
            .windows(2)
            .any(|items| items[0].work_item_id >= items[1].work_item_id)
        || page
            .first()
            .is_some_and(|item| after.is_some_and(|last| item.work_item_id <= last))
    {
        return Err(AcquisitionError::SourceChanged);
    }
    identities.extend(page);
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
impl ProvisionalAdminLifecycleScope {
    fn revalidate(&self) -> Result<(), AcquisitionError> {
        let data = self
            .inner
            .lock()
            .map_err(|_| AcquisitionError::SourceChanged)?;
        data.locks
            .revalidate()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let first = data
            .sources
            .first()
            .ok_or(AcquisitionError::SourceChanged)?;
        for binding in &data.sources {
            binding.namespace.revalidate(&binding.source)?;
            binding.stage.revalidate(&binding.source)?;
            if binding.namespace.topology != first.namespace.topology
                || lifecycle_file_identity(&binding.stage.private)? != binding.stage_generation
            {
                return Err(AcquisitionError::SourceChanged);
            }
        }
        revalidate_legacy_topology(&first.namespace, &data.topology)?;
        if let Some(roots) = &data.registered_roots {
            roots
                .revalidate()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        }
        Ok(())
    }

    fn lifetime_holds(&self) -> Vec<crate::StageLifetimeHold> {
        vec![crate::StageLifetimeHold::from_arc(self.inner.clone())]
    }
}

#[cfg(all(test, target_os = "linux"))]
fn provisional_configured_roots(
    config: &crate::config::IngestionConfig,
    asset_root: std::path::PathBuf,
) -> Result<Vec<melodee_custody::ConfiguredRoot>, AcquisitionError> {
    let mut roots: Vec<_> = config
        .sources
        .iter()
        .map(|source| melodee_custody::ConfiguredRoot {
            role: "source".to_owned(),
            source_library_id: source.id,
            path: source.root.clone(),
        })
        .collect();
    for (role, path) in [
        ("work", Some(config.work_area_base.clone())),
        ("stage", config.staging_root.clone()),
        ("canonical", config.storage_root.clone()),
        ("asset", Some(asset_root)),
        (
            "lock",
            config
                .protected_custody
                .as_ref()
                .map(|settings| settings.private_root.clone()),
        ),
    ] {
        roots.push(melodee_custody::ConfiguredRoot {
            role: role.to_owned(),
            source_library_id: None,
            path: path.ok_or(AcquisitionError::SourceNotConfigured)?,
        });
    }
    Ok(roots)
}

/// Acquire every item in the reserved scope in deterministic order. The DB
/// query is repeated only after all nonblocking filesystem locks are held;
/// neither the reservation nor this provisional scope authorizes deletion.
#[cfg(all(test, target_os = "linux"))]
async fn acquire_provisional_admin_lifecycle_scope(
    config: &crate::config::IngestionConfig,
    asset_root: std::path::PathBuf,
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    reservation: &melodee_persistence::AdminMediaReservation,
) -> Result<ProvisionalAdminLifecycleScope, AcquisitionError> {
    let mut identities = Vec::new();
    loop {
        let after = identities
            .last()
            .map(|item: &melodee_persistence::AdminReservedLifecycle| item.work_item_id);
        let page = store
            .admin_reservation_lifecycle_page(pool, reservation, after)
            .await?;
        if page.is_empty() {
            break;
        }
        extend_admin_lifecycle_page(&mut identities, page)?;
    }
    let settings = config
        .protected_custody
        .clone()
        .ok_or(AcquisitionError::SourceNotConfigured)?;
    let stage_root = config
        .staging_root
        .clone()
        .ok_or(AcquisitionError::SourceNotConfigured)?;
    if config.sources.len() > MAX_LEGACY_TOPOLOGY_ROOTS.saturating_sub(4) {
        return Err(AcquisitionError::SourceChanged);
    }
    let configured_roots = provisional_configured_roots(config, asset_root.clone())?;
    let mut roots: Vec<_> = config
        .sources
        .iter()
        .map(|source| source.root.clone())
        .collect();
    roots.push(stage_root.clone());
    roots.push(config.work_area_base.clone());
    if let Some(canonical) = &config.storage_root {
        roots.push(canonical.clone());
    }
    roots.push(asset_root);
    let sources = config.sources.clone();
    let for_task = identities.clone();
    let scope_identities = identities.clone();
    let reservation_id = reservation.id();
    let canonical_root = config.storage_root.clone();
    let scope = crate::admission::run(move || {
        let locks = melodee_custody::HeldLifecycleLocks::acquire(
            &settings.private_root,
            &for_task
                .iter()
                .map(|identity| melodee_custody::LifecycleLockIdentity {
                    work_item_id: identity.work_item_id,
                    lock_root_generation: identity.lock_root_generation.clone(),
                    lock_generation: identity.lock_generation.clone(),
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let mut bindings: Vec<ProvisionalAdminSourceBinding> = Vec::new();
        let mut source_indices: std::collections::BTreeMap<Option<uuid::Uuid>, usize> =
            std::collections::BTreeMap::new();
        for identity in for_task {
            if identity.lock_name != format!("work-{}.lock", identity.work_item_id.simple()) {
                return Err(AcquisitionError::SourceChanged);
            }
            if let Some(index) = source_indices.get(&identity.source_library_id) {
                if bindings[*index].stage_generation != identity.stage_root_generation {
                    return Err(AcquisitionError::SourceChanged);
                }
                continue;
            }
            let mut candidates = sources
                .iter()
                .filter(|source| source.id == identity.source_library_id);
            let source = candidates
                .next()
                .cloned()
                .ok_or(AcquisitionError::SourceNotConfigured)?;
            if candidates.next().is_some() {
                return Err(AcquisitionError::SourceChanged);
            }
            let root = GuardedRoot::open(source.clone())
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            let namespace = validate_legacy_lifecycle_namespace(
                &settings.private_root,
                &source.root,
                settings.producer_uid,
            )?;
            let stage = validate_legacy_lifecycle_namespace(
                &stage_root,
                &source.root,
                settings.producer_uid,
            )?;
            stage.revalidate(&root)?;
            if lifecycle_file_identity(&stage.private)? != identity.stage_root_generation {
                return Err(AcquisitionError::SourceChanged);
            }
            source_indices.insert(identity.source_library_id, bindings.len());
            bindings.push(ProvisionalAdminSourceBinding {
                namespace,
                stage,
                source: root,
                stage_generation: identity.stage_root_generation,
            });
        }
        let first = bindings.first().ok_or(AcquisitionError::SourceChanged)?;
        let topology = validate_legacy_other_roots(&first.namespace, &roots)?;
        let scope = ProvisionalAdminLifecycleScope {
            reservation_id,
            identities: scope_identities,
            canonical_root,
            configured_roots,
            inner: std::sync::Arc::new(std::sync::Mutex::new(ProvisionalAdminScopeData {
                locks,
                sources: bindings,
                topology,
                registered_roots: None,
            })),
        };
        scope.revalidate()?;
        Ok::<_, AcquisitionError>(scope)
    })
    .await
    .map_err(AcquisitionError::Task)??;
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    // Revalidate after the database round trip while retaining all locks.
    crate::admission::run(move || {
        scope.revalidate()?;
        Ok::<_, AcquisitionError>(scope)
    })
    .await
    .map_err(AcquisitionError::Task)?
}

// Exact-intent binding for a read-only experiment. This deliberately cannot
// acknowledge an intent, unlink a file, or construct a deletion proof.
#[cfg(all(test, feature = "test-support", target_os = "linux"))]
struct ProvisionalIntentScan {
    intent_id: uuid::Uuid,
    snapshot_revision: uuid::Uuid,
    reservation_id: uuid::Uuid,
    identities: Vec<melodee_persistence::AdminReservedLifecycle>,
    registered_roots: Vec<melodee_persistence::AdminMediaRootExpectation>,
    entry_index: usize,
    relative_path: std::path::PathBuf,
    expected_size: u64,
    expected_hash: [u8; 32],
    scan: melodee_custody::IncrementalFileCheck<ProvisionalAdminLifecycleScope>,
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
fn exact_lower_hex_hash(value: &str) -> Result<[u8; 32], AcquisitionError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let mut out = [0; 32];
    for (index, chunk) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let digit = |byte: u8| {
            if byte.is_ascii_digit() {
                byte - b'0'
            } else {
                byte - b'a' + 10
            }
        };
        out[index] = digit(chunk[0]) * 16 + digit(chunk[1]);
    }
    Ok(out)
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
async fn begin_provisional_intent_scan(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    fence: &melodee_persistence::LifecycleMaintenance,
    reservation: &melodee_persistence::AdminMediaReservation,
    intent: &melodee_persistence::AdminDeletionPageIntent,
    scope: ProvisionalAdminLifecycleScope,
    entry_index: usize,
) -> Result<ProvisionalIntentScan, AcquisitionError> {
    if scope.reservation_id != reservation.id()
        || intent.reservation_id() != reservation.id()
        || scope.identities.is_empty()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let entry = intent
        .entries()
        .get(entry_index)
        .ok_or(AcquisitionError::SourceChanged)?;
    // Directory entries have no complete tree proof. Artwork has a different
    // root and digest format and is also unsupported by this experiment.
    if entry.kind != "media" {
        return Err(AcquisitionError::SourceChanged);
    }
    let hash = exact_lower_hex_hash(
        entry
            .byte_hash
            .as_deref()
            .ok_or(AcquisitionError::SourceChanged)?,
    )?;
    let size = u64::try_from(entry.byte_size.ok_or(AcquisitionError::SourceChanged)?)
        .map_err(|_| AcquisitionError::SourceChanged)?;
    let path = std::path::PathBuf::from(&entry.object_key);
    let relative_path = path.clone();
    let root = scope
        .canonical_root
        .clone()
        .ok_or(AcquisitionError::SourceNotConfigured)?;
    let identities = scope.identities.clone();
    store
        .revalidate_maintenance_deletion_page(pool, fence, reservation, intent)
        .await?;
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    let registered_roots = store
        .read_maintenance_intent_roots(pool, fence, reservation, intent)
        .await?;
    let expected_roots: Vec<_> = registered_roots
        .iter()
        .map(|entry| {
            if entry.path_format != "unix-hex-v1" {
                return Err(AcquisitionError::SourceChanged);
            }
            Ok(melodee_custody::RootExpectation {
                role: entry.role.clone(),
                source_library_id: entry.source_library_id,
                path_hex: entry.path_hex.clone(),
                generation: entry.generation.clone(),
                mount_id: entry.mount_id.clone(),
                filesystem_magic: entry.filesystem_magic.clone(),
            })
        })
        .collect::<Result<_, _>>()?;
    let scan = crate::admission::run(move || {
        let checked =
            melodee_custody::ExactConfiguredRootSet::open(&scope.configured_roots, &expected_roots)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        scope
            .inner
            .lock()
            .map_err(|_| AcquisitionError::SourceChanged)?
            .registered_roots = Some(checked);
        melodee_custody::IncrementalFileCheck::begin(
            scope,
            &root,
            melodee_custody::ExpectedFile {
                path: &path,
                size,
                hash: melodee_custody::ExpectedHash::Blake3(hash),
            },
        )
        .map_err(|error| AcquisitionError::Task(error.to_string()))
    })
    .await
    .map_err(AcquisitionError::Task)??;
    store
        .revalidate_maintenance_deletion_page(pool, fence, reservation, intent)
        .await?;
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    if store
        .read_maintenance_intent_roots(pool, fence, reservation, intent)
        .await?
        != registered_roots
    {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok(ProvisionalIntentScan {
        intent_id: intent.id(),
        snapshot_revision: intent.snapshot_revision(),
        reservation_id: reservation.id(),
        identities,
        registered_roots,
        entry_index,
        relative_path,
        expected_size: size,
        expected_hash: hash,
        scan,
    })
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
async fn advance_provisional_intent_scan(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    fence: &melodee_persistence::LifecycleMaintenance,
    reservation: &melodee_persistence::AdminMediaReservation,
    intent: &melodee_persistence::AdminDeletionPageIntent,
    mut session: ProvisionalIntentScan,
    budget: usize,
) -> Result<
    (
        ProvisionalIntentScan,
        melodee_custody::IncrementalFileProgress,
    ),
    AcquisitionError,
> {
    if session.intent_id != intent.id()
        || session.snapshot_revision != intent.snapshot_revision()
        || session.reservation_id != reservation.id()
        || intent.reservation_id() != reservation.id()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    store
        .revalidate_maintenance_deletion_page(pool, fence, reservation, intent)
        .await?;
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &session.identities)
        .await?;
    if store
        .read_maintenance_intent_roots(pool, fence, reservation, intent)
        .await?
        != session.registered_roots
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let (session, progress) = crate::admission::run(move || {
        let progress = session
            .scan
            .step(budget, || Ok(()), || false)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        Ok::<_, AcquisitionError>((session, progress))
    })
    .await
    .map_err(AcquisitionError::Task)??;
    store
        .revalidate_maintenance_deletion_page(pool, fence, reservation, intent)
        .await?;
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &session.identities)
        .await?;
    if store
        .read_maintenance_intent_roots(pool, fence, reservation, intent)
        .await?
        != session.registered_roots
    {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok((session, progress))
}

// This owned-fixture path records a real DB intent and preserves every moved
// byte. It never marks the move verified, unlinks it, or acknowledges a page.
#[cfg(all(test, feature = "test-support", target_os = "linux"))]
struct ProvisionalQuarantineRequest {
    relative_parent: std::path::PathBuf,
    id: uuid::Uuid,
    after_name_check: Option<Box<dyn FnOnce() -> std::io::Result<()> + Send>>,
    interrupt_after_rename: bool,
    interrupt_after_move: bool,
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
struct ProvisionalQuarantinePreparation {
    scope: ProvisionalAdminLifecycleScope,
    source_parent: File,
    source_parent_path: std::path::PathBuf,
    source_name: std::ffi::OsString,
    original: File,
    quarantine_parent: File,
    quarantine_path: std::path::PathBuf,
    quarantine_namespace: CustodyNamespace,
    source_root: GuardedRoot,
    source_generation: String,
    source_parent_generation: String,
    quarantine_parent_generation: String,
    mount_id: String,
    size: u64,
    hash: [u8; 32],
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
struct ProvisionalQuarantinedFile {
    _scope: ProvisionalAdminLifecycleScope,
    _file: File,
    _quarantine_parent: File,
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
fn provisional_verify_quarantine_file(
    file: &mut File,
    size: u64,
    expected: [u8; 32],
) -> Result<(), AcquisitionError> {
    use std::{
        io::{Read, Seek, SeekFrom},
        os::unix::fs::MetadataExt,
    };
    const MAX_TEST_BYTES: u64 = 8 * 1024 * 1024;
    let before = file
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if !before.is_file() || before.nlink() != 1 || before.len() != size || size > MAX_TEST_BYTES {
        return Err(AcquisitionError::SourceChanged);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let mut hash = blake3::Hasher::new();
    let mut count = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if read == 0 {
            break;
        }
        count = count
            .checked_add(read as u64)
            .ok_or(AcquisitionError::SourceChanged)?;
        if count > size {
            return Err(AcquisitionError::SourceChanged);
        }
        hash.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if count != size
        || hash.finalize().as_bytes() != &expected
        || (
            before.dev(),
            before.ino(),
            before.len(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (
            after.dev(),
            after.ino(),
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        )
    {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok(())
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
async fn provisional_quarantine_completed_scan(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    fence: &melodee_persistence::LifecycleMaintenance,
    reservation: &melodee_persistence::AdminMediaReservation,
    intent: &melodee_persistence::AdminDeletionPageIntent,
    session: ProvisionalIntentScan,
    mut request: ProvisionalQuarantineRequest,
) -> Result<ProvisionalQuarantinedFile, AcquisitionError> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    if session.intent_id != intent.id()
        || session.reservation_id != reservation.id()
        || session.snapshot_revision != intent.snapshot_revision()
        || request.id.is_nil()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let mut parts = request.relative_parent.components();
    if !matches!(parts.next(), Some(std::path::Component::Normal(_))) || parts.next().is_some() {
        return Err(AcquisitionError::SourceChanged);
    }
    let bytes = request.relative_parent.as_os_str().as_bytes();
    let mut relative_hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut relative_hex, "{byte:02x}")
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    }
    let entry_index = session.entry_index;
    let identities = session.identities.clone();
    let registered_roots = session.registered_roots.clone();
    let relative = session.relative_path.clone();
    let size = session.expected_size;
    let hash = session.expected_hash;
    store
        .revalidate_maintenance_deletion_page(pool, fence, reservation, intent)
        .await?;
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    if store
        .read_maintenance_intent_roots(pool, fence, reservation, intent)
        .await?
        != registered_roots
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let scope = session
        .scan
        .into_scope()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let canonical = scope
        .canonical_root
        .clone()
        .ok_or(AcquisitionError::SourceNotConfigured)?;
    let relative_parent = request.relative_parent.clone();
    let prepared = crate::admission::run(move || {
        scope.revalidate()?;
        let source_parent_path =
            canonical.join(relative.parent().ok_or(AcquisitionError::SourceChanged)?);
        let source_name = relative
            .file_name()
            .ok_or(AcquisitionError::SourceChanged)?
            .to_os_string();
        let quarantine_path = canonical.join(relative_parent);
        let (source_root, producer_uid, source_path) = {
            let inner = scope
                .inner
                .lock()
                .map_err(|_| AcquisitionError::SourceChanged)?;
            let binding = inner
                .sources
                .first()
                .ok_or(AcquisitionError::SourceChanged)?;
            (
                binding.source.clone(),
                binding.namespace.producer_uid,
                binding.namespace.source_path.clone(),
            )
        };
        let quarantine_namespace =
            validate_legacy_lifecycle_namespace(&quarantine_path, &source_path, producer_uid)?;
        quarantine_namespace.revalidate(&source_root)?;
        let source_parent = crate::secure_fs::open_directory(&source_parent_path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let quarantine_parent = crate::secure_fs::open_directory(&quarantine_path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let source_mount = linux_mount_id(&source_parent)?;
        if source_mount != linux_mount_id(&quarantine_parent)?
            || source_parent
                .metadata()
                .map_err(|e| AcquisitionError::Task(e.to_string()))?
                .dev()
                != quarantine_parent
                    .metadata()
                    .map_err(|e| AcquisitionError::Task(e.to_string()))?
                    .dev()
        {
            return Err(AcquisitionError::SourceChanged);
        }
        let mut original = File::from(
            rustix::fs::openat(
                &source_parent,
                &source_name,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
        );
        provisional_verify_quarantine_file(&mut original, size, hash)?;
        let prepared = ProvisionalQuarantinePreparation {
            source_generation: lifecycle_file_identity(&original)?,
            source_parent_generation: lifecycle_file_identity(&source_parent)?,
            quarantine_parent_generation: lifecycle_file_identity(&quarantine_parent)?,
            mount_id: format!("{source_mount:016x}"),
            scope,
            source_parent,
            source_parent_path,
            source_name,
            original,
            quarantine_parent,
            quarantine_path,
            quarantine_namespace,
            source_root,
            size,
            hash,
        };
        Ok::<_, AcquisitionError>(prepared)
    })
    .await
    .map_err(AcquisitionError::Task)??;
    let ordinal = u16::try_from(entry_index + 1).map_err(|_| AcquisitionError::SourceChanged)?;
    let stored = store
        .prepare_admin_quarantine_intent(
            pool,
            fence,
            reservation,
            intent,
            &melodee_persistence::AdminQuarantineRequest {
                ordinal,
                id: request.id,
                observation: melodee_persistence::AdminQuarantineObservation {
                    source_generation: &prepared.source_generation,
                    source_parent_generation: &prepared.source_parent_generation,
                    quarantine_parent_relative_hex: &relative_hex,
                    quarantine_parent_generation: &prepared.quarantine_parent_generation,
                    mount_id: &prepared.mount_id,
                },
            },
        )
        .await?;
    let binding = store
        .read_admin_quarantine_binding(pool, fence, reservation, intent, stored.id())
        .await?;
    if binding.state != "intent"
        || binding.root_set_revision
            != intent
                .root_set_revision()
                .ok_or(AcquisitionError::SourceChanged)?
        || binding.ordinal != i16::try_from(ordinal).map_err(|_| AcquisitionError::SourceChanged)?
        || binding.quarantine_root_role != "canonical"
        || binding.quarantine_parent_relative_hex != relative_hex
        || binding.source_generation != prepared.source_generation
        || binding.source_parent_generation != prepared.source_parent_generation
        || binding.quarantine_parent_generation != prepared.quarantine_parent_generation
        || binding.mount_id != prepared.mount_id
        || binding.leaf_name != stored.leaf_name()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    let mut prepared = prepared;
    let leaf = stored.leaf_name().to_owned();
    let moved = crate::admission::run(move || {
        prepared.scope.revalidate()?;
        prepared
            .quarantine_namespace
            .revalidate(&prepared.source_root)?;
        let source_now = crate::secure_fs::open_directory(&prepared.source_parent_path)
            .map_err(|e| AcquisitionError::Task(e.to_string()))?;
        let quarantine_now = crate::secure_fs::open_directory(&prepared.quarantine_path)
            .map_err(|e| AcquisitionError::Task(e.to_string()))?;
        if lifecycle_file_identity(&source_now)? != prepared.source_parent_generation
            || lifecycle_file_identity(&quarantine_now)? != prepared.quarantine_parent_generation
        {
            return Err(AcquisitionError::SourceChanged);
        }
        provisional_verify_quarantine_file(&mut prepared.original, prepared.size, prepared.hash)?;
        let named = File::from(
            rustix::fs::openat(
                &prepared.source_parent,
                &prepared.source_name,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|e| AcquisitionError::Task(e.to_string()))?,
        );
        if lifecycle_file_identity(&named)? != prepared.source_generation {
            return Err(AcquisitionError::SourceChanged);
        }
        if let Some(hook) = request.after_name_check.take() {
            hook().map_err(|e| AcquisitionError::Task(e.to_string()))?;
        }
        rename_child_noreplace(
            &prepared.source_parent,
            &prepared.source_name,
            &prepared.quarantine_parent,
            OsStr::new(&leaf),
        )
        .map_err(|e| AcquisitionError::Task(e.to_string()))?;
        if request.interrupt_after_rename {
            return Err(AcquisitionError::Task(
                "simulated interruption before quarantine fsync".to_owned(),
            ));
        }
        prepared
            .source_parent
            .sync_all()
            .map_err(|e| AcquisitionError::Task(e.to_string()))?;
        prepared
            .quarantine_parent
            .sync_all()
            .map_err(|e| AcquisitionError::Task(e.to_string()))?;
        if request.interrupt_after_move {
            return Err(AcquisitionError::Task(
                "simulated interruption after quarantine move".to_owned(),
            ));
        }
        let mut file = File::from(
            rustix::fs::openat(
                &prepared.quarantine_parent,
                leaf.as_str(),
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|e| AcquisitionError::Task(e.to_string()))?,
        );
        if lifecycle_file_identity(&file)? != prepared.source_generation {
            return Err(AcquisitionError::SourceChanged);
        }
        provisional_verify_quarantine_file(&mut file, prepared.size, prepared.hash)?;
        prepared.scope.revalidate()?;
        prepared
            .quarantine_namespace
            .revalidate(&prepared.source_root)?;
        Ok::<_, AcquisitionError>(ProvisionalQuarantinedFile {
            _scope: prepared.scope,
            _file: file,
            _quarantine_parent: prepared.quarantine_parent,
        })
    })
    .await
    .map_err(AcquisitionError::Task)??;
    if store
        .read_admin_quarantine_binding(pool, fence, reservation, intent, stored.id())
        .await?
        != binding
    {
        return Err(AcquisitionError::SourceChanged);
    }
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    Ok(moved)
}

// These are transient observations, not proof that either name stays absent
// or that a persisted dev/inode survived a crash without reuse. The only safe
// automated result here is to keep the intent and both occurrences intact.
#[cfg(all(test, feature = "test-support", target_os = "linux"))]
#[derive(Debug, PartialEq, Eq)]
enum ProvisionalNameObservation {
    Missing,
    Matching,
    Different,
    Unreadable,
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
struct ProvisionalTwoNameObservation {
    original: ProvisionalNameObservation,
    quarantine: ProvisionalNameObservation,
    _scope: ProvisionalAdminLifecycleScope,
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
fn provisional_decode_quarantine_component(
    encoded: &str,
) -> Result<std::path::PathBuf, AcquisitionError> {
    use std::os::unix::ffi::OsStringExt;
    if encoded.is_empty()
        || encoded.len() > 262_200
        || !encoded.len().is_multiple_of(2)
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let digit = |byte: u8| {
        if byte.is_ascii_digit() {
            byte - b'0'
        } else {
            byte - b'a' + 10
        }
    };
    let bytes: Vec<_> = encoded
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| digit(pair[0]) * 16 + digit(pair[1]))
        .collect();
    if bytes.contains(&0) || bytes.contains(&b'/') || bytes == b"." || bytes == b".." {
        return Err(AcquisitionError::SourceChanged);
    }
    let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes));
    let mut components = path.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok(path)
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
fn provisional_observe_name(
    parent: &File,
    name: &OsStr,
    generation: &str,
    size: u64,
    hash: [u8; 32],
) -> ProvisionalNameObservation {
    let mut file = match rustix::fs::openat(
        parent,
        name,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(rustix::io::Errno::NOENT) => return ProvisionalNameObservation::Missing,
        Err(_) => return ProvisionalNameObservation::Unreadable,
    };
    match lifecycle_file_identity(&file) {
        Ok(current) if current == generation => {}
        Ok(_) => return ProvisionalNameObservation::Different,
        Err(_) => return ProvisionalNameObservation::Unreadable,
    }
    match provisional_verify_quarantine_file(&mut file, size, hash) {
        Ok(()) => ProvisionalNameObservation::Matching,
        Err(AcquisitionError::SourceChanged) => ProvisionalNameObservation::Different,
        Err(_) => ProvisionalNameObservation::Unreadable,
    }
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
async fn provisional_inspect_quarantine_names(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    fence: &melodee_persistence::LifecycleMaintenance,
    reservation: &melodee_persistence::AdminMediaReservation,
    intent: &melodee_persistence::AdminDeletionPageIntent,
    scope: ProvisionalAdminLifecycleScope,
    quarantine_id: uuid::Uuid,
) -> Result<ProvisionalTwoNameObservation, AcquisitionError> {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    if scope.reservation_id != reservation.id()
        || intent.reservation_id() != reservation.id()
        || scope.identities.is_empty()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let recovery = store
        .inspect_admin_quarantine_recovery(pool, fence, reservation, quarantine_id)
        .await?;
    if recovery.page_id != intent.id() {
        return Err(AcquisitionError::SourceChanged);
    }
    let binding = recovery.binding;
    let registered = recovery.roots;
    if !matches!(binding.state.as_str(), "intent" | "uncertain")
        || binding.root_set_revision
            != intent
                .root_set_revision()
                .ok_or(AcquisitionError::SourceChanged)?
        || binding.quarantine_root_role != "canonical"
        || binding.leaf_name != format!("quarantine-{}.entry", quarantine_id.simple())
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let index = usize::try_from(binding.ordinal)
        .ok()
        .and_then(|v| v.checked_sub(1))
        .ok_or(AcquisitionError::SourceChanged)?;
    let entry = intent
        .entries()
        .get(index)
        .ok_or(AcquisitionError::SourceChanged)?;
    if *entry != recovery.entry || entry.kind != "media" {
        return Err(AcquisitionError::SourceChanged);
    }
    let expected_hash = exact_lower_hex_hash(
        entry
            .byte_hash
            .as_deref()
            .ok_or(AcquisitionError::SourceChanged)?,
    )?;
    let expected_size = u64::try_from(entry.byte_size.ok_or(AcquisitionError::SourceChanged)?)
        .map_err(|_| AcquisitionError::SourceChanged)?;
    let relative = std::path::PathBuf::from(&entry.object_key);
    if relative.is_absolute()
        || relative
            .as_os_str()
            .as_bytes()
            .split(|byte| *byte == b'/')
            .any(|part| part.is_empty() || part == b"." || part == b"..")
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || relative.file_name().is_none()
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let quarantine_relative =
        provisional_decode_quarantine_component(&binding.quarantine_parent_relative_hex)?;
    let identities = scope.identities.clone();
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    let expected_roots: Vec<_> = registered
        .iter()
        .map(|entry| {
            if entry.path_format != "unix-hex-v1" {
                return Err(AcquisitionError::SourceChanged);
            }
            Ok(melodee_custody::RootExpectation {
                role: entry.role.clone(),
                source_library_id: entry.source_library_id,
                path_hex: entry.path_hex.clone(),
                generation: entry.generation.clone(),
                mount_id: entry.mount_id.clone(),
                filesystem_magic: entry.filesystem_magic.clone(),
            })
        })
        .collect::<Result<_, _>>()?;
    let for_worker = binding.clone();
    let observation = crate::admission::run(move || {
        let checked =
            melodee_custody::ExactConfiguredRootSet::open(&scope.configured_roots, &expected_roots)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        scope
            .inner
            .lock()
            .map_err(|_| AcquisitionError::SourceChanged)?
            .registered_roots = Some(checked);
        scope.revalidate()?;
        let canonical = scope
            .canonical_root
            .clone()
            .ok_or(AcquisitionError::SourceNotConfigured)?;
        let source_parent_path =
            canonical.join(relative.parent().ok_or(AcquisitionError::SourceChanged)?);
        let source_name = relative
            .file_name()
            .ok_or(AcquisitionError::SourceChanged)?;
        let quarantine_path = canonical.join(quarantine_relative);
        let (source_root, producer_uid, source_path) = {
            let inner = scope
                .inner
                .lock()
                .map_err(|_| AcquisitionError::SourceChanged)?;
            let source = inner
                .sources
                .first()
                .ok_or(AcquisitionError::SourceChanged)?;
            (
                source.source.clone(),
                source.namespace.producer_uid,
                source.namespace.source_path.clone(),
            )
        };
        let quarantine_namespace =
            validate_legacy_lifecycle_namespace(&quarantine_path, &source_path, producer_uid)?;
        quarantine_namespace.revalidate(&source_root)?;
        let source_parent = crate::secure_fs::open_directory(&source_parent_path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let quarantine_parent = crate::secure_fs::open_directory(&quarantine_path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if lifecycle_file_identity(&source_parent)? != for_worker.source_parent_generation
            || lifecycle_file_identity(&quarantine_parent)?
                != for_worker.quarantine_parent_generation
            || format!("{:016x}", linux_mount_id(&source_parent)?) != for_worker.mount_id
            || linux_mount_id(&source_parent)? != linux_mount_id(&quarantine_parent)?
            || source_parent
                .metadata()
                .map_err(|e| AcquisitionError::Task(e.to_string()))?
                .dev()
                != quarantine_parent
                    .metadata()
                    .map_err(|e| AcquisitionError::Task(e.to_string()))?
                    .dev()
        {
            return Err(AcquisitionError::SourceChanged);
        }
        let original = provisional_observe_name(
            &source_parent,
            source_name,
            &for_worker.source_generation,
            expected_size,
            expected_hash,
        );
        let quarantine = provisional_observe_name(
            &quarantine_parent,
            OsStr::new(&for_worker.leaf_name),
            &for_worker.source_generation,
            expected_size,
            expected_hash,
        );
        let source_now = crate::secure_fs::open_directory(&source_parent_path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let quarantine_now = crate::secure_fs::open_directory(&quarantine_path)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if lifecycle_file_identity(&source_now)? != for_worker.source_parent_generation
            || lifecycle_file_identity(&quarantine_now)? != for_worker.quarantine_parent_generation
        {
            return Err(AcquisitionError::SourceChanged);
        }
        quarantine_namespace.revalidate(&source_root)?;
        scope.revalidate()?;
        Ok::<_, AcquisitionError>(ProvisionalTwoNameObservation {
            original,
            quarantine,
            _scope: scope,
        })
    })
    .await
    .map_err(AcquisitionError::Task)??;
    let again = store
        .inspect_admin_quarantine_recovery(pool, fence, reservation, quarantine_id)
        .await?;
    if again.page_id != intent.id()
        || again.binding != binding
        || again.entry != *entry
        || again.roots != registered
    {
        return Err(AcquisitionError::SourceChanged);
    }
    store
        .revalidate_admin_reservation_lifecycles(pool, reservation, &identities)
        .await?;
    Ok(observation)
}

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
async fn provisional_ready_quarantine_scan(
    config: &crate::config::IngestionConfig,
    assets: std::path::PathBuf,
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    fence: &melodee_persistence::LifecycleMaintenance,
    reservation: &melodee_persistence::AdminMediaReservation,
    intent: &melodee_persistence::AdminDeletionPageIntent,
) -> Result<ProvisionalIntentScan, AcquisitionError> {
    let scope =
        acquire_provisional_admin_lifecycle_scope(config, assets, store, pool, reservation).await?;
    let scan =
        begin_provisional_intent_scan(store, pool, fence, reservation, intent, scope, 0).await?;
    let (scan, progress) =
        advance_provisional_intent_scan(store, pool, fence, reservation, intent, scan, 1024)
            .await?;
    if progress != melodee_custody::IncrementalFileProgress::Complete {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok(scan)
}

/// Filesystem half of a previously reserved maintenance step. The blocking
/// worker owns its admission permit and lock until it actually exits, even if
/// its async waiter is cancelled. DB acknowledgement must be separately fenced.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
async fn create_reserved_legacy_lifecycle(
    namespace: std::sync::Arc<CustodyNamespace>,
    root: GuardedRoot,
    item: uuid::Uuid,
) -> Result<LegacyLifecycleCapability, AcquisitionError> {
    crate::admission::run(move || LegacyLifecycleCapability::create_reserved(namespace, root, item))
        .await
        .map_err(AcquisitionError::Task)?
}

/// Result of one bounded maintenance provisioning turn.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyLifecycleProvisionOutcome {
    /// The current pass has reached its end.
    PassComplete,
    /// The named item has a durably acknowledged lock identity.
    Ready(uuid::Uuid),
    /// The named item remains preserved for operator investigation.
    Quarantined(uuid::Uuid),
}

/// Provision at most one legacy lifecycle lock under an externally enforced
/// stop-all-writers maintenance fence. This function does not establish that
/// old processes stopped, does not enable runtime enforcement, and never
/// adopts or removes an unacknowledged filesystem name.
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub async fn provision_legacy_lifecycle_once(
    config: &crate::config::IngestionConfig,
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    fence: &melodee_persistence::LifecycleMaintenance,
) -> Result<LegacyLifecycleProvisionOutcome, AcquisitionError> {
    let asset_root = std::env::var_os(melodee_persistence::asset_files::ENV_ASSET_ROOT)
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|home| std::path::PathBuf::from(home).join(".local/share"))
                })
                .map(|base| base.join("melodee/assets"))
        });
    provision_legacy_lifecycle_once_with_asset_root(config, store, pool, fence, asset_root).await
}

#[cfg(target_os = "linux")]
async fn provision_legacy_lifecycle_once_with_asset_root(
    config: &crate::config::IngestionConfig,
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    fence: &melodee_persistence::LifecycleMaintenance,
    asset_root: Option<std::path::PathBuf>,
) -> Result<LegacyLifecycleProvisionOutcome, AcquisitionError> {
    use melodee_persistence::LifecycleQuarantineReason;
    let Some(id) = store.next_lifecycle_provision_item(pool, fence).await? else {
        return Ok(LegacyLifecycleProvisionOutcome::PassComplete);
    };
    let identity = match store.reserve_work_item_lifecycle(pool, fence, id).await {
        Ok(identity) => identity,
        Err(melodee_persistence::WorkStoreError::InvalidRequest(_)) => {
            store
                .quarantine_lifecycle_provision_item(
                    pool,
                    fence,
                    id,
                    LifecycleQuarantineReason::Ambiguous,
                )
                .await?;
            return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
        }
        Err(error) => return Err(error.into()),
    };
    let Some(settings) = config.protected_custody.clone() else {
        store
            .quarantine_lifecycle_provision_item(
                pool,
                fence,
                id,
                LifecycleQuarantineReason::Unsupported,
            )
            .await?;
        return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
    };
    let Some(stage_root) = config.staging_root.clone() else {
        store
            .quarantine_lifecycle_provision_item(
                pool,
                fence,
                id,
                LifecycleQuarantineReason::Missing,
            )
            .await?;
        return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
    };
    let Some(asset_root) = asset_root else {
        store
            .quarantine_lifecycle_provision_item(
                pool,
                fence,
                id,
                LifecycleQuarantineReason::Unsupported,
            )
            .await?;
        return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
    };
    if config.sources.len() > MAX_LEGACY_TOPOLOGY_ROOTS.saturating_sub(4) {
        store
            .quarantine_lifecycle_provision_item(
                pool,
                fence,
                id,
                LifecycleQuarantineReason::Unsupported,
            )
            .await?;
        return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
    }
    let mut other_roots: Vec<_> = config
        .sources
        .iter()
        .map(|source| source.root.clone())
        .collect();
    other_roots.push(stage_root.clone());
    other_roots.push(config.work_area_base.clone());
    if let Some(canonical) = &config.storage_root {
        other_roots.push(canonical.clone());
    }
    other_roots.push(asset_root);
    let mut matches = config
        .sources
        .iter()
        .filter(|source| source.id == identity.source_library_id);
    let source = matches.next().cloned();
    if source.is_none() || matches.next().is_some() {
        store
            .quarantine_lifecycle_provision_item(
                pool,
                fence,
                id,
                LifecycleQuarantineReason::Ambiguous,
            )
            .await?;
        return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
    }
    let source = source.ok_or(AcquisitionError::SourceNotConfigured)?;
    let identity_for_task = identity.clone();
    let stage_for_task = stage_root.clone();
    let prepared = crate::admission::run(move || {
        let root = GuardedRoot::open(source.clone())
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let namespace = std::sync::Arc::new(validate_legacy_lifecycle_namespace(
            &settings.private_root,
            &source.root,
            settings.producer_uid,
        )?);
        // Establish every configured root before creating an inode. A missing
        // root or obvious overlap must not strand an avoidable reservation.
        validate_legacy_other_roots(&namespace, &other_roots)?;
        let stage = validate_legacy_lifecycle_namespace(
            &stage_for_task,
            &source.root,
            settings.producer_uid,
        )?;
        stage.revalidate(&root)?;
        let stage_generation = lifecycle_file_identity(&stage.private)?;
        let capability = match identity_for_task.state.as_str() {
            "reserved" => LegacyLifecycleCapability::create_reserved(namespace, root.clone(), id)?,
            "ready" => {
                if identity_for_task.stage_root_generation.as_deref()
                    != Some(stage_generation.as_str())
                {
                    return Err(AcquisitionError::SourceChanged);
                }
                LegacyLifecycleCapability::acquire(
                    namespace,
                    root.clone(),
                    id,
                    identity_for_task
                        .lock_root_generation
                        .clone()
                        .ok_or(AcquisitionError::SourceChanged)?,
                    identity_for_task
                        .lock_generation
                        .clone()
                        .ok_or(AcquisitionError::SourceChanged)?,
                )?
            }
            _ => return Err(AcquisitionError::SourceChanged),
        };
        capability.bind_other_roots(&other_roots)?;
        stage.revalidate(&root)?;
        capability.revalidate()?;
        let (lock_root, lock_inode) = capability.generations()?;
        Ok::<_, AcquisitionError>((
            capability,
            root,
            stage,
            lock_root,
            lock_inode,
            stage_generation,
        ))
    })
    .await
    .map_err(AcquisitionError::Task)?;
    let (capability, root, stage, lock_root, lock_inode, stage_generation) = match prepared {
        Ok(prepared) => prepared,
        Err(_) => {
            // Existing unacknowledged names and unsupported topologies must
            // remain intact; the bounded cursor advances only with quarantine.
            store
                .quarantine_lifecycle_provision_item(
                    pool,
                    fence,
                    id,
                    LifecycleQuarantineReason::Ambiguous,
                )
                .await?;
            return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
        }
    };
    let acknowledged = store
        .acknowledge_work_item_lifecycle(
            pool,
            fence,
            id,
            &lock_root,
            &lock_inode,
            &stage_generation,
        )
        .await?;
    let verified = crate::admission::run(move || {
        capability.revalidate()?;
        stage.revalidate(&root)?;
        if lifecycle_file_identity(&stage.private)? != stage_generation {
            return Err(AcquisitionError::SourceChanged);
        }
        Ok::<(), AcquisitionError>(())
    })
    .await
    .map_err(AcquisitionError::Task)?;
    if verified.is_err() {
        store
            .invalidate_work_item_lifecycle(
                pool,
                fence,
                &acknowledged,
                LifecycleQuarantineReason::Changed,
            )
            .await?;
        return Ok(LegacyLifecycleProvisionOutcome::Quarantined(id));
    }
    // Runtime use must independently acquire this inode and revalidate its
    // claim; this maintenance acknowledgement grants no filesystem authority.
    Ok(LegacyLifecycleProvisionOutcome::Ready(id))
}

/// A pinned directory reached only through a verified private namespace.
/// The stage-file prototype is test-only until the complete receipt protocol
/// can enforce this capability through its entire publication lifetime.
#[cfg(all(test, target_os = "linux"))]
pub(crate) struct PrivateStageParent {
    namespace: std::sync::Arc<CustodyNamespace>,
    source_root: GuardedRoot,
    relative: std::path::PathBuf,
    held: File,
}

#[cfg(all(test, target_os = "linux"))]
impl PrivateStageParent {
    pub(crate) fn open(
        namespace: std::sync::Arc<CustodyNamespace>,
        source_root: GuardedRoot,
        relative: &std::path::Path,
    ) -> Result<Self, AcquisitionError> {
        namespace.revalidate(&source_root)?;
        let held = Self::open_relative(&namespace, relative)?;
        let parent = Self {
            namespace,
            source_root,
            relative: relative.to_owned(),
            held,
        };
        parent.checked_file()?;
        Ok(parent)
    }

    fn open_relative(
        namespace: &CustodyNamespace,
        relative: &std::path::Path,
    ) -> Result<File, AcquisitionError> {
        use std::os::unix::fs::MetadataExt;
        use std::path::Component;

        if source_has_submount(&namespace.topology, &namespace.private_path)? {
            return Err(AcquisitionError::Task(
                "stage namespace has a submount".to_owned(),
            ));
        }
        let mut directory = namespace
            .private
            .try_clone()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(AcquisitionError::Task(
                    "invalid private stage parent".to_owned(),
                ));
            };
            directory = open_child_directory(&directory, name)
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            verify_acl_free(&directory)?;
            let metadata = directory
                .metadata()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0
            {
                return Err(AcquisitionError::Task(
                    "stage ancestry is not private and service-owned".to_owned(),
                ));
            }
        }
        Ok(directory)
    }

    pub(crate) fn checked_file(&self) -> Result<&File, AcquisitionError> {
        use std::os::unix::fs::MetadataExt;
        self.namespace.revalidate(&self.source_root)?;
        let current = Self::open_relative(&self.namespace, &self.relative)?;
        let held = self
            .held
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let current = current
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if (held.dev(), held.ino()) != (current.dev(), current.ino()) {
            return Err(AcquisitionError::Task(
                "private stage parent changed".to_owned(),
            ));
        }
        Ok(&self.held)
    }
}

/// A private attempt directory and its exclusive marker. This test-only
/// capability proves namespace provenance, not source or staged content.
/// Production adoption also requires a live DB-issued attempt/lease fence.
#[cfg(all(test, target_os = "linux"))]
struct ProtectedStageAttempt {
    parent: std::sync::Arc<PrivateStageParent>,
    lock: ProtectedAttemptLock,
    attempt_id: uuid::Uuid,
    relative: std::path::PathBuf,
    namespace_generation: String,
    marker_generation: String,
}

#[cfg(all(test, target_os = "linux"))]
impl ProtectedStageAttempt {
    fn identity(file: &File) -> Result<String, AcquisitionError> {
        use std::os::unix::fs::MetadataExt;
        let metadata = file
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }

    fn create(
        lock: ProtectedAttemptLock,
        attempt_id: uuid::Uuid,
    ) -> Result<Self, AcquisitionError> {
        use std::io::Write;
        if lock.attempt_id != attempt_id {
            return Err(AcquisitionError::SourceChanged);
        }
        lock.verify()?;
        let relative = std::path::PathBuf::from(format!("protected-{}", attempt_id.simple()));
        let parent = &lock.parent;
        mkdir_private_child(parent.checked_file()?, relative.as_os_str())
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        parent
            .checked_file()?
            .sync_all()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let attempt_parent = std::sync::Arc::new(PrivateStageParent::open(
            lock.parent.namespace.clone(),
            lock.parent.source_root.clone(),
            &relative,
        )?);
        let mut marker = File::from(
            rustix::fs::openat(
                attempt_parent.checked_file()?,
                ".melodee-attempt",
                rustix::fs::OFlags::WRONLY
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
        );
        marker
            .write_all(attempt_id.to_string().as_bytes())
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        marker
            .sync_all()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        attempt_parent
            .checked_file()?
            .sync_all()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let attempt = Self {
            namespace_generation: Self::identity(attempt_parent.checked_file()?)?,
            marker_generation: Self::identity(&marker)?,
            parent: attempt_parent,
            lock,
            attempt_id,
            relative,
        };
        attempt.verify()?;
        Ok(attempt)
    }

    fn reopen(
        lock: ProtectedAttemptLock,
        attempt_id: uuid::Uuid,
        namespace_generation: String,
        marker_generation: String,
    ) -> Result<Self, AcquisitionError> {
        if lock.attempt_id != attempt_id {
            return Err(AcquisitionError::SourceChanged);
        }
        lock.verify()?;
        let relative = std::path::PathBuf::from(format!("protected-{}", attempt_id.simple()));
        let parent = std::sync::Arc::new(PrivateStageParent::open(
            lock.parent.namespace.clone(),
            lock.parent.source_root.clone(),
            &relative,
        )?);
        let attempt = Self {
            parent,
            lock,
            attempt_id,
            relative,
            namespace_generation,
            marker_generation,
        };
        attempt.verify()?;
        Ok(attempt)
    }

    fn verify(&self) -> Result<(), AcquisitionError> {
        use std::io::Read;
        use std::os::unix::fs::MetadataExt;
        self.lock.verify()?;
        let parent = self.parent.checked_file()?;
        if Self::identity(parent)? != self.namespace_generation {
            return Err(AcquisitionError::SourceChanged);
        }
        let mut marker = File::from(
            rustix::fs::openat(
                parent,
                ".melodee-attempt",
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK,
                rustix::fs::Mode::empty(),
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
        );
        let metadata = marker
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || Self::identity(&marker)? != self.marker_generation
        {
            return Err(AcquisitionError::SourceChanged);
        }
        verify_acl_free(&marker)?;
        let mut contents = Vec::with_capacity(37);
        marker
            .by_ref()
            .take(37)
            .read_to_end(&mut contents)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if contents != self.attempt_id.to_string().as_bytes() {
            return Err(AcquisitionError::SourceChanged);
        }
        self.parent.checked_file()?;
        self.lock.verify()?;
        Ok(())
    }
}

/// An independently opened, exclusive per-attempt lock in the protected
/// parent. The owner must keep this value in the blocking worker until it
/// terminates; a DB lease alone cannot revoke stale filesystem handles.
/// This is advisory and requires every trusted app writer and cleanup path to
/// acquire it before opening the attempt and recheck DB state afterward.
#[cfg(all(test, target_os = "linux"))]
struct ProtectedAttemptLock {
    parent: std::sync::Arc<PrivateStageParent>,
    attempt_id: uuid::Uuid,
    file: File,
    name: std::ffi::OsString,
    generation: String,
    // The locked open-file description has one owner; shared references may
    // not be sent to competing threads and act as separate lock acquisitions.
    _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
}

#[cfg(all(test, target_os = "linux"))]
impl ProtectedAttemptLock {
    fn create(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        id: uuid::Uuid,
    ) -> Result<Self, AcquisitionError> {
        Self::create_named(
            namespace,
            root,
            id,
            format!("protected-{}.lock", id.simple()).into(),
        )
    }

    fn create_named(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        id: uuid::Uuid,
        name: std::ffi::OsString,
    ) -> Result<Self, AcquisitionError> {
        let parent = std::sync::Arc::new(PrivateStageParent::open(
            namespace,
            root,
            std::path::Path::new(""),
        )?);
        let file = File::from(
            rustix::fs::openat(
                parent.checked_file()?,
                &name,
                rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
        );
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        file.sync_all()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        parent
            .checked_file()?
            .sync_all()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let generation = ProtectedStageAttempt::identity(&file)?;
        let lock = Self {
            parent,
            attempt_id: id,
            file,
            name,
            generation,
            _not_sync: std::marker::PhantomData,
        };
        lock.verify()?;
        Ok(lock)
    }

    fn acquire(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        id: uuid::Uuid,
        expected_generation: &str,
    ) -> Result<Self, AcquisitionError> {
        Self::acquire_named(
            namespace,
            root,
            id,
            format!("protected-{}.lock", id.simple()).into(),
            expected_generation,
        )
    }

    fn acquire_named(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        id: uuid::Uuid,
        name: std::ffi::OsString,
        expected_generation: &str,
    ) -> Result<Self, AcquisitionError> {
        let parent = std::sync::Arc::new(PrivateStageParent::open(
            namespace,
            root,
            std::path::Path::new(""),
        )?);
        let file = File::from(
            rustix::fs::openat(
                parent.checked_file()?,
                &name,
                rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK,
                rustix::fs::Mode::empty(),
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
        );
        if ProtectedStageAttempt::identity(&file)? != expected_generation {
            return Err(AcquisitionError::SourceChanged);
        }
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let lock = Self {
            parent,
            attempt_id: id,
            file,
            name,
            generation: expected_generation.to_owned(),
            _not_sync: std::marker::PhantomData,
        };
        lock.verify()?;
        Ok(lock)
    }

    fn verify(&self) -> Result<(), AcquisitionError> {
        use std::os::unix::fs::MetadataExt;
        let metadata = self
            .file
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || ProtectedStageAttempt::identity(&self.file)? != self.generation
        {
            return Err(AcquisitionError::SourceChanged);
        }
        verify_acl_free(&self.file)?;
        let current = File::from(
            rustix::fs::openat(
                self.parent.checked_file()?,
                &self.name,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK,
                rustix::fs::Mode::empty(),
            )
            .map_err(|error| AcquisitionError::Task(error.to_string()))?,
        );
        if ProtectedStageAttempt::identity(&current)? != self.generation {
            return Err(AcquisitionError::SourceChanged);
        }
        self.parent.checked_file()?;
        Ok(())
    }
}

/// A separate lock survives attempt cleanup and fences final-stage lifecycle
/// operations. It is only a test primitive until every production user of the
/// stage participates and the persisted identity is atomically published.
#[cfg(all(test, target_os = "linux"))]
struct ProtectedFinalStageLock(ProtectedAttemptLock);

#[cfg(all(test, target_os = "linux"))]
impl ProtectedFinalStageLock {
    fn create(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        baseline_id: uuid::Uuid,
    ) -> Result<Self, AcquisitionError> {
        ProtectedAttemptLock::create_named(
            namespace,
            root,
            baseline_id,
            format!("stage-{}.lock", baseline_id.simple()).into(),
        )
        .map(Self)
    }

    fn acquire(
        namespace: std::sync::Arc<CustodyNamespace>,
        root: GuardedRoot,
        baseline_id: uuid::Uuid,
        expected_generation: &str,
    ) -> Result<Self, AcquisitionError> {
        ProtectedAttemptLock::acquire_named(
            namespace,
            root,
            baseline_id,
            format!("stage-{}.lock", baseline_id.simple()).into(),
            expected_generation,
        )
        .map(Self)
    }
}

/// Move an attempt's stage directory to the final private slot only after a
/// durable promotion intent. This filesystem step by itself is provisional:
/// any error after rename leaves both namespaces untouched for fenced replay.
#[cfg(all(test, target_os = "linux"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProtectedPromotionStep {
    BeforeRename,
    AfterRename,
    AfterAttemptSync,
    AfterFinalSync,
}

#[cfg(all(test, target_os = "linux"))]
fn promote_protected_stage_with_hooks(
    attempt: &ProtectedStageAttempt,
    expected_generation: &str,
    mut hook: impl FnMut(ProtectedPromotionStep) -> Result<(), AcquisitionError>,
) -> Result<(), AcquisitionError> {
    attempt.verify()?;
    let held = open_child_directory(attempt.parent.checked_file()?, OsStr::new("source"))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let generation = ProtectedStageAttempt::identity(&held)?;
    if generation != expected_generation {
        return Err(AcquisitionError::SourceChanged);
    }
    let final_parent = PrivateStageParent::open(
        attempt.parent.namespace.clone(),
        attempt.parent.source_root.clone(),
        std::path::Path::new(""),
    )?;
    hook(ProtectedPromotionStep::BeforeRename)?;
    rename_child_noreplace(
        attempt.parent.checked_file()?,
        OsStr::new("source"),
        final_parent.checked_file()?,
        OsStr::new("source"),
    )
    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    hook(ProtectedPromotionStep::AfterRename)?;
    attempt
        .parent
        .checked_file()?
        .sync_all()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    hook(ProtectedPromotionStep::AfterAttemptSync)?;
    final_parent
        .checked_file()?
        .sync_all()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    hook(ProtectedPromotionStep::AfterFinalSync)?;
    let moved = open_child_directory(final_parent.checked_file()?, OsStr::new("source"))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if ProtectedStageAttempt::identity(&moved)? != generation
        || ProtectedStageAttempt::identity(&held)? != generation
    {
        return Err(AcquisitionError::SourceChanged);
    }
    attempt.verify()?;
    final_parent.checked_file()?;
    Ok(())
}

/// A replay hint, never an authorization to publish, clean or infer absence.
#[cfg(all(test, target_os = "linux"))]
#[derive(Debug, PartialEq, Eq)]
enum ProtectedPromotionLocation {
    Attempt,
    Final,
    Ambiguous,
}

#[cfg(all(test, target_os = "linux"))]
fn inspect_protected_promotion(
    attempt: &ProtectedStageAttempt,
    expected_generation: &str,
) -> Result<ProtectedPromotionLocation, AcquisitionError> {
    attempt.verify()?;
    let final_parent = PrivateStageParent::open(
        attempt.parent.namespace.clone(),
        attempt.parent.source_root.clone(),
        std::path::Path::new(""),
    )?;
    let probe = |parent: &File| -> Result<Option<String>, AcquisitionError> {
        match open_child_directory(parent, OsStr::new("source")) {
            Ok(file) => ProtectedStageAttempt::identity(&file).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AcquisitionError::Task(error.to_string())),
        }
    };
    let original = probe(attempt.parent.checked_file()?)?;
    let final_slot = probe(final_parent.checked_file()?)?;
    attempt.verify()?;
    Ok(match (original.as_deref(), final_slot.as_deref()) {
        (Some(generation), None) if generation == expected_generation => {
            ProtectedPromotionLocation::Attempt
        }
        (None, Some(generation)) if generation == expected_generation => {
            ProtectedPromotionLocation::Final
        }
        _ => ProtectedPromotionLocation::Ambiguous,
    })
}

/// Verify one sealed file through the promoted private namespace. A caller
/// must still verify complete bounded coverage, the ordered source proofs and
/// the namespace/claim again at publication; one file is never a receipt.
#[cfg(all(test, target_os = "linux"))]
fn verify_protected_stage_file(
    attempt: &ProtectedStageAttempt,
    lifecycle: &ProtectedFinalStageLock,
    expected_stage_generation: &str,
    path_key: &str,
    proof: &melodee_persistence::RetainedFileVerity,
) -> Result<(), AcquisitionError> {
    attempt.verify()?;
    lifecycle.0.verify()?;
    if ProtectedStageAttempt::identity(lifecycle.0.parent.checked_file()?)?
        != ProtectedStageAttempt::identity(attempt.lock.parent.checked_file()?)?
    {
        return Err(AcquisitionError::SourceChanged);
    }
    verify_cleaned_protected_stage_file(lifecycle, expected_stage_generation, path_key, proof)?;
    attempt.verify()?;
    Ok(())
}

/// Remeasure one file after attempt cleanup without using the removed attempt
/// as authority. The caller must first acquire this lifecycle lock and then
/// revalidate the fresh claim and persisted receipt; this check alone cannot
/// establish complete stage coverage or authorize recovery.
#[cfg(all(test, target_os = "linux"))]
fn verify_cleaned_protected_stage_file(
    lifecycle: &ProtectedFinalStageLock,
    expected_stage_generation: &str,
    path_key: &str,
    proof: &melodee_persistence::RetainedFileVerity,
) -> Result<(), AcquisitionError> {
    lifecycle.0.verify()?;
    let root = PrivateStageParent::open(
        lifecycle.0.parent.namespace.clone(),
        lifecycle.0.parent.source_root.clone(),
        std::path::Path::new(""),
    )?;
    let held_stage = open_child_directory(root.checked_file()?, OsStr::new("source"))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if ProtectedStageAttempt::identity(&held_stage)? != expected_stage_generation {
        return Err(AcquisitionError::SourceChanged);
    }
    let path = crate::source::decode_candidate_relative_path(path_key)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let parent = path.parent().ok_or(AcquisitionError::SourceChanged)?;
    let name = path.file_name().ok_or(AcquisitionError::SourceChanged)?;
    let stage_parent = std::sync::Arc::new(PrivateStageParent::open(
        lifecycle.0.parent.namespace.clone(),
        lifecycle.0.parent.source_root.clone(),
        &std::path::Path::new("source").join(parent),
    )?);
    let sealed = crate::stage_verity::SealedStageFile::reopen(
        &stage_parent,
        name,
        proof.digest,
        proof.device,
        proof.inode,
    )
    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    sealed
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let current_stage = open_child_directory(root.checked_file()?, OsStr::new("source"))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if ProtectedStageAttempt::identity(&current_stage)? != expected_stage_generation {
        return Err(AcquisitionError::SourceChanged);
    }
    lifecycle.0.verify()?;
    Ok(())
}

/// Check one stored directory through no-follow private-parent handles. This
/// does not establish membership completeness: the caller must also inspect
/// bounded native directory pages against the authoritative baseline.
#[cfg(all(test, target_os = "linux"))]
fn verify_cleaned_protected_stage_directory(
    lifecycle: &ProtectedFinalStageLock,
    expected_stage_generation: &str,
    entry: &melodee_persistence::ProtectedBaselineDirectoryEntry,
) -> Result<(), AcquisitionError> {
    lifecycle.0.verify()?;
    let root = lifecycle.0.parent.checked_file()?;
    let stage = open_child_directory(root, OsStr::new("source"))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if ProtectedStageAttempt::identity(&stage)? != expected_stage_generation {
        return Err(AcquisitionError::SourceChanged);
    }
    if entry.path_key.is_empty() {
        // The synthetic root stores the entire tree digest, not a directory
        // entry digest. Its binding must be checked against the fresh receipt
        // by the caller; this branch only checks its structural identity.
        if entry.ordinal != 0 || !entry.parent_key.is_empty() {
            return Err(AcquisitionError::SourceChanged);
        }
    } else {
        let path = crate::source::decode_candidate_relative_path(&entry.path_key)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let parent = path.parent().ok_or(AcquisitionError::SourceChanged)?;
        if crate::source::candidate_relative_path(parent) != entry.parent_key
            || crate::source::candidate_relative_path(&path) != entry.path_key
        {
            return Err(AcquisitionError::SourceChanged);
        }
        let directory = PrivateStageParent::open(
            lifecycle.0.parent.namespace.clone(),
            lifecycle.0.parent.source_root.clone(),
            &std::path::Path::new("source").join(&path),
        )?;
        directory.checked_file()?;
        let mut digest = blake3::Hasher::new();
        digest.update(b"melodee-custody-entry-v3\0");
        digest.update(b"unix\0");
        crate::acquisition::hash_custody_path(&mut digest, &path, true, 0)?;
        if digest.finalize().to_hex().as_str() != entry.stage_digest {
            return Err(AcquisitionError::SourceChanged);
        }
    }
    let current = open_child_directory(lifecycle.0.parent.checked_file()?, OsStr::new("source"))
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if ProtectedStageAttempt::identity(&current)? != expected_stage_generation {
        return Err(AcquisitionError::SourceChanged);
    }
    lifecycle.0.verify()?;
    Ok(())
}

/// Keep the exact claim epoch live while a bounded filesystem operation is in
/// flight. Poll the operation while a heartbeat waits for a database lock so a
/// producer holding that lock cannot deadlock its own renewal.
#[cfg(all(test, target_os = "linux"))]
async fn with_protected_claim_lease<T>(
    future: impl std::future::Future<Output = Result<T, AcquisitionError>>,
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    claim: &melodee_persistence::ProtectedClaimEpoch,
    duration: time::Duration,
) -> Result<T, AcquisitionError> {
    let interval = crate::normalize::heartbeat_interval(duration)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    tokio::pin!(future);
    loop {
        tokio::select! {
            outcome = &mut future => return outcome,
            _ = ticker.tick() => {
                let heartbeat = store.renew_protected_claim_epoch(pool, claim, duration);
                tokio::pin!(heartbeat);
                tokio::select! {
                    outcome = &mut future => return outcome,
                    result = &mut heartbeat => { result?; }
                }
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
struct ProtectedAttemptProducer<T: Send + 'static> {
    receiver: Option<tokio::sync::mpsc::Receiver<Result<T, AcquisitionError>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    reaper_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

#[cfg(all(test, target_os = "linux"))]
impl<T: Send + 'static> ProtectedAttemptProducer<T> {
    async fn finish(mut self) -> Result<T, AcquisitionError> {
        let result = self
            .receiver
            .as_mut()
            .ok_or_else(|| AcquisitionError::Task("attempt producer missing".to_owned()))?
            .recv()
            .await
            .ok_or_else(|| AcquisitionError::Task("attempt producer stopped".to_owned()))?;
        loop {
            let thread = self
                .thread
                .as_ref()
                .ok_or_else(|| AcquisitionError::Task("attempt producer missing".to_owned()))?;
            if thread.is_finished() {
                let thread = self
                    .thread
                    .take()
                    .ok_or_else(|| AcquisitionError::Task("attempt producer missing".to_owned()))?;
                thread
                    .join()
                    .map_err(|_| AcquisitionError::Task("attempt producer panicked".to_owned()))?;
                return result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
impl<T: Send + 'static> Drop for ProtectedAttemptProducer<T> {
    fn drop(&mut self) {
        self.receiver.take();
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

/// Verify one bounded persistence page while retaining the lifecycle lock in
/// the worker until it actually exits, even if the async caller is cancelled.
/// The returned lock must remain held across all subsequent pages and the
/// fenced receipt decision; a page is never a complete-stage proof.
#[cfg(all(test, target_os = "linux"))]
async fn spawn_cleaned_stage_page(
    lifecycle: ProtectedFinalStageLock,
    stage_generation: String,
    entries: Vec<melodee_persistence::ProtectedBaselineVerityEntry>,
) -> Result<ProtectedAttemptProducer<(ProtectedFinalStageLock, Option<i64>)>, AcquisitionError> {
    spawn_cleaned_stage_page_with_hook(lifecycle, stage_generation, entries, || {}).await
}

#[cfg(all(test, target_os = "linux"))]
async fn spawn_cleaned_stage_page_with_hook(
    lifecycle: ProtectedFinalStageLock,
    stage_generation: String,
    entries: Vec<melodee_persistence::ProtectedBaselineVerityEntry>,
    hook: impl FnOnce() + Send + 'static,
) -> Result<ProtectedAttemptProducer<(ProtectedFinalStageLock, Option<i64>)>, AcquisitionError> {
    if entries.len() > 16 || entries.windows(2).any(|w| w[0].ordinal >= w[1].ordinal) {
        return Err(AcquisitionError::SourceChanged);
    }
    let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
        .await
        .map_err(AcquisitionError::Task)?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let thread = std::thread::Builder::new()
        .name("melodee-stage-remeasure".to_owned())
        .spawn(move || {
            let _permit = permit;
            let result = (|| {
                hook();
                let mut last = None;
                for entry in entries {
                    if sender.is_closed() {
                        return Err(AcquisitionError::Task(
                            "stage remeasurement cancelled".to_owned(),
                        ));
                    }
                    verify_cleaned_protected_stage_file(
                        &lifecycle,
                        &stage_generation,
                        &entry.path_key,
                        &entry.evidence,
                    )?;
                    last = Some(entry.ordinal);
                }
                lifecycle.0.verify()?;
                Ok((lifecycle, last))
            })();
            let _ = sender.blocking_send(result);
        })
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(ProtectedAttemptProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    })
}

/// Enumerate one bounded native directory page under the retained lifecycle
/// lock. The result is only candidate membership evidence: partial pages and
/// directory cookies do not prove that an unobserved entry is absent.
#[cfg(all(test, target_os = "linux"))]
struct CleanedStageDirectoryPage {
    lifecycle: ProtectedFinalStageLock,
    entries: Vec<(String, Option<bool>)>,
    continuation: RetainedDirectoryContinuation,
    generation: String,
}

#[cfg(all(test, target_os = "linux"))]
async fn spawn_cleaned_directory_page(
    lifecycle: ProtectedFinalStageLock,
    lifecycle_root_generation: String,
    stage_generation: String,
    parent_key: String,
    cookie: Option<u64>,
) -> Result<ProtectedAttemptProducer<CleanedStageDirectoryPage>, AcquisitionError> {
    let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
        .await
        .map_err(AcquisitionError::Task)?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let thread = std::thread::Builder::new()
        .name("melodee-stage-directory".to_owned())
        .spawn(move || {
            let _permit = permit;
            let result = (|| {
                lifecycle.0.verify()?;
                if ProtectedStageAttempt::identity(lifecycle.0.parent.checked_file()?)?
                    != lifecycle_root_generation
                {
                    return Err(AcquisitionError::SourceChanged);
                }
                let stage =
                    open_child_directory(lifecycle.0.parent.checked_file()?, OsStr::new("source"))
                        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                if ProtectedStageAttempt::identity(&stage)? != stage_generation {
                    return Err(AcquisitionError::SourceChanged);
                }
                let relative = if parent_key.is_empty() {
                    std::path::PathBuf::new()
                } else {
                    let path = crate::source::decode_candidate_relative_path(&parent_key)
                        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                    if crate::source::candidate_relative_path(&path) != parent_key {
                        return Err(AcquisitionError::SourceChanged);
                    }
                    path
                };
                let parent = PrivateStageParent::open(
                    lifecycle.0.parent.namespace.clone(),
                    lifecycle.0.parent.source_root.clone(),
                    &std::path::Path::new("source").join(relative),
                )?;
                parent.checked_file()?;
                let directory = cap_std::fs::Dir::from_std_file(stage);
                let (entries, continuation, generation) = read_retained_directory_page(
                    &directory,
                    &parent_key,
                    cookie,
                    || sender.is_closed(),
                    |_| {},
                )?;
                let current =
                    open_child_directory(lifecycle.0.parent.checked_file()?, OsStr::new("source"))
                        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                if ProtectedStageAttempt::identity(&current)? != stage_generation {
                    return Err(AcquisitionError::SourceChanged);
                }
                parent.checked_file()?;
                lifecycle.0.verify()?;
                if ProtectedStageAttempt::identity(lifecycle.0.parent.checked_file()?)?
                    != lifecycle_root_generation
                {
                    return Err(AcquisitionError::SourceChanged);
                }
                Ok(CleanedStageDirectoryPage {
                    lifecycle,
                    entries,
                    continuation,
                    generation,
                })
            })();
            let _ = sender.blocking_send(result);
        })
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(ProtectedAttemptProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    })
}

/// Check a single observed page against a freshly fenced cleaned receipt.
/// This is provisional evidence only: missing baseline entries, complete
/// tree coverage, retained-source reobservation and final use are separate.
#[cfg(all(test, target_os = "linux"))]
struct CleanedDirectoryPageRequest {
    attempt_id: uuid::Uuid,
    lifecycle: ProtectedFinalStageLock,
    staging_root: std::path::PathBuf,
    parent_key: String,
    cookie: Option<u64>,
}

/// Holds a kernel lock and claim-bound receipt lookup, but intentionally does
/// not authorize stage use: full content, directory and source proofs remain.
#[cfg(all(test, target_os = "linux"))]
struct ProvisionalCleanedStageOwner {
    lifecycle: ProtectedFinalStageLock,
    receipt: melodee_persistence::ProtectedCleanedReceiptRecovery,
}

#[cfg(all(test, target_os = "linux"))]
async fn bind_provisional_cleaned_stage_owner(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    claim: &melodee_persistence::ProtectedClaimEpoch,
    lifecycle: ProtectedFinalStageLock,
    config: &crate::config::ProtectedCustodyConfig,
    staging_root: &std::path::Path,
) -> Result<ProvisionalCleanedStageOwner, AcquisitionError> {
    let receipt = match store.claimed_stage_receipt(pool, claim).await? {
        melodee_persistence::ClaimedStageReceipt::ProtectedCleaned(receipt) => receipt,
        _ => return Err(AcquisitionError::SourceChanged),
    };
    let binding = std::path::Path::new(&receipt.stage_binding);
    if !staging_root.is_absolute()
        || binding.as_os_str().is_empty()
        || binding
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || staging_root.join(binding) != lifecycle.0.parent.namespace.private_path.join("source")
        || !lifecycle
            .0
            .parent
            .namespace
            .private_path
            .starts_with(&config.private_root)
        || lifecycle.0.parent.namespace.producer_uid != config.producer_uid
        || lifecycle.0.attempt_id != receipt.baseline_id
        || lifecycle.0.name != OsStr::new(&receipt.lifecycle_lock_name)
        || lifecycle.0.generation != receipt.lifecycle_lock_generation
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let page = spawn_cleaned_directory_page(
        lifecycle,
        receipt.lifecycle_root_generation.clone(),
        receipt.stage_generation.clone(),
        String::new(),
        None,
    )
    .await?
    .finish()
    .await?;
    let current = match store.claimed_stage_receipt(pool, claim).await? {
        melodee_persistence::ClaimedStageReceipt::ProtectedCleaned(current) => current,
        _ => return Err(AcquisitionError::SourceChanged),
    };
    if current.attempt_id != receipt.attempt_id
        || current.work_item_id != receipt.work_item_id
        || current.baseline_id != receipt.baseline_id
        || current.session_id != receipt.session_id
        || current.source_library_id != receipt.source_library_id
        || current.source_path != receipt.source_path
        || current.source_path_format != receipt.source_path_format
        || current.source_generation != receipt.source_generation
        || current.source_digest != receipt.source_digest
        || current.stage_binding != receipt.stage_binding
        || current.stage_generation != receipt.stage_generation
        || current.legacy_hash != receipt.legacy_hash
        || current.root_generation != receipt.root_generation
        || current.final_parent_generation != receipt.final_parent_generation
        || current.lifecycle_lock_name != receipt.lifecycle_lock_name
        || current.lifecycle_root_generation != receipt.lifecycle_root_generation
        || current.lifecycle_lock_generation != receipt.lifecycle_lock_generation
    {
        return Err(AcquisitionError::SourceChanged);
    }
    Ok(ProvisionalCleanedStageOwner {
        lifecycle: page.lifecycle,
        receipt: current,
    })
}

#[cfg(all(test, target_os = "linux"))]
async fn check_cleaned_directory_membership_page(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    claim: &melodee_persistence::ProtectedClaimEpoch,
    request: CleanedDirectoryPageRequest,
) -> Result<CleanedStageDirectoryPage, AcquisitionError> {
    check_cleaned_directory_membership_page_with_hook(store, pool, claim, request, || async {})
        .await
}

#[cfg(all(test, target_os = "linux"))]
async fn check_cleaned_directory_membership_page_with_hook<F>(
    store: &melodee_persistence::WorkItemStore,
    pool: &sqlx::PgPool,
    claim: &melodee_persistence::ProtectedClaimEpoch,
    request: CleanedDirectoryPageRequest,
    after_membership: impl FnOnce() -> F,
) -> Result<CleanedStageDirectoryPage, AcquisitionError>
where
    F: std::future::Future<Output = ()>,
{
    let CleanedDirectoryPageRequest {
        attempt_id,
        lifecycle,
        staging_root,
        parent_key,
        cookie,
    } = request;
    let receipt = store
        .find_cleaned_protected_receipt(pool, claim, attempt_id)
        .await?
        .ok_or(AcquisitionError::SourceChanged)?;
    let binding = std::path::Path::new(&receipt.stage_binding);
    if !staging_root.is_absolute()
        || binding.as_os_str().is_empty()
        || binding
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || staging_root.join(binding) != lifecycle.0.parent.namespace.private_path.join("source")
        || lifecycle.0.attempt_id != receipt.baseline_id
        || lifecycle.0.name != OsStr::new(&receipt.lifecycle_lock_name)
        || lifecycle.0.generation != receipt.lifecycle_lock_generation
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let page = spawn_cleaned_directory_page(
        lifecycle,
        receipt.lifecycle_root_generation.clone(),
        receipt.stage_generation.clone(),
        parent_key.clone(),
        cookie,
    )
    .await?
    .finish()
    .await?;
    if page.continuation == RetainedDirectoryContinuation::Invalidated {
        return Err(AcquisitionError::SourceChanged);
    }
    let mut members = Vec::with_capacity(page.entries.len());
    for (path, is_directory) in &page.entries {
        members.push(melodee_persistence::RetainedDirectoryMember {
            path_key: path.clone(),
            is_directory: is_directory.ok_or(AcquisitionError::SourceChanged)?,
        });
    }
    let matches = store
        .cleaned_protected_directory_membership(pool, claim, attempt_id, &parent_key, &members)
        .await?;
    if matches.len() != members.len() || matches.iter().any(|matched| !matched) {
        return Err(AcquisitionError::SourceChanged);
    }
    after_membership().await;
    // Membership performs a fresh epoch/receipt check under the DB lock. Read
    // the same bounded page again after that await while still holding the
    // lifecycle lock; a changed stage or page invalidates the observation.
    let CleanedStageDirectoryPage {
        lifecycle,
        entries,
        continuation,
        generation,
    } = page;
    let checked = spawn_cleaned_directory_page(
        lifecycle,
        receipt.lifecycle_root_generation,
        receipt.stage_generation,
        parent_key,
        cookie,
    )
    .await?
    .finish()
    .await?;
    if checked.entries != entries
        || checked.continuation != continuation
        || checked.generation != generation
    {
        return Err(AcquisitionError::SourceChanged);
    }
    // A later receipt/use transaction must still check the same claim epoch;
    // this provisional page cannot authorize publication or stage access.
    Ok(checked)
}

#[cfg(all(test, target_os = "linux"))]
async fn spawn_protected_lock_create(
    namespace: std::sync::Arc<CustodyNamespace>,
    root: GuardedRoot,
    attempt_id: uuid::Uuid,
) -> Result<ProtectedAttemptProducer<ProtectedAttemptLock>, AcquisitionError> {
    let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
        .await
        .map_err(AcquisitionError::Task)?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let thread = std::thread::Builder::new()
        .name("melodee-protected-lock".to_owned())
        .spawn(move || {
            let _permit = permit;
            let result = if sender.is_closed() {
                Err(AcquisitionError::Task(
                    "protected lock cancelled".to_owned(),
                ))
            } else {
                ProtectedAttemptLock::create(namespace, root, attempt_id)
            };
            let _ = sender.blocking_send(result);
        })
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(ProtectedAttemptProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    })
}

#[cfg(all(test, target_os = "linux"))]
async fn spawn_protected_attempt_create(
    lock: ProtectedAttemptLock,
    root: GuardedRoot,
    target: ObservationTarget,
    expected_root_generation: String,
    attempt_id: uuid::Uuid,
) -> Result<ProtectedAttemptProducer<ProtectedStageAttempt>, AcquisitionError> {
    let reaper_permit = crate::reconcile::acquire_custody_reaper_permit()
        .await
        .map_err(AcquisitionError::Task)?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let thread = std::thread::Builder::new()
        .name("melodee-protected-attempt".to_owned())
        .spawn(move || {
            let _permit = permit;
            let result = (|| {
                if sender.is_closed() {
                    return Err(AcquisitionError::Task(
                        "protected attempt cancelled".to_owned(),
                    ));
                }
                (target.open_verified)()?;
                let held = root
                    .custody_root_handle()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                if source_generation_from_directory(&held, std::path::Path::new("source-root"))?
                    != expected_root_generation
                {
                    return Err(AcquisitionError::SourceChanged);
                }
                if sender.is_closed() {
                    return Err(AcquisitionError::Task(
                        "protected attempt cancelled".to_owned(),
                    ));
                }
                ProtectedStageAttempt::create(lock, attempt_id)
            })();
            let _ = sender.blocking_send(result);
        })
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(ProtectedAttemptProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    })
}

/// The database row is reserved before filesystem creation. Losing the
/// worker or the DB acknowledgement leaves an ambiguous reserved attempt;
/// neither this wrapper nor its caller may delete or adopt those bytes.
#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::too_many_arguments)]
async fn reserve_and_create_protected_attempt(
    pool: &sqlx::PgPool,
    store: &melodee_persistence::WorkItemStore,
    claim: &melodee_persistence::ProtectedClaimEpoch,
    namespace: std::sync::Arc<CustodyNamespace>,
    root: GuardedRoot,
    target: ObservationTarget,
    root_generation: &str,
    final_binding: &str,
    lease_duration: time::Duration,
) -> Result<
    (
        melodee_persistence::ProtectedStageAttempt,
        ProtectedStageAttempt,
    ),
    AcquisitionError,
> {
    let reserved = store
        .reserve_protected_stage_attempt(
            pool,
            claim,
            root_generation,
            &target.generation,
            final_binding,
        )
        .await?;
    if reserved.source_path() != target.binding
        || reserved.source_generation() != target.generation
        || reserved.root_generation() != root_generation
        || reserved.namespace_name() != format!("protected-{}", reserved.id().simple())
    {
        return Err(AcquisitionError::SourceChanged);
    }
    let operation = async {
        let lock = spawn_protected_lock_create(namespace, root.clone(), reserved.id())
            .await?
            .finish()
            .await?;
        store
            .validate_reserved_protected_stage_attempt(pool, claim, &reserved)
            .await?;
        let physical = spawn_protected_attempt_create(
            lock,
            root,
            target,
            root_generation.to_owned(),
            reserved.id(),
        )
        .await?
        .finish()
        .await?;
        let acknowledged = store
            .acknowledge_protected_stage_created(
                pool,
                claim,
                &reserved,
                &melodee_persistence::ProtectedStageIdentity {
                    root_generation,
                    namespace_generation: &physical.namespace_generation,
                    marker_generation: &physical.marker_generation,
                    lock_generation: &physical.lock.generation,
                },
            )
            .await?;
        Ok((acknowledged, physical))
    };
    with_protected_claim_lease(operation, store, pool, claim, lease_duration).await
}

/// An intent replay must inspect the protected custody entry before deciding
/// whether the source should be revisited. Neither result authorizes removal.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) enum PendingCustodyObservation {
    Missing,
    Matching { held: File, legacy_hash: String },
    Different { held: File },
}

/// Inspect the deterministic custody name through an already validated private
/// parent. A changed root aborts replay; callers must revalidate again at the
/// staging boundary. Unexpected bytes remain held intact for review.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn inspect_pending_custody(
    guarded_root: &GuardedRoot,
    private_parent: &File,
    private_name: &OsStr,
    expected_generation: &str,
    expected_tree_hash: &str,
) -> Result<PendingCustodyObservation, AcquisitionError> {
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let held = match open_child_directory(private_parent, private_name) {
        Ok(held) => held,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            guarded_root
                .verify_current()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            return Ok(PendingCustodyObservation::Missing);
        }
        Err(error) => return Err(AcquisitionError::Task(error.to_string())),
    };
    let generation = source_generation_from_directory(&held, std::path::Path::new(private_name))?;
    let (hash, legacy_hash) = crate::acquisition::tree_hashes_from_pinned_directory(&held)?;
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if generation == expected_generation && hash == expected_tree_hash {
        Ok(PendingCustodyObservation::Matching { held, legacy_hash })
    } else {
        Ok(PendingCustodyObservation::Different { held })
    }
}

/// Validate the namespace protecting a proposed custody directory. This does
/// not prove that producers have relinquished writable file handles, and is
/// therefore insufficient on its own to authorize draining any source.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
#[allow(dead_code)]
pub(crate) fn validate_custody_namespace(
    private_path: &std::path::Path,
    source_path: &std::path::Path,
    producer_uid: u32,
) -> Result<CustodyNamespace, AcquisitionError> {
    validate_custody_namespace_with_policy(
        private_path,
        source_path,
        producer_uid,
        CustodyTopologyPolicy::RenameSameMount,
    )
}

#[cfg(target_os = "linux")]
fn validate_legacy_lifecycle_namespace(
    private_path: &std::path::Path,
    source_path: &std::path::Path,
    producer_uid: u32,
) -> Result<CustodyNamespace, AcquisitionError> {
    validate_custody_namespace_with_policy(
        private_path,
        source_path,
        producer_uid,
        CustodyTopologyPolicy::LegacyLocalMounts,
    )
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn validate_custody_namespace_with_policy(
    private_path: &std::path::Path,
    source_path: &std::path::Path,
    producer_uid: u32,
    policy: CustodyTopologyPolicy,
) -> Result<CustodyNamespace, AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    let service_uid = rustix::process::geteuid().as_raw();
    if producer_uid == service_uid || producer_uid == 0 {
        return Err(AcquisitionError::Task(
            "custody requires a distinct, unprivileged producer identity".to_owned(),
        ));
    }
    let normalized_absolute = |path: &std::path::Path| {
        path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    };
    if !normalized_absolute(private_path)
        || !normalized_absolute(source_path)
        || private_path.starts_with(source_path)
        || source_path.starts_with(private_path)
    {
        return Err(AcquisitionError::Task(
            "custody and source roots must be distinct absolute paths".to_owned(),
        ));
    }
    let source = crate::secure_fs::open_directory(source_path)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let source_metadata = source
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let private = crate::secure_fs::open_directory(private_path)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let private_metadata = private
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let topology = namespace_mount_topology(&source, &private, source_path, private_path, policy)?;
    if (matches!(policy, CustodyTopologyPolicy::RenameSameMount)
        && source_metadata.dev() != private_metadata.dev())
        || private_metadata.uid() != service_uid
        || private_metadata.mode() & 0o077 != 0
    {
        return Err(AcquisitionError::Task(
            "custody root must be private, service-owned and on the source filesystem".to_owned(),
        ));
    }
    for ancestor in private_path.ancestors() {
        let directory = crate::secure_fs::open_directory(ancestor)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let metadata = directory
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        verify_acl_free(&directory)?;
        if (metadata.dev(), metadata.ino()) == (source_metadata.dev(), source_metadata.ino())
            || (metadata.uid() != service_uid && metadata.uid() != 0)
            || metadata.mode() & 0o022 != 0
        {
            return Err(AcquisitionError::Task(
                "custody ancestry is not protected from the producer".to_owned(),
            ));
        }
    }
    for ancestor in source_path.ancestors() {
        let directory = crate::secure_fs::open_directory(ancestor)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let metadata = directory
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if (metadata.dev(), metadata.ino()) == (private_metadata.dev(), private_metadata.ino()) {
            return Err(AcquisitionError::Task(
                "custody root contains the source root".to_owned(),
            ));
        }
    }
    if namespace_mount_topology(&source, &private, source_path, private_path, policy)? != topology {
        return Err(AcquisitionError::Task(
            "mount topology changed during custody validation".to_owned(),
        ));
    }
    Ok(CustodyNamespace {
        source_path: source_path.to_path_buf(),
        private_path: private_path.to_path_buf(),
        producer_uid,
        source,
        private,
        topology,
        policy,
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn linux_mount_id(directory: &File) -> Result<u64, AcquisitionError> {
    use rustix::fs::{AtFlags, StatxFlags, statx};
    let stat = statx(directory, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if stat.stx_mask & StatxFlags::MNT_ID.bits() == 0 {
        return Err(AcquisitionError::Task(
            "mount identity unavailable".to_owned(),
        ));
    }
    Ok(stat.stx_mnt_id)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_linux_mountinfo() -> Result<Vec<u8>, AcquisitionError> {
    use std::io::Read;
    const MAX_MOUNTINFO_BYTES: u64 = 8 * 1024 * 1024;
    let file = File::open("/proc/self/mountinfo")
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let procfs =
        rustix::fs::fstatfs(&file).map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if procfs.f_type as u32 != 0x9fa0 {
        return Err(AcquisitionError::Task(
            "mount topology is not procfs".to_owned(),
        ));
    }
    let mut data = Vec::new();
    file.take(MAX_MOUNTINFO_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if data.len() as u64 > MAX_MOUNTINFO_BYTES || data.is_empty() {
        return Err(AcquisitionError::Task(
            "mount topology unavailable or too large".to_owned(),
        ));
    }
    Ok(data)
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn namespace_mount_topology(
    source: &File,
    private: &File,
    source_path: &std::path::Path,
    _private_path: &std::path::Path,
    policy: CustodyTopologyPolicy,
) -> Result<Vec<u8>, AcquisitionError> {
    match policy {
        CustodyTopologyPolicy::RenameSameMount => {
            custody_mount_topology(source, private, source_path)
        }
        #[cfg(target_os = "linux")]
        CustodyTopologyPolicy::LegacyLocalMounts => {
            legacy_local_mount_topology(source, private, source_path, _private_path)
        }
    }
}

/// Separate from rename custody: only recognized local filesystems with
/// distinct device identities, or exactly the same mount, are eligible. Any
/// nested mount or same-device/different-mount case fails closed.
#[cfg(target_os = "linux")]
fn legacy_local_mount_topology(
    source: &File,
    private: &File,
    source_path: &std::path::Path,
    private_path: &std::path::Path,
) -> Result<Vec<u8>, AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    verify_supported_local_filesystem(source)?;
    verify_supported_local_filesystem(private)?;
    let source_dev = source
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?
        .dev();
    let private_dev = private
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?
        .dev();
    let source_mount = linux_mount_id(source)?;
    let private_mount = linux_mount_id(private)?;
    if !legacy_mount_identities_compatible(source_dev, private_dev, source_mount, private_mount) {
        return Err(AcquisitionError::Task(
            "legacy lifecycle mount alias is ambiguous".to_owned(),
        ));
    }
    let data = read_linux_mountinfo()?;
    if source_has_submount(&data, source_path)? || source_has_submount(&data, private_path)? {
        return Err(AcquisitionError::Task(
            "legacy lifecycle roots contain nested mounts".to_owned(),
        ));
    }
    Ok(data)
}

#[cfg(target_os = "linux")]
fn legacy_mount_identities_compatible(
    source_dev: u64,
    private_dev: u64,
    source_mount: u64,
    private_mount: u64,
) -> bool {
    (source_dev == private_dev) == (source_mount == private_mount)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn custody_mount_topology(
    source: &File,
    private: &File,
    source_path: &std::path::Path,
) -> Result<Vec<u8>, AcquisitionError> {
    if linux_mount_id(source)? != linux_mount_id(private)? {
        return Err(AcquisitionError::Task(
            "source and custody must use the same mount".to_owned(),
        ));
    }
    let data = read_linux_mountinfo()?;
    if source_has_submount(&data, source_path)? {
        return Err(AcquisitionError::Task(
            "custody cannot establish a bounded mount topology without source submounts".to_owned(),
        ));
    }
    Ok(data)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn source_has_submount(data: &[u8], source: &std::path::Path) -> Result<bool, AcquisitionError> {
    use std::os::unix::ffi::OsStringExt;
    for line in data
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let mut fields = line.split(|byte| *byte == b' ');
        for _ in 0..4 {
            if fields.next().is_none_or(|field| field.is_empty()) {
                return Err(AcquisitionError::Task(
                    "malformed kernel mount topology".to_owned(),
                ));
            }
        }
        let field = fields
            .next()
            .filter(|field| !field.is_empty())
            .ok_or_else(|| AcquisitionError::Task("malformed kernel mount topology".to_owned()))?;
        if !fields.any(|field| field == b"-") {
            return Err(AcquisitionError::Task(
                "malformed kernel mount topology".to_owned(),
            ));
        }
        let mut decoded = Vec::with_capacity(field.len());
        let mut pos = 0;
        while pos < field.len() {
            if field[pos] == b'\\' {
                let escape = field.get(pos + 1..pos + 4).ok_or_else(|| {
                    AcquisitionError::Task("malformed mount path escape".to_owned())
                })?;
                let byte = match escape {
                    b"040" => b' ',
                    b"011" => b'\t',
                    b"012" => b'\n',
                    b"134" => b'\\',
                    _ => {
                        return Err(AcquisitionError::Task(
                            "unknown mount path escape".to_owned(),
                        ));
                    }
                };
                decoded.push(byte);
                pos += 4;
            } else {
                decoded.push(field[pos]);
                pos += 1;
            }
        }
        let mount = std::path::PathBuf::from(std::ffi::OsString::from_vec(decoded));
        if !mount.is_absolute() {
            return Err(AcquisitionError::Task("non-absolute mount path".to_owned()));
        }
        if mount != source && mount.starts_with(source) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_vendor = "apple")]
fn custody_mount_topology(
    _source: &File,
    _private: &File,
    _source_path: &std::path::Path,
) -> Result<Vec<u8>, AcquisitionError> {
    Err(AcquisitionError::Task(
        "custody mount inspection is unavailable on this platform".to_owned(),
    ))
}

// Whitelist only local Linux filesystem types whose POSIX ACLs are exposed
// through these xattrs. Unknown filesystems and unsupported ACL inspection
// fail closed. This remains a namespace check, not an open-writer proof.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_supported_local_filesystem(directory: &File) -> Result<(), AcquisitionError> {
    let fs = rustix::fs::fstatfs(directory)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    const LOCAL_POSIX_ACL_FILESYSTEMS: [u32; 4] = [0x9123_683e, 0xef53, 0x5846_5342, 0x0102_1994];
    if !LOCAL_POSIX_ACL_FILESYSTEMS.contains(&(fs.f_type as u32)) {
        return Err(AcquisitionError::Task(
            "custody ACL inspection is unsupported for this filesystem".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_acl_free(directory: &File) -> Result<(), AcquisitionError> {
    verify_supported_local_filesystem(directory)?;
    for name in ["system.posix_acl_access", "system.posix_acl_default"] {
        match rustix::fs::fgetxattr_query(directory, name) {
            Err(rustix::io::Errno::NODATA) => {}
            Ok(_) => {
                return Err(AcquisitionError::Task(
                    "custody ancestry has an ACL".to_owned(),
                ));
            }
            Err(error) => {
                return Err(AcquisitionError::Task(format!(
                    "custody ACL inspection failed: {error}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn verify_acl_free(_directory: &File) -> Result<(), AcquisitionError> {
    Err(AcquisitionError::Task(
        "custody ACL inspection is unavailable on this platform".to_owned(),
    ))
}

/// Move a candidate using pinned parents, then compare the held directory
/// with the still-open original occurrence and its preclaim content hash.
/// Mismatches are restored only with NOREPLACE or retained intact for recovery.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(crate) fn quarantine_verified_occurrence(
    guarded_root: &GuardedRoot,
    source_parent: &File,
    source_name: &OsStr,
    original: &File,
    private_namespace: &CustodyNamespace,
    private_name: &OsStr,
    expected_generation: &str,
    expected_tree_hash: &str,
) -> Result<QuarantineOutcome, AcquisitionError> {
    private_namespace.revalidate(guarded_root)?;
    let outcome = quarantine_verified_occurrence_with_hook(
        guarded_root,
        source_parent,
        source_name,
        original,
        &private_namespace.private,
        private_name,
        expected_generation,
        expected_tree_hash,
        || Ok(()),
    )?;
    private_namespace.revalidate(guarded_root)?;
    Ok(outcome)
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn quarantine_verified_occurrence_with_hook(
    guarded_root: &GuardedRoot,
    source_parent: &File,
    source_name: &OsStr,
    original: &File,
    private_parent: &File,
    private_name: &OsStr,
    expected_generation: &str,
    expected_tree_hash: &str,
    after_move: impl FnOnce() -> Result<(), AcquisitionError>,
) -> Result<QuarantineOutcome, AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let source_meta = source_parent
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let private_meta = private_parent
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if source_meta.dev() != private_meta.dev() {
        return Err(AcquisitionError::Task(
            "custody parent must be on the source filesystem".to_owned(),
        ));
    }
    if !private_meta.is_dir() || private_meta.mode() & 0o077 != 0 {
        return Err(AcquisitionError::Task(
            "custody parent is not private".to_owned(),
        ));
    }
    let original_generation =
        source_generation_from_directory(original, std::path::Path::new(source_name))?;
    if original_generation != expected_generation {
        return Err(AcquisitionError::SourceChanged);
    }
    rename_child_noreplace(source_parent, source_name, private_parent, private_name)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    after_move()?;
    source_parent
        .sync_all()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    private_parent
        .sync_all()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let held = open_child_directory(private_parent, private_name)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let held_generation =
        source_generation_from_directory(&held, std::path::Path::new(private_name))?;
    let held_hash = tree_hash_from_pinned_directory(&held);
    if held_generation != original_generation
        || !matches!(held_hash, Ok(ref hash) if hash == expected_tree_hash)
    {
        return match rename_child_noreplace(
            private_parent,
            private_name,
            source_parent,
            source_name,
        ) {
            Ok(()) => {
                source_parent
                    .sync_all()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                private_parent
                    .sync_all()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                Ok(QuarantineOutcome::ReplacementRestored)
            }
            Err(_) => Ok(QuarantineOutcome::ReplacementPreserved),
        };
    }
    // An error here leaves the object in custody for intent recovery; it must
    // never authorize staging bytes from a detached source root.
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(QuarantineOutcome::Held)
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
mod tests {
    #![allow(clippy::expect_used)]
    #[cfg(target_os = "linux")]
    #[test]
    fn other_root_and_ancestor_remain_pinned_after_unlink() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let source = fixture.path().join("source");
        let private = fixture.path().join("private");
        let parent = fixture.path().join("other-parent");
        let other = parent.join("other");
        for dir in [&source, &private, &parent, &other] {
            fs::create_dir(dir).expect("owned directory");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("private mode");
        }
        let uid = rustix::process::geteuid().as_raw();
        let namespace =
            validate_legacy_lifecycle_namespace(&private, &source, if uid == 1 { 2 } else { 1 })
                .expect("namespace");
        let held = validate_legacy_other_roots(&namespace, std::slice::from_ref(&other))
            .expect("other root binding");
        fs::remove_dir(&other).expect("unlink owned root");
        fs::remove_dir(&parent).expect("unlink owned ancestor");
        assert_eq!(held[0].pins[0].metadata().expect("pinned root").nlink(), 0);
        assert_eq!(
            held[0].pins[1].metadata().expect("pinned ancestor").nlink(),
            0
        );
        for dir in [&parent, &other] {
            fs::create_dir(dir).expect("replacement directory");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("replacement mode");
        }
        let current =
            validate_legacy_other_roots(&namespace, &[other]).expect("replacement topology");
        assert!(!same_legacy_topology(&held, &current).expect("compare pinned identities"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_topology_revalidates_128_locks_without_second_fd_set() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let source = fixture.path().join("source");
        let private = fixture.path().join("private");
        for directory in [&source, &private] {
            fs::create_dir(directory).expect("owned directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("private mode");
        }
        let uid = rustix::process::geteuid().as_raw();
        let namespace =
            validate_legacy_lifecycle_namespace(&private, &source, if uid == 1 { 2 } else { 1 })
                .expect("namespace");
        let root_generation =
            lifecycle_file_identity(&namespace.private).expect("lock root identity");
        let mut identities = Vec::new();
        let mut roots = vec![source.clone()];
        for number in 1..=128_u128 {
            let item = uuid::Uuid::from_u128(number);
            let lock = private.join(format!("work-{}.lock", item.simple()));
            fs::write(&lock, b"owned lock").expect("lock");
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
            identities.push(melodee_custody::LifecycleLockIdentity {
                work_item_id: item,
                lock_root_generation: root_generation.clone(),
                lock_generation: lifecycle_file_identity(&File::open(&lock).expect("lock fd"))
                    .expect("lock identity"),
            });
            let root = fixture.path().join(format!("data-{number}"));
            fs::create_dir(&root).expect("owned data root");
            roots.push(root);
        }
        let locks = melodee_custody::HeldLifecycleLocks::acquire(&private, &identities)
            .expect("128 sorted locks");
        let topology = validate_legacy_other_roots(&namespace, &roots).expect("shared topology");
        assert!(std::sync::Arc::ptr_eq(
            topology[0].pins.last().expect("root ancestor"),
            topology[1].pins.last().expect("same root ancestor")
        ));
        let fd_count = || {
            fs::read_dir("/proc/self/fd")
                .expect("proc fd directory")
                .count()
        };
        let baseline = fd_count();
        let mut peak = baseline;
        revalidate_legacy_topology_with(&namespace, &topology, || {
            peak = peak.max(fd_count());
            Ok(())
        })
        .expect("bounded topology revalidation");
        assert!(
            peak <= baseline + 3,
            "revalidation must not open another full topology: baseline={baseline}, peak={peak}"
        );
        locks.revalidate().expect("all 128 locks remain held");
        assert!(melodee_custody::HeldLifecycleLocks::acquire(&private, &identities).is_err());
    }
    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[test]
    fn provisional_intent_hash_parser_requires_exact_lowercase_digest() {
        assert_eq!(
            super::exact_lower_hex_hash(&"ab".repeat(32)).expect("valid digest"),
            [0xab; 32]
        );
        assert!(super::exact_lower_hex_hash(&"AB".repeat(32)).is_err());
        assert!(super::exact_lower_hex_hash(&"ab".repeat(31)).is_err());
        assert!(super::exact_lower_hex_hash(&"ag".repeat(32)).is_err());
    }
    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[test]
    fn provisional_quarantine_name_decoder_preserves_native_bytes_and_rejects_traversal() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            super::provisional_decode_quarantine_component("2e71756172616e74696e652dff")
                .expect("native component")
                .as_os_str()
                .as_bytes(),
            b".quarantine-\xff"
        );
        for invalid in ["", "2F", "2f", "2e", "2e2e", "610062", "612f62", "6", "gg"] {
            assert!(
                super::provisional_decode_quarantine_component(invalid).is_err(),
                "invalid encoding must fail"
            );
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn admin_scope_pages_reject_replays_and_unbounded_input() {
        fn identity(number: u128) -> melodee_persistence::AdminReservedLifecycle {
            let work_item_id = uuid::Uuid::from_u128(number);
            melodee_persistence::AdminReservedLifecycle {
                work_item_id,
                source_library_id: None,
                source_path: "source".into(),
                source_path_format: 1,
                lock_name: format!("work-{}.lock", work_item_id.simple()),
                lock_root_generation: "root".into(),
                lock_generation: "inode".into(),
                stage_root_generation: "stage".into(),
            }
        }
        let mut accepted = Vec::new();
        super::extend_admin_lifecycle_page(&mut accepted, (1..=16).map(identity).collect())
            .expect("ordered first page");
        assert!(super::extend_admin_lifecycle_page(&mut accepted, vec![identity(16)]).is_err());
        assert!(
            super::extend_admin_lifecycle_page(&mut accepted, vec![identity(18), identity(17)])
                .is_err()
        );
        assert!(
            super::extend_admin_lifecycle_page(&mut accepted, (17..=33).map(identity).collect())
                .is_err()
        );
        for start in (17..=113).step_by(16) {
            super::extend_admin_lifecycle_page(
                &mut accepted,
                (start..start + 16).map(identity).collect(),
            )
            .expect("bounded ordered page");
        }
        assert_eq!(accepted.len(), 128);
        assert!(super::extend_admin_lifecycle_page(&mut accepted, vec![identity(129)]).is_err());
        assert_eq!(accepted.len(), 128);
    }
    use super::*;
    use crate::config::InboundSource;
    use crate::secure_fs::open_directory;
    use std::{fs, path::Path};

    #[cfg(target_os = "linux")]
    fn protected_probe_executable(
        configured: Option<std::ffi::OsString>,
    ) -> std::io::Result<std::path::PathBuf> {
        match configured {
            None => std::env::current_exe(),
            Some(path) if path == "/test-binary" => Ok(path.into()),
            Some(_) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid protected lock test executable",
            )),
        }
    }

    #[cfg(target_os = "linux")]
    fn run_protected_lock_probe(lock_path: &std::path::Path, expected: &str) {
        // The isolated runner bind-mounts the verified test binary at this
        // fixed path; its inherited stdin is already open before chroot.
        let executable = protected_probe_executable(std::env::var_os("MELODEE_TEST_EXECUTABLE"))
            .expect("test executable");
        let output = std::process::Command::new(executable)
            .args([
                "--exact",
                "custody::tests::protected_attempt_lock_child_probe",
            ])
            .env("MELODEE_TEST_PROTECTED_LOCK_PATH", lock_path)
            .env("MELODEE_TEST_PROTECTED_LOCK_EXPECTED", expected)
            .stdin(std::process::Stdio::inherit())
            .output()
            .expect("child probe");
        assert!(output.status.success(), "cross-process lock probe failed");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn protected_lock_probe_accepts_only_fixed_namespace_executable() {
        assert_eq!(
            protected_probe_executable(Some("/test-binary".into())).expect("fixed path"),
            Path::new("/test-binary")
        );
        for path in ["", "/bin/true", "/test-binary/../bin/true", "test-binary"] {
            assert!(protected_probe_executable(Some(path.into())).is_err());
        }
        assert_eq!(
            protected_probe_executable(None).expect("current executable"),
            std::env::current_exe().expect("current executable")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn protected_attempt_lock_child_probe() {
        let Some(path) = std::env::var_os("MELODEE_TEST_PROTECTED_LOCK_PATH") else {
            return;
        };
        let expected =
            std::env::var_os("MELODEE_TEST_PROTECTED_LOCK_EXPECTED").expect("expected state");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("lock file");
        let acquired =
            rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_ok();
        assert_eq!(acquired, expected == "acquired");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn retained_directory_pages_resume_without_losing_native_names_on_a_stable_directory() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("temp");
        for index in 0..130 {
            fs::write(temp.path().join(format!("file-{index:03}")), b"x").expect("file");
        }
        let invalid = std::ffi::OsString::from_vec(b"bad-\xff".to_vec());
        fs::write(temp.path().join(&invalid), b"invalid-name").expect("native name");
        symlink("file-000", temp.path().join("alias")).expect("alias");
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())
            .expect("root");
        let mut cookie = None;
        let mut found = std::collections::HashMap::new();
        loop {
            let (page, continuation, _) =
                read_retained_directory_page(&root, "", cookie, || false, |_| {})
                    .expect("directory page");
            assert!(page.len() <= 64);
            for (path, kind) in page {
                assert!(
                    found.insert(path, kind).is_none(),
                    "stable entries appear once"
                );
            }
            assert_ne!(
                continuation,
                RetainedDirectoryContinuation::Invalidated,
                "stable directory invalidated"
            );
            if let RetainedDirectoryContinuation::More(next) = continuation {
                cookie = Some(next);
            } else {
                break;
            }
        }
        assert_eq!(found.len(), 132);
        assert_eq!(
            found.get("alias"),
            Some(&None),
            "symlinks request verification"
        );
        let invalid_key = crate::source::candidate_relative_path(Path::new(&invalid));
        assert_eq!(found.get(&invalid_key), Some(&Some(false)));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn retained_directory_mutation_invalidates_the_page_cookie() {
        let temp = tempfile::tempdir().expect("temp");
        for index in 0..100 {
            fs::write(temp.path().join(format!("file-{index:03}")), b"x").expect("file");
        }
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())
            .expect("root");
        let mutated = std::cell::Cell::new(false);
        let (_, continuation, _) = read_retained_directory_page(
            &root,
            "",
            None,
            || false,
            |_| {
                if !mutated.replace(true) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    fs::write(temp.path().join("new"), b"new bytes").expect("new file");
                }
            },
        )
        .expect("page");
        assert_eq!(
            continuation,
            RetainedDirectoryContinuation::Invalidated,
            "changed directory requests verification and restarts its hint"
        );
        assert_eq!(
            fs::read(temp.path().join("new")).expect("new file retained"),
            b"new bytes"
        );
    }

    #[test]
    fn retained_baseline_page_detects_changed_reread_chunks_without_claiming_whole_tree() {
        let temp = tempfile::tempdir().expect("temp");
        let mut bytes = vec![0x5a; 2 * 64 * 1024 + 17];
        fs::write(temp.path().join("track"), &bytes).expect("file");
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())
            .expect("root");
        let chunks = vec![melodee_persistence::RetainedBaselineChunk {
            index: 1,
            length: 64 * 1024,
            digest: crate::legacy_custody_verify::staging_chunk_digest(
                1,
                &bytes[64 * 1024..2 * 64 * 1024],
            ),
        }];
        assert!(
            !read_retained_baseline_page(&root, "track", bytes.len() as i64, &chunks, 1024, || {
                false
            })
            .expect("matching page")
        );
        bytes[0] ^= 1;
        fs::write(temp.path().join("track"), &bytes).expect("other chunk changed");
        assert!(
            !read_retained_baseline_page(&root, "track", bytes.len() as i64, &chunks, 1024, || {
                false
            })
            .expect("unvisited chunk"),
            "a matching page does not imply a matching tree"
        );
        bytes[64 * 1024] ^= 1;
        fs::write(temp.path().join("track"), &bytes).expect("selected chunk changed");
        assert!(
            read_retained_baseline_page(&root, "track", bytes.len() as i64, &chunks, 1024, || {
                false
            })
            .expect("changed page")
        );
    }

    #[test]
    fn retained_baseline_page_does_not_follow_a_replaced_symlink() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("temp");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("track"), b"outside").expect("outside bytes");
        symlink(outside.path().join("track"), temp.path().join("track")).expect("symlink");
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())
            .expect("root");
        let chunks = vec![melodee_persistence::RetainedBaselineChunk {
            index: 0,
            length: 7,
            digest: crate::legacy_custody_verify::staging_chunk_digest(0, b"outside"),
        }];
        assert!(read_retained_baseline_page(&root, "track", 7, &chunks, 1024, || false).is_err());
        assert_eq!(
            fs::read(outside.path().join("track")).expect("outside unchanged"),
            b"outside"
        );
    }

    #[test]
    fn retained_baseline_missing_parent_preserves_not_found_reason() {
        let temp = tempfile::tempdir().expect("temp");
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())
            .expect("root");
        let error = read_retained_baseline_page(&root, "missing/track", 0, &[], 1024, || false)
            .expect_err("missing parent");
        assert!(
            matches!(error, AcquisitionError::Filesystem { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn byte_identical_replacement_is_restored_or_preserved_without_deletion() {
        use std::os::unix::fs::PermissionsExt;
        for block_restore in [false, true] {
            let temp = tempfile::tempdir().expect("temp");
            let inbound = temp.path().join("inbound");
            let private = temp.path().join("private");
            fs::create_dir_all(inbound.join("candidate")).expect("original");
            fs::create_dir(&private).expect("private");
            fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private mode");
            fs::write(inbound.join("candidate/file"), b"identical").expect("file");
            let guarded_root = GuardedRoot::open(
                InboundSource::from_root(inbound.clone(), None, Some("test".into()))
                    .expect("source"),
            )
            .expect("root");
            let source_parent = open_directory(&inbound).expect("source parent");
            let private_parent = open_directory(&private).expect("private parent");
            let original = open_child_directory(&source_parent, OsStr::new("candidate"))
                .expect("original handle");
            let generation = source_generation_from_directory(&original, Path::new("candidate"))
                .expect("generation");
            let hash = tree_hash_from_pinned_directory(&original).expect("hash");
            fs::rename(inbound.join("candidate"), inbound.join("parked-original"))
                .expect("park original");
            fs::create_dir(inbound.join("candidate")).expect("replacement");
            fs::write(inbound.join("candidate/file"), b"identical").expect("replacement bytes");
            let outcome = quarantine_verified_occurrence_with_hook(
                &guarded_root,
                &source_parent,
                OsStr::new("candidate"),
                &original,
                &private_parent,
                OsStr::new("held"),
                &generation,
                &hash,
                || {
                    if block_restore {
                        fs::create_dir(inbound.join("candidate")).expect("occupy original name");
                    }
                    Ok(())
                },
            )
            .expect("quarantine attempt");
            let replacement = if block_restore {
                assert_eq!(outcome, QuarantineOutcome::ReplacementPreserved);
                private.join("held/file")
            } else {
                assert_eq!(outcome, QuarantineOutcome::ReplacementRestored);
                inbound.join("candidate/file")
            };
            assert_eq!(
                fs::read(replacement).expect("replacement retained"),
                b"identical"
            );
            assert_eq!(
                fs::read(inbound.join("parked-original/file")).expect("original retained"),
                b"identical"
            );
        }
    }

    #[test]
    fn non_private_custody_parent_is_rejected_before_move() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let inbound = temp.path().join("inbound");
        let private = temp.path().join("not-private");
        fs::create_dir_all(inbound.join("candidate")).expect("candidate");
        fs::write(inbound.join("candidate/file"), b"keep").expect("file");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).expect("permissions");
        let service_uid = rustix::process::geteuid().as_raw();
        let other_uid = if service_uid == 1 { 2 } else { 1 };
        assert!(validate_custody_namespace(&private, &inbound, other_uid).is_err());
        assert_eq!(
            fs::read(inbound.join("candidate/file")).expect("source preserved"),
            b"keep"
        );
        assert!(!private.join("held").exists());
    }

    #[test]
    fn source_root_replacement_after_move_retains_custody_and_rejects_handoff() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let inbound = temp.path().join("inbound");
        let old_root = temp.path().join("old-root");
        let private = temp.path().join("private");
        fs::create_dir_all(inbound.join("candidate")).expect("candidate");
        fs::write(inbound.join("candidate/file"), b"original").expect("file");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let guarded_root = GuardedRoot::open(
            InboundSource::from_root(inbound.clone(), None, Some("test".into())).expect("source"),
        )
        .expect("root");
        let source_parent = open_directory(&inbound).expect("source parent");
        let private_parent = open_directory(&private).expect("private parent");
        let original = open_child_directory(&source_parent, OsStr::new("candidate"))
            .expect("candidate handle");
        let generation = source_generation_from_directory(&original, Path::new("candidate"))
            .expect("generation");
        let hash = tree_hash_from_pinned_directory(&original).expect("hash");
        let result = quarantine_verified_occurrence_with_hook(
            &guarded_root,
            &source_parent,
            OsStr::new("candidate"),
            &original,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
            || {
                fs::rename(&inbound, &old_root).expect("replace pinned root");
                fs::create_dir_all(inbound.join("candidate")).expect("replacement");
                fs::write(inbound.join("candidate/file"), b"replacement").expect("new file");
                Ok(())
            },
        );
        assert!(result.is_err(), "detached root cannot authorize handoff");
        assert_eq!(
            fs::read(private.join("held/file")).expect("held"),
            b"original"
        );
        assert_eq!(
            fs::read(inbound.join("candidate/file")).expect("replacement"),
            b"replacement"
        );
    }

    #[test]
    fn custody_namespace_rejects_same_identity_overlaps_and_writable_ancestors() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let untrusted = temp.path().join("writable");
        let private = untrusted.join("private");
        fs::create_dir(&source).expect("source");
        fs::create_dir_all(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        fs::set_permissions(&untrusted, fs::Permissions::from_mode(0o777)).expect("mode");
        let service_uid = rustix::process::geteuid().as_raw();
        let other_uid = if service_uid == 1 { 2 } else { 1 };
        assert!(validate_custody_namespace(&private, &source, service_uid).is_err());
        assert!(validate_custody_namespace(&source, &source, other_uid).is_err());
        assert!(validate_custody_namespace(&private, &source, other_uid).is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn protected_service_owned_custody_namespace_is_accepted() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let service_uid = rustix::process::geteuid().as_raw();
        let other_uid = if service_uid == 1 { 2 } else { 1 };
        let candidates = [
            std::env::var_os("XDG_RUNTIME_DIR").map(Into::into),
            std::env::current_dir().ok(),
            std::env::var_os("HOME").map(Into::into),
        ];
        let base = candidates.into_iter().flatten().find(|path| {
            path.is_absolute()
                && path.ancestors().all(|ancestor| {
                    crate::secure_fs::open_directory(ancestor).is_ok_and(|directory| {
                        verify_acl_free(&directory).is_ok()
                            && directory.metadata().is_ok_and(|metadata| {
                                (metadata.uid() == service_uid || metadata.uid() == 0)
                                    && metadata.mode() & 0o022 == 0
                            })
                    })
                })
        });
        let Some(base) = base else {
            eprintln!("no protected writable test parent available");
            return;
        };
        let temp = tempfile::tempdir_in(base).expect("protected temp");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        fs::create_dir(&source).expect("source");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let validated = std::sync::Arc::new(
            validate_custody_namespace(&private, &source, other_uid).expect("protected namespace"),
        );
        let actual = validated.private.metadata().expect("validated metadata");
        let expected = fs::metadata(&private).expect("path metadata");
        assert_eq!(
            (actual.dev(), actual.ino()),
            (expected.dev(), expected.ino())
        );
        #[cfg(target_os = "linux")]
        {
            fs::create_dir_all(source.join("candidate/disc")).expect("candidate directory");
            fs::write(source.join("candidate/disc/track"), b"producer bytes")
                .expect("source bytes");
            let root = GuardedRoot::open(
                InboundSource::from_root(source.clone(), None, Some("test".into()))
                    .expect("inbound"),
            )
            .expect("guarded root");
            let legacy_id = uuid::Uuid::now_v7();
            let legacy_name: std::ffi::OsString =
                format!("work-{}.lock", legacy_id.simple()).into();
            let provisioned = LegacyLifecycleCapability::create_reserved(
                validated.clone(),
                root.clone(),
                legacy_id,
            )
            .expect("provisioned legacy lock fixture");
            let (legacy_root, legacy_generation) =
                provisioned.generations().expect("legacy lock identity");
            assert!(
                LegacyLifecycleCapability::create_reserved(
                    validated.clone(),
                    root.clone(),
                    legacy_id,
                )
                .is_err(),
                "an existing reserved inode is never adopted"
            );
            let interrupted_id = uuid::Uuid::now_v7();
            let interrupted_path = private.join(format!("work-{}.lock", interrupted_id.simple()));
            fs::write(&interrupted_path, b"ambiguous").expect("interrupted lock fixture");
            assert!(
                LegacyLifecycleCapability::create_reserved(
                    validated.clone(),
                    root.clone(),
                    interrupted_id,
                )
                .is_err(),
                "an interrupted pre-acknowledgement lock is quarantined"
            );
            assert_eq!(
                fs::read(&interrupted_path).expect("ambiguous lock retained"),
                b"ambiguous"
            );
            drop(provisioned);
            assert!(
                LegacyLifecycleCapability::acquire(
                    validated.clone(),
                    root.clone(),
                    legacy_id,
                    "unix:0:0".to_owned(),
                    legacy_generation.clone(),
                )
                .is_err(),
                "a mismatched lock root cannot authorize work"
            );
            assert!(
                LegacyLifecycleCapability::acquire(
                    validated.clone(),
                    root.clone(),
                    legacy_id,
                    legacy_root.clone(),
                    "unix:0:0".to_owned(),
                )
                .is_err(),
                "a mismatched lock inode cannot authorize work"
            );
            let capability = LegacyLifecycleCapability::acquire(
                validated.clone(),
                root.clone(),
                legacy_id,
                legacy_root.clone(),
                legacy_generation.clone(),
            )
            .expect("legacy lifecycle capability");
            assert!(
                capability
                    .bind_other_roots(std::slice::from_ref(&private))
                    .is_err(),
                "a configured data root cannot overlap the lock root"
            );
            assert!(
                capability
                    .bind_other_roots(&[private.join("nested")])
                    .is_err(),
                "a nested data root cannot contain or alias the lock namespace"
            );
            assert!(
                capability
                    .bind_other_roots(&[private.join("missing/../nested")])
                    .is_err(),
                "parent traversal must be rejected before opening roots"
            );
            let symlink = temp.path().join("private-alias");
            std::os::unix::fs::symlink(&private, &symlink).expect("symlink fixture");
            assert!(
                capability.bind_other_roots(&[symlink]).is_err(),
                "symlink roots fail closed"
            );
            assert!(
                capability
                    .bind_other_roots(&[source.clone(), source.clone()])
                    .is_err(),
                "duplicate data roots fail closed"
            );
            assert!(
                capability
                    .bind_other_roots(&[source.clone(), source.join("candidate")])
                    .is_err(),
                "nested data roots fail closed"
            );
            let other = temp.path().join("other-root");
            let parked_other = temp.path().join("parked-other-root");
            fs::create_dir(&other).expect("other root");
            capability
                .bind_other_roots(&[source.clone(), other.clone()])
                .expect("distinct roots");
            fs::rename(&other, &parked_other).expect("park other root");
            fs::create_dir(&other).expect("replacement other root");
            assert!(
                capability.revalidate().is_err(),
                "root replacement must invalidate the capability"
            );
            fs::remove_dir(&other).expect("remove owned replacement");
            fs::rename(&parked_other, &other).expect("restore owned root");
            capability.revalidate().expect("original roots restored");
            let lifetime = capability.lifetime_hold();
            drop(capability);
            run_protected_lock_probe(&private.join(&legacy_name), "busy");
            assert!(
                LegacyLifecycleCapability::acquire(
                    validated.clone(),
                    root.clone(),
                    legacy_id,
                    legacy_root.clone(),
                    legacy_generation.clone(),
                )
                .is_err(),
                "lifetime-only hold excludes another operation"
            );
            drop(lifetime);
            run_protected_lock_probe(&private.join(&legacy_name), "acquired");
            let capability = LegacyLifecycleCapability::acquire(
                validated.clone(),
                root.clone(),
                legacy_id,
                legacy_root.clone(),
                legacy_generation.clone(),
            )
            .expect("legacy lock released");
            fs::rename(
                private.join(&legacy_name),
                private.join("parked-legacy-lock"),
            )
            .expect("park legacy lock");
            fs::write(private.join(&legacy_name), b"replacement").expect("replacement lock");
            assert!(capability.revalidate().is_err());
            assert!(private.join("parked-legacy-lock").exists());
            assert_eq!(
                fs::read(private.join(&legacy_name)).expect("replacement retained"),
                b"replacement"
            );
            drop(capability);
            let lock_id = uuid::Uuid::now_v7();
            let lock = ProtectedAttemptLock::create(validated.clone(), root.clone(), lock_id)
                .expect("exclusive attempt lock");
            let lock_path = private.join(&lock.name);
            run_protected_lock_probe(&lock_path, "busy");
            let lock_generation = lock.generation.clone();
            drop(lock);
            run_protected_lock_probe(&lock_path, "acquired");
            let lock = ProtectedAttemptLock::acquire(
                validated.clone(),
                root.clone(),
                lock_id,
                &lock_generation,
            )
            .expect("reacquire original lock");
            fs::rename(&lock_path, private.join("preserved-lock")).expect("preserve old lock");
            fs::write(&lock_path, b"replacement").expect("replacement lock");
            assert!(
                lock.verify().is_err(),
                "a replaced lock path cannot authorize cleanup"
            );
            assert!(private.join("preserved-lock").exists());
            assert_eq!(
                fs::read(&lock_path).expect("replacement retained"),
                b"replacement"
            );
            drop(lock);
            let baseline_id = uuid::Uuid::now_v7();
            let lifecycle =
                ProtectedFinalStageLock::create(validated.clone(), root.clone(), baseline_id)
                    .expect("create final-stage lifecycle lock");
            let lifecycle_path = private.join(&lifecycle.0.name);
            let lifecycle_generation = lifecycle.0.generation.clone();
            run_protected_lock_probe(&lifecycle_path, "busy");
            assert!(
                ProtectedFinalStageLock::acquire(
                    validated.clone(),
                    root.clone(),
                    baseline_id,
                    &lifecycle_generation,
                )
                .is_err()
            );
            drop(lifecycle);
            run_protected_lock_probe(&lifecycle_path, "acquired");
            let lifecycle = ProtectedFinalStageLock::acquire(
                validated.clone(),
                root.clone(),
                baseline_id,
                &lifecycle_generation,
            )
            .expect("reacquire final-stage lifecycle lock");
            fs::rename(&lifecycle_path, private.join("preserved-stage-lock"))
                .expect("preserve original lifecycle lock");
            fs::write(&lifecycle_path, b"replacement").expect("replacement lifecycle lock");
            assert!(lifecycle.0.verify().is_err());
            assert!(private.join("preserved-stage-lock").exists());
            assert_eq!(
                fs::read(&lifecycle_path).expect("replacement retained"),
                b"replacement"
            );
            drop(lifecycle);
            let attempt_id = uuid::Uuid::now_v7();
            let attempt_lock =
                ProtectedAttemptLock::create(validated.clone(), root.clone(), attempt_id)
                    .expect("attempt lock");
            let attempt_lock_generation = attempt_lock.generation.clone();
            let attempt =
                ProtectedStageAttempt::create(attempt_lock, attempt_id).expect("exclusive attempt");
            assert!(
                ProtectedAttemptLock::acquire(
                    validated.clone(),
                    root.clone(),
                    attempt_id,
                    &attempt_lock_generation
                )
                .is_err(),
                "a retry cannot enter an actively locked attempt"
            );
            let unacknowledged_id = uuid::Uuid::now_v7();
            let unacknowledged = private.join(format!("protected-{}", unacknowledged_id.simple()));
            fs::create_dir(&unacknowledged).expect("interrupted namespace");
            fs::set_permissions(&unacknowledged, fs::Permissions::from_mode(0o700))
                .expect("private mode");
            let unacknowledged_lock =
                ProtectedAttemptLock::create(validated.clone(), root.clone(), unacknowledged_id)
                    .expect("unacknowledged lock");
            assert!(
                ProtectedStageAttempt::reopen(
                    unacknowledged_lock,
                    unacknowledged_id,
                    "unacknowledged".to_owned(),
                    "unacknowledged".to_owned()
                )
                .is_err()
            );
            assert!(
                unacknowledged.exists(),
                "an unacknowledged namespace must be preserved"
            );
            let candidate = root
                .resolve(OsStr::new("candidate"))
                .expect("resolve")
                .expect("candidate");
            let source_dir = root.cap_directory(&candidate).expect("pinned candidate");
            let mut observations = Vec::new();
            let mut sealed_files = Vec::new();
            let copied = stream_protected_stage_copy(
                &source_dir,
                &attempt,
                || false,
                |event| {
                    match event {
                        ProtectedCopyEvent::Observation(observation) => {
                            observations.push(observation)
                        }
                        ProtectedCopyEvent::SealedFile {
                            path_key,
                            digest,
                            device,
                            inode,
                        } => sealed_files.push((path_key, digest, device, inode)),
                    }
                    Ok(())
                },
            );
            match copied {
                Ok(count) => {
                    eprintln!("protected tree copy sealed all file contents");
                    assert_eq!(count, 2);
                    assert_eq!(sealed_files.len(), 1);
                    assert_eq!(sealed_files[0].0, "disc/track");
                    assert_eq!(observations.len(), 2);
                    assert_eq!(
                        fs::read(private.join(&attempt.relative).join("source/disc/track"))
                            .expect("stage bytes"),
                        b"producer bytes"
                    );
                }
                Err(error) => {
                    assert!(
                        error.to_string().contains("does not support fs-verity"),
                        "unexpected protected copy failure: {error}"
                    );
                    assert!(
                        sealed_files.is_empty(),
                        "unsupported filesystem must not mint sealed evidence"
                    );
                    eprintln!(
                        "protected tree copy retained source on unsupported fs-verity filesystem"
                    );
                }
            }
            assert_eq!(
                fs::read(source.join("candidate/disc/track")).expect("source retained"),
                b"producer bytes"
            );
            let stage = open_child_directory(
                attempt.parent.checked_file().expect("attempt parent"),
                OsStr::new("source"),
            )
            .expect("provisional stage");
            let stage_generation = ProtectedStageAttempt::identity(&stage).expect("stage identity");
            let stage_lock = ProtectedFinalStageLock::create(
                validated.clone(),
                root.clone(),
                uuid::Uuid::now_v7(),
            )
            .expect("stage lifecycle lock");
            assert_eq!(
                inspect_protected_promotion(&attempt, &stage_generation).expect("before promotion"),
                ProtectedPromotionLocation::Attempt
            );
            let interrupted =
                promote_protected_stage_with_hooks(&attempt, &stage_generation, |step| {
                    if step == ProtectedPromotionStep::AfterRename {
                        Err(AcquisitionError::Task("injected after rename".to_owned()))
                    } else {
                        Ok(())
                    }
                });
            assert!(interrupted.is_err());
            assert_eq!(
                inspect_protected_promotion(&attempt, &stage_generation)
                    .expect("after interruption"),
                ProtectedPromotionLocation::Final
            );
            let mut directory_hash = blake3::Hasher::new();
            directory_hash.update(b"melodee-custody-entry-v3\0unix\0");
            crate::acquisition::hash_custody_path(&mut directory_hash, Path::new("disc"), true, 0)
                .expect("directory digest");
            let directory_entry = melodee_persistence::ProtectedBaselineDirectoryEntry {
                ordinal: 1,
                path_key: "disc".to_owned(),
                parent_key: String::new(),
                stage_digest: directory_hash.finalize().to_hex().to_string(),
            };
            verify_cleaned_protected_stage_directory(
                &stage_lock,
                &stage_generation,
                &directory_entry,
            )
            .expect("verify protected directory");
            let mut forged_directory = directory_entry.clone();
            forged_directory.parent_key = "different".to_owned();
            assert!(
                verify_cleaned_protected_stage_directory(
                    &stage_lock,
                    &stage_generation,
                    &forged_directory,
                )
                .is_err()
            );
            if let Some((path, digest, device, inode)) = sealed_files.first() {
                let proof = melodee_persistence::RetainedFileVerity {
                    digest: *digest,
                    device: *device,
                    inode: *inode,
                };
                verify_protected_stage_file(&attempt, &stage_lock, &stage_generation, path, &proof)
                    .expect("remeasure sealed promoted file");
                verify_cleaned_protected_stage_file(&stage_lock, &stage_generation, path, &proof)
                    .expect("remeasure without attempt identity");
                let mut forged = proof.clone();
                forged.digest[0] ^= 1;
                assert!(
                    verify_protected_stage_file(
                        &attempt,
                        &stage_lock,
                        &stage_generation,
                        path,
                        &forged
                    )
                    .is_err()
                );
                let live_lock_path = private.join(&stage_lock.0.name);
                fs::rename(&live_lock_path, private.join("parked-live-stage-lock"))
                    .expect("park stage lock");
                fs::write(&live_lock_path, b"replacement").expect("replace stage lock");
                assert!(
                    verify_protected_stage_file(
                        &attempt,
                        &stage_lock,
                        &stage_generation,
                        path,
                        &proof
                    )
                    .is_err(),
                    "a replaced lifecycle lock cannot authorize remeasurement"
                );
                assert!(
                    verify_cleaned_protected_stage_file(
                        &stage_lock,
                        &stage_generation,
                        path,
                        &proof
                    )
                    .is_err(),
                    "a replaced lifecycle lock cannot authorize cleaned-stage remeasurement"
                );
                assert!(private.join("parked-live-stage-lock").exists());
            }
            assert_eq!(
                fs::read(private.join("source/disc/track")).expect("moved stage retained"),
                b"producer bytes"
            );
            assert_eq!(
                fs::read(source.join("candidate/disc/track")).expect("source retained"),
                b"producer bytes"
            );
            let private2 = temp.path().join("private2");
            fs::create_dir(&private2).expect("second private root");
            fs::set_permissions(&private2, fs::Permissions::from_mode(0o700))
                .expect("private mode");
            let second_namespace = std::sync::Arc::new(
                validate_custody_namespace(&private2, &source, other_uid)
                    .expect("second namespace"),
            );
            let second_id = uuid::Uuid::now_v7();
            let second_lock =
                ProtectedAttemptLock::create(second_namespace, root.clone(), second_id)
                    .expect("second lock");
            let second =
                ProtectedStageAttempt::create(second_lock, second_id).expect("second attempt");
            let second_dir = private2.join(&second.relative);
            fs::create_dir(second_dir.join("source")).expect("original stage");
            fs::write(second_dir.join("source/track"), b"original stage").expect("stage bytes");
            let original = open_child_directory(
                second.parent.checked_file().expect("parent"),
                OsStr::new("source"),
            )
            .expect("original stage handle");
            let original_generation =
                ProtectedStageAttempt::identity(&original).expect("original generation");
            let replaced =
                promote_protected_stage_with_hooks(&second, &original_generation, |step| {
                    if step == ProtectedPromotionStep::BeforeRename {
                        fs::rename(second_dir.join("source"), second_dir.join("parked"))
                            .expect("park original");
                        fs::create_dir(second_dir.join("source")).expect("replacement stage");
                        fs::write(second_dir.join("source/track"), b"replacement stage")
                            .expect("replacement bytes");
                    }
                    Ok(())
                });
            assert!(
                replaced.is_err(),
                "a replacement moved by rename cannot be accepted"
            );
            assert_eq!(
                fs::read(second_dir.join("parked/track")).expect("original preserved"),
                b"original stage"
            );
            assert_eq!(
                fs::read(private2.join("source/track")).expect("replacement preserved"),
                b"replacement stage"
            );
            for (index, fault) in [
                ProtectedPromotionStep::AfterAttemptSync,
                ProtectedPromotionStep::AfterFinalSync,
            ]
            .into_iter()
            .enumerate()
            {
                let private_sync = temp.path().join(format!("private-sync-{index}"));
                fs::create_dir(&private_sync).expect("private root");
                fs::set_permissions(&private_sync, fs::Permissions::from_mode(0o700))
                    .expect("private mode");
                let sync_namespace = std::sync::Arc::new(
                    validate_custody_namespace(&private_sync, &source, other_uid)
                        .expect("namespace"),
                );
                let sync_id = uuid::Uuid::now_v7();
                let sync_lock = ProtectedAttemptLock::create(sync_namespace, root.clone(), sync_id)
                    .expect("sync lock");
                let sync_attempt =
                    ProtectedStageAttempt::create(sync_lock, sync_id).expect("attempt");
                let sync_dir = private_sync.join(&sync_attempt.relative);
                fs::create_dir(sync_dir.join("source")).expect("stage");
                fs::write(sync_dir.join("source/track"), b"retained stage").expect("stage bytes");
                let stage = open_child_directory(
                    sync_attempt.parent.checked_file().expect("parent"),
                    OsStr::new("source"),
                )
                .expect("stage handle");
                let generation = ProtectedStageAttempt::identity(&stage).expect("generation");
                let before =
                    promote_protected_stage_with_hooks(&sync_attempt, &generation, |step| {
                        if step == ProtectedPromotionStep::BeforeRename {
                            Err(AcquisitionError::Task("injected before rename".to_owned()))
                        } else {
                            Ok(())
                        }
                    });
                assert!(before.is_err());
                assert_eq!(
                    inspect_protected_promotion(&sync_attempt, &generation)
                        .expect("pre-rename hint"),
                    ProtectedPromotionLocation::Attempt
                );
                let result =
                    promote_protected_stage_with_hooks(&sync_attempt, &generation, |step| {
                        if step == fault {
                            Err(AcquisitionError::Task("injected after sync".to_owned()))
                        } else {
                            Ok(())
                        }
                    });
                assert!(result.is_err());
                assert_eq!(
                    inspect_protected_promotion(&sync_attempt, &generation).expect("replay hint"),
                    ProtectedPromotionLocation::Final
                );
                assert_eq!(
                    fs::read(private_sync.join("source/track")).expect("stage retained"),
                    b"retained stage"
                );
                assert_eq!(
                    fs::read(source.join("candidate/disc/track")).expect("source retained"),
                    b"producer bytes"
                );
            }
            let namespace_generation = attempt.namespace_generation.clone();
            let marker_generation = attempt.marker_generation.clone();
            let attempt_dir = private.join(&attempt.relative);
            drop(attempt);
            let attempt_lock = ProtectedAttemptLock::acquire(
                validated.clone(),
                root.clone(),
                attempt_id,
                &attempt_lock_generation,
            )
            .expect("reacquire attempt lock");
            let attempt = ProtectedStageAttempt::reopen(
                attempt_lock,
                attempt_id,
                namespace_generation.clone(),
                marker_generation.clone(),
            )
            .expect("acknowledged attempt can be revalidated");
            fs::rename(
                attempt_dir.join(".melodee-attempt"),
                attempt_dir.join("saved-marker"),
            )
            .expect("preserve marker");
            fs::write(attempt_dir.join(".melodee-attempt"), attempt_id.to_string())
                .expect("replacement marker");
            assert!(
                attempt.verify().is_err(),
                "replacement marker cannot prove provenance"
            );
            drop(attempt);
            let attempt_lock = ProtectedAttemptLock::acquire(
                validated.clone(),
                root.clone(),
                attempt_id,
                &attempt_lock_generation,
            )
            .expect("reacquire attempt lock");
            assert!(
                ProtectedStageAttempt::reopen(
                    attempt_lock,
                    attempt_id,
                    namespace_generation,
                    marker_generation
                )
                .is_err()
            );
            assert!(attempt_dir.join("saved-marker").exists());
            let nested = private.join("nested");
            fs::create_dir(&nested).expect("nested");
            fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).expect("private mode");
            assert!(
                PrivateStageParent::open(validated.clone(), root.clone(), Path::new("../source"))
                    .is_err()
            );
            let parent = PrivateStageParent::open(validated.clone(), root, Path::new("nested"))
                .expect("private stage parent");
            parent.checked_file().expect("current private parent");
            fs::rename(&nested, private.join("original-nested")).expect("rename original");
            fs::create_dir(&nested).expect("replacement");
            fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).expect("private mode");
            assert!(
                parent.checked_file().is_err(),
                "replacement must not authorize a stage file"
            );
        }
        eprintln!("protected namespace positive assertion executed");

        // Linux POSIX ACL xattr format: version 2 followed by fixed-width
        // little-endian tag, permissions and ID entries. This named user has
        // no effective rights; even a benign nontrivial ACL must fail closed.
        // A private user namespace may map only the service UID. In that
        // case use its mapped UID for the named ACL entry; on the native host
        // retain the distinct-user case. Both are nontrivial ACLs.
        let uid_map = fs::read_to_string("/proc/self/uid_map").expect("read UID map");
        let other_is_mapped = uid_map.lines().any(|line| {
            let values: Vec<u64> = line
                .split_whitespace()
                .filter_map(|part| part.parse().ok())
                .collect();
            values.len() == 3
                && u64::from(other_uid) >= values[0]
                && u64::from(other_uid) < values[0].saturating_add(values[2])
        });
        let named_uid = if other_is_mapped {
            other_uid
        } else {
            service_uid
        };
        let mut acl = 2_u32.to_le_bytes().to_vec();
        for (tag, permission, id) in [
            (1_u16, 7_u16, u32::MAX),
            (2, 0, named_uid),
            (4, 0, u32::MAX),
            (16, 0, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend_from_slice(&tag.to_le_bytes());
            acl.extend_from_slice(&permission.to_le_bytes());
            acl.extend_from_slice(&id.to_le_bytes());
        }
        rustix::fs::fsetxattr(
            &validated.private,
            "system.posix_acl_access",
            &acl,
            rustix::fs::XattrFlags::empty(),
        )
        .expect("set test ACL");
        assert!(validate_custody_namespace(&private, &source, other_uid).is_err());
        eprintln!("named-user ACL rejection assertion executed (distinct user: {other_is_mapped})");
    }

    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn maintenance_provisions_replays_and_quarantines_ambiguous_locks() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("protected runtime parent");
        let temp = tempfile::tempdir_in(runtime).expect("owned fixture");
        let inbound = temp.path().join("inbound");
        let private = temp.path().join("private");
        let staging = temp.path().join("staging");
        let work = temp.path().join("work");
        let assets = temp.path().join("assets");
        let canonical = temp.path().join("canonical");
        for path in [&inbound, &private, &staging, &work, &assets, &canonical] {
            fs::create_dir(path).expect("fixture directory");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private mode");
        }
        fs::write(inbound.join("original"), b"producer bytes").expect("source bytes");
        fs::write(staging.join("existing"), b"stage bytes").expect("stage bytes");
        fs::write(assets.join("existing"), b"asset bytes").expect("asset bytes");
        fs::write(canonical.join("existing"), b"canonical bytes").expect("canonical bytes");
        let source = InboundSource::from_root(inbound.clone(), None, Some("fixture".into()))
            .expect("source");
        let mut config = crate::config::IngestionConfig::with_sources(vec![source]);
        let uid = rustix::process::geteuid().as_raw();
        config.protected_custody = Some(crate::config::ProtectedCustodyConfig {
            private_root: private.clone(),
            producer_uid: if uid == 1 { 2 } else { 1 },
        });
        config.staging_root = Some(staging.clone());
        config.work_area_base = work;
        config.storage_root = Some(canonical.clone());
        let db = crate::common::TestDb::create("lifecycle_fs_provision").await;
        let store = melodee_persistence::WorkItemStore;
        let now = time::OffsetDateTime::now_utc();
        let first = melodee_domain::ReleaseWorkItem::discovered(
            "first".to_owned(),
            None,
            uuid::Uuid::now_v7(),
            now,
        );
        let second = melodee_domain::ReleaseWorkItem::discovered(
            "second".to_owned(),
            None,
            uuid::Uuid::now_v7(),
            now,
        );
        let third = melodee_domain::ReleaseWorkItem::discovered(
            "third".to_owned(),
            None,
            uuid::Uuid::now_v7(),
            now,
        );
        store.create(&db.pool, &first).await.expect("first item");
        store.create(&db.pool, &second).await.expect("second item");
        store.create(&db.pool, &third).await.expect("third item");
        let mut ids = [first.id, second.id, third.id];
        ids.sort_unstable();
        let [ready_id, ambiguous_id, overlap_id] = ids;
        let ambiguous = private.join(format!("work-{}.lock", ambiguous_id.simple()));
        let fence = store
            .begin_lifecycle_maintenance(&db.pool, "owned-test", time::Duration::minutes(2))
            .await
            .expect("maintenance fence");
        // This owned fixture has no external writers; real operators must stop
        // and verify all writers before making the equivalent assertion.
        store
            .confirm_lifecycle_quiescence(
                &db.pool,
                &fence,
                &melodee_persistence::LifecycleQuiescenceAttestation {
                    reason: "owned test",
                    backup_reference: "owned test fixture",
                    stop_all_writers: true,
                    inspection_complete: true,
                },
            )
            .await
            .expect("quiescence");
        assert_eq!(
            provision_legacy_lifecycle_once_with_asset_root(
                &config,
                &store,
                &db.pool,
                &fence,
                Some(assets.clone())
            )
            .await
            .expect("first turn"),
            LegacyLifecycleProvisionOutcome::Ready(ready_id)
        );
        let reservation = store
            .begin_maintenance_admin_media_reservation(
                &db.pool,
                &[ready_id],
                &[],
                "owned-test",
                time::Duration::minutes(2),
                &fence,
            )
            .await
            .expect("reserve owned item");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("acquire complete scope");
        let holds = scope.lifetime_holds();
        assert_eq!(holds.len(), 1);
        assert!(
            acquire_provisional_admin_lifecycle_scope(
                &config,
                assets.clone(),
                &store,
                &db.pool,
                &reservation,
            )
            .await
            .is_err(),
            "a second operation cannot acquire the held lock"
        );
        drop(scope);
        assert!(
            acquire_provisional_admin_lifecycle_scope(
                &config,
                assets.clone(),
                &store,
                &db.pool,
                &reservation,
            )
            .await
            .is_err(),
            "a lifetime hold keeps the lock after its owner drops"
        );
        drop(holds);
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("reacquire after actual lifetime ends");
        scope.revalidate().expect("revalidate held scope");
        let displaced_stage = temp.path().join("displaced-stage");
        fs::rename(&staging, &displaced_stage).expect("displace stage root");
        fs::create_dir(&staging).expect("replace stage root");
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).expect("replacement mode");
        assert!(
            scope.revalidate().is_err(),
            "stage replacement must fail closed"
        );
        assert_eq!(
            fs::read(displaced_stage.join("existing")).expect("displaced bytes"),
            b"stage bytes"
        );
        fs::remove_dir(&staging).expect("remove owned empty replacement");
        fs::rename(&displaced_stage, &staging).expect("restore stage root");
        scope.revalidate().expect("restored exact stage root");
        let payload = b"canonical bytes";
        let scan = melodee_custody::IncrementalFileCheck::begin(
            scope,
            &canonical,
            melodee_custody::ExpectedFile {
                path: std::path::Path::new("existing"),
                size: payload.len() as u64,
                hash: melodee_custody::ExpectedHash::Blake3(*blake3::hash(payload).as_bytes()),
            },
        )
        .expect("create observational scan with entire scope");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = tokio::spawn(async move {
            crate::deletion::run_with_owned_operation_scope(scan, move |scan| {
                assert!(matches!(
                    scan.step(1, || Ok(()), || false).expect("scan first byte"),
                    melodee_custody::IncrementalFileProgress::More { .. }
                ));
                started_tx.send(()).expect("worker started");
                release_rx.recv().expect("release worker");
                assert_eq!(
                    scan.step(1024, || Ok(()), || false)
                        .expect("finish observational scan"),
                    melodee_custody::IncrementalFileProgress::Complete
                );
                Ok(())
            })
            .await
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker started");
        worker.abort();
        assert!(
            acquire_provisional_admin_lifecycle_scope(
                &config,
                assets.clone(),
                &store,
                &db.pool,
                &reservation,
            )
            .await
            .is_err(),
            "cancelled waiter cannot release a running worker's lock"
        );
        release_tx.send(()).expect("release owned worker");
        let scope = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(scope) = acquire_provisional_admin_lifecycle_scope(
                    &config,
                    assets.clone(),
                    &store,
                    &db.pool,
                    &reservation,
                )
                .await
                {
                    break scope;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker released lock after termination");
        drop(scope);
        store
            .finish_admin_media_reservation(&db.pool, &reservation)
            .await
            .expect("release owned reservation");
        assert!(
            acquire_provisional_admin_lifecycle_scope(
                &config,
                assets.clone(),
                &store,
                &db.pool,
                &reservation,
            )
            .await
            .is_err(),
            "a released reservation cannot acquire locks"
        );
        assert_eq!(
            store
                .next_lifecycle_provision_item(&db.pool, &fence)
                .await
                .expect("next item"),
            Some(ambiguous_id)
        );
        store
            .reserve_work_item_lifecycle(&db.pool, &fence, ambiguous_id)
            .await
            .expect("reserve interrupted item");
        let guarded_root = GuardedRoot::open(config.sources[0].clone()).expect("guarded root");
        let namespace = std::sync::Arc::new(
            validate_custody_namespace(&private, &inbound, if uid == 1 { 2 } else { 1 })
                .expect("private namespace"),
        );
        let interrupted = create_reserved_legacy_lifecycle(namespace, guarded_root, ambiguous_id)
            .await
            .expect("create before interrupted acknowledgement");
        let (_, interrupted_inode) = interrupted.generations().expect("interrupted identity");
        drop(interrupted);
        fs::write(&ambiguous, b"unacknowledged").expect("ambiguous fixture");
        assert_eq!(
            provision_legacy_lifecycle_once_with_asset_root(
                &config,
                &store,
                &db.pool,
                &fence,
                Some(assets.clone())
            )
            .await
            .expect("ambiguous turn"),
            LegacyLifecycleProvisionOutcome::Quarantined(ambiguous_id)
        );
        assert_eq!(
            fs::read(&ambiguous).expect("ambiguous retained"),
            b"unacknowledged"
        );
        assert_eq!(
            lifecycle_file_identity(&File::open(&ambiguous).expect("ambiguous inode"))
                .expect("identity retained"),
            interrupted_inode
        );
        assert_eq!(
            fs::read(inbound.join("original")).expect("inbound retained"),
            b"producer bytes"
        );
        assert_eq!(
            fs::read(staging.join("existing")).expect("stage retained"),
            b"stage bytes"
        );
        assert_eq!(
            fs::read(assets.join("existing")).expect("asset retained"),
            b"asset bytes"
        );
        assert_eq!(
            fs::read(canonical.join("existing")).expect("canonical retained"),
            b"canonical bytes"
        );
        let mut overlap_config = config.clone();
        overlap_config
            .protected_custody
            .as_mut()
            .expect("custody config")
            .private_root = assets.clone();
        assert_eq!(
            provision_legacy_lifecycle_once_with_asset_root(
                &overlap_config,
                &store,
                &db.pool,
                &fence,
                Some(assets.clone()),
            )
            .await
            .expect("overlap turn"),
            LegacyLifecycleProvisionOutcome::Quarantined(overlap_id),
        );
        assert!(
            !assets
                .join(format!("work-{}.lock", overlap_id.simple()))
                .exists(),
            "overlap must be rejected before creating a lock"
        );
        assert_eq!(
            fs::read(assets.join("existing")).expect("asset retained"),
            b"asset bytes"
        );
        assert_eq!(
            provision_legacy_lifecycle_once_with_asset_root(
                &config,
                &store,
                &db.pool,
                &fence,
                Some(assets.clone())
            )
            .await
            .expect("pass end"),
            LegacyLifecycleProvisionOutcome::PassComplete
        );
        assert_eq!(
            provision_legacy_lifecycle_once_with_asset_root(
                &config,
                &store,
                &db.pool,
                &fence,
                Some(assets.clone())
            )
            .await
            .expect("ready replay"),
            LegacyLifecycleProvisionOutcome::Ready(ready_id)
        );
        assert!(
            store
                .finish_lifecycle_maintenance(&db.pool, &fence)
                .await
                .is_err(),
            "quarantined work cannot complete the maintenance pass"
        );
        db.drop().await;
    }

    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[tokio::test]
    async fn shared_admin_scope_accepts_128_owned_items() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let source = fixture.path().join("source");
        let private = fixture.path().join("private");
        let stage = fixture.path().join("stage");
        let work = fixture.path().join("work");
        let canonical = fixture.path().join("canonical");
        let assets = fixture.path().join("assets");
        for dir in [&source, &private, &stage, &work, &canonical, &assets] {
            fs::create_dir(dir).expect("owned directory");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("private mode");
        }
        let source_config =
            InboundSource::from_root(source, None, Some("fixture".into())).expect("source");
        let mut config = crate::config::IngestionConfig::with_sources(vec![source_config]);
        let uid = rustix::process::geteuid().as_raw();
        config.protected_custody = Some(crate::config::ProtectedCustodyConfig {
            private_root: private,
            producer_uid: if uid == 1 { 2 } else { 1 },
        });
        config.staging_root = Some(stage);
        config.work_area_base = work;
        config.storage_root = Some(canonical);
        let db = crate::common::TestDb::create("admin_scope_128").await;
        let store = melodee_persistence::WorkItemStore;
        let mut ids = Vec::new();
        for number in 0..128 {
            let item = melodee_domain::ReleaseWorkItem::discovered(
                format!("owned-{number:03}"),
                None,
                uuid::Uuid::now_v7(),
                time::OffsetDateTime::now_utc(),
            );
            store
                .create(&db.pool, &item)
                .await
                .expect("create owned item");
            ids.push(item.id);
        }
        let fence = store
            .begin_lifecycle_maintenance(&db.pool, "owned-test", time::Duration::minutes(10))
            .await
            .expect("maintenance");
        store
            .confirm_lifecycle_quiescence(
                &db.pool,
                &fence,
                &melodee_persistence::LifecycleQuiescenceAttestation {
                    reason: "owned test",
                    backup_reference: "owned fixture",
                    stop_all_writers: true,
                    inspection_complete: true,
                },
            )
            .await
            .expect("quiescence");
        for _ in 0..128 {
            assert!(matches!(
                provision_legacy_lifecycle_once_with_asset_root(
                    &config,
                    &store,
                    &db.pool,
                    &fence,
                    Some(assets.clone())
                )
                .await
                .expect("provision"),
                LegacyLifecycleProvisionOutcome::Ready(_)
            ));
        }
        let reservation = store
            .begin_maintenance_admin_media_reservation(
                &db.pool,
                &ids,
                &[],
                "owned-test",
                time::Duration::minutes(10),
                &fence,
            )
            .await
            .expect("reserve 128 items");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("shared 128-item scope");
        assert_eq!(scope.identities.len(), 128);
        assert_eq!(scope.inner.lock().expect("scope lock").sources.len(), 1);
        scope.revalidate().expect("shared scope remains live");
        drop(scope);
        store
            .finish_admin_media_reservation(&db.pool, &reservation)
            .await
            .expect("release owned reservation");
    }

    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provisional_scan_revalidates_exact_db_intent_and_retains_source() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("protected runtime parent");
        let fixture_dir = tempfile::tempdir_in(runtime).expect("owned fixture");
        let inbound = fixture_dir.path().join("inbound");
        let private = fixture_dir.path().join("private");
        let staging = fixture_dir.path().join("staging");
        let work = fixture_dir.path().join("work");
        let assets = fixture_dir.path().join("assets");
        let canonical = fixture_dir.path().join("canonical");
        for path in [&inbound, &private, &staging, &work, &assets, &canonical] {
            fs::create_dir(path).expect("owned directory");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private mode");
        }
        let source = InboundSource::from_root(inbound.clone(), None, Some("fixture".into()))
            .expect("source");
        let mut config = crate::config::IngestionConfig::with_sources(vec![source]);
        let uid = rustix::process::geteuid().as_raw();
        config.protected_custody = Some(crate::config::ProtectedCustodyConfig {
            private_root: private,
            producer_uid: if uid == 1 { 2 } else { 1 },
        });
        config.staging_root = Some(staging);
        config.work_area_base = work;
        config.storage_root = Some(canonical.clone());
        let db = crate::common::TestDb::create("intent_scan").await;
        let payload = [0x51_u8; 42];
        let fixture = crate::catalog_common::seed_published_release_with_normalized_hash(
            &db,
            901,
            crate::catalog_common::CatalogFixturePresentation::default(),
            blake3::hash(&payload).to_hex().to_string(),
        )
        .await;
        let catalog = melodee_persistence::CatalogStore;
        let worker = melodee_persistence::WorkerId::new();
        let claimed = catalog
            .claim_cataloging(
                &db.pool,
                &worker,
                &crate::common::test_config(),
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("catalog claim");
        assert_eq!(claimed.len(), 1);
        catalog
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &melodee_persistence::CatalogImportRequest::new("intent-scan", "fixture"),
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("catalog fixture");
        let release_id = fixture.release_id.into_uuid();
        let public_id: uuid::Uuid =
            sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
                .bind(release_id)
                .fetch_one(&db.pool)
                .await
                .expect("public id");
        let relative: String =
            sqlx::query_scalar("SELECT canonical_relative_path FROM media_assets LIMIT 1")
                .fetch_one(&db.pool)
                .await
                .expect("asset path");
        let file = canonical.join(&relative);
        fs::create_dir_all(file.parent().expect("asset parent")).expect("owned asset parent");
        fs::write(&file, payload).expect("canonical bytes");
        fs::write(inbound.join("original"), b"source preserved").expect("source bytes");
        let store = melodee_persistence::WorkItemStore;
        let fence = store
            .begin_lifecycle_maintenance(&db.pool, "owned-test", time::Duration::minutes(2))
            .await
            .expect("maintenance");
        store
            .confirm_lifecycle_quiescence(
                &db.pool,
                &fence,
                &melodee_persistence::LifecycleQuiescenceAttestation {
                    reason: "owned test",
                    backup_reference: "owned fixture",
                    stop_all_writers: true,
                    inspection_complete: true,
                },
            )
            .await
            .expect("quiescence");
        assert_eq!(
            provision_legacy_lifecycle_once_with_asset_root(
                &config,
                &store,
                &db.pool,
                &fence,
                Some(assets.clone())
            )
            .await
            .expect("provision"),
            LegacyLifecycleProvisionOutcome::Ready(fixture.work_item_id)
        );
        let reservation = store
            .begin_maintenance_admin_media_reservation(
                &db.pool,
                &[],
                &[release_id],
                "owned-test",
                time::Duration::minutes(2),
                &fence,
            )
            .await
            .expect("reserve release");
        store
            .bind_maintenance_catalog_targets(
                &db.pool,
                &fence,
                &reservation,
                melodee_persistence::AdminCatalogDeleteKind::Release,
                &[public_id],
            )
            .await
            .expect("bind target");
        store
            .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
            .await
            .expect("begin snapshot");
        let configured =
            provisional_configured_roots(&config, assets.clone()).expect("configured roots");
        let mut expected_roots = Vec::new();
        let mut registry_roots = Vec::new();
        for root in &configured {
            let observed = melodee_custody::RootExpectation::observe(
                root.role.clone(),
                root.source_library_id,
                &root.path,
            )
            .expect("observe owned root");
            registry_roots.push(melodee_persistence::AdminMediaRootExpectation {
                role: observed.role.clone(),
                source_library_id: observed.source_library_id,
                path_format: "unix-hex-v1".to_owned(),
                path_hex: observed.path_hex.clone(),
                generation: observed.generation.clone(),
                mount_id: observed.mount_id.clone(),
                filesystem_magic: observed.filesystem_magic.clone(),
            });
            expected_roots.push(observed);
        }
        let roots = melodee_custody::ExactConfiguredRootSet::open(&configured, &expected_roots)
            .expect("verify owned configured roots");
        store
            .bind_maintenance_media_roots(
                &db.pool,
                &fence,
                &reservation,
                uuid::Uuid::now_v7(),
                &registry_roots,
            )
            .await
            .expect("bind owned roots");
        drop(roots);
        let mut frozen = false;
        for _ in 0..20 {
            if store
                .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
                .await
                .expect("capture snapshot")
            {
                frozen = true;
                break;
            }
        }
        assert!(frozen);
        let intent = store
            .prepare_maintenance_deletion_page(
                &db.pool,
                &fence,
                &reservation,
                melodee_persistence::AdminDeletionPageKind::Media,
                uuid::Uuid::now_v7(),
            )
            .await
            .expect("prepare intent")
            .expect("one page");
        assert_eq!(intent.entries().len(), 1);
        let alternate = fixture_dir.path().join("alternate-canonical");
        fs::create_dir(&alternate).expect("owned alternate root");
        fs::set_permissions(&alternate, fs::Permissions::from_mode(0o700)).expect("alternate mode");
        let mut altered_config = config.clone();
        altered_config.storage_root = Some(alternate.clone());
        let wrong_scope = acquire_provisional_admin_lifecycle_scope(
            &altered_config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope with different configured root");
        assert!(
            begin_provisional_intent_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                wrong_scope,
                0
            )
            .await
            .is_err(),
            "configured-root drift must reject the persisted root binding"
        );
        assert_eq!(
            fs::read(&file).expect("canonical retained after root mismatch"),
            payload
        );
        let credentials = reservation
            .custody_credentials(&fence)
            .expect("opaque credentials");
        credentials
            .observe_original_epochs(&db.pool, intent.id())
            .await
            .expect("matching original epochs");
        let wrong_credentials = melodee_custody::MaintenanceCredentials::from_parts(
            "owned-test".to_owned(),
            reservation.id(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
        )
        .expect("well-shaped wrong credentials");
        assert!(
            wrong_credentials
                .observe_original_epochs(&db.pool, intent.id())
                .await
                .is_err()
        );
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("exact scope");
        let scan = begin_provisional_intent_scan(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            0,
        )
        .await
        .expect("intent bound scan");
        let (scan, progress) = advance_provisional_intent_scan(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scan,
            10,
        )
        .await
        .expect("first fenced step");
        assert_eq!(
            progress,
            melodee_custody::IncrementalFileProgress::More { bytes_read: 10 }
        );
        let asset_id = intent.entries()[0].object_id;
        sqlx::query("UPDATE media_assets SET byte_hash=repeat('f',64) WHERE id=$1")
            .bind(asset_id)
            .execute(&db.pool)
            .await
            .expect("simulate same-size metadata change");
        credentials
            .observe_original_epochs(&db.pool, intent.id())
            .await
            .expect("epoch-only observation does not verify target content");
        assert!(
            advance_provisional_intent_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scan,
                10
            )
            .await
            .is_err(),
            "changed DB evidence must reject and drop the scan"
        );
        sqlx::query("UPDATE media_assets SET byte_hash=$2 WHERE id=$1")
            .bind(asset_id)
            .bind(blake3::hash(&payload).to_hex().to_string())
            .execute(&db.pool)
            .await
            .expect("restore owned fixture metadata");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("reacquire scope");
        let scan = begin_provisional_intent_scan(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            0,
        )
        .await
        .expect("fresh scan");
        let (scan, progress) = advance_provisional_intent_scan(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scan,
            1024,
        )
        .await
        .expect("fresh complete step");
        assert_eq!(progress, melodee_custody::IncrementalFileProgress::Complete);
        drop(scan);
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope for expired reservation");
        let scan = begin_provisional_intent_scan(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            0,
        )
        .await
        .expect("scan before expiry");
        sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(reservation.id()).execute(&db.pool).await.expect("expire owned reservation");
        assert!(
            credentials
                .observe_original_epochs(&db.pool, intent.id())
                .await
                .is_err(),
            "expired reservation is not live"
        );
        assert!(
            advance_provisional_intent_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scan,
                10
            )
            .await
            .is_err(),
            "expired reservation must reject the scan"
        );
        // Restore only this disposable fixture to separately test the other
        // fence; real expired reservations are never revived by this path.
        sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()+interval '2 minutes' WHERE id=$1").bind(reservation.id()).execute(&db.pool).await.expect("restore owned fixture reservation");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope for expired maintenance fence");
        let scan = begin_provisional_intent_scan(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            0,
        )
        .await
        .expect("scan before fence expiry");
        sqlx::query("UPDATE work_item_lifecycle_maintenance SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE singleton").execute(&db.pool).await.expect("expire owned maintenance fence");
        assert!(
            credentials
                .observe_original_epochs(&db.pool, intent.id())
                .await
                .is_err(),
            "expired maintenance epoch is not live"
        );
        assert!(
            advance_provisional_intent_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scan,
                10
            )
            .await
            .is_err(),
            "expired maintenance fence must reject the scan"
        );
        assert_eq!(fs::read(&file).expect("canonical retained"), payload);
        assert_eq!(
            fs::read(inbound.join("original")).expect("source retained"),
            b"source preserved"
        );
        // Restore only the disposable fixture fence to test a separate
        // non-authorizing quarantine intent; real expired epochs are never
        // revived by this path.
        sqlx::query("UPDATE work_item_lifecycle_maintenance SET lease_expires_at=clock_timestamp()+interval '2 minutes' WHERE singleton").execute(&db.pool).await.expect("restore owned maintenance fixture");
        use std::os::unix::ffi::OsStringExt;
        let relative_parent =
            std::path::PathBuf::from(std::ffi::OsString::from_vec(b".quarantine-\xff".to_vec()));
        let quarantine_parent = canonical.join(&relative_parent);
        fs::create_dir(&quarantine_parent).expect("owned quarantine directory");
        fs::set_permissions(&quarantine_parent, fs::Permissions::from_mode(0o700))
            .expect("private quarantine mode");
        let quarantine_id = uuid::Uuid::now_v7();
        let scan = provisional_ready_quarantine_scan(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
        )
        .await
        .expect("scan before interrupted move");
        assert!(
            provisional_quarantine_completed_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scan,
                ProvisionalQuarantineRequest {
                    relative_parent: relative_parent.clone(),
                    id: quarantine_id,
                    after_name_check: Some(Box::new(|| Err(std::io::Error::from(
                        std::io::ErrorKind::Interrupted
                    )))),
                    interrupt_after_rename: false,
                    interrupt_after_move: false,
                },
            )
            .await
            .is_err(),
            "interruption before rename leaves intent unresolved"
        );
        let binding = store
            .read_admin_quarantine_binding(&db.pool, &fence, &reservation, &intent, quarantine_id)
            .await
            .expect("recover exact quarantine intent");
        assert_eq!(
            binding.state, "intent",
            "observational move does not acknowledge or verify deletion"
        );
        assert_eq!(
            binding.quarantine_parent_relative_hex,
            "2e71756172616e74696e652dff"
        );
        let quarantine_file = quarantine_parent.join(&binding.leaf_name);
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope after pre-move interruption");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe both names before move");
        assert_eq!(observed.original, ProvisionalNameObservation::Matching);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Missing);
        drop(observed);

        let absent_parked = canonical.join("test-temporarily-parked");
        fs::rename(&file, &absent_parked).expect("park owned original for missing-name case");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope for missing names");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe two missing names");
        assert_eq!(observed.original, ProvisionalNameObservation::Missing);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Missing);
        assert_eq!(
            fs::read(&absent_parked).expect("parked bytes retained"),
            payload
        );
        drop(observed);
        fs::rename(&absent_parked, &file).expect("restore owned original");
        std::os::unix::fs::symlink(&file, &quarantine_file).expect("owned symlink fixture");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope for unreadable name");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe symlink without following it");
        assert_eq!(observed.original, ProvisionalNameObservation::Matching);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Unreadable);
        drop(observed);
        fs::remove_file(&quarantine_file).expect("remove only owned test symlink");

        let scan = provisional_ready_quarantine_scan(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
        )
        .await
        .expect("scan before pre-fsync interruption");
        assert!(
            provisional_quarantine_completed_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scan,
                ProvisionalQuarantineRequest {
                    relative_parent: relative_parent.clone(),
                    id: quarantine_id,
                    after_name_check: None,
                    interrupt_after_rename: true,
                    interrupt_after_move: false
                }
            )
            .await
            .is_err(),
            "interruption after rename but before fsync remains unresolved"
        );
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope after pre-fsync interruption");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe after pre-fsync interruption");
        assert_eq!(observed.original, ProvisionalNameObservation::Missing);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Matching);
        assert_eq!(
            fs::read(&quarantine_file).expect("pre-fsync bytes retained"),
            payload
        );
        drop(observed);
        fs::rename(&quarantine_file, &file).expect("restore only owned test fixture");

        // Cancel the async waiter while the actual worker is paused after its
        // last name check. Its held scope must survive until rename terminates.
        let scan = provisional_ready_quarantine_scan(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
        )
        .await
        .expect("scan before cancellation");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let pool_for_worker = db.pool.clone();
        let fence_for_worker = fence.clone();
        let reservation_for_worker = reservation.clone();
        let intent_for_worker = intent.clone();
        let relative_for_worker = relative_parent.clone();
        let task = tokio::spawn(async move {
            provisional_quarantine_completed_scan(
                &melodee_persistence::WorkItemStore,
                &pool_for_worker,
                &fence_for_worker,
                &reservation_for_worker,
                &intent_for_worker,
                scan,
                ProvisionalQuarantineRequest {
                    relative_parent: relative_for_worker,
                    id: quarantine_id,
                    after_name_check: Some(Box::new(move || {
                        started_tx.send(()).map_err(std::io::Error::other)?;
                        release_rx.recv().map_err(std::io::Error::other)?;
                        Ok(())
                    })),
                    interrupt_after_rename: false,
                    interrupt_after_move: false,
                },
            )
            .await
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker paused before rename");
        task.abort();
        assert!(
            acquire_provisional_admin_lifecycle_scope(
                &config,
                assets.clone(),
                &store,
                &db.pool,
                &reservation
            )
            .await
            .is_err(),
            "cancelled waiter must not release a running worker's locks"
        );
        release_tx.send(()).expect("release owned worker");
        let scope = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(scope) = acquire_provisional_admin_lifecycle_scope(
                    &config,
                    assets.clone(),
                    &store,
                    &db.pool,
                    &reservation,
                )
                .await
                {
                    break scope;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker terminates before scope can be reacquired");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe after cancelled waiter");
        assert_eq!(observed.original, ProvisionalNameObservation::Missing);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Matching);
        assert_eq!(
            fs::read(&quarantine_file).expect("cancelled worker retained bytes"),
            payload
        );
        drop(observed);
        // Test-only fixture restoration; the recovery API itself never moves
        // bytes or treats this observation as authorization.
        fs::rename(&quarantine_file, &file).expect("restore owned fixture for race case");

        let parked_original = canonical.join("test-parked-original");
        let replacement = vec![0x52_u8; payload.len()];
        let replacement_for_hook = replacement.clone();
        let original_for_hook = file.clone();
        let parked_for_hook = parked_original.clone();
        let scan = provisional_ready_quarantine_scan(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
        )
        .await
        .expect("scan before replacement race");
        assert!(
            provisional_quarantine_completed_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scan,
                ProvisionalQuarantineRequest {
                    relative_parent: relative_parent.clone(),
                    id: quarantine_id,
                    after_name_check: Some(Box::new(move || {
                        fs::rename(&original_for_hook, &parked_for_hook)?;
                        fs::write(&original_for_hook, &replacement_for_hook)?;
                        Ok(())
                    })),
                    interrupt_after_rename: false,
                    interrupt_after_move: false,
                }
            )
            .await
            .is_err(),
            "a replacement moved after the name check must not verify"
        );
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope after replacement race");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe replacement");
        assert_eq!(observed.original, ProvisionalNameObservation::Missing);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Different);
        assert_eq!(
            fs::read(&parked_original).expect("original retained"),
            payload
        );
        assert_eq!(
            fs::read(&quarantine_file).expect("replacement retained"),
            replacement
        );
        drop(observed);
        let parked_replacement = canonical.join("test-parked-replacement");
        fs::rename(&quarantine_file, &parked_replacement)
            .expect("preserve replacement in owned fixture");
        fs::rename(&parked_original, &file).expect("restore original in owned fixture");

        let scan = provisional_ready_quarantine_scan(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
        )
        .await
        .expect("scan before post-move interruption");
        assert!(
            provisional_quarantine_completed_scan(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scan,
                ProvisionalQuarantineRequest {
                    relative_parent: relative_parent.clone(),
                    id: quarantine_id,
                    after_name_check: None,
                    interrupt_after_rename: false,
                    interrupt_after_move: true
                }
            )
            .await
            .is_err(),
            "post-fsync interruption remains unresolved"
        );
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope after post-move interruption");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe both names after move");
        assert_eq!(observed.original, ProvisionalNameObservation::Missing);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Matching);
        assert_eq!(
            fs::read(&quarantine_file).expect("original retained in quarantine"),
            payload
        );
        drop(observed);
        // Both names existing is explicitly reported and does not move either.
        let later = vec![0x53_u8; payload.len()];
        fs::write(&file, &later).expect("later owned occurrence");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope for two names");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &fence,
            &reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("observe two names");
        assert_eq!(observed.original, ProvisionalNameObservation::Different);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Matching);
        drop(observed);
        sqlx::query("UPDATE work_item_lifecycle_maintenance SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE singleton").execute(&db.pool).await.expect("expire fixture fence");
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &reservation,
        )
        .await
        .expect("scope despite stale maintenance token");
        assert!(
            provisional_inspect_quarantine_names(
                &store,
                &db.pool,
                &fence,
                &reservation,
                &intent,
                scope,
                quarantine_id
            )
            .await
            .is_err(),
            "stale epoch cannot inspect as a live recovery"
        );
        // The owned fixture externally stops its worker before reclaim. The
        // attestation records that precondition; DB expiry is not proof.
        sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(reservation.id()).execute(&db.pool).await.expect("expire owned reservation for recovery");
        let recovered_fence = store
            .reclaim_lifecycle_maintenance(&db.pool, "owned-recovery", time::Duration::minutes(2))
            .await
            .expect("fresh maintenance epoch");
        let recovered_reservation = store
            .reclaim_admin_media_reservation(
                &db.pool,
                reservation.id(),
                "owned-recovery",
                time::Duration::minutes(2),
            )
            .await
            .expect("fresh reservation epoch");
        store
            .confirm_lifecycle_recovery_quiescence(
                &db.pool,
                &recovered_fence,
                &recovered_reservation,
                &melodee_persistence::LifecycleQuiescenceAttestation {
                    reason: "owned stopped-worker recovery",
                    backup_reference: "owned fixture",
                    stop_all_writers: true,
                    inspection_complete: true,
                },
            )
            .await
            .expect("attest recovery quiescence");
        assert!(
            credentials
                .observe_original_epochs(&db.pool, intent.id())
                .await
                .is_err(),
            "old credentials cannot revive"
        );
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets.clone(),
            &store,
            &db.pool,
            &recovered_reservation,
        )
        .await
        .expect("fresh recovery scope");
        assert!(
            begin_provisional_intent_scan(
                &store,
                &db.pool,
                &recovered_fence,
                &recovered_reservation,
                &intent,
                scope,
                0
            )
            .await
            .is_err(),
            "old intent cannot authorize a new-epoch scan"
        );
        let scope = acquire_provisional_admin_lifecycle_scope(
            &config,
            assets,
            &store,
            &db.pool,
            &recovered_reservation,
        )
        .await
        .expect("fresh observational recovery scope");
        let observed = provisional_inspect_quarantine_names(
            &store,
            &db.pool,
            &recovered_fence,
            &recovered_reservation,
            &intent,
            scope,
            quarantine_id,
        )
        .await
        .expect("fresh epoch independently observes both names");
        assert_eq!(observed.original, ProvisionalNameObservation::Different);
        assert_eq!(observed.quarantine, ProvisionalNameObservation::Matching);
        drop(observed);
        let state: String =
            sqlx::query_scalar("SELECT state FROM admin_media_quarantine_intents WHERE id=$1")
                .bind(quarantine_id)
                .fetch_one(&db.pool)
                .await
                .expect("intent state");
        assert_eq!(state, "intent");
        let acknowledged:i64=sqlx::query_scalar("SELECT count(*) FROM admin_media_snapshot_entries WHERE reservation_id=$1 AND acknowledgement IS NOT NULL").bind(reservation.id()).fetch_one(&db.pool).await.expect("ack count");
        assert_eq!(
            acknowledged, 0,
            "observation must never acknowledge deletion"
        );
        assert_eq!(
            fs::read(&quarantine_file).expect("quarantined bytes retained"),
            payload
        );
        assert_eq!(fs::read(&file).expect("later bytes retained"), later);
        assert_eq!(
            fs::read(&parked_replacement).expect("replacement retained"),
            replacement
        );
        assert_eq!(
            fs::read(inbound.join("original")).expect("inbound source retained"),
            b"source preserved"
        );
    }

    // Synthetic DB metadata exercises orchestration only. It does not prove
    // fs-verity, complete tree verification, or production receipt safety.
    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[tokio::test]
    async fn cleaned_membership_page_requires_live_claim_and_detects_mutation() {
        use melodee_persistence::{
            CustodyHashObservation, CustodyHashPurpose, ProtectedPublication,
            ProtectedStageIdentity, RetainedBaselineChunk, RetainedFileVerity, WorkEngineConfig,
            WorkItemStore, WorkerId,
        };
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("protected runtime parent");
        let temp = tempfile::tempdir_in(runtime).expect("owned fixture");
        let source = temp.path().join("inbound");
        let private = temp.path().join("private");
        fs::create_dir(&source).expect("source");
        fs::create_dir(source.join("candidate")).expect("inbound candidate");
        fs::write(source.join("candidate/track"), b"original inbound").expect("inbound bytes");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private mode");
        fs::create_dir(private.join("source")).expect("stage");
        fs::set_permissions(private.join("source"), fs::Permissions::from_mode(0o700))
            .expect("stage mode");
        fs::write(private.join("source/track"), b"x").expect("stage track");
        let uid = rustix::process::geteuid().as_raw();
        let namespace = std::sync::Arc::new(
            validate_custody_namespace(&private, &source, if uid == 1 { 2 } else { 1 })
                .expect("namespace"),
        );
        let root = GuardedRoot::open(
            InboundSource::from_root(source.clone(), None, Some("test".into()))
                .expect("source config"),
        )
        .expect("root");
        let stage = open_directory(&private.join("source")).expect("stage handle");
        let stage_generation = ProtectedStageAttempt::identity(&stage).expect("stage generation");
        let db = crate::common::TestDb::create("cleaned_page_ingestion").await;
        let now = time::OffsetDateTime::now_utc();
        let mut item = melodee_domain::ReleaseWorkItem::discovered(
            "candidate".to_owned(),
            None,
            uuid::Uuid::now_v7(),
            now,
        );
        item.state = melodee_domain::IngestionState::Ready;
        let store = WorkItemStore;
        let worker = WorkerId::new();
        let config = WorkEngineConfig::default();
        store.create(&db.pool, &item).await.expect("item");
        let (_, claim) = store
            .claim_with_protected_epoch(&db.pool, &worker, &config, now)
            .await
            .expect("claim")
            .remove(0)
            .into_parts();
        store
            .begin_protected_staging_handoff(
                &db.pool,
                &claim,
                &worker,
                "private/source",
                "source-gen",
                now,
            )
            .await
            .expect("handoff");
        let reserved = store
            .reserve_protected_stage_attempt(
                &db.pool,
                &claim,
                "root-gen",
                "source-gen",
                "private/source",
            )
            .await
            .expect("reservation");
        let created = store
            .acknowledge_protected_stage_created(
                &db.pool,
                &claim,
                &reserved,
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "attempt-gen",
                    marker_generation: "marker-gen",
                    lock_generation: "attempt-lock-gen",
                },
            )
            .await
            .expect("synthetic created");
        let private_handle = open_directory(&private).expect("private handle");
        let lifecycle_root =
            ProtectedStageAttempt::identity(&private_handle).expect("root generation");
        let intent = store
            .begin_protected_stage_promotion(
                &db.pool,
                &claim,
                &created,
                &stage_generation,
                &lifecycle_root,
            )
            .await
            .expect("promotion intent");
        let promoted = store
            .acknowledge_protected_stage_promotion(
                &db.pool,
                &claim,
                &intent,
                &stage_generation,
                &lifecycle_root,
            )
            .await
            .expect("synthetic promotion");
        let entry = CustodyHashObservation {
            path_key: "track".to_owned(),
            parent_key: String::new(),
            name_key: "747261636b".to_owned(),
            legacy_path_hex: "747261636b00".to_owned(),
            is_directory: false,
            legacy_length: 1,
            digest: "c".repeat(64),
        };
        let source_capture = store
            .begin_protected_custody_hash_spool(
                &db.pool,
                &claim,
                &worker,
                CustodyHashPurpose::Source,
                "source-gen",
                "candidate",
                now,
            )
            .await
            .expect("source capture");
        store
            .append_protected_custody_hash_spool(
                &db.pool,
                &claim,
                source_capture,
                &worker,
                std::slice::from_ref(&entry),
                now,
            )
            .await
            .expect("source observation");
        store
            .freeze_protected_custody_hash_spool(&db.pool, &claim, source_capture, &worker, 1, now)
            .await
            .expect("source freeze");
        let before = store
            .seal_protected_custody_hash_spool(&db.pool, &claim, source_capture, &worker)
            .await
            .expect("source before");
        let session = store
            .begin_protected_retention_session(&db.pool, &claim, &before)
            .await
            .expect("protected ordered session");
        let legacy = "a".repeat(64);
        let stage_capture = store
            .begin_protected_custody_hash_spool(
                &db.pool,
                &claim,
                &worker,
                CustodyHashPurpose::Staging,
                &stage_generation,
                "private/source",
                now,
            )
            .await
            .expect("stage capture");
        store
            .append_protected_custody_hash_spool(
                &db.pool,
                &claim,
                stage_capture,
                &worker,
                std::slice::from_ref(&entry),
                now,
            )
            .await
            .expect("stage observation");
        store
            .freeze_protected_custody_hash_spool(&db.pool, &claim, stage_capture, &worker, 1, now)
            .await
            .expect("stage freeze");
        let stage_seal = store
            .seal_protected_custody_hash_spool(&db.pool, &claim, stage_capture, &worker)
            .await
            .expect("stage seal");
        store
            .register_protected_retention_stage(
                &db.pool,
                &claim,
                &session,
                &before,
                &stage_seal,
                &legacy,
            )
            .await
            .expect("protected staging session");
        let baseline_id = store
            .begin_retained_baseline(&db.pool, &worker, &stage_seal)
            .await
            .expect("baseline");
        let file = store
            .begin_retained_baseline_file(&db.pool, &worker, baseline_id, &stage_seal, "track")
            .await
            .expect("baseline file");
        store
            .append_retained_baseline_chunks(
                &db.pool,
                &worker,
                baseline_id,
                &stage_seal,
                file.ordinal,
                &[RetainedBaselineChunk {
                    index: 0,
                    length: 1,
                    digest: "d".repeat(64),
                }],
            )
            .await
            .expect("baseline chunk");
        store
            .finish_retained_baseline_file(
                &db.pool,
                &worker,
                baseline_id,
                &stage_seal,
                file.ordinal,
                &"c".repeat(64),
            )
            .await
            .expect("baseline file complete");
        store
            .record_retained_baseline_file_verity(
                &db.pool,
                &worker,
                baseline_id,
                &stage_seal,
                file.ordinal,
                &RetainedFileVerity {
                    device: 1,
                    inode: 1,
                    digest: [0; 32],
                },
            )
            .await
            .expect("synthetic verity metadata");
        let baseline = store
            .freeze_protected_retained_baseline(&db.pool, &worker, baseline_id, &stage_seal)
            .await
            .expect("baseline freeze");
        store
            .revalidate_protected_custody_hash_spool(
                &db.pool,
                &claim,
                source_capture,
                &worker,
                "source-gen",
                uuid::Uuid::now_v7(),
                now,
            )
            .await
            .expect("source revalidation");
        store
            .append_protected_custody_hash_spool(
                &db.pool,
                &claim,
                source_capture,
                &worker,
                std::slice::from_ref(&entry),
                now,
            )
            .await
            .expect("source after observation");
        store
            .freeze_protected_custody_hash_spool(&db.pool, &claim, source_capture, &worker, 1, now)
            .await
            .expect("source after freeze");
        let after = store
            .seal_protected_custody_hash_spool(&db.pool, &claim, source_capture, &worker)
            .await
            .expect("source after seal");
        let lifecycle =
            ProtectedFinalStageLock::create(namespace.clone(), root.clone(), baseline_id)
                .expect("lifecycle lock");
        let lifecycle_generation = lifecycle.0.generation.clone();
        store
            .publish_protected_retained(
                &db.pool,
                ProtectedPublication {
                    claim: &claim,
                    attempt: &promoted,
                    baseline: &baseline,
                    source_before: &before,
                    stage: &stage_seal,
                    source_after: &after,
                    session_id: session.id(),
                    legacy_hash: &legacy,
                    lifecycle_root_generation: &lifecycle_root,
                    lifecycle_lock_generation: &lifecycle_generation,
                },
            )
            .await
            .expect("synthetic publication");
        let cleanup = store
            .claim_protected_stage_cleanup(&db.pool, &worker, 1, time::Duration::minutes(1))
            .await
            .expect("cleanup claim")
            .remove(0);
        store
            .begin_protected_stage_cleanup(&db.pool, &cleanup)
            .await
            .expect("cleanup intent");
        store
            .complete_protected_stage_cleanup(&db.pool, &cleanup)
            .await
            .expect("synthetic cleanup");
        let claimed_receipt = store
            .claimed_stage_receipt(&db.pool, &claim)
            .await
            .expect("claimed receipt");
        assert!(matches!(
            &claimed_receipt,
            melodee_persistence::ClaimedStageReceipt::ProtectedCleaned(_)
        ));
        if let melodee_persistence::ClaimedStageReceipt::ProtectedCleaned(receipt) = claimed_receipt
        {
            assert_eq!(receipt.baseline_id, baseline_id);
            assert_eq!(receipt.lifecycle_lock_generation, lifecycle_generation);
            assert_eq!(receipt.stage_generation, stage_generation);
        }
        let owner = bind_provisional_cleaned_stage_owner(
            &store,
            &db.pool,
            &claim,
            lifecycle,
            &crate::config::ProtectedCustodyConfig {
                private_root: private.clone(),
                producer_uid: if uid == 1 { 2 } else { 1 },
            },
            temp.path(),
        )
        .await
        .expect("provisional claim-bound owner");
        assert_eq!(owner.receipt.baseline_id, baseline_id);
        let request = CleanedDirectoryPageRequest {
            attempt_id: promoted.id(),
            lifecycle: owner.lifecycle,
            staging_root: temp.path().to_path_buf(),
            parent_key: String::new(),
            cookie: None,
        };
        let page = check_cleaned_directory_membership_page(&store, &db.pool, &claim, request)
            .await
            .expect("matching cleaned directory page");
        assert!(page.entries.contains(&("track".to_owned(), Some(false))));
        assert_eq!(page.continuation, RetainedDirectoryContinuation::Eof);
        drop(page.lifecycle);
        let cancelled_lock = ProtectedFinalStageLock::acquire(
            namespace.clone(),
            root.clone(),
            baseline_id,
            &lifecycle_generation,
        )
        .expect("cancellation lock");
        let cancelled_request = CleanedDirectoryPageRequest {
            attempt_id: promoted.id(),
            lifecycle: cancelled_lock,
            staging_root: temp.path().to_path_buf(),
            parent_key: String::new(),
            cookie: None,
        };
        let cancelled_pool = db.pool.clone();
        let cancelled_claim = claim.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (_release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            check_cleaned_directory_membership_page_with_hook(
                &WorkItemStore,
                &cancelled_pool,
                &cancelled_claim,
                cancelled_request,
                || async move {
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                },
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .expect("membership reached")
            .expect("membership signal");
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                baseline_id,
                &lifecycle_generation
            )
            .is_err()
        );
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                baseline_id,
                &lifecycle_generation
            )
            .is_ok()
        );
        sqlx::query("UPDATE release_work_items SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
            .bind(item.id).execute(&db.pool).await.expect("expire old claim");
        let (_, fresh) = store
            .claim_with_protected_epoch(&db.pool, &worker, &config, time::OffsetDateTime::now_utc())
            .await
            .expect("fresh claim")
            .remove(0)
            .into_parts();
        let stale_lock = ProtectedFinalStageLock::acquire(
            namespace.clone(),
            root.clone(),
            baseline_id,
            &lifecycle_generation,
        )
        .expect("stale test lock");
        let request = CleanedDirectoryPageRequest {
            attempt_id: promoted.id(),
            lifecycle: stale_lock,
            staging_root: temp.path().to_path_buf(),
            parent_key: String::new(),
            cookie: None,
        };
        assert!(
            check_cleaned_directory_membership_page(&store, &db.pool, &claim, request)
                .await
                .is_err()
        );
        let stale_owner_lock = ProtectedFinalStageLock::acquire(
            namespace.clone(),
            root.clone(),
            baseline_id,
            &lifecycle_generation,
        )
        .expect("stale owner lock");
        assert!(
            bind_provisional_cleaned_stage_owner(
                &store,
                &db.pool,
                &claim,
                stale_owner_lock,
                &crate::config::ProtectedCustodyConfig {
                    private_root: private.clone(),
                    producer_uid: if uid == 1 { 2 } else { 1 },
                },
                temp.path(),
            )
            .await
            .is_err()
        );
        let mutation_lock =
            ProtectedFinalStageLock::acquire(namespace, root, baseline_id, &lifecycle_generation)
                .expect("mutation test lock");
        let request = CleanedDirectoryPageRequest {
            attempt_id: promoted.id(),
            lifecycle: mutation_lock,
            staging_root: temp.path().to_path_buf(),
            parent_key: String::new(),
            cookie: None,
        };
        let injected = private.join("source/extra");
        assert!(
            check_cleaned_directory_membership_page_with_hook(
                &store,
                &db.pool,
                &fresh,
                request,
                || async {
                    fs::write(&injected, b"new bytes").expect("inject extra entry");
                }
            )
            .await
            .is_err()
        );
        assert_eq!(
            fs::read(private.join("source/track")).expect("track retained"),
            b"x"
        );
        assert_eq!(fs::read(injected).expect("extra retained"), b"new bytes");
        assert_eq!(
            fs::read(source.join("candidate/track")).expect("inbound retained"),
            b"original inbound"
        );
    }

    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[tokio::test]
    async fn protected_attempt_reservation_creates_marker_only_for_claimed_epoch() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("protected runtime parent");
        let temp = tempfile::tempdir_in(runtime).expect("owned fixture");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        fs::create_dir_all(source.join("candidate")).expect("source");
        fs::write(source.join("candidate/track"), b"producer bytes").expect("source bytes");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private mode");
        let service_uid = rustix::process::geteuid().as_raw();
        let producer_uid = if service_uid == 1 { 2 } else { 1 };
        let namespace = std::sync::Arc::new(
            validate_custody_namespace(&private, &source, producer_uid).expect("namespace"),
        );
        let root = GuardedRoot::open(
            InboundSource::from_root(source.clone(), None, Some("test".into()))
                .expect("source config"),
        )
        .expect("root");
        let provision_id = uuid::Uuid::now_v7();
        let provisioned =
            create_reserved_legacy_lifecycle(namespace.clone(), root.clone(), provision_id)
                .await
                .expect("admitted legacy lock creation");
        let (provision_root, provision_lock) = provisioned.generations().expect("lock identity");
        drop(provisioned);
        assert!(
            LegacyLifecycleCapability::acquire(
                namespace.clone(),
                root.clone(),
                provision_id,
                provision_root,
                provision_lock,
            )
            .is_ok()
        );
        let baseline_id = uuid::Uuid::now_v7();
        let lifecycle =
            ProtectedFinalStageLock::create(namespace.clone(), root.clone(), baseline_id)
                .expect("lifecycle lock");
        let lock_generation = lifecycle.0.generation.clone();
        let hold: std::sync::Arc<dyn Send + Sync> =
            std::sync::Arc::new(std::sync::Mutex::new(lifecycle));
        let owned = crate::acquisition::GuardedOwnedSource::with_test_hold(
            crate::acquisition::OwnedSource {
                root: private.join("source"),
                tree_hash: String::new(),
            },
            hold,
        );
        let shared_lifetime = owned.clone();
        drop(owned);
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                baseline_id,
                &lock_generation,
            )
            .is_err(),
            "a shared lifetime cannot acquire separate mutation authority"
        );
        drop(shared_lifetime);
        let lifecycle = ProtectedFinalStageLock::acquire(
            namespace.clone(),
            root.clone(),
            baseline_id,
            &lock_generation,
        )
        .expect("reacquire after all shared lifetimes end");
        let page = spawn_cleaned_stage_page(lifecycle, "unused-for-empty-page".to_owned(), vec![])
            .await
            .expect("start empty page");
        let (lifecycle, last) = page.finish().await.expect("finish empty page");
        assert_eq!(last, None);
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                baseline_id,
                &lock_generation,
            )
            .is_err(),
            "the lifecycle lock remains held between pages"
        );
        drop(lifecycle);
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                baseline_id,
                &lock_generation,
            )
            .is_ok()
        );
        let oversized_id = uuid::Uuid::now_v7();
        let oversized_lock =
            ProtectedFinalStageLock::create(namespace.clone(), root.clone(), oversized_id)
                .expect("oversized page lock");
        let oversized_generation = oversized_lock.0.generation.clone();
        let oversized: Vec<_> = (0..17)
            .map(
                |ordinal| melodee_persistence::ProtectedBaselineVerityEntry {
                    ordinal,
                    path_key: format!("file-{ordinal}"),
                    evidence: melodee_persistence::RetainedFileVerity {
                        device: 1,
                        inode: 1,
                        digest: [0; 32],
                    },
                },
            )
            .collect();
        assert!(
            spawn_cleaned_stage_page(oversized_lock, "stage".to_owned(), oversized)
                .await
                .is_err()
        );
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                oversized_id,
                &oversized_generation,
            )
            .is_ok()
        );
        let cancelled_id = uuid::Uuid::now_v7();
        let cancelled_lock =
            ProtectedFinalStageLock::create(namespace.clone(), root.clone(), cancelled_id)
                .expect("cancellable lifecycle lock");
        let cancelled_generation = cancelled_lock.0.generation.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let producer = spawn_cleaned_stage_page_with_hook(
            cancelled_lock,
            "unused-for-empty-page".to_owned(),
            vec![],
            move || {
                started_tx.send(()).expect("started");
                release_rx.recv().expect("release worker");
            },
        )
        .await
        .expect("start paused page");
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker started");
        drop(producer);
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                cancelled_id,
                &cancelled_generation,
            )
            .is_err()
        );
        release_tx.send(()).expect("release worker");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(lock) = ProtectedFinalStageLock::acquire(
                    namespace.clone(),
                    root.clone(),
                    cancelled_id,
                    &cancelled_generation,
                ) {
                    drop(lock);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled worker released lifecycle lock");
        fs::create_dir_all(private.join("source/disc")).expect("private stage directory");
        fs::set_permissions(private.join("source"), fs::Permissions::from_mode(0o700))
            .expect("private stage mode");
        fs::set_permissions(
            private.join("source/disc"),
            fs::Permissions::from_mode(0o700),
        )
        .expect("private directory mode");
        fs::write(private.join("source/disc/track"), b"stage bytes").expect("stage fixture");
        let stage_handle = open_directory(&private.join("source")).expect("stage handle");
        let stage_generation = ProtectedStageAttempt::identity(&stage_handle).expect("generation");
        let directory_id = uuid::Uuid::now_v7();
        let directory_lock =
            ProtectedFinalStageLock::create(namespace.clone(), root.clone(), directory_id)
                .expect("directory lock");
        let directory_lock_generation = directory_lock.0.generation.clone();
        let lifecycle_root_generation = ProtectedStageAttempt::identity(
            directory_lock.0.parent.checked_file().expect("parent"),
        )
        .expect("lifecycle root generation");
        let worker = spawn_cleaned_directory_page(
            directory_lock,
            lifecycle_root_generation.clone(),
            stage_generation.clone(),
            "disc".to_owned(),
            None,
        )
        .await
        .expect("directory page worker");
        let page = worker.finish().await.expect("directory page");
        assert_eq!(page.continuation, RetainedDirectoryContinuation::Eof);
        assert!(!page.generation.is_empty());
        assert!(
            page.entries
                .contains(&("disc/track".to_owned(), Some(false)))
        );
        let directory_lock = page.lifecycle;
        assert!(
            ProtectedFinalStageLock::acquire(
                namespace.clone(),
                root.clone(),
                directory_id,
                &directory_lock_generation,
            )
            .is_err()
        );
        let wrong_root = spawn_cleaned_directory_page(
            directory_lock,
            "unix:0:0".to_owned(),
            stage_generation.clone(),
            String::new(),
            None,
        )
        .await
        .expect("wrong-root worker");
        assert!(wrong_root.finish().await.is_err());
        let directory_lock = ProtectedFinalStageLock::acquire(
            namespace.clone(),
            root.clone(),
            directory_id,
            &directory_lock_generation,
        )
        .expect("reacquire after rejected root");
        fs::rename(private.join("source"), private.join("parked-stage"))
            .expect("park original stage");
        fs::create_dir(private.join("source")).expect("replacement stage");
        fs::set_permissions(private.join("source"), fs::Permissions::from_mode(0o700))
            .expect("replacement private mode");
        fs::write(private.join("source/replacement"), b"replacement bytes")
            .expect("replacement bytes");
        let worker = spawn_cleaned_directory_page(
            directory_lock,
            lifecycle_root_generation,
            stage_generation,
            String::new(),
            None,
        )
        .await
        .expect("replacement page worker");
        assert!(worker.finish().await.is_err());
        assert_eq!(
            fs::read(private.join("parked-stage/disc/track")).expect("original retained"),
            b"stage bytes"
        );
        assert_eq!(
            fs::read(private.join("source/replacement")).expect("replacement retained"),
            b"replacement bytes"
        );
        let candidate = root
            .resolve(OsStr::new("candidate"))
            .expect("resolve")
            .expect("candidate");
        let target = ObservationTarget::source(root.clone(), candidate).expect("target");
        let root_handle = root.custody_root_handle().expect("root handle");
        let root_generation = source_generation_from_directory(&root_handle, Path::new("root"))
            .expect("root generation");
        let db = crate::common::TestDb::create("protected_attempt_ingestion").await;
        let now = time::OffsetDateTime::now_utc();
        let mut item = melodee_domain::ReleaseWorkItem::discovered(
            "candidate".to_owned(),
            None,
            uuid::Uuid::now_v7(),
            now,
        );
        item.state = melodee_domain::IngestionState::Ready;
        let store = melodee_persistence::WorkItemStore;
        store.create(&db.pool, &item).await.expect("item");
        let worker = melodee_persistence::WorkerId::new();
        let config = melodee_persistence::WorkEngineConfig::default();
        let (_, epoch) = store
            .claim_with_protected_epoch(&db.pool, &worker, &config, now)
            .await
            .expect("claim")
            .remove(0)
            .into_parts();
        let unknown_id = uuid::Uuid::now_v7();
        let unknown_lock =
            ProtectedFinalStageLock::create(namespace.clone(), root.clone(), uuid::Uuid::now_v7())
                .expect("unknown receipt lock");
        assert!(
            check_cleaned_directory_membership_page(
                &store,
                &db.pool,
                &epoch,
                CleanedDirectoryPageRequest {
                    attempt_id: unknown_id,
                    lifecycle: unknown_lock,
                    staging_root: temp.path().to_path_buf(),
                    parent_key: String::new(),
                    cookie: None,
                },
            )
            .await
            .is_err()
        );
        assert_eq!(
            fs::read(private.join("source/replacement")).expect("replacement preserved"),
            b"replacement bytes"
        );
        let final_binding = format!("releases/{}/source", item.id);
        store
            .begin_protected_staging_handoff(
                &db.pool,
                &epoch,
                &worker,
                &final_binding,
                &target.generation,
                now,
            )
            .await
            .expect("intent");
        let (acknowledged, physical) = reserve_and_create_protected_attempt(
            &db.pool,
            &store,
            &epoch,
            namespace.clone(),
            root.clone(),
            target,
            &root_generation,
            &final_binding,
            config.lease_duration,
        )
        .await
        .expect("acknowledged protected attempt");
        assert!(acknowledged.is_created());
        assert_eq!(
            acknowledged.namespace_name(),
            physical.relative.to_string_lossy()
        );
        physical.verify().expect("marker still matches");
        assert_eq!(
            fs::read(source.join("candidate/track")).expect("source retained"),
            b"producer bytes"
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM release_work_item_staging_handoffs WHERE work_item_id=$1",
        )
        .bind(item.id)
        .fetch_one(&db.pool)
        .await
        .expect("handoff");
        assert_eq!(
            status, "intent",
            "attempt creation does not publish a receipt"
        );
        sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
            .bind(item.id)
            .bind(now - time::Duration::seconds(1))
            .execute(&db.pool)
            .await
            .expect("expire old claim");
        let newer = store
            .claim_with_protected_epoch(&db.pool, &worker, &config, now)
            .await
            .expect("same-worker reclaim");
        assert_eq!(newer.len(), 1);
        let candidate = root
            .resolve(OsStr::new("candidate"))
            .expect("resolve")
            .expect("candidate");
        let stale_target = ObservationTarget::source(root.clone(), candidate).expect("target");
        assert!(
            reserve_and_create_protected_attempt(
                &db.pool,
                &store,
                &epoch,
                namespace.clone(),
                root.clone(),
                stale_target,
                &root_generation,
                &final_binding,
                config.lease_duration,
            )
            .await
            .is_err(),
            "a stale same-worker epoch cannot reserve another namespace"
        );
        physical.verify().expect("existing attempt preserved");
        let attempt_id = physical.attempt_id;
        let lock_generation = physical.lock.generation.clone();
        assert!(
            ProtectedAttemptLock::acquire(
                namespace.clone(),
                root.clone(),
                attempt_id,
                &lock_generation
            )
            .is_err(),
            "reclaimed DB lease must not bypass a stale worker's held lock"
        );
        drop(physical);
        let cleanup_lock = ProtectedAttemptLock::acquire(
            namespace.clone(),
            root.clone(),
            attempt_id,
            &lock_generation,
        )
        .expect("lock becomes available only after worker drops it");
        drop(cleanup_lock);
        let paused_id = uuid::Uuid::now_v7();
        let paused_lock = ProtectedAttemptLock::create(namespace.clone(), root.clone(), paused_id)
            .expect("paused worker lock");
        let paused_generation = paused_lock.generation.clone();
        let reaper = crate::reconcile::acquire_custody_reaper_permit()
            .await
            .expect("reaper permit");
        let admission = crate::admission::heavy_ingestion()
            .await
            .expect("admission");
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<(), AcquisitionError>>(1);
        let thread = std::thread::spawn(move || {
            let _admission = admission;
            let _lock = paused_lock;
            let _ = ready_tx.send(());
            let _ = release_rx.recv();
            let _ = sender.blocking_send(Ok(()));
        });
        let producer = ProtectedAttemptProducer {
            receiver: Some(receiver),
            thread: Some(thread),
            reaper_permit: Some(reaper),
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), ready_rx)
            .await
            .expect("worker started")
            .expect("worker alive");
        drop(producer); // Cancellation transfers the still-running worker to the reaper.
        assert!(
            ProtectedAttemptLock::acquire(
                namespace.clone(),
                root.clone(),
                paused_id,
                &paused_generation
            )
            .is_err(),
            "cancelled waiter cannot release a live worker's lock"
        );
        release_tx.send(()).expect("release worker");
        let released = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(lock) = ProtectedAttemptLock::acquire(
                    namespace.clone(),
                    root.clone(),
                    paused_id,
                    &paused_generation,
                ) {
                    break lock;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker termination releases lock");
        drop(released);
        let all_admission = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::admission::saturate_for_test(),
        )
        .await
        .expect("worker termination returns admission")
        .expect("admission available");
        drop(all_admission);
        let attempts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM inbound_protected_stage_attempts WHERE work_item_id=$1",
        )
        .bind(item.id)
        .fetch_one(&db.pool)
        .await
        .expect("attempt count");
        assert_eq!(attempts, 1);
        db.drop().await;
    }

    #[test]
    fn private_namespace_does_not_revoke_existing_writer_descriptor() {
        use std::{io::Write, os::unix::fs::PermissionsExt};
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        fs::create_dir_all(source.join("candidate")).expect("candidate");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let mut writer = fs::File::create(source.join("candidate/file")).expect("writer");
        writer.write_all(b"before").expect("write");
        fs::rename(source.join("candidate"), private.join("held")).expect("move");
        writer.write_all(b"after").expect("writer remains open");
        assert_eq!(
            fs::read(private.join("held/file")).expect("held"),
            b"beforeafter"
        );
    }

    #[test]
    fn streamed_content_observations_are_order_independent_and_detect_mutation() {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempfile::tempdir().expect("temp");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(first.join("disc")).expect("first");
        fs::create_dir_all(second.join("disc")).expect("second");
        let native = std::ffi::OsString::from_vec(b"invalid-\xff".to_vec());
        for (root, reverse) in [(&first, false), (&second, true)] {
            let files = if reverse {
                vec!["b", "a"]
            } else {
                vec!["a", "b"]
            };
            for file in files {
                fs::write(root.join("disc").join(file), file.as_bytes()).expect("file");
            }
            fs::write(root.join(&native), b"bytes").expect("native filename");
        }
        let observe = |path: &Path| {
            let root = cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
                .expect("root");
            let mut rows = Vec::new();
            let count = stream_custody_hash_observations(
                &root,
                || false,
                |entry| {
                    rows.push(entry);
                    Ok(())
                },
            )
            .expect("capture");
            rows.sort_unstable_by(|left, right| left.path_key.cmp(&right.path_key));
            assert_eq!(count as usize, rows.len());
            rows
        };
        let original = observe(&first);
        assert_eq!(original, observe(&second));
        assert!(
            original
                .iter()
                .any(|row| row.path_key.starts_with("//melodee-unix-hex:"))
        );
        fs::write(second.join("disc/a"), b"z").expect("same size mutation");
        assert_ne!(original, observe(&second));
        let root =
            cap_std::fs::Dir::open_ambient_dir(&first, cap_std::ambient_authority()).expect("root");
        assert!(stream_custody_hash_observations(&root, || true, |_| Ok(())).is_err());
    }

    #[test]
    fn bounded_copy_stream_hashes_bytes_without_draining_source_or_overwriting() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let staging = temp.path().join("staging");
        fs::create_dir_all(source.join("disc")).expect("source");
        fs::create_dir(&staging).expect("staging");
        fs::write(source.join("disc/file"), b"before").expect("source bytes");
        let source_dir = cap_std::fs::Dir::open_ambient_dir(&source, cap_std::ambient_authority())
            .expect("source root");
        let staging_dir =
            cap_std::fs::Dir::open_ambient_dir(&staging, cap_std::ambient_authority())
                .expect("stage root");
        let mut before = Vec::new();
        stream_custody_copy_observations(
            &source_dir,
            &staging_dir,
            || false,
            |entry| {
                if entry.path_key == "disc/file" {
                    fs::write(source.join("disc/file"), b"change").expect("concurrent mutation");
                }
                before.push((entry.path_key, entry.digest));
                Ok(())
            },
        )
        .expect("copy");
        assert_eq!(
            fs::read(source.join("disc/file")).expect("source retained"),
            b"change"
        );
        assert_eq!(
            fs::read(staging.join("disc/file")).expect("copied bytes"),
            b"before"
        );
        let mut after = Vec::new();
        stream_custody_hash_observations(
            &source_dir,
            || false,
            |entry| {
                after.push((entry.path_key, entry.digest));
                Ok(())
            },
        )
        .expect("source reobserve");
        before.sort_unstable();
        after.sort_unstable();
        assert_ne!(
            before, after,
            "a same-size change after copying must be detected by source-after"
        );
        assert!(
            stream_custody_copy_observations(&source_dir, &staging_dir, || false, |_| Ok(()))
                .is_err(),
            "existing staging files cannot be overwritten"
        );
        assert_eq!(
            fs::read(staging.join("disc/file")).expect("stage intact"),
            b"before"
        );
    }

    #[test]
    fn cancelled_copy_retains_source_and_partial_stage_for_recovery() {
        use std::cell::Cell;

        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let staging = temp.path().join("staging");
        fs::create_dir(&source).expect("source");
        fs::create_dir(&staging).expect("staging");
        let contents = vec![0x5a; 512 * 1024];
        fs::write(source.join("file"), &contents).expect("source bytes");
        let source_dir = cap_std::fs::Dir::open_ambient_dir(&source, cap_std::ambient_authority())
            .expect("source root");
        let staging_dir =
            cap_std::fs::Dir::open_ambient_dir(&staging, cap_std::ambient_authority())
                .expect("stage root");
        let checks = Cell::new(0);
        let result = stream_custody_copy_observations(
            &source_dir,
            &staging_dir,
            || {
                checks.set(checks.get() + 1);
                checks.get() >= 4
            },
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(source.join("file")).expect("source"), contents);
        let partial = fs::read(staging.join("file")).expect("owned partial");
        assert!(!partial.is_empty() && partial.len() < contents.len());
        assert!(contents.starts_with(&partial));
    }

    #[test]
    fn retained_reads_share_a_bounded_pace_and_cancellation_interrupts_wait() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    for _ in 0..4 {
                        pace_retained_read(64 * 1024, 1, &|| false).expect("pace");
                    }
                });
            }
        });
        assert!(
            start.elapsed() >= Duration::from_millis(400),
            "two callers must share one MiB/s timeline"
        );
        let cancelled = AtomicBool::new(false);
        let start = Instant::now();
        std::thread::scope(|scope| {
            let waiter = scope
                .spawn(|| pace_retained_read(64 * 1024, 1, &|| cancelled.load(Ordering::Acquire)));
            std::thread::sleep(Duration::from_millis(10));
            cancelled.store(true, Ordering::Release);
            assert!(waiter.join().expect("worker").is_err());
        });
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn retained_visitor_paces_real_file_reads() {
        use std::time::{Duration, Instant};
        let temp = tempfile::tempdir().expect("temp");
        fs::write(temp.path().join("file"), vec![0x5a; 256 * 1024]).expect("source");
        let root = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())
            .expect("root");
        let mut observations = 0;
        let start = Instant::now();
        let count = stream_custody_observations_inner(
            &root,
            None,
            Some(1),
            || false,
            |_| {
                observations += 1;
                Ok(())
            },
        )
        .expect("capture");
        assert_eq!((count, observations), (1, 1));
        assert!(
            start.elapsed() >= Duration::from_millis(180),
            "file reads must follow the shared rate"
        );
    }

    #[test]
    fn custody_runtime_shutdown_keeps_stalled_producer_owned_until_exit() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let reaper_permit = runtime
            .block_on(crate::reconcile::acquire_custody_reaper_permit())
            .expect("reaper");
        let budget = Arc::new(tokio::sync::Semaphore::new(1));
        let work_permit = runtime
            .block_on(budget.clone().acquire_owned())
            .expect("work permit");
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let (ready, ready_wait) = std::sync::mpsc::channel();
        let (release, release_wait) = std::sync::mpsc::channel();
        let terminated = Arc::new(AtomicBool::new(false));
        let terminated_thread = terminated.clone();
        let thread = std::thread::spawn(move || {
            let _permit = work_permit;
            ready.send(()).expect("ready");
            release_wait.recv().expect("release");
            assert!(
                sender.is_closed(),
                "cancelled consumer cannot receive observations"
            );
            terminated_thread.store(true, Ordering::Release);
            Ok(0_u64)
        });
        ready_wait.recv().expect("stalled");
        drop(CustodyProducer {
            receiver: Some(receiver),
            thread: Some(thread),
            reaper_permit: Some(reaper_permit),
        });
        let started = std::time::Instant::now();
        drop(runtime);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(
            budget.available_permits(),
            0,
            "permit stays with running producer"
        );
        assert!(!terminated.load(Ordering::Acquire));
        release.send(()).expect("release stalled producer");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while budget.available_permits() == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(budget.available_permits(), 1);
        assert!(terminated.load(Ordering::Acquire));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn mountinfo_rejects_submounts_and_decodes_escaped_paths() {
        let base = b"21 1 8:1 / / rw - ext4 /dev/test rw\n";
        assert!(!source_has_submount(base, Path::new("/inbound here")).expect("base"));
        let descendant = b"21 1 8:1 / / rw - ext4 /dev/test rw\n22 21 8:1 /other /inbound\\040here/alias rw - ext4 /dev/test rw\n";
        assert!(source_has_submount(descendant, Path::new("/inbound here")).expect("submount"));
        let sibling = b"22 21 8:1 /other /inbound\\040here-other rw - ext4 /dev/test rw\n";
        assert!(!source_has_submount(sibling, Path::new("/inbound here")).expect("sibling"));
        assert!(source_has_submount(b"malformed\n", Path::new("/inbound")).is_err());
        assert!(
            source_has_submount(
                b"22 21 8:1 / /inbound\\999bad rw - ext4 x rw\n",
                Path::new("/inbound")
            )
            .is_err()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn legacy_mount_identity_rules_reject_same_device_bind_aliases() {
        assert!(legacy_mount_identities_compatible(1, 1, 10, 10));
        assert!(legacy_mount_identities_compatible(1, 2, 10, 11));
        assert!(!legacy_mount_identities_compatible(1, 1, 10, 11));
        assert!(!legacy_mount_identities_compatible(1, 2, 10, 10));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires two owned, supported, different-device directories"]
    fn legacy_lifecycle_accepts_owned_distinct_local_filesystems() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let workspace = std::env::current_dir().expect("workspace");
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("private runtime parent");
        let source_fixture = tempfile::tempdir_in(&workspace).expect("owned source fixture");
        let private_fixture = tempfile::tempdir_in(runtime).expect("owned private fixture");
        let source = source_fixture.path().join("source");
        let private = private_fixture.path().join("private");
        fs::create_dir(&source).expect("source");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private mode");
        fs::write(source.join("keep"), b"original").expect("source bytes");
        assert_ne!(
            fs::metadata(&source).expect("source device").dev(),
            fs::metadata(&private).expect("private device").dev(),
            "the owned fixture must actually use different devices"
        );
        let uid = rustix::process::geteuid().as_raw();
        let producer_uid = if uid == 1 { 2 } else { 1 };
        assert!(
            validate_custody_namespace(&private, &source, producer_uid).is_err(),
            "rename custody must remain same-mount only"
        );
        let namespace = std::sync::Arc::new(
            validate_legacy_lifecycle_namespace(&private, &source, producer_uid)
                .expect("supported distinct local devices"),
        );
        let root = GuardedRoot::open(
            InboundSource::from_root(source.clone(), None, Some("cross-device".into()))
                .expect("source config"),
        )
        .expect("guarded source");
        let capability =
            LegacyLifecycleCapability::create_reserved(namespace, root, uuid::Uuid::now_v7())
                .expect("private lifecycle lock");
        capability
            .bind_other_roots(std::slice::from_ref(&source))
            .expect("bind source topology");
        capability
            .revalidate()
            .expect("revalidate distinct devices");
        assert_eq!(
            fs::read(source.join("keep")).expect("source retained"),
            b"original"
        );
        eprintln!("legacy lifecycle different-device assertion executed");
    }

    #[cfg(all(feature = "test-support", target_os = "linux"))]
    #[tokio::test]
    #[ignore = "requires two owned, supported, different-device directories"]
    async fn maintenance_provisions_owned_distinct_local_filesystems() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let workspace = std::env::current_dir().expect("workspace");
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("private runtime parent");
        let source_fixture = tempfile::tempdir_in(&workspace).expect("owned source fixture");
        let private_fixture = tempfile::tempdir_in(runtime).expect("owned private fixture");
        let inbound = source_fixture.path().join("inbound");
        let staging = source_fixture.path().join("staging");
        let work = source_fixture.path().join("work");
        let canonical = source_fixture.path().join("canonical");
        let assets = source_fixture.path().join("assets");
        let private = private_fixture.path().join("private");
        for path in [&inbound, &staging, &work, &canonical, &assets, &private] {
            fs::create_dir(path).expect("owned directory");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private mode");
        }
        assert_ne!(
            fs::metadata(&inbound).expect("source device").dev(),
            fs::metadata(&private).expect("lock device").dev(),
            "the owned fixture must actually use different devices"
        );
        fs::write(inbound.join("keep"), b"inbound").expect("source bytes");
        fs::write(staging.join("keep"), b"stage").expect("stage bytes");
        fs::write(assets.join("keep"), b"asset").expect("asset bytes");
        let source = InboundSource::from_root(inbound.clone(), None, Some("cross-device".into()))
            .expect("source");
        let mut config = crate::config::IngestionConfig::with_sources(vec![source]);
        let uid = rustix::process::geteuid().as_raw();
        let producer_uid = if uid == 1 { 2 } else { 1 };
        config.protected_custody = Some(crate::config::ProtectedCustodyConfig {
            private_root: private.clone(),
            producer_uid,
        });
        config.staging_root = Some(staging.clone());
        config.work_area_base = work;
        config.storage_root = Some(canonical);
        assert!(
            validate_custody_namespace(&private, &inbound, producer_uid).is_err(),
            "rename custody remains same-mount only"
        );
        let db = crate::common::TestDb::create("lifecycle_cross_device").await;
        let store = melodee_persistence::WorkItemStore;
        let item = melodee_domain::ReleaseWorkItem::discovered(
            "candidate".to_owned(),
            None,
            uuid::Uuid::now_v7(),
            time::OffsetDateTime::now_utc(),
        );
        store.create(&db.pool, &item).await.expect("item");
        let fence = store
            .begin_lifecycle_maintenance(&db.pool, "owned-test", time::Duration::minutes(2))
            .await
            .expect("fence");
        store
            .confirm_lifecycle_quiescence(
                &db.pool,
                &fence,
                &melodee_persistence::LifecycleQuiescenceAttestation {
                    reason: "owned test",
                    backup_reference: "owned test fixture",
                    stop_all_writers: true,
                    inspection_complete: true,
                },
            )
            .await
            .expect("owned quiescence");
        assert_eq!(
            provision_legacy_lifecycle_once_with_asset_root(
                &config,
                &store,
                &db.pool,
                &fence,
                Some(assets.clone()),
            )
            .await
            .expect("provision"),
            LegacyLifecycleProvisionOutcome::Ready(item.id)
        );
        assert!(
            private
                .join(format!("work-{}.lock", item.id.simple()))
                .is_file()
        );
        assert_eq!(
            fs::read(inbound.join("keep")).expect("inbound retained"),
            b"inbound"
        );
        assert_eq!(
            fs::read(staging.join("keep")).expect("stage retained"),
            b"stage"
        );
        assert_eq!(
            fs::read(assets.join("keep")).expect("asset retained"),
            b"asset"
        );
        eprintln!("legacy provisioning different-device assertion executed");
        db.drop().await;
    }

    #[cfg(target_os = "linux")]
    fn run_only_with_private_mounts<T>(
        mountinfo: &[u8],
        command: impl FnOnce() -> T,
    ) -> Result<T, &'static str> {
        if mountinfo.is_empty() || mountinfo.len() > 8 * 1024 * 1024 {
            return Err("mount topology unavailable or too large");
        }
        let mut saw_mount = false;
        for line in mountinfo
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            saw_mount = true;
            let mut fields = line.split(|byte| *byte == b' ');
            for _ in 0..6 {
                if fields.next().is_none_or(|field| field.is_empty()) {
                    return Err("malformed mount topology");
                }
            }
            let mut separator = false;
            for field in fields.by_ref() {
                if field == b"-" {
                    separator = true;
                    break;
                }
                if field.starts_with(b"shared:")
                    || field.starts_with(b"master:")
                    || field.starts_with(b"propagate_from:")
                {
                    return Err("mount propagation is not private");
                }
                if field != b"unbindable" {
                    return Err("unknown mount propagation field");
                }
            }
            if !separator || (0..3).any(|_| fields.next().is_none_or(|field| field.is_empty())) {
                return Err("malformed mount topology");
            }
        }
        if !saw_mount {
            return Err("mount topology is empty");
        }
        Ok(command())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unsafe_mount_propagation_never_runs_the_command() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for data in [
            b"21 1 8:1 / / rw shared:2 - ext4 /dev/test rw\n".as_slice(),
            b"21 1 8:1 / / rw master:2 - ext4 /dev/test rw\n".as_slice(),
            b"21 1 8:1 / / rw propagate_from:2 - ext4 /dev/test rw\n".as_slice(),
            b"21 1 8:1 / / rw mystery:2 - ext4 /dev/test rw\n".as_slice(),
            b"malformed\n".as_slice(),
            b"\n".as_slice(),
            b"".as_slice(),
        ] {
            let invoked = AtomicBool::new(false);
            assert!(
                run_only_with_private_mounts(data, || invoked.store(true, Ordering::SeqCst))
                    .is_err()
            );
            assert!(!invoked.load(Ordering::SeqCst));
        }
        let invoked = AtomicBool::new(false);
        let oversized = vec![b'x'; 8 * 1024 * 1024 + 1];
        assert!(
            run_only_with_private_mounts(&oversized, || invoked.store(true, Ordering::SeqCst))
                .is_err()
        );
        assert!(!invoked.load(Ordering::SeqCst));
        let invoked = AtomicBool::new(false);
        run_only_with_private_mounts(b"21 1 8:1 / / rw - ext4 /dev/test rw\n", || {
            invoked.store(true, Ordering::SeqCst);
        })
        .expect("private mount topology");
        assert!(invoked.load(Ordering::SeqCst));
    }

    #[cfg(target_os = "linux")]
    fn run_private_mount_command(
        command: &mut std::process::Command,
    ) -> std::io::Result<std::process::ExitStatus> {
        use std::io::Read;
        let mountinfo = File::open("/proc/self/mountinfo")?;
        let fs = rustix::fs::fstatfs(&mountinfo)
            .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
        if fs.f_type as u32 != 0x9fa0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mountinfo is not procfs",
            ));
        }
        let mut data = Vec::new();
        mountinfo.take(8 * 1024 * 1024 + 1).read_to_end(&mut data)?;
        run_only_with_private_mounts(&data, || command.status())
            .map_err(|reason| std::io::Error::new(std::io::ErrorKind::InvalidData, reason))?
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "run only in a private user and mount namespace"]
    fn private_namespace_bind_aliases_are_rejected() {
        use std::{path::PathBuf, process::Command};

        let mapping = fs::read_to_string("/proc/self/uid_map").expect("uid map");
        let entries: Vec<_> = mapping.split_whitespace().collect();
        let outside_uid: u32 = entries.get(1).expect("outside uid").parse().expect("uid");
        assert_eq!(entries.len(), 3, "must have exactly one uid mapping");
        assert_eq!(entries[0], "0");
        assert_ne!(outside_uid, 0, "must map an unprivileged host uid");
        assert_eq!(entries[2], "1");
        let initial_namespace = std::env::var_os("MELODEE_TEST_ORIGINAL_MOUNT_NS")
            .expect("launcher must record its original mount namespace");
        assert_ne!(
            fs::read_link("/proc/self/ns/mnt")
                .expect("mount namespace")
                .as_os_str(),
            initial_namespace,
            "refuse to mount outside a private namespace"
        );
        struct Mounted(Option<PathBuf>);
        impl Mounted {
            fn unmount(mut self) {
                let target = self.0.as_ref().expect("mount target");
                assert!(
                    run_private_mount_command(Command::new("umount").arg(target))
                        .expect("umount")
                        .success()
                );
                self.0 = None;
            }
        }
        impl Drop for Mounted {
            fn drop(&mut self) {
                if let Some(target) = &self.0 {
                    let _ = run_private_mount_command(Command::new("umount").arg(target));
                }
            }
        }
        let temp = tempfile::tempdir().expect("owned fixture");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        let alias = temp.path().join("alias");
        fs::create_dir_all(source.join("inside")).expect("source");
        fs::create_dir(&private).expect("private");
        fs::create_dir(&alias).expect("alias");
        assert!(
            run_private_mount_command(
                Command::new("mount")
                    .args(["--bind"])
                    .arg(source.join("inside"))
                    .arg(&alias)
            )
            .expect("mount")
            .success()
        );
        let alias_mount = Mounted(Some(alias.clone()));
        let source_fd = open_directory(&source).expect("source");
        let alias_fd = open_directory(&alias).expect("alias");
        assert!(custody_mount_topology(&source_fd, &alias_fd, &source).is_err());
        let private_fd = open_directory(&private).expect("private");
        let topology = custody_mount_topology(&source_fd, &private_fd, &source)
            .expect("source/private topology");
        let namespace = CustodyNamespace {
            source_path: source.clone(),
            private_path: private.clone(),
            producer_uid: 1,
            source: source_fd.try_clone().expect("source handle"),
            private: private_fd,
            topology,
            policy: CustodyTopologyPolicy::LegacyLocalMounts,
        };
        assert!(
            validate_legacy_other_roots(&namespace, std::slice::from_ref(&alias)).is_err(),
            "a bind-mounted data-root alias must fail the lifecycle topology check"
        );
        drop(namespace);
        drop(alias_fd);
        alias_mount.unmount();
        let nested_alias = source.join("nested-alias");
        fs::create_dir(&nested_alias).expect("nested alias");
        assert!(
            run_private_mount_command(
                Command::new("mount")
                    .args(["--bind"])
                    .arg(&private)
                    .arg(&nested_alias)
            )
            .expect("mount")
            .success()
        );
        let nested_mount = Mounted(Some(nested_alias));
        let private_fd = open_directory(&private).expect("private");
        assert!(custody_mount_topology(&source_fd, &private_fd, &source).is_err());
        nested_mount.unmount();
    }

    #[test]
    fn interrupted_move_can_be_inspected_from_the_deterministic_custody_name() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        fs::create_dir_all(source.join("candidate")).expect("candidate");
        fs::write(source.join("candidate/file"), b"held bytes").expect("file");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let guarded = GuardedRoot::open(
            InboundSource::from_root(source.clone(), None, Some("test".into())).expect("source"),
        )
        .expect("root");
        let source_parent = open_directory(&source).expect("source parent");
        let private_parent = open_directory(&private).expect("private parent");
        let original = open_child_directory(&source_parent, OsStr::new("candidate"))
            .expect("candidate handle");
        let generation = source_generation_from_directory(&original, Path::new("candidate"))
            .expect("generation");
        let hash = tree_hash_from_pinned_directory(&original).expect("hash");
        let error = quarantine_verified_occurrence_with_hook(
            &guarded,
            &source_parent,
            OsStr::new("candidate"),
            &original,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
            || {
                Err(AcquisitionError::Task(
                    "injected post-move interruption".to_owned(),
                ))
            },
        );
        assert!(error.is_err());
        assert!(!source.join("candidate").exists());
        match inspect_pending_custody(
            &guarded,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
        )
        .expect("inspect held entry")
        {
            PendingCustodyObservation::Matching { held, legacy_hash } => {
                assert!(!legacy_hash.is_empty());
                assert_eq!(
                    source_generation_from_directory(&held, Path::new("held")).expect("generation"),
                    generation
                );
            }
            _ => unreachable!("held occurrence must be recovered for further verification"),
        }
        assert_eq!(
            fs::read(private.join("held/file")).expect("held bytes"),
            b"held bytes"
        );
        fs::write(private.join("held/file"), b"new  bytes").expect("mutation");
        match inspect_pending_custody(
            &guarded,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
        )
        .expect("inspect changed entry")
        {
            PendingCustodyObservation::Different { held } => {
                assert!(held.metadata().expect("held metadata").is_dir());
            }
            _ => unreachable!("changed custody must remain held for review"),
        }
        assert_eq!(
            fs::read(private.join("held/file")).expect("changed bytes"),
            b"new  bytes"
        );
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
pub(crate) fn quarantine_verified_occurrence(
    _guarded_root: &GuardedRoot,
    _source_parent: &File,
    _source_name: &OsStr,
    _original: &File,
    _private_namespace: &CustodyNamespace,
    _private_name: &OsStr,
    _expected_generation: &str,
    _expected_tree_hash: &str,
) -> Result<QuarantineOutcome, AcquisitionError> {
    Err(AcquisitionError::Task(
        io::Error::new(
            io::ErrorKind::Unsupported,
            "descriptor-relative custody unavailable",
        )
        .to_string(),
    ))
}

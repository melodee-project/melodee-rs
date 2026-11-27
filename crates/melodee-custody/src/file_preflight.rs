//! Bounded, read-only content checking. Results are not deletion evidence.

use rustix::fs::{Mode, OFlags, openat};
use sha2::Digest;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, Read},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf},
};

const MAX_PAGE: usize = 16;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DEPTH: usize = 256;
const MAX_STEP_BYTES: usize = 8 * 1024 * 1024;

/// The expected digest over complete file bytes.
#[derive(Clone, Copy)]
pub enum ExpectedHash {
    Blake3([u8; 32]),
    Sha256([u8; 32]),
}

/// An untrusted relative name and expected content. This is not an intent.
pub struct ExpectedFile<'a> {
    pub path: &'a Path,
    pub size: u64,
    pub hash: ExpectedHash,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn os_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

fn no_dot_segments(path: &Path) -> bool {
    !path
        .as_os_str()
        .as_bytes()
        .split(|byte| *byte == b'/')
        .any(|part| part == b"." || part == b"..")
}

fn open_root(path: &Path) -> io::Result<File> {
    if !path.is_absolute() || !no_dot_segments(path) || path.components().count() > MAX_DEPTH {
        return Err(invalid("invalid root"));
    }
    let mut current = File::from(
        openat(
            rustix::fs::CWD,
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(os_error)?,
    );
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = File::from(
                    openat(
                        &current,
                        name.as_bytes(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(os_error)?,
                );
            }
            _ => return Err(invalid("invalid root component")),
        }
    }
    Ok(current)
}

fn open_file(root: &File, path: &Path) -> io::Result<File> {
    if path.is_absolute() || !no_dot_segments(path) || path.components().count() > MAX_DEPTH {
        return Err(invalid("invalid relative path"));
    }
    let mut parts = path.components().peekable();
    let mut parent = root.try_clone()?;
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(invalid("invalid path component"));
        };
        let flags = if parts.peek().is_some() {
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
        } else {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
        };
        let file =
            File::from(openat(&parent, name.as_bytes(), flags, Mode::empty()).map_err(os_error)?);
        if parts.peek().is_none() {
            return Ok(file);
        }
        parent = file;
    }
    Err(invalid("empty relative path"))
}

fn open_root_walk(path: &Path) -> io::Result<Vec<File>> {
    if !path.is_absolute() || !no_dot_segments(path) || path.components().count() > MAX_DEPTH {
        return Err(invalid("invalid root"));
    }
    let root = File::from(
        openat(
            rustix::fs::CWD,
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(os_error)?,
    );
    let mut handles = vec![root];
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                let parent = handles.last().ok_or_else(|| invalid("missing root"))?;
                handles.push(File::from(
                    openat(
                        parent,
                        name.as_bytes(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(os_error)?,
                ));
            }
            _ => return Err(invalid("invalid root component")),
        }
    }
    Ok(handles)
}

fn open_file_walk(root: &File, path: &Path) -> io::Result<Vec<File>> {
    if path.is_absolute() || !no_dot_segments(path) || path.components().count() > MAX_DEPTH {
        return Err(invalid("invalid relative path"));
    }
    let mut parts = path.components().peekable();
    let mut handles = vec![root.try_clone()?];
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(invalid("invalid path component"));
        };
        let flags = if parts.peek().is_some() {
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
        } else {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
        };
        let parent = handles.last().ok_or_else(|| invalid("missing parent"))?;
        handles.push(File::from(
            openat(parent, name.as_bytes(), flags, Mode::empty()).map_err(os_error)?,
        ));
    }
    if handles.len() < 2 {
        return Err(invalid("empty relative path"));
    }
    Ok(handles)
}

fn recheck_walk(
    mut current: File,
    path: &Path,
    held: &[File],
    final_is_file: bool,
) -> io::Result<File> {
    let mut index = 0;
    let compare = |file: &File, index: usize| -> io::Result<()> {
        let old = held
            .get(index)
            .ok_or_else(|| invalid("walk changed"))?
            .metadata()?;
        let now = file.metadata()?;
        if (old.dev(), old.ino()) != (now.dev(), now.ino()) {
            return Err(invalid("walk changed"));
        }
        Ok(())
    };
    compare(&current, index)?;
    let mut parts = path.components().peekable();
    while let Some(part) = parts.next() {
        match part {
            Component::RootDir if !final_is_file => continue,
            Component::Normal(name) => {
                let is_file = final_is_file && parts.peek().is_none();
                let flags = OFlags::RDONLY
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC
                    | if is_file {
                        OFlags::NONBLOCK
                    } else {
                        OFlags::DIRECTORY
                    };
                current = File::from(
                    openat(&current, name.as_bytes(), flags, Mode::empty()).map_err(os_error)?,
                );
                index += 1;
                compare(&current, index)?;
            }
            _ => return Err(invalid("walk changed")),
        }
    }
    if index + 1 != held.len() {
        return Err(invalid("walk changed"));
    }
    Ok(current)
}

fn same_file_state(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    (
        left.dev(),
        left.ino(),
        left.len(),
        left.mtime(),
        left.mtime_nsec(),
        left.ctime(),
        left.ctime_nsec(),
    ) == (
        right.dev(),
        right.ino(),
        right.len(),
        right.mtime(),
        right.mtime_nsec(),
        right.ctime(),
        right.ctime_nsec(),
    )
}

enum RunningHash {
    Blake3(Box<blake3::Hasher>),
    Sha256(sha2::Sha256),
}

/// Lifetime and revalidation hook for observational scans only. Any caller can
/// implement this trait, so it must never be used as a deletion proof.
pub trait ObservationScope: Send {
    fn revalidate_observation_scope(&self) -> io::Result<()>;
}

impl ObservationScope for crate::HeldLifecycleLocks {
    fn revalidate_observation_scope(&self) -> io::Result<()> {
        self.revalidate()
    }
}

/// Progress within one live process. A completed check is only an observation,
/// never a snapshot proof or permission to remove a file.
#[derive(Debug, PartialEq, Eq)]
pub enum IncrementalFileProgress {
    More { bytes_read: u64 },
    Complete,
}

/// Holds a unique lock scope and every path descriptor until completion/drop.
/// It is not serializable: cancellation, a failed fence, or process death
/// discards the hash state and requires a fresh scan from byte zero.
pub struct IncrementalFileCheck<S: ObservationScope> {
    scope: S,
    root_path: PathBuf,
    relative_path: PathBuf,
    root_walk: Vec<File>,
    file_walk: Vec<File>,
    initial: std::fs::Metadata,
    expected: ExpectedHash,
    hash: RunningHash,
    read: u64,
    poisoned: bool,
    complete: bool,
}

impl<S: ObservationScope> IncrementalFileCheck<S> {
    /// Recover the owned scope after a complete observational scan. This does
    /// not return a proof or authorize a filesystem or database mutation.
    pub fn into_scope(self) -> io::Result<S> {
        if !self.complete || self.poisoned {
            return Err(invalid("scan is incomplete"));
        }
        Ok(self.scope)
    }

    /// Start a read-only scan. This does not validate a database intent or
    /// establish that external writers have stopped.
    pub fn begin(scope: S, root: &Path, expected: ExpectedFile<'_>) -> io::Result<Self> {
        scope.revalidate_observation_scope()?;
        let root_walk = open_root_walk(root)?;
        let root_file = root_walk.last().ok_or_else(|| invalid("missing root"))?;
        let file_walk = open_file_walk(root_file, expected.path)?;
        let initial = file_walk
            .last()
            .ok_or_else(|| invalid("missing file"))?
            .metadata()?;
        if !initial.is_file() || initial.len() != expected.size {
            return Err(invalid("unexpected file size or type"));
        }
        let hash = match expected.hash {
            ExpectedHash::Blake3(_) => RunningHash::Blake3(Box::new(blake3::Hasher::new())),
            ExpectedHash::Sha256(_) => RunningHash::Sha256(sha2::Sha256::new()),
        };
        let check = Self {
            scope,
            root_path: root.to_path_buf(),
            relative_path: expected.path.to_path_buf(),
            root_walk,
            file_walk,
            initial,
            expected: expected.hash,
            hash,
            read: 0,
            poisoned: false,
            complete: false,
        };
        check.revalidate()?;
        Ok(check)
    }

    fn revalidate(&self) -> io::Result<()> {
        self.scope.revalidate_observation_scope()?;
        let root = File::from(
            openat(
                rustix::fs::CWD,
                "/",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(os_error)?,
        );
        let root = recheck_walk(root, &self.root_path, &self.root_walk, false)?;
        let current = recheck_walk(root, &self.relative_path, &self.file_walk, true)?;
        if !same_file_state(&self.initial, &current.metadata()?) {
            return Err(invalid("file changed"));
        }
        Ok(())
    }

    /// Read no more than `byte_budget` bytes. The caller's fence check is
    /// invoked before and after each step; it is not itself DB authority.
    pub fn step(
        &mut self,
        byte_budget: usize,
        mut check_fence: impl FnMut() -> io::Result<()>,
        mut cancelled: impl FnMut() -> bool,
    ) -> io::Result<IncrementalFileProgress> {
        if self.poisoned || self.complete || byte_budget == 0 || byte_budget > MAX_STEP_BYTES {
            return Err(invalid("invalid scan state or budget"));
        }
        self.poisoned = true;
        check_fence()?;
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        self.revalidate()?;
        let mut remaining = byte_budget;
        let mut buffer = [0_u8; 64 * 1024];
        while remaining > 0 && self.read < self.initial.len() {
            if cancelled() {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let allowed = remaining
                .min(buffer.len())
                .min((self.initial.len() - self.read).min(usize::MAX as u64) as usize);
            let count = self
                .file_walk
                .last_mut()
                .ok_or_else(|| invalid("missing file"))?
                .read(&mut buffer[..allowed])?;
            if count == 0 {
                return Err(invalid("unexpected end of file"));
            }
            match &mut self.hash {
                RunningHash::Blake3(hash) => {
                    hash.update(&buffer[..count]);
                }
                RunningHash::Sha256(hash) => hash.update(&buffer[..count]),
            }
            self.read += count as u64;
            remaining -= count;
        }
        self.revalidate()?;
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        check_fence()?;
        if self.read == self.initial.len() {
            let matches = match (&self.hash, self.expected) {
                (RunningHash::Blake3(hash), ExpectedHash::Blake3(expected)) => {
                    hash.finalize().as_bytes() == &expected
                }
                (RunningHash::Sha256(hash), ExpectedHash::Sha256(expected)) => {
                    <[u8; 32]>::from(hash.clone().finalize()) == expected
                }
                _ => false,
            };
            if !matches {
                return Err(invalid("file content differs"));
            }
            self.complete = true;
            self.poisoned = false;
            Ok(IncrementalFileProgress::Complete)
        } else {
            self.poisoned = false;
            Ok(IncrementalFileProgress::More {
                bytes_read: self.read,
            })
        }
    }
}

/// Read and hash at most 16 files within an explicit byte budget. Failure has
/// no filesystem side effects. Success is only a transient observation: it
/// does not establish a stable snapshot, authorize unlinking, or acknowledge
/// an administrative intent. Callers must provide admission and keep resources
/// held until an in-flight blocking read actually returns after cancellation.
pub fn preflight_file_page(
    root: &Path,
    files: &[ExpectedFile<'_>],
    byte_budget: u64,
    mut cancelled: impl FnMut() -> bool,
) -> io::Result<()> {
    if files.is_empty() || files.len() > MAX_PAGE || byte_budget == 0 || byte_budget > MAX_BYTES {
        return Err(invalid("invalid page budget"));
    }
    let mut seen = BTreeSet::new();
    let total = files
        .iter()
        .try_fold(0_u64, |sum, entry| {
            if !seen.insert(entry.path) {
                return None;
            }
            sum.checked_add(entry.size)
        })
        .ok_or_else(|| invalid("duplicate path or oversized page"))?;
    if total > byte_budget {
        return Err(invalid("page exceeds byte budget"));
    }
    let root = open_root(root)?;
    for entry in files {
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let mut file = open_file(&root, entry.path)?;
        let before = file.metadata()?;
        if !before.is_file() || before.len() != entry.size {
            return Err(invalid("unexpected file size or type"));
        }
        let mut blake = blake3::Hasher::new();
        let mut sha = sha2::Sha256::new();
        let mut bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if cancelled() {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes = bytes
                .checked_add(count as u64)
                .ok_or_else(|| invalid("file too large"))?;
            if bytes > entry.size {
                return Err(invalid("file grew while reading"));
            }
            match entry.hash {
                ExpectedHash::Blake3(_) => {
                    blake.update(&buffer[..count]);
                }
                ExpectedHash::Sha256(_) => sha.update(&buffer[..count]),
            }
        }
        let after = file.metadata()?;
        if cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if bytes != entry.size
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
            return Err(invalid("file changed while reading"));
        }
        let matches = match entry.hash {
            ExpectedHash::Blake3(expected) => blake.finalize().as_bytes() == &expected,
            ExpectedHash::Sha256(expected) => <[u8; 32]>::from(sha.finalize()) == expected,
        };
        if !matches {
            return Err(invalid("file content differs"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn hashes_exact_bytes_and_rejects_changed_symlink_or_over_budget() {
        let fixture = tempfile::tempdir().expect("owned fixture");
        fs::write(fixture.path().join("file"), b"original").expect("owned file");
        let blake = ExpectedFile {
            path: Path::new("file"),
            size: 8,
            hash: ExpectedHash::Blake3(*blake3::hash(b"original").as_bytes()),
        };
        preflight_file_page(fixture.path(), std::slice::from_ref(&blake), 8, || false)
            .expect("exact blake3");
        assert!(preflight_file_page(fixture.path(), &[], 8, || false).is_err());
        assert!(
            preflight_file_page(fixture.path(), std::slice::from_ref(&blake), 0, || false).is_err()
        );
        let many: Vec<_> = (0..17)
            .map(|_| ExpectedFile {
                path: Path::new("file"),
                size: 8,
                hash: ExpectedHash::Blake3(*blake3::hash(b"original").as_bytes()),
            })
            .collect();
        assert!(preflight_file_page(fixture.path(), &many, MAX_BYTES, || false).is_err());
        let sha = ExpectedFile {
            path: Path::new("file"),
            size: 8,
            hash: ExpectedHash::Sha256(sha2::Sha256::digest(b"original").into()),
        };
        preflight_file_page(fixture.path(), &[sha], 8, || false).expect("exact sha256");
        assert!(
            preflight_file_page(fixture.path(), std::slice::from_ref(&blake), 7, || false).is_err()
        );
        fs::write(fixture.path().join("file"), b"modified").expect("same length change");
        assert!(
            preflight_file_page(fixture.path(), std::slice::from_ref(&blake), 8, || false).is_err()
        );
        symlink("file", fixture.path().join("link")).expect("owned symlink");
        let link = ExpectedFile {
            path: Path::new("link"),
            size: 8,
            hash: ExpectedHash::Blake3(*blake3::hash(b"modified").as_bytes()),
        };
        assert!(preflight_file_page(fixture.path(), &[link], 8, || false).is_err());
        let traversal = ExpectedFile {
            path: Path::new("../file"),
            size: 8,
            hash: ExpectedHash::Blake3([0; 32]),
        };
        assert!(preflight_file_page(fixture.path(), &[traversal], 8, || false).is_err());
        assert_eq!(
            fs::read(fixture.path().join("file")).expect("content preserved"),
            b"modified"
        );
    }

    #[test]
    fn cancellation_during_bounded_hash_leaves_bytes_untouched() {
        let fixture = tempfile::tempdir().expect("owned fixture");
        let bytes = vec![0x51; 256 * 1024];
        fs::write(fixture.path().join("file"), &bytes).expect("owned bytes");
        let entry = ExpectedFile {
            path: Path::new("file"),
            size: bytes.len() as u64,
            hash: ExpectedHash::Blake3(*blake3::hash(&bytes).as_bytes()),
        };
        let mut polls = 0;
        let error = preflight_file_page(fixture.path(), &[entry], bytes.len() as u64, || {
            polls += 1;
            polls >= 3
        })
        .expect_err("cancel hashing");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(
            fs::read(fixture.path().join("file")).expect("retained bytes"),
            bytes
        );
    }

    #[test]
    fn incremental_check_matches_full_hash_and_poisoned_fences_cannot_resume() {
        use std::os::unix::fs::PermissionsExt;
        fn generation(path: &Path) -> String {
            let meta = fs::metadata(path).expect("fixture identity");
            format!("unix:{}:{}", meta.dev(), meta.ino())
        }
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let locks = fixture.path().join("locks");
        let data = fixture.path().join("data");
        for dir in [&locks, &data] {
            fs::create_dir(dir).expect("owned directory");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("private directory");
        }
        let item = uuid::Uuid::now_v7();
        let lock = locks.join(format!("work-{}.lock", item.simple()));
        fs::write(&lock, b"lock").expect("owned lock");
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
        let identity = crate::LifecycleLockIdentity {
            work_item_id: item,
            lock_root_generation: generation(&locks),
            lock_generation: generation(&lock),
        };
        let bytes = vec![0x61; 150_000];
        fs::write(data.join("file"), &bytes).expect("owned bytes");
        for hash in [
            ExpectedHash::Blake3(*blake3::hash(&bytes).as_bytes()),
            ExpectedHash::Sha256(sha2::Sha256::digest(&bytes).into()),
        ] {
            let scope = crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
                .expect("scope");
            let expected = ExpectedFile {
                path: Path::new("file"),
                size: bytes.len() as u64,
                hash,
            };
            let mut scan = IncrementalFileCheck::begin(scope, &data, expected).expect("begin");
            assert_eq!(
                scan.step(64 * 1024, || Ok(()), || false)
                    .expect("first step"),
                IncrementalFileProgress::More { bytes_read: 65_536 }
            );
            assert!(
                crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
                    .is_err()
            );
            assert_eq!(
                scan.step(64 * 1024, || Ok(()), || false)
                    .expect("second step"),
                IncrementalFileProgress::More {
                    bytes_read: 131_072
                }
            );
            assert_eq!(
                scan.step(64 * 1024, || Ok(()), || false).expect("complete"),
                IncrementalFileProgress::Complete
            );
            assert!(scan.step(1, || Ok(()), || false).is_err());
            let scope = scan
                .into_scope()
                .expect("recover owned scope after observation");
            assert!(
                crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
                    .is_err()
            );
            drop(scope);
        }
        let scope = crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
            .expect("scope after completion");
        let expected = ExpectedFile {
            path: Path::new("file"),
            size: bytes.len() as u64,
            hash: ExpectedHash::Blake3(*blake3::hash(&bytes).as_bytes()),
        };
        let mut scan = IncrementalFileCheck::begin(scope, &data, expected).expect("begin again");
        scan.step(64 * 1024, || Ok(()), || false)
            .expect("scan prefix");
        let mut changed = bytes.clone();
        changed[0] ^= 1;
        fs::write(data.join("file"), &changed).expect("same-length prefix mutation");
        assert!(scan.step(64 * 1024, || Ok(()), || false).is_err());
        assert!(
            scan.step(64 * 1024, || Ok(()), || false).is_err(),
            "failed scans cannot resume"
        );
        assert_eq!(
            fs::read(data.join("file")).expect("preserved bytes"),
            changed
        );
        drop(scan);
        let scope = crate::HeldLifecycleLocks::acquire(&locks, &[identity])
            .expect("scope after failed scan");
        let expected = ExpectedFile {
            path: Path::new("file"),
            size: changed.len() as u64,
            hash: ExpectedHash::Blake3(*blake3::hash(&changed).as_bytes()),
        };
        let mut scan =
            IncrementalFileCheck::begin(scope, &data, expected).expect("begin fence test");
        let mut checks = 0;
        assert!(
            scan.step(
                1024,
                || {
                    checks += 1;
                    if checks == 2 {
                        Err(invalid("stale fence"))
                    } else {
                        Ok(())
                    }
                },
                || false
            )
            .is_err()
        );
        assert_eq!(checks, 2);
        assert!(
            scan.step(1024, || Ok(()), || false).is_err(),
            "a stale fence poisons the scan"
        );
    }

    #[test]
    fn incremental_check_completes_owned_sparse_file_over_page_limit() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let locks = fixture.path().join("locks");
        let data = fixture.path().join("data");
        for dir in [&locks, &data] {
            fs::create_dir(dir).expect("owned directory");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("private directory");
        }
        let item = uuid::Uuid::now_v7();
        let lock = locks.join(format!("work-{}.lock", item.simple()));
        fs::write(&lock, b"lock").expect("owned lock");
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
        let identity_of = |path: &Path| {
            let metadata = fs::metadata(path).expect("identity");
            format!("unix:{}:{}", metadata.dev(), metadata.ino())
        };
        let identity = crate::LifecycleLockIdentity {
            work_item_id: item,
            lock_root_generation: identity_of(&locks),
            lock_generation: identity_of(&lock),
        };
        let size = MAX_BYTES + 1024 * 1024;
        let file = fs::File::create(data.join("sparse")).expect("owned sparse file");
        file.set_len(size).expect("sparse length");
        drop(file);
        let zero = [0_u8; 1024 * 1024];
        let mut expected_hash = blake3::Hasher::new();
        for _ in 0..(size / zero.len() as u64) {
            expected_hash.update(&zero);
        }
        let scope = crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
            .expect("scope");
        let expected = ExpectedFile {
            path: Path::new("sparse"),
            size,
            hash: ExpectedHash::Blake3(*expected_hash.finalize().as_bytes()),
        };
        let mut scan =
            IncrementalFileCheck::begin(scope, &data, expected).expect("begin large file");
        let mut steps = 0;
        let mut checks = 0;
        loop {
            steps += 1;
            match scan
                .step(
                    MAX_STEP_BYTES,
                    || {
                        checks += 1;
                        Ok(())
                    },
                    || false,
                )
                .expect("bounded step")
            {
                IncrementalFileProgress::More { .. } => {}
                IncrementalFileProgress::Complete => break,
            }
        }
        assert_eq!(steps, 33);
        assert_eq!(checks, 66);
        assert!(
            crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity)).is_err()
        );
        assert_eq!(
            fs::metadata(data.join("sparse"))
                .expect("source retained")
                .len(),
            size
        );
    }

    #[test]
    fn incremental_scan_revalidates_every_lock_in_multi_item_scope() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let locks = fixture.path().join("locks");
        let data = fixture.path().join("data");
        for directory in [&locks, &data] {
            fs::create_dir(directory).expect("owned directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("private mode");
        }
        let generation = |path: &Path| {
            let m = fs::metadata(path).expect("metadata");
            format!("unix:{}:{}", m.dev(), m.ino())
        };
        let ids = [uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2)];
        let mut identities = Vec::new();
        for id in ids {
            let lock = locks.join(format!("work-{}.lock", id.simple()));
            fs::write(&lock, b"owned lock").expect("lock file");
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
            identities.push(crate::LifecycleLockIdentity {
                work_item_id: id,
                lock_root_generation: generation(&locks),
                lock_generation: generation(&lock),
            });
        }
        fs::write(data.join("file"), b"observation").expect("source");
        let scope =
            crate::HeldLifecycleLocks::acquire(&locks, &identities).expect("complete scope");
        let expected = ExpectedFile {
            path: Path::new("file"),
            size: 11,
            hash: ExpectedHash::Blake3(*blake3::hash(b"observation").as_bytes()),
        };
        let mut scan = IncrementalFileCheck::begin(scope, &data, expected).expect("scan");
        assert_eq!(
            scan.step(1, || Ok(()), || false).expect("first byte"),
            IncrementalFileProgress::More { bytes_read: 1 }
        );
        let second = locks.join(format!("work-{}.lock", ids[1].simple()));
        let displaced = locks.join("displaced");
        fs::rename(&second, &displaced).expect("displace owned lock");
        fs::write(&second, b"replacement").expect("replacement");
        fs::set_permissions(&second, fs::Permissions::from_mode(0o600)).expect("replacement mode");
        assert!(scan.step(10, || Ok(()), || false).is_err());
        assert!(
            scan.step(10, || Ok(()), || false).is_err(),
            "scope failure poisons the scan"
        );
        assert_eq!(
            fs::read(&displaced).expect("original preserved"),
            b"owned lock"
        );
        assert_eq!(
            fs::read(&second).expect("replacement preserved"),
            b"replacement"
        );
        assert_eq!(
            fs::read(data.join("file")).expect("source preserved"),
            b"observation"
        );
    }
}

//! Inbound source model and candidate discovery.

use std::{
    fs::{self, DirEntry},
    io,
    path::{Path, PathBuf},
};

use crate::config::InboundSource;

#[derive(Debug, Clone)]
pub struct DiscoveredCandidate {
    pub source_library_id: Option<uuid::Uuid>,
    pub source_name: String,
    pub source_root: PathBuf,
    pub relative_path: PathBuf,
    pub absolute_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct InboundDiscoverySnapshot {
    pub source_root: PathBuf,
    pub candidates_seen: usize,
    pub candidates: Vec<DiscoveredCandidate>,
    pub candidate_paths: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceDiscoveryError {
    #[error("inbound source root is not configured")]
    MissingConfig,

    #[error("inbound source root not found: {0}")]
    MissingRoot(PathBuf),

    #[error("inbound source root is not a directory: {0}")]
    NotDirectory(PathBuf),

    #[error("inbound source root is not readable: {0}")]
    NotReadable(PathBuf),

    #[error("candidate path escaped source root: {0}")]
    PathTraversal(PathBuf),

    #[error("inbound source root changed during discovery: {0}")]
    RootChanged(PathBuf),

    #[error("candidate path is not a directory: {0}")]
    NotDirectoryCandidate(PathBuf),

    #[error("candidate symlink target for {0} is missing")]
    SymlinkTargetMissing(PathBuf),

    #[error("candidate candidate is unreadable: {0}")]
    SourceRootInaccessible(PathBuf),

    #[error("source discovery failed at {0}: {1}")]
    Io(PathBuf, String),
}

impl SourceDiscoveryError {
    fn io(path: PathBuf, error: io::Error) -> Self {
        Self::Io(path, error.to_string())
    }
}

#[must_use]
pub fn candidate_relative_path(path: &Path) -> String {
    if path.to_str().is_some() {
        return path_to_posix(path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut encoded = String::from("//melodee-unix-hex:");
        for byte in path.as_os_str().as_bytes() {
            use std::fmt::Write;
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut encoded = String::from("//melodee-windows-u16:");
        for unit in path.as_os_str().encode_wide() {
            use std::fmt::Write;
            let _ = write!(encoded, "{unit:04x}");
        }
        encoded
    }
    #[cfg(not(any(unix, windows)))]
    path_to_posix(path)
}

pub(crate) fn legacy_candidate_relative_path(path: &Path) -> String {
    path_to_posix(path)
}

/// Restore an occurrence path without confusing an encoded byte sequence with
/// a valid relative Unicode path. The `//` prefix cannot occur in a normalized
/// relative path; callers must still enforce normal components and confinement.
pub(crate) fn decode_candidate_relative_path(value: &str) -> Result<PathBuf, SourceDiscoveryError> {
    #[cfg(unix)]
    if let Some(hex) = value.strip_prefix("//melodee-unix-hex:") {
        use std::os::unix::ffi::OsStringExt;
        if hex.is_empty()
            || hex.len() > 262_144
            || hex.len() % 2 != 0
            || !hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(SourceDiscoveryError::PathTraversal(PathBuf::from(value)));
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().as_chunks::<2>().0 {
            let value = std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| SourceDiscoveryError::PathTraversal(PathBuf::from(value)))?;
            bytes.push(value);
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
        if path.to_str().is_some() || candidate_relative_path(&path) != value {
            return Err(SourceDiscoveryError::PathTraversal(PathBuf::from(value)));
        }
        return Ok(path);
    }
    #[cfg(windows)]
    if let Some(hex) = value.strip_prefix("//melodee-windows-u16:") {
        use std::os::windows::ffi::OsStringExt;
        if hex.is_empty()
            || hex.len() > 262_144
            || hex.len() % 4 != 0
            || !hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(SourceDiscoveryError::PathTraversal(PathBuf::from(value)));
        }
        let mut units = Vec::with_capacity(hex.len() / 4);
        for group in hex.as_bytes().chunks_exact(4) {
            let unit = std::str::from_utf8(group)
                .ok()
                .and_then(|group| u16::from_str_radix(group, 16).ok())
                .ok_or_else(|| SourceDiscoveryError::PathTraversal(PathBuf::from(value)))?;
            units.push(unit);
        }
        let path = PathBuf::from(std::ffi::OsString::from_wide(&units));
        if path.to_str().is_some() || candidate_relative_path(&path) != value {
            return Err(SourceDiscoveryError::PathTraversal(PathBuf::from(value)));
        }
        return Ok(path);
    }
    Ok(PathBuf::from(value))
}

/// Discover immediate child directories of an inbound root as Release candidates.
pub fn discover_release_candidates(
    source: &InboundSource,
) -> Result<Vec<DiscoveredCandidate>, SourceDiscoveryError> {
    Ok(snapshot_source_candidates(source)?.candidates)
}

/// Discover and normalize candidate directory metadata in one pass.
pub fn snapshot_source_candidates(
    source: &InboundSource,
) -> Result<InboundDiscoverySnapshot, SourceDiscoveryError> {
    if source.root.as_os_str().is_empty() {
        return Err(SourceDiscoveryError::MissingConfig);
    }

    let entries = read_dir_sorted(&source.root)?;
    let mut candidates = Vec::with_capacity(entries.len());

    for entry in entries {
        if let Some(candidate) = entry_to_candidate(source, &entry).ok().flatten() {
            candidates.push(candidate);
        }
    }

    let mut paths: Vec<PathBuf> = candidates
        .iter()
        .map(|candidate| candidate.relative_path.clone())
        .collect();
    paths.sort_unstable();
    Ok(InboundDiscoverySnapshot {
        source_root: source.root.clone(),
        candidates_seen: paths.len(),
        candidates,
        candidate_paths: paths,
    })
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct DiscoveryResolveTestHook {
    target: std::ffi::OsString,
    state: std::sync::Arc<DiscoveryResolveTestState>,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Default)]
struct DiscoveryResolveTestState {
    before: std::sync::atomic::AtomicBool,
    before_release: std::sync::atomic::AtomicBool,
    after: std::sync::atomic::AtomicBool,
    after_release: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

#[cfg(feature = "test-support")]
impl DiscoveryResolveTestHook {
    #[must_use]
    pub fn new(target: impl Into<std::ffi::OsString>) -> Self {
        Self {
            target: target.into(),
            state: std::sync::Arc::new(DiscoveryResolveTestState::default()),
        }
    }

    async fn wait_for(&self, state: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering;
        loop {
            if state.load(Ordering::SeqCst) {
                return;
            }
            let notified = self.state.notify.notified();
            if state.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    pub async fn wait_before(&self) {
        self.wait_for(&self.state.before).await;
    }
    pub async fn wait_after(&self) {
        self.wait_for(&self.state.after).await;
    }
    pub fn release_before(&self) {
        self.state
            .before_release
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn release_after(&self) {
        self.state
            .after_release
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn pause(&self, name: &std::ffi::OsStr, before: bool) {
        use std::sync::atomic::Ordering;
        if name != self.target.as_os_str() || self.state.after.load(Ordering::SeqCst) {
            return;
        }
        let (reached, release) = if before {
            (&self.state.before, &self.state.before_release)
        } else {
            (&self.state.after, &self.state.after_release)
        };
        reached.store(true, Ordering::SeqCst);
        self.state.notify.notify_waiters();
        while !release.load(Ordering::SeqCst) {
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
}

/// Source-root and candidate handles for race-resistant periodic discovery.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod guarded {
    use super::*;
    use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, openat, readlinkat, statat};
    use std::{
        collections::VecDeque,
        ffi::{OsStr, OsString},
        fs::File,
        os::{
            fd::AsRawFd,
            unix::{
                ffi::{OsStrExt, OsStringExt},
                fs::MetadataExt,
            },
        },
        sync::Arc,
    };

    fn rustix_error(path: &Path, error: rustix::io::Errno) -> SourceDiscoveryError {
        SourceDiscoveryError::io(
            path.to_path_buf(),
            io::Error::from_raw_os_error(error.raw_os_error()),
        )
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn proc_path(file: &File) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
    }
    #[cfg(target_vendor = "apple")]
    fn proc_path(file: &File) -> PathBuf {
        PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()))
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn current_path(file: &File, label: &Path) -> Result<PathBuf, SourceDiscoveryError> {
        fs::read_link(proc_path(file))
            .map_err(|error| SourceDiscoveryError::io(label.to_path_buf(), error))
    }
    #[cfg(target_vendor = "apple")]
    fn current_path(file: &File, label: &Path) -> Result<PathBuf, SourceDiscoveryError> {
        rustix::fs::getpath(file)
            .map(|path| PathBuf::from(OsString::from_vec(path.into_bytes())))
            .map_err(|error| rustix_error(label, error))
    }

    #[derive(Clone)]
    pub(crate) struct GuardedRoot {
        source: InboundSource,
        directory: Arc<File>,
        #[cfg(feature = "test-support")]
        test_hook: Option<super::DiscoveryResolveTestHook>,
    }

    #[derive(Clone)]
    pub(crate) struct GuardedCandidate {
        pub(crate) relative_path: PathBuf,
        pub(crate) directory: Arc<File>,
        #[allow(dead_code)]
        parent: Arc<File>,
        leaf: OsString,
    }

    impl GuardedRoot {
        pub(crate) fn open(source: InboundSource) -> Result<Self, SourceDiscoveryError> {
            let directory = crate::secure_fs::open_directory(&source.root)
                .map_err(|error| SourceDiscoveryError::io(source.root.clone(), error))?;
            // Descriptor paths are used for snapshots; fail closed when the
            // platform cannot open the pinned directory through its fd path.
            current_path(&directory, &source.root)?;
            fs::read_dir(proc_path(&directory))
                .map_err(|error| SourceDiscoveryError::io(source.root.clone(), error))?;
            Ok(Self {
                source,
                directory: Arc::new(directory),
                #[cfg(feature = "test-support")]
                test_hook: None,
            })
        }

        pub(crate) fn verify_current(&self) -> Result<(), SourceDiscoveryError> {
            let current = crate::secure_fs::open_directory(&self.source.root)
                .map_err(|error| SourceDiscoveryError::io(self.source.root.clone(), error))?;
            let pinned = self
                .directory
                .metadata()
                .map_err(|error| SourceDiscoveryError::io(self.source.root.clone(), error))?;
            let observed = current
                .metadata()
                .map_err(|error| SourceDiscoveryError::io(self.source.root.clone(), error))?;
            if pinned.dev() != observed.dev() || pinned.ino() != observed.ino() {
                return Err(SourceDiscoveryError::RootChanged(self.source.root.clone()));
            }
            Ok(())
        }

        pub(crate) fn verify_candidate(
            &self,
            candidate: &GuardedCandidate,
        ) -> Result<(), SourceDiscoveryError> {
            let root = current_path(&self.directory, &self.source.root)?;
            let actual = current_path(&candidate.directory, &self.source.root)?;
            if actual
                .strip_prefix(&root)
                .ok()
                .is_none_or(|path| path != candidate.relative_path)
            {
                return Err(SourceDiscoveryError::RootChanged(self.source.root.clone()));
            }
            Ok(())
        }

        #[cfg(feature = "test-support")]
        pub(crate) fn set_test_hook(&mut self, hook: Option<super::DiscoveryResolveTestHook>) {
            self.test_hook = hook;
        }

        pub(crate) fn resolve(
            &self,
            name: &OsStr,
        ) -> Result<Option<GuardedCandidate>, SourceDiscoveryError> {
            #[cfg(feature = "test-support")]
            if let Some(hook) = &self.test_hook {
                hook.pause(name, true);
            }
            let result = self.resolve_inner(Path::new(name));
            #[cfg(feature = "test-support")]
            if let Some(hook) = &self.test_hook {
                hook.pause(name, false);
            }
            result
        }

        pub(crate) fn resolve_relative(
            &self,
            path: &Path,
        ) -> Result<Option<GuardedCandidate>, SourceDiscoveryError> {
            #[cfg(feature = "test-support")]
            if let Some(hook) = &self.test_hook {
                hook.pause(path.as_os_str(), true);
            }
            let result = self.resolve_inner(path);
            #[cfg(feature = "test-support")]
            if let Some(hook) = &self.test_hook {
                hook.pause(path.as_os_str(), false);
            }
            result
        }

        fn resolve_inner(
            &self,
            path: &Path,
        ) -> Result<Option<GuardedCandidate>, SourceDiscoveryError> {
            use std::path::Component;
            const MAX_SYMLINKS: usize = 40;
            const MAX_COMPONENTS: usize = 4096;

            let root_components: Vec<OsString> = self
                .source
                .root
                .components()
                .filter_map(|part| {
                    if let Component::Normal(name) = part {
                        Some(name.to_os_string())
                    } else {
                        None
                    }
                })
                .collect();
            let slash = openat(
                CWD,
                b"/".as_slice(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map(File::from)
            .map_err(|error| rustix_error(&self.source.root, error))?;
            let mut stack = vec![if root_components.is_empty() {
                self.directory.clone()
            } else {
                Arc::new(slash)
            }];
            let mut resolved: Vec<OsString> = Vec::new();
            let mut pending: VecDeque<OsString> = root_components.iter().cloned().collect();
            if path.as_os_str().is_empty() {
                return Err(SourceDiscoveryError::PathTraversal(path.to_path_buf()));
            }
            for component in path.components() {
                match component {
                    Component::Normal(name) => pending.push_back(name.to_os_string()),
                    _ => return Err(SourceDiscoveryError::PathTraversal(path.to_path_buf())),
                }
            }
            let mut symlinks = 0;

            while let Some(component) = pending.pop_front() {
                if component == OsStr::new(".") {
                    continue;
                }
                if component == OsStr::new("..") {
                    if !resolved.is_empty() {
                        resolved.pop();
                        stack.pop();
                    }
                    continue;
                }
                if resolved.len() >= MAX_COMPONENTS || pending.len() > MAX_COMPONENTS {
                    return Err(SourceDiscoveryError::Io(
                        self.source.root.clone(),
                        "symlink resolution exceeds bounded component budget".to_owned(),
                    ));
                }
                let mut next_path = resolved.clone();
                next_path.push(component.clone());
                if next_path == root_components {
                    resolved.push(component);
                    stack.push(self.directory.clone());
                    continue;
                }
                let Some(parent) = stack.last() else {
                    return Err(SourceDiscoveryError::Io(
                        self.source.root.clone(),
                        "directory resolution lost its root".to_owned(),
                    ));
                };
                let stat = match statat(&**parent, component.as_bytes(), AtFlags::SYMLINK_NOFOLLOW)
                {
                    Ok(stat) => stat,
                    Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                    Err(error) => return Err(rustix_error(&self.source.root.join(path), error)),
                };
                match FileType::from_raw_mode(stat.st_mode) {
                    FileType::Symlink => {
                        symlinks += 1;
                        if symlinks > MAX_SYMLINKS {
                            return Ok(None);
                        }
                        let target = readlinkat(&**parent, component.as_bytes(), Vec::new())
                            .map_err(|error| rustix_error(&self.source.root.join(path), error))?;
                        let target = PathBuf::from(OsString::from_vec(target.into_bytes()));
                        if target.is_absolute() {
                            resolved.clear();
                            stack.truncate(1);
                        }
                        let parts: Vec<OsString> = target
                            .components()
                            .filter_map(|part| match part {
                                Component::Normal(value) => Some(value.to_os_string()),
                                Component::ParentDir => Some(OsString::from("..")),
                                _ => None,
                            })
                            .collect();
                        for part in parts.into_iter().rev() {
                            pending.push_front(part);
                        }
                    }
                    FileType::Directory => {
                        let directory = match openat(
                            &**parent,
                            component.as_bytes(),
                            OFlags::RDONLY
                                | OFlags::DIRECTORY
                                | OFlags::CLOEXEC
                                | OFlags::NOFOLLOW
                                | OFlags::NONBLOCK,
                            Mode::empty(),
                        ) {
                            Ok(fd) => File::from(fd),
                            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                            Err(error) => {
                                return Err(rustix_error(&self.source.root.join(path), error));
                            }
                        };
                        resolved.push(component);
                        stack.push(Arc::new(directory));
                    }
                    _ => return Ok(None),
                }
            }
            if !resolved.starts_with(&root_components) || resolved.len() == root_components.len() {
                return Ok(None);
            }
            let mut relative_path = PathBuf::new();
            for component in &resolved[root_components.len()..] {
                relative_path.push(component);
            }
            let Some(directory) = stack.pop() else {
                return Err(SourceDiscoveryError::Io(
                    self.source.root.clone(),
                    "directory resolution lost its target".to_owned(),
                ));
            };
            let parent = stack.last().cloned().ok_or_else(|| {
                SourceDiscoveryError::Io(
                    self.source.root.clone(),
                    "directory resolution lost its parent".to_owned(),
                )
            })?;
            let leaf = resolved.last().cloned().ok_or_else(|| {
                SourceDiscoveryError::Io(
                    self.source.root.clone(),
                    "directory resolution lost its leaf".to_owned(),
                )
            })?;
            let candidate = GuardedCandidate {
                relative_path,
                directory,
                parent,
                leaf,
            };
            self.verify_candidate(&candidate)?;
            Ok(Some(candidate))
        }

        pub(crate) fn candidate_path(&self, candidate: &GuardedCandidate) -> PathBuf {
            proc_path(&candidate.directory)
        }

        #[allow(dead_code)]
        pub(crate) fn custody_source_handles(
            &self,
            candidate: &GuardedCandidate,
        ) -> Result<(File, OsString, File), SourceDiscoveryError> {
            self.verify_current()?;
            self.verify_candidate(candidate)?;
            let parent = candidate
                .parent
                .try_clone()
                .map_err(|error| SourceDiscoveryError::io(self.source.root.clone(), error))?;
            let directory = candidate
                .directory
                .try_clone()
                .map_err(|error| SourceDiscoveryError::io(self.source.root.clone(), error))?;
            Ok((parent, candidate.leaf.clone(), directory))
        }

        pub(crate) fn custody_root_handle(&self) -> Result<File, SourceDiscoveryError> {
            self.verify_current()?;
            self.directory
                .try_clone()
                .map_err(|error| SourceDiscoveryError::io(self.source.root.clone(), error))
        }

        pub(crate) fn cap_directory(
            &self,
            candidate: &GuardedCandidate,
        ) -> Result<cap_std::fs::Dir, SourceDiscoveryError> {
            candidate
                .directory
                .try_clone()
                .map(cap_std::fs::Dir::from_std_file)
                .map_err(|error| SourceDiscoveryError::io(self.source.root.clone(), error))
        }

        pub(crate) fn ready_marker_seen(&self, candidate: &GuardedCandidate, marker: &str) -> bool {
            let path = self.candidate_path(candidate).join(marker);
            path.is_file() || path.is_dir()
        }

        pub(crate) fn generation(
            &self,
            candidate: &GuardedCandidate,
        ) -> Result<String, crate::acquisition::AcquisitionError> {
            crate::acquisition::source_generation_from_directory(
                &candidate.directory,
                &candidate.relative_path,
            )
        }
    }

    pub(crate) struct CandidateReader {
        root: GuardedRoot,
        entries: Dir,
    }

    impl CandidateReader {
        pub(crate) fn open(root: GuardedRoot) -> Result<Self, SourceDiscoveryError> {
            let entries = Dir::read_from(&*root.directory)
                .map_err(|error| rustix_error(&root.source.root, error))?;
            Ok(Self { root, entries })
        }

        pub(crate) fn next_batch(
            mut self,
            partition: u8,
        ) -> Result<(Self, Vec<OsString>, bool), SourceDiscoveryError> {
            const BATCH_SIZE: usize = 128;
            let mut batch = Vec::with_capacity(BATCH_SIZE);
            let mut examined = 0;
            while examined < BATCH_SIZE {
                match self.entries.read() {
                    Some(Ok(entry)) => {
                        examined += 1;
                        let name = OsStr::from_bytes(entry.file_name().to_bytes());
                        if name == OsStr::new(".") || name == OsStr::new("..") {
                            continue;
                        }
                        if blake3::hash(name.as_encoded_bytes()).as_bytes()[0] & 15 == partition {
                            batch.push(name.to_os_string());
                        }
                    }
                    Some(Err(error)) => return Err(rustix_error(&self.root.source.root, error)),
                    None => return Ok((self, batch, true)),
                }
            }
            Ok((self, batch, false))
        }
    }
}

#[cfg(windows)]
#[path = "source_windows.rs"]
mod guarded;

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple"
)))]
mod guarded {
    use super::*;
    use std::{
        collections::VecDeque,
        ffi::{OsStr, OsString},
        fs::File,
        sync::Arc,
    };
    fn unsupported() -> SourceDiscoveryError {
        SourceDiscoveryError::Io(
            PathBuf::new(),
            "handle-relative inbound discovery is unsupported on this platform".to_owned(),
        )
    }
    #[derive(Clone)]
    pub(crate) struct GuardedRoot;
    #[derive(Clone)]
    pub(crate) struct GuardedCandidate {
        pub(crate) relative_path: PathBuf,
        pub(crate) directory: Arc<File>,
    }
    impl GuardedRoot {
        #[cfg(feature = "test-support")]
        pub(crate) fn set_test_hook(&mut self, _hook: Option<super::DiscoveryResolveTestHook>) {}
        pub(crate) fn open(_source: InboundSource) -> Result<Self, SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn verify_current(&self) -> Result<(), SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn verify_candidate(
            &self,
            _candidate: &GuardedCandidate,
        ) -> Result<(), SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn resolve(
            &self,
            _name: &OsStr,
        ) -> Result<Option<GuardedCandidate>, SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn resolve_relative(
            &self,
            _path: &Path,
        ) -> Result<Option<GuardedCandidate>, SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn candidate_path(&self, _candidate: &GuardedCandidate) -> PathBuf {
            PathBuf::new()
        }
        pub(crate) fn cap_directory(
            &self,
            _candidate: &GuardedCandidate,
        ) -> Result<cap_std::fs::Dir, SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn custody_source_handles(
            &self,
            _candidate: &GuardedCandidate,
        ) -> Result<(File, std::ffi::OsString, File), SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn custody_root_handle(&self) -> Result<File, SourceDiscoveryError> {
            Err(unsupported())
        }

        pub(crate) fn ready_marker_seen(
            &self,
            _candidate: &GuardedCandidate,
            _marker: &str,
        ) -> bool {
            false
        }

        pub(crate) fn generation(
            &self,
            _candidate: &GuardedCandidate,
        ) -> Result<String, crate::acquisition::AcquisitionError> {
            Err(crate::acquisition::AcquisitionError::Task(
                "handle-relative discovery unsupported".to_owned(),
            ))
        }
    }
    pub(crate) struct CandidateReader;
    impl CandidateReader {
        pub(crate) fn open(_root: GuardedRoot) -> Result<Self, SourceDiscoveryError> {
            Err(unsupported())
        }
        pub(crate) fn next_batch(
            self,
            _partition: u8,
        ) -> Result<(Self, Vec<OsString>, bool), SourceDiscoveryError> {
            Err(unsupported())
        }
    }
}

pub(crate) use guarded::{CandidateReader, GuardedCandidate, GuardedRoot};

fn discovery_io<T>(path: &Path, result: io::Result<T>) -> Result<Option<T>, SourceDiscoveryError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound || symlink_loop(&error) => Ok(None),
        Err(error) => Err(SourceDiscoveryError::io(path.to_path_buf(), error)),
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn symlink_loop(error: &io::Error) -> bool {
    error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error())
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn symlink_loop(_error: &io::Error) -> bool {
    false
}

fn entry_to_candidate(
    source: &InboundSource,
    entry: &DirEntry,
) -> Result<Option<DiscoveredCandidate>, SourceDiscoveryError> {
    let entry_name = entry.file_name();
    let entry_path = source.root.join(&entry_name);

    let Some(metadata) = discovery_io(&entry_path, fs::symlink_metadata(&entry_path))? else {
        return Ok(None);
    };
    if !metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Ok(None);
    }

    let candidate_absolute = if metadata.file_type().is_symlink() {
        let Some(link_target) = discovery_io(&entry_path, fs::read_link(&entry_path))? else {
            return Ok(None);
        };
        let candidate_target = if link_target.is_absolute() {
            link_target
        } else {
            entry_path
                .parent()
                .unwrap_or(source.root.as_path())
                .join(link_target)
        };
        let Some(resolved) = discovery_io(&candidate_target, candidate_target.canonicalize())?
        else {
            return Ok(None);
        };
        let Some(target_meta) = discovery_io(&resolved, fs::metadata(&resolved))? else {
            return Ok(None);
        };
        if !target_meta.is_dir() {
            return Ok(None);
        }
        resolved
    } else {
        entry_path
    };

    if !candidate_within_root(&source.root, &candidate_absolute) {
        return Ok(None);
    }
    let Ok(relative_path) = candidate_absolute.strip_prefix(&source.root) else {
        return Ok(None);
    };
    let relative_path = relative_path.to_path_buf();
    if relative_path.as_os_str().is_empty() {
        return Ok(None);
    }
    Ok(Some(DiscoveredCandidate {
        source_library_id: source.id,
        source_name: source.name.clone(),
        source_root: source.root.clone(),
        relative_path,
        absolute_path: candidate_absolute,
    }))
}

fn candidate_within_root(root: &Path, candidate: &Path) -> bool {
    match candidate.strip_prefix(root) {
        Ok(relative) => !relative.as_os_str().is_empty(),
        Err(_) => false,
    }
}

fn read_dir_sorted(root: &Path) -> Result<Vec<DirEntry>, SourceDiscoveryError> {
    let mut entries = fs::read_dir(root)
        .map_err(|error| SourceDiscoveryError::io(root.to_path_buf(), error))?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| SourceDiscoveryError::io(root.to_path_buf(), error))?;

    entries.sort_unstable_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn path_to_posix(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::config::InboundSource;
    use std::fs::{self, File};

    #[cfg(unix)]
    #[test]
    fn byte_distinct_occurrences_have_reversible_noncolliding_keys() {
        use std::os::unix::ffi::OsStringExt;
        let first = PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0x80]));
        let second = PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0x81]));
        let first_key = candidate_relative_path(&first);
        let second_key = candidate_relative_path(&second);
        assert_ne!(first_key, second_key);
        assert_eq!(
            decode_candidate_relative_path(&first_key).expect("decode first"),
            first
        );
        assert_eq!(
            decode_candidate_relative_path(&second_key).expect("decode second"),
            second
        );
        assert_eq!(
            candidate_relative_path(Path::new("normal/name")),
            "normal/name"
        );
        assert!(
            decode_candidate_relative_path("//melodee-unix-hex:616263").is_err(),
            "valid Unicode cannot alias an encoded key"
        );
    }

    fn temp_root(prefix: &str) -> PathBuf {
        use uuid::Uuid;
        let root = std::env::temp_dir().join(format!(
            "melodee-ingest-source-test-{prefix}-{}",
            Uuid::now_v7().simple()
        ));
        fs::create_dir_all(&root).expect("create root");
        root
    }

    #[test]
    fn discovers_only_directories() {
        let root = temp_root("dirs");
        let directory = root.join("release-1");
        let file = root.join("ignore-file.txt");
        fs::create_dir(&directory).expect("dir");
        File::create(&file).expect("file");

        let source =
            InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source");
        let candidates = discover_release_candidates(&source).expect("discover");
        assert_eq!(1, candidates.len());
        assert_eq!(candidates[0].relative_path, PathBuf::from("release-1"));
    }

    #[test]
    fn ignores_non_directories() {
        let root = temp_root("files");
        let file = root.join("ignore-file.txt");
        File::create(file).expect("file");

        let source =
            InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source");
        let candidates = discover_release_candidates(&source).expect("discover");
        assert!(candidates.is_empty());
    }

    #[test]
    fn ignores_symlink_to_file_as_candidate() {
        let root = temp_root("symlink-file");
        let target = root.join("file-target");
        let link = root.join("candidate-link");
        fs::write(&target, b"audio").expect("write target");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("file-target", &link).expect("create symlink");
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_file;
            symlink_file("file-target", &link).expect("create symlink");
        }

        let source = InboundSource::from_root(root.clone(), None, Some("test".into())).unwrap();
        let candidates = discover_release_candidates(&source).expect("discover");
        assert!(candidates.is_empty());
    }

    #[test]
    fn missing_root_is_error() {
        let result = InboundSource::from_root(
            PathBuf::from("/definitely-missing-root-dir"),
            None,
            Some("x".to_owned()),
        );
        assert!(result.is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[test]
    fn partition_reader_bounds_examined_entries_even_without_candidates() {
        let root = temp_root("bounded-noncandidates");
        for index in 0..300 {
            File::create(root.join(format!("not-a-release-{index:03}"))).expect("file");
        }
        let source =
            InboundSource::from_root(root.clone(), None, Some("test".into())).expect("source");
        let guarded = GuardedRoot::open(source).expect("root");
        let mut reader = CandidateReader::open(guarded.clone()).expect("reader");
        let mut batches = 0;
        loop {
            let (next, candidates, done) = reader.next_batch(0).expect("batch");
            reader = next;
            batches += 1;
            for candidate in candidates {
                assert!(guarded.resolve(&candidate).expect("file").is_none());
            }
            if done {
                break;
            }
        }
        assert!(batches >= 3, "non-candidates must consume the batch budget");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn missing_entries_are_ignored_but_access_errors_abort_discovery() {
        let path = Path::new("candidate");
        let missing: io::Result<()> = Err(io::Error::from(io::ErrorKind::NotFound));
        assert!(
            discovery_io(path, missing)
                .expect("missing entry")
                .is_none()
        );
        let denied: io::Result<()> = Err(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(discovery_io(path, denied).is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[test]
    fn self_referential_symlink_is_ignored_by_partition_reader() {
        use std::os::unix::fs::symlink;
        let root = temp_root("loop-symlink");
        symlink("loop", root.join("loop")).expect("symlink");
        let source =
            InboundSource::from_root(root.clone(), None, Some("test".into())).expect("source");
        let partition = blake3::hash(b"loop").as_bytes()[0] & 15;
        let guarded = GuardedRoot::open(source).expect("root");
        let reader = CandidateReader::open(guarded.clone()).expect("reader");
        let (_, candidates, _) = reader.next_batch(partition).expect("read loop");
        assert_eq!(candidates.len(), 1);
        assert!(
            guarded
                .resolve(&candidates[0])
                .expect("ignore loop")
                .is_none()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[test]
    fn guarded_candidate_stays_on_pinned_root_during_swap_away_and_back() {
        let root = temp_root("swap-root");
        fs::create_dir(root.join("release")).expect("old release");
        fs::write(root.join("release/old"), b"old").expect("old file");
        let source =
            InboundSource::from_root(root.clone(), None, Some("test".into())).expect("source");
        let guarded = GuardedRoot::open(source).expect("pin root");
        let old = root.with_extension("old");
        fs::rename(&root, &old).expect("move old root");
        fs::create_dir_all(root.join("release")).expect("replacement release");
        fs::write(root.join("release/new"), b"replacement").expect("replacement file");
        assert!(
            guarded.verify_current().is_err(),
            "replacement must be detected"
        );
        let candidate = guarded
            .resolve(std::ffi::OsStr::new("release"))
            .expect("resolve")
            .expect("old candidate");
        let observed = crate::snapshot::snapshot_release_directory(
            &guarded.candidate_path(&candidate),
            ".ready",
        )
        .expect("snapshot");
        assert!(
            observed
                .files
                .iter()
                .any(|file| file.relative_path == "old")
        );
        assert!(
            !observed
                .files
                .iter()
                .any(|file| file.relative_path == "new")
        );
        fs::remove_dir_all(&root).expect("remove replacement");
        fs::rename(&old, &root).expect("restore original");
        guarded
            .verify_candidate(&candidate)
            .expect("candidate remains in original root");
        guarded.verify_current().expect("original root restored");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[test]
    fn guarded_resolver_preserves_absolute_in_root_aliases_and_rejects_outside_targets() {
        use std::os::unix::fs::symlink;
        let root = temp_root("absolute-alias");
        let outside = temp_root("outside-alias");
        fs::create_dir_all(root.join("holder/release")).expect("inside");
        symlink(root.join("holder/release"), root.join("inside-alias")).expect("inside link");
        symlink(&outside, root.join("outside-alias")).expect("outside link");
        let source =
            InboundSource::from_root(root.clone(), None, Some("test".into())).expect("source");
        let guarded = GuardedRoot::open(source).expect("root");
        let inside = guarded
            .resolve(std::ffi::OsStr::new("inside-alias"))
            .expect("inside")
            .expect("candidate");
        assert_eq!(inside.relative_path, PathBuf::from("holder/release"));
        assert!(
            guarded
                .resolve(std::ffi::OsStr::new("outside-alias"))
                .expect("outside")
                .is_none()
        );
        fs::remove_dir_all(root).expect("cleanup");
        fs::remove_dir_all(outside).expect("cleanup outside");
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[test]
    fn absolute_symlink_chain_stays_on_pinned_root_during_swap() {
        use std::os::unix::fs::symlink;
        let root = temp_root("absolute-swap");
        fs::create_dir_all(root.join("holder/release")).expect("original release");
        fs::write(root.join("holder/release/old"), b"old").expect("old file");
        symlink(root.join("holder/release"), root.join("redirect")).expect("absolute redirect");
        symlink("redirect", root.join("inside")).expect("nested alias");
        let source =
            InboundSource::from_root(root.clone(), None, Some("test".into())).expect("source");
        let guarded = GuardedRoot::open(source).expect("pin root");
        let old = root.with_extension("old");
        fs::rename(&root, &old).expect("move original");
        fs::create_dir_all(root.join("holder/release")).expect("replacement release");
        fs::write(root.join("holder/release/new"), b"new").expect("replacement file");
        let candidate = guarded
            .resolve(std::ffi::OsStr::new("inside"))
            .expect("resolve chain")
            .expect("original candidate");
        assert_eq!(candidate.relative_path, PathBuf::from("holder/release"));
        let snapshot = crate::snapshot::snapshot_release_directory(
            &guarded.candidate_path(&candidate),
            ".ready",
        )
        .expect("snapshot");
        assert!(
            snapshot
                .files
                .iter()
                .any(|file| file.relative_path == "old")
        );
        assert!(
            !snapshot
                .files
                .iter()
                .any(|file| file.relative_path == "new")
        );
        fs::remove_dir_all(&root).expect("remove replacement");
        fs::rename(&old, &root).expect("restore root");
        guarded
            .verify_candidate(&candidate)
            .expect("candidate still beneath pinned root");
        guarded.verify_current().expect("original restored");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn snapshot_report_is_sorted() {
        let root = temp_root("snapshot-order");
        fs::create_dir(root.join("b")).expect("b");
        fs::create_dir(root.join("a")).expect("a");

        let source = InboundSource::from_root(root, None, Some("test".into())).unwrap();
        let candidates = snapshot_source_candidates(&source).expect("snapshot");
        assert!(
            candidates
                .candidate_paths
                .windows(2)
                .all(|pair| pair[0] <= pair[1])
        );
    }
}

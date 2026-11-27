//! Safe, capability-relative Windows inbound discovery.
use super::*;
use cap_fs_ext::DirExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, ReadDir},
};
use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    path::Component,
    sync::Arc,
};

fn io_error(path: &Path, error: io::Error) -> SourceDiscoveryError {
    SourceDiscoveryError::io(path.to_path_buf(), error)
}

fn split_absolute(path: &Path) -> Result<(PathBuf, Vec<OsString>), SourceDiscoveryError> {
    let mut anchor = PathBuf::new();
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => anchor.push(prefix.as_os_str()),
            Component::RootDir => anchor.push(component.as_os_str()),
            Component::Normal(name) => parts.push(name.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => parts.push(OsString::from("..")),
        }
    }
    if !anchor.is_absolute() {
        return Err(SourceDiscoveryError::Io(
            path.to_path_buf(),
            "expected an absolute Windows source path".to_owned(),
        ));
    }
    Ok((anchor, parts))
}

fn same_anchor(left: &Path, right: &Path) -> bool {
    use std::path::Prefix;
    let left = left.components().find_map(|part| {
        if let Component::Prefix(prefix) = part {
            Some(prefix.kind())
        } else {
            None
        }
    });
    let right = right.components().find_map(|part| {
        if let Component::Prefix(prefix) = part {
            Some(prefix.kind())
        } else {
            None
        }
    });
    match (left, right) {
        (
            Some(Prefix::Disk(a) | Prefix::VerbatimDisk(a)),
            Some(Prefix::Disk(b) | Prefix::VerbatimDisk(b)),
        ) => a.eq_ignore_ascii_case(&b),
        (
            Some(Prefix::UNC(a_server, a_share) | Prefix::VerbatimUNC(a_server, a_share)),
            Some(Prefix::UNC(b_server, b_share) | Prefix::VerbatimUNC(b_server, b_share)),
        ) => a_server.eq_ignore_ascii_case(b_server) && a_share.eq_ignore_ascii_case(b_share),
        _ => left == right,
    }
}

fn absolute_from_parts(anchor: &Path, parts: &[OsString]) -> PathBuf {
    let mut path = anchor.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path
}

fn open_anchor(anchor: &Path) -> Result<Arc<Dir>, SourceDiscoveryError> {
    Dir::open_ambient_dir(anchor, ambient_authority())
        .map(Arc::new)
        .map_err(|error| io_error(anchor, error))
}

#[derive(Clone)]
pub(crate) struct GuardedRoot {
    source: InboundSource,
    anchor: PathBuf,
    root_components: Vec<OsString>,
    ancestors: Arc<Vec<Arc<Dir>>>,
    #[cfg(feature = "test-support")]
    test_hook: Option<super::DiscoveryResolveTestHook>,
}

#[derive(Clone)]
pub(crate) struct GuardedCandidate {
    pub(crate) relative_path: PathBuf,
    directory: Arc<Dir>,
    // Pin every opened ancestor without delete sharing until snapshot finishes.
    _ancestors: Arc<Vec<Arc<Dir>>>,
}

impl GuardedRoot {
    pub(crate) fn open(source: InboundSource) -> Result<Self, SourceDiscoveryError> {
        let (anchor, root_components) = split_absolute(&source.root)?;
        let mut ancestors = vec![open_anchor(&anchor)?];
        for name in &root_components {
            let Some(parent) = ancestors.last() else {
                return Err(SourceDiscoveryError::MissingConfig);
            };
            let next = parent
                .open_dir_nofollow(name)
                .map(Arc::new)
                .map_err(|error| io_error(&source.root, error))?;
            ancestors.push(next);
        }
        Ok(Self {
            source,
            anchor,
            root_components,
            ancestors: Arc::new(ancestors),
            #[cfg(feature = "test-support")]
            test_hook: None,
        })
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn set_test_hook(&mut self, hook: Option<super::DiscoveryResolveTestHook>) {
        self.test_hook = hook;
    }

    pub(crate) fn verify_current(&self) -> Result<(), SourceDiscoveryError> {
        let mut current = open_anchor(&self.anchor)?;
        for name in &self.root_components {
            current = current
                .open_dir_nofollow(name)
                .map(Arc::new)
                .map_err(|error| io_error(&self.source.root, error))?;
        }
        use cap_fs_ext::MetadataExt;
        let Some(pinned) = self.ancestors.last() else {
            return Err(SourceDiscoveryError::MissingConfig);
        };
        let before = pinned
            .dir_metadata()
            .map_err(|error| io_error(&self.source.root, error))?;
        let after = current
            .dir_metadata()
            .map_err(|error| io_error(&self.source.root, error))?;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(SourceDiscoveryError::RootChanged(self.source.root.clone()));
        }
        Ok(())
    }

    pub(crate) fn verify_candidate(
        &self,
        _candidate: &GuardedCandidate,
    ) -> Result<(), SourceDiscoveryError> {
        self.verify_current()
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

    fn resolve_inner(&self, path: &Path) -> Result<Option<GuardedCandidate>, SourceDiscoveryError> {
        const MAX_SYMLINKS: usize = 40;
        const MAX_COMPONENTS: usize = 4096;
        let mut anchor = self.anchor.clone();
        let mut resolved = Vec::<OsString>::new();
        let mut stack = vec![open_anchor(&anchor)?];
        let mut pending: VecDeque<OsString> = self.root_components.iter().cloned().collect();
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
            if same_anchor(&anchor, &self.anchor) && next_path == self.root_components {
                let Some(root) = self.ancestors.last() else {
                    return Err(SourceDiscoveryError::MissingConfig);
                };
                resolved.push(component);
                stack.push(root.clone());
                continue;
            }
            let Some(parent) = stack.last() else {
                return Err(SourceDiscoveryError::MissingConfig);
            };
            let metadata = match parent.symlink_metadata(&component) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(io_error(&self.source.root.join(path), error)),
            };
            if metadata.is_symlink() {
                symlinks += 1;
                if symlinks > MAX_SYMLINKS {
                    return Ok(None);
                }
                let target = parent
                    .read_link_contents(&component)
                    .map_err(|error| io_error(&self.source.root.join(path), error))?;
                let parts = if target.is_absolute() {
                    let (new_anchor, parts) = split_absolute(&target)?;
                    anchor = new_anchor;
                    resolved.clear();
                    stack = vec![open_anchor(&anchor)?];
                    parts
                } else {
                    if matches!(target.components().next(), Some(Component::Prefix(_))) {
                        return Err(SourceDiscoveryError::Io(
                            self.source.root.join(path),
                            "drive-relative symlink target is ambiguous".to_owned(),
                        ));
                    }
                    if target.has_root() {
                        resolved.clear();
                        stack = vec![open_anchor(&anchor)?];
                    }
                    target
                        .components()
                        .filter_map(|part| match part {
                            Component::Normal(value) => Some(value.to_os_string()),
                            Component::ParentDir => Some(OsString::from("..")),
                            _ => None,
                        })
                        .collect()
                };
                for part in parts.into_iter().rev() {
                    pending.push_front(part);
                }
            } else if metadata.is_dir() {
                let child = parent
                    .open_dir_nofollow(&component)
                    .map(Arc::new)
                    .map_err(|error| io_error(&self.source.root.join(path), error))?;
                if same_anchor(&anchor, &self.anchor)
                    && next_path.len() == self.root_components.len()
                {
                    // Both the parent chain and this child are pinned without
                    // delete sharing. Canonicalization therefore compares the
                    // actual names, including case and verbatim spelling,
                    // without trusting a truncated Windows file ID alone.
                    let actual = fs::canonicalize(absolute_from_parts(&anchor, &next_path))
                        .map_err(|error| io_error(&self.source.root.join(path), error))?;
                    if actual == self.source.root {
                        let Some(root) = self.ancestors.last() else {
                            return Err(SourceDiscoveryError::MissingConfig);
                        };
                        resolved.clone_from(&self.root_components);
                        stack.push(root.clone());
                        continue;
                    }
                }
                resolved.push(component);
                stack.push(child);
            } else {
                return Ok(None);
            }
        }
        if !same_anchor(&anchor, &self.anchor) {
            return Ok(None);
        }
        // The entire component chain remains pinned without delete sharing;
        // this canonicalization cannot be redirected by a rename.
        let actual = fs::canonicalize(absolute_from_parts(&anchor, &resolved))
            .map_err(|error| io_error(&self.source.root.join(path), error))?;
        let Ok(relative) = actual.strip_prefix(&self.source.root) else {
            return Ok(None);
        };
        if relative.as_os_str().is_empty() {
            return Ok(None);
        }
        let relative_path = relative.to_path_buf();
        let Some(directory) = stack.last().cloned() else {
            return Err(SourceDiscoveryError::MissingConfig);
        };
        Ok(Some(GuardedCandidate {
            relative_path,
            directory,
            _ancestors: Arc::new(stack),
        }))
    }

    pub(crate) fn cap_directory(
        &self,
        candidate: &GuardedCandidate,
    ) -> Result<cap_std::fs::Dir, SourceDiscoveryError> {
        candidate
            .directory
            .try_clone()
            .map_err(|error| io_error(&candidate.relative_path, error))
    }

    pub(crate) fn custody_source_handles(
        &self,
        _candidate: &GuardedCandidate,
    ) -> Result<(std::fs::File, OsString, std::fs::File), SourceDiscoveryError> {
        Err(SourceDiscoveryError::Io(
            self.source.root.clone(),
            "descriptor-relative custody is not yet implemented on Windows".to_owned(),
        ))
    }

    pub(crate) fn custody_root_handle(&self) -> Result<std::fs::File, SourceDiscoveryError> {
        Err(SourceDiscoveryError::Io(
            self.source.root.clone(),
            "descriptor-relative custody is not yet implemented on Windows".to_owned(),
        ))
    }

    pub(crate) fn ready_marker_seen(&self, candidate: &GuardedCandidate, marker: &str) -> bool {
        // The candidate and every ancestor stay pinned without delete sharing.
        // Use the previous marker semantics, which follow absolute as well as
        // relative marker symlinks, while those handles prevent replacement.
        let path = self.source.root.join(&candidate.relative_path).join(marker);
        path.is_file() || path.is_dir()
    }

    pub(crate) fn generation(
        &self,
        candidate: &GuardedCandidate,
    ) -> Result<String, crate::acquisition::AcquisitionError> {
        let file = candidate
            .directory
            .try_clone()
            .map_err(|source| crate::acquisition::AcquisitionError::Filesystem {
                path: candidate.relative_path.clone(),
                source,
            })?
            .into_std_file();
        crate::acquisition::source_generation_from_directory(&file, &candidate.relative_path)
    }
}

pub(crate) struct CandidateReader {
    root: GuardedRoot,
    entries: ReadDir,
}
impl CandidateReader {
    pub(crate) fn open(root: GuardedRoot) -> Result<Self, SourceDiscoveryError> {
        let Some(directory) = root.ancestors.last() else {
            return Err(SourceDiscoveryError::MissingConfig);
        };
        let entries = directory
            .entries()
            .map_err(|error| io_error(&root.source.root, error))?;
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
            match self.entries.next() {
                Some(Ok(entry)) => {
                    examined += 1;
                    let name = entry.file_name();
                    if blake3::hash(name.as_encoded_bytes()).as_bytes()[0] & 15 == partition {
                        batch.push(name);
                    }
                }
                Some(Err(error)) => return Err(io_error(&self.root.source.root, error)),
                None => return Ok((self, batch, true)),
            }
        }
        Ok((self, batch, false))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use std::os::windows::fs::{symlink_dir, symlink_file};

    #[test]
    fn disk_and_unc_verbatim_anchors_match_their_standard_forms() {
        assert!(same_anchor(Path::new(r"C:\"), Path::new(r"\\?\c:\")));
        assert!(same_anchor(
            Path::new(r"\\server\share\"),
            Path::new(r"\\?\UNC\SERVER\SHARE\")
        ));
        assert!(!same_anchor(Path::new(r"C:\"), Path::new(r"D:\")));
    }

    #[test]
    fn absolute_and_drive_rooted_aliases_resolve_inside_pinned_root() {
        let temp = tempfile::tempdir().expect("temporary root");
        fs::create_dir_all(temp.path().join("holder/release")).expect("release");
        let source =
            InboundSource::from_root(temp.path().to_path_buf(), None, Some("test".to_owned()))
                .expect("source");
        let root_text = source.root.to_str().expect("unicode temporary path");
        let normal_root = root_text.strip_prefix(r"\\?\").unwrap_or(root_text);
        let differently_cased =
            PathBuf::from(normal_root.to_ascii_uppercase()).join("holder/release");
        symlink_dir(&differently_cased, temp.path().join("absolute-alias"))
            .expect("absolute alias requires Windows symlink privilege");
        let rooted: PathBuf = differently_cased.components().skip(1).collect();
        symlink_dir(&rooted, temp.path().join("rooted-alias"))
            .expect("rooted alias requires Windows symlink privilege");
        let root = GuardedRoot::open(source).expect("pinned root");
        for alias in ["absolute-alias", "rooted-alias"] {
            let candidate = root
                .resolve(OsStr::new(alias))
                .expect("resolve")
                .expect("in-root candidate");
            assert_eq!(candidate.relative_path, PathBuf::from("holder/release"));
            root.verify_candidate(&candidate).expect("pinned candidate");
        }
    }

    #[test]
    fn ready_marker_follows_absolute_symlink_while_ancestors_are_pinned() {
        let temp = tempfile::tempdir().expect("temporary root");
        let inbound = temp.path().join("inbound");
        let release = inbound.join("release");
        fs::create_dir_all(&release).expect("release");
        let target = temp.path().join("marker-target");
        fs::write(&target, b"ready").expect("marker target");
        symlink_file(&target, release.join(".melodee-ready"))
            .expect("marker alias requires Windows symlink privilege");
        let source =
            InboundSource::from_root(inbound, None, Some("test".to_owned())).expect("source");
        let root = GuardedRoot::open(source).expect("pinned root");
        let candidate = root
            .resolve(OsStr::new("release"))
            .expect("resolve")
            .expect("candidate");
        assert!(root.ready_marker_seen(&candidate, ".melodee-ready"));
    }
}

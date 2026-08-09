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
    path_to_posix(path)
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
        if let Some(candidate) = entry_to_candidate(source, &entry) {
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

fn entry_to_candidate(source: &InboundSource, entry: &DirEntry) -> Option<DiscoveredCandidate> {
    let entry_name = entry.file_name();
    let entry_path = source.root.join(&entry_name);

    let metadata = fs::symlink_metadata(&entry_path).ok()?;
    if !metadata.is_dir() && !metadata.file_type().is_symlink() {
        return None;
    }

    let candidate_absolute = if metadata.file_type().is_symlink() {
        let link_target = fs::read_link(&entry_path).ok()?;

        let candidate_target = if link_target.is_absolute() {
            link_target
        } else {
            entry_path
                .parent()
                .unwrap_or(source.root.as_path())
                .join(link_target)
        };

        let resolved = match candidate_target.canonicalize() {
            Ok(path) => path,
            Err(_) => return None,
        };

        let _target_meta = match fs::metadata(&resolved) {
            Ok(meta) if meta.is_dir() => meta,
            _ => return None,
        };
        resolved
    } else {
        entry_path
    };

    if !candidate_within_root(&source.root, &candidate_absolute) {
        return None;
    }

    let relative_path = candidate_absolute
        .strip_prefix(&source.root)
        .ok()?
        .to_path_buf();

    if relative_path.as_os_str().is_empty() {
        return None;
    }

    Some(DiscoveredCandidate {
        source_library_id: source.id,
        source_name: source.name.clone(),
        source_root: source.root.clone(),
        relative_path,
        absolute_path: candidate_absolute,
    })
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

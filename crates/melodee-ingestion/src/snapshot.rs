//! Deterministic file-tree snapshots used for release settling.

use std::{
    fs::{self, DirEntry},
    io,
    path::{Path, PathBuf},
};

use thiserror::Error;
use time::{OffsetDateTime, UtcOffset};

#[derive(Debug, Clone)]
pub struct SnapshotFile {
    pub relative_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub modified_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct CandidateSnapshot {
    pub signature: String,
    pub file_count: i64,
    pub total_bytes: i64,
    pub files: Vec<SnapshotFile>,
    pub ready_marker_seen: bool,
    pub bypassed_settling: bool,
}

#[derive(Debug, Error)]
pub enum SnapshotCaptureError {
    #[error("snapshot capture failed for {path:?}: {message}")]
    Capture { path: PathBuf, message: String },
}

impl SnapshotCaptureError {
    fn io(path: &Path, error: io::Error) -> Self {
        Self::Capture {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

/// Capture a deterministic snapshot of a release directory.
pub fn snapshot_release_directory(
    release_root: &Path,
    ready_marker_name: &str,
) -> Result<CandidateSnapshot, SnapshotCaptureError> {
    if !release_root.exists() {
        return Err(SnapshotCaptureError::Capture {
            path: release_root.to_path_buf(),
            message: "missing release directory".to_owned(),
        });
    }

    let ready_marker_seen = release_root.join(ready_marker_name).is_file()
        || release_root.join(ready_marker_name).is_dir();

    let mut files = Vec::new();
    let mut queue = vec![(release_root.to_path_buf(), PathBuf::new())];

    while let Some((absolute_dir, relative_dir)) = queue.pop() {
        let entries = read_dir_sorted(&absolute_dir)?;
        for entry in entries {
            let entry_path = entry.path();
            let relative = if relative_dir.as_os_str().is_empty() {
                PathBuf::from(entry.file_name())
            } else {
                relative_dir.join(entry.file_name())
            };

            let metadata = fs::symlink_metadata(&entry_path)
                .map_err(|error| SnapshotCaptureError::io(&entry_path, error))?;

            let modified_at =
                metadata
                    .modified()
                    .map_err(|error| SnapshotCaptureError::Capture {
                        path: entry_path.clone(),
                        message: error.to_string(),
                    })?;

            let modified_at = OffsetDateTime::from(modified_at).to_offset(UtcOffset::UTC);

            let file_size = if metadata.is_file() {
                i64::try_from(metadata.len()).map_err(|_| SnapshotCaptureError::Capture {
                    path: entry_path.clone(),
                    message: "file size exceeds i64".to_owned(),
                })?
            } else {
                0
            };

            files.push(SnapshotFile {
                relative_path: path_to_posix(&relative),
                file_type: file_type_name(&metadata.file_type()).to_owned(),
                file_size,
                modified_at,
            });

            if metadata.is_dir() {
                queue.push((entry_path, relative));
            }
        }
    }

    files.sort_unstable_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.file_type.cmp(&right.file_type))
            .then_with(|| left.file_size.cmp(&right.file_size))
    });

    let signature = files
        .iter()
        .map(|file| {
            format!(
                "{}\u{001f}{}\u{001f}{}\u{001f}{}\n",
                file.relative_path,
                file.file_type,
                file.file_size,
                file.modified_at.unix_timestamp_nanos()
            )
        })
        .collect::<String>();

    let file_count = i64::try_from(files.len()).map_err(|_| SnapshotCaptureError::Capture {
        path: release_root.to_path_buf(),
        message: "too many files for i64 count".to_owned(),
    })?;

    let total_bytes = files.iter().try_fold(0_i64, |total, file| {
        total
            .checked_add(file.file_size)
            .ok_or_else(|| SnapshotCaptureError::Capture {
                path: release_root.to_path_buf(),
                message: "total byte count overflow".to_owned(),
            })
    })?;

    Ok(CandidateSnapshot {
        signature,
        file_count,
        total_bytes,
        files,
        ready_marker_seen,
        bypassed_settling: false,
    })
}

fn file_type_name(file_type: &fs::FileType) -> &'static str {
    if file_type.is_dir() {
        "dir"
    } else if file_type.is_file() {
        "file"
    } else if file_type.is_symlink() {
        "symlink"
    } else {
        "other"
    }
}

fn read_dir_sorted(path: &Path) -> Result<Vec<DirEntry>, SnapshotCaptureError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| SnapshotCaptureError::io(path, error))?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| SnapshotCaptureError::io(path, error))?;

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
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root() -> PathBuf {
        use uuid::Uuid;
        let root =
            std::env::temp_dir().join(format!("melodee-snapshot-test-{}", Uuid::now_v7().simple()));
        fs::create_dir_all(&root).expect("temp root");
        root
    }

    #[test]
    fn snapshots_are_deterministic_for_same_tree() {
        let root = temp_root();
        fs::create_dir_all(root.join("a")).expect("a dir");
        fs::write(root.join("z.txt"), b"z").expect("write z");
        fs::write(root.join("a").join("b.txt"), b"b").expect("write b");

        let first = snapshot_release_directory(&root, ".melodee-ready").expect("first snapshot");
        let second = snapshot_release_directory(&root, ".melodee-ready").expect("second snapshot");
        assert_eq!(first.signature, second.signature);
    }

    #[test]
    fn snapshot_includes_all_directories_and_files() {
        let root = temp_root();
        fs::create_dir_all(root.join("folder")).expect("folder");
        fs::write(root.join("x.txt"), b"x").expect("write");
        fs::write(root.join("folder").join("y.txt"), b"y").expect("write");

        let snapshot = snapshot_release_directory(&root, ".melodee-ready").expect("snapshot");
        assert!(
            snapshot
                .files
                .iter()
                .any(|file| file.relative_path == "x.txt")
        );
        assert!(
            snapshot
                .files
                .iter()
                .any(|file| file.relative_path == "folder/y.txt")
        );
    }

    #[test]
    fn missing_directory_is_error() {
        let missing = temp_root().join("missing");
        let result = snapshot_release_directory(&missing, ".melodee-ready");
        assert!(result.is_err());
    }
}

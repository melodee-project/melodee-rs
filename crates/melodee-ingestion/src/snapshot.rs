//! Deterministic file-tree snapshots used for release settling.

use std::{
    fs, io,
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
        // The final records are sorted below, so retaining every directory
        // entry before inspecting it only duplicates peak memory.
        let entries = fs::read_dir(&absolute_dir)
            .map_err(|error| SnapshotCaptureError::io(&absolute_dir, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| SnapshotCaptureError::io(&absolute_dir, error))?;
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

    finish_snapshot(release_root, ready_marker_seen, files)
}

fn finish_snapshot(
    release_root: &Path,
    ready_marker_seen: bool,
    mut files: Vec<SnapshotFile>,
) -> Result<CandidateSnapshot, SnapshotCaptureError> {
    files.sort_unstable_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.file_type.cmp(&right.file_type))
            .then_with(|| left.file_size.cmp(&right.file_size))
    });

    // Preserve the legacy record encoding, but hash it incrementally so the
    // signature itself has constant size. This is metadata evidence, not a
    // substitute for the content hashes used at custody boundaries.
    let mut digest = blake3::Hasher::new();
    for file in &files {
        let record = format!(
            "{}\u{001f}{}\u{001f}{}\u{001f}{}\n",
            file.relative_path,
            file.file_type,
            file.file_size,
            file.modified_at.unix_timestamp_nanos()
        );
        digest.update(record.as_bytes());
    }
    let signature = format!("{SIGNATURE_PREFIX}{}", digest.finalize().to_hex());

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

const SIGNATURE_PREFIX: &str = "melodee-snapshot-v2-blake3:";

fn is_compact_signature(value: &str) -> bool {
    value.strip_prefix(SIGNATURE_PREFIX).is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Compare compact signatures against either the current version or a legacy
/// literal signature captured before the upgrade. A legacy nonempty record
/// always contains separators, so it cannot be mistaken for the compact form.
pub(crate) fn signatures_equal(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    match (is_compact_signature(left), is_compact_signature(right)) {
        (true, false) => {
            left == format!(
                "{SIGNATURE_PREFIX}{}",
                blake3::hash(right.as_bytes()).to_hex()
            )
        }
        (false, true) => {
            right
                == format!(
                    "{SIGNATURE_PREFIX}{}",
                    blake3::hash(left.as_bytes()).to_hex()
                )
        }
        _ => false,
    }
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
    fn compact_signature_matches_legacy_and_detects_metadata_changes() {
        let root = temp_root();
        fs::write(root.join("track.flac"), b"audio").expect("write");
        let snapshot = snapshot_release_directory(&root, ".ready").expect("snapshot");
        let legacy: String = snapshot
            .files
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
            .collect();
        assert!(signatures_equal(&legacy, &snapshot.signature));
        assert!(signatures_equal(&snapshot.signature, &legacy));
        assert!(!signatures_equal(
            &(legacy + "changed"),
            &snapshot.signature
        ));
        assert!(snapshot.signature.len() < 100);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn compact_signature_matches_an_empty_legacy_snapshot() {
        let compact = format!("{SIGNATURE_PREFIX}{}", blake3::hash(b"").to_hex());
        assert!(signatures_equal("", &compact));
    }

    #[test]
    fn streamed_snapshot_matches_legacy_and_handles_deep_tree() {
        let root = temp_root();
        for number in 0..180 {
            fs::write(
                root.join(format!("track-{number:03}.flac")),
                format!("audio-{number}"),
            )
            .expect("track");
        }
        let mut deep = root.clone();
        for _ in 0..(MAX_STREAM_DIRECTORY_DEPTH + 4) {
            deep.push("d");
            fs::create_dir(&deep).expect("nested directory");
        }
        fs::write(deep.join("deep.txt"), b"nested").expect("nested file");
        let legacy = snapshot_release_directory(&root, ".ready").expect("legacy snapshot");
        let directory = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .expect("capability");
        let mut streamed = Vec::new();
        let summary = stream_release_snapshot(
            &directory,
            false,
            || false,
            |file| {
                streamed.push(file);
                Ok(())
            },
        )
        .expect("stream");
        let result = finish_snapshot(&root, summary.ready_marker_seen, streamed).expect("finish");
        assert_eq!(result.signature, legacy.signature);
        assert_eq!(result.file_count, legacy.file_count);
        assert_eq!(result.total_bytes, legacy.total_bytes);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn streamed_snapshot_stops_when_consumer_cancels() {
        let root = temp_root();
        for number in 0..10 {
            fs::write(root.join(format!("file-{number}")), b"x").expect("file");
        }
        let directory = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .expect("capability");
        let mut observed = 0;
        let result = stream_release_snapshot(
            &directory,
            false,
            || false,
            |_| {
                observed += 1;
                if observed == 3 {
                    return Err(SnapshotCaptureError::Capture {
                        path: PathBuf::new(),
                        message: "consumer cancelled".to_owned(),
                    });
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(observed, 3);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn streamed_snapshot_checks_cancellation_during_large_deep_rescan() {
        let root = temp_root();
        let mut deep = root.clone();
        for _ in 0..(MAX_STREAM_DIRECTORY_DEPTH + 1) {
            deep.push("d");
            fs::create_dir(&deep).expect("nested directory");
        }
        for number in 0..200 {
            fs::write(deep.join(format!("file-{number:03}")), b"x").expect("file");
        }
        let directory = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .expect("capability");
        let checks = std::cell::Cell::new(0);
        let mut emitted = 0;
        let result = stream_release_snapshot(
            &directory,
            false,
            || {
                checks.set(checks.get() + 1);
                checks.get() > 70
            },
            |_| {
                emitted += 1;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(emitted, MAX_STREAM_DIRECTORY_DEPTH + 1);
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// Manual release-profile characterization of the deep rescan fallback.
    #[test]
    #[ignore = "explicit filesystem performance characterization"]
    fn characterize_deep_directory_rescan() {
        let root = temp_root();
        let mut deep = root.clone();
        for _ in 0..(MAX_STREAM_DIRECTORY_DEPTH + 1) {
            deep.push("d");
            fs::create_dir(&deep).expect("nested directory");
        }
        for number in 0..1_000 {
            fs::write(deep.join(format!("file-{number:04}")), b"x").expect("file");
        }
        let directory = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .expect("capability");
        let start = std::time::Instant::now();
        let summary =
            stream_release_snapshot(&directory, false, || false, |_| Ok(())).expect("stream");
        eprintln!(
            "deep_rescan entries={} elapsed_ms={}",
            summary.file_count,
            start.elapsed().as_millis()
        );
        assert_eq!(
            summary.file_count,
            1_000 + (MAX_STREAM_DIRECTORY_DEPTH + 1) as i64
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn missing_directory_is_error() {
        let missing = temp_root().join("missing");
        let result = snapshot_release_directory(&missing, ".melodee-ready");
        assert!(result.is_err());
    }
}

/// A streamed snapshot's bounded aggregate. File records are emitted separately.
#[derive(Debug, Clone)]
pub(crate) struct StreamSnapshotSummary {
    pub file_count: i64,
    pub total_bytes: i64,
    pub ready_marker_seen: bool,
}

/// Maximum number of retained directory cursors in one blocking traversal.
pub(crate) const MAX_STREAM_DIRECTORY_DEPTH: usize = 16;
/// A conservative bound compatible with extended-length Windows paths.
pub(crate) const MAX_STREAM_PATH_BYTES: usize = 131_072;

pub(crate) struct StreamFrame {
    cursor: StreamCursor,
}

enum StreamCursor {
    Live {
        directory: cap_std::fs::Dir,
        entries: cap_std::fs::ReadDir,
    },
    // Beyond the live-handle budget, reopen and select the next name. This
    // trades extra directory reads for bounded descriptors without truncation.
    Rescan {
        last: Option<std::ffi::OsString>,
    },
}

pub(crate) fn open_cap_relative(
    root: &cap_std::fs::Dir,
    relative: &Path,
) -> Result<cap_std::fs::Dir, SnapshotCaptureError> {
    use cap_fs_ext::DirExt;
    use std::path::Component;
    let mut directory = root
        .try_clone()
        .map_err(|error| SnapshotCaptureError::io(relative, error))?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(SnapshotCaptureError::Capture {
                path: relative.to_path_buf(),
                message: "invalid relative snapshot path".to_owned(),
            });
        };
        directory = directory
            .open_dir_nofollow(name)
            .map_err(|error| SnapshotCaptureError::io(relative, error))?;
    }
    Ok(directory)
}

impl StreamFrame {
    pub(crate) fn open(
        root: &cap_std::fs::Dir,
        relative: &Path,
        live: bool,
    ) -> Result<Self, SnapshotCaptureError> {
        let cursor = if live {
            let directory = open_cap_relative(root, relative)?;
            let entries = directory
                .entries()
                .map_err(|error| SnapshotCaptureError::io(&relative, error))?;
            StreamCursor::Live { directory, entries }
        } else {
            StreamCursor::Rescan { last: None }
        };
        Ok(Self { cursor })
    }

    pub(crate) fn next(
        &mut self,
        root: &cap_std::fs::Dir,
        relative: &Path,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<(std::ffi::OsString, cap_std::fs::Metadata)>, SnapshotCaptureError> {
        match &mut self.cursor {
            StreamCursor::Live { directory, entries } => {
                let Some(entry) = entries.next() else {
                    return Ok(None);
                };
                let entry = entry.map_err(|error| SnapshotCaptureError::io(relative, error))?;
                let name = entry.file_name();
                let metadata = directory
                    .symlink_metadata(&name)
                    .map_err(|error| SnapshotCaptureError::io(&relative.join(&name), error))?;
                Ok(Some((name, metadata)))
            }
            StreamCursor::Rescan { last } => {
                let directory = open_cap_relative(root, relative)?;
                let entries = directory
                    .entries()
                    .map_err(|error| SnapshotCaptureError::io(relative, error))?;
                let mut next: Option<std::ffi::OsString> = None;
                for entry in entries {
                    if cancelled() {
                        return Err(SnapshotCaptureError::Capture {
                            path: relative.to_path_buf(),
                            message: "snapshot consumer stopped".to_owned(),
                        });
                    }
                    let name = entry
                        .map_err(|error| SnapshotCaptureError::io(relative, error))?
                        .file_name();
                    if last.as_ref().is_some_and(|previous| name <= *previous) {
                        continue;
                    }
                    if next.as_ref().is_none_or(|current| name < *current) {
                        next = Some(name);
                    }
                }
                let Some(name) = next else {
                    return Ok(None);
                };
                let metadata = directory
                    .symlink_metadata(&name)
                    .map_err(|error| SnapshotCaptureError::io(&relative.join(&name), error))?;
                *last = Some(name.clone());
                Ok(Some((name, metadata)))
            }
        }
    }
}

/// Traverse through no-follow capabilities and emit one metadata record at a
/// time. The callback must provide bounded backpressure and stop on cancellation.
pub(crate) fn stream_release_snapshot(
    root: &cap_std::fs::Dir,
    ready_marker_seen: bool,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(SnapshotFile) -> Result<(), SnapshotCaptureError>,
) -> Result<StreamSnapshotSummary, SnapshotCaptureError> {
    let mut summary = StreamSnapshotSummary {
        file_count: 0,
        total_bytes: 0,
        ready_marker_seen,
    };
    let mut current_path = PathBuf::new();
    let mut stack = vec![StreamFrame::open(root, &current_path, true)?];
    while let Some(frame) = stack.last_mut() {
        if cancelled() {
            return Err(SnapshotCaptureError::Capture {
                path: current_path,
                message: "snapshot consumer stopped".to_owned(),
            });
        }
        let Some((name, metadata)) = frame.next(root, &current_path, &cancelled)? else {
            stack.pop();
            if !stack.is_empty() {
                current_path.pop();
            }
            continue;
        };
        let relative = current_path.join(&name);
        if relative.as_os_str().as_encoded_bytes().len() > MAX_STREAM_PATH_BYTES {
            return Err(SnapshotCaptureError::Capture {
                path: relative,
                message: "snapshot path exceeds bounded platform path budget".to_owned(),
            });
        }
        let modified_at = metadata
            .modified()
            .map_err(|error| SnapshotCaptureError::io(&relative, error))?;
        let file_size = if metadata.is_file() {
            i64::try_from(metadata.len()).map_err(|_| SnapshotCaptureError::Capture {
                path: relative.clone(),
                message: "file size exceeds i64".to_owned(),
            })?
        } else {
            0
        };
        let file_type = if metadata.is_dir() {
            "dir"
        } else if metadata.is_file() {
            "file"
        } else if metadata.is_symlink() {
            "symlink"
        } else {
            "other"
        };
        summary.file_count =
            summary
                .file_count
                .checked_add(1)
                .ok_or_else(|| SnapshotCaptureError::Capture {
                    path: relative.clone(),
                    message: "snapshot file count overflow".to_owned(),
                })?;
        summary.total_bytes = summary.total_bytes.checked_add(file_size).ok_or_else(|| {
            SnapshotCaptureError::Capture {
                path: relative.clone(),
                message: "snapshot byte count overflow".to_owned(),
            }
        })?;
        emit(SnapshotFile {
            relative_path: path_to_posix(&relative),
            file_type: file_type.to_owned(),
            file_size,
            modified_at: OffsetDateTime::from(modified_at.into_std()).to_offset(UtcOffset::UTC),
        })?;
        if metadata.is_dir() {
            let live = stack.len() < MAX_STREAM_DIRECTORY_DEPTH;
            current_path.push(&name);
            stack.push(StreamFrame::open(root, &current_path, live)?);
        }
    }
    Ok(summary)
}

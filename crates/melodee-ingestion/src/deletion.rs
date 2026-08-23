//! Explicit administrator-authorized cleanup of one Melodee-owned Release.
//!
//! This module removes only directory trees derived from durable relative
//! paths below the configured staging and canonical roots. The underlying
//! descriptor-relative operation refuses symlinks and special files.

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::secure_fs::remove_directory_tree;

/// Relative paths removed by one confirmed Release deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedReleasePaths {
    pub canonical_relative_path: String,
    pub staging_relative_path: Option<String>,
}

/// Failure while validating or removing an administrator-authorized Release.
#[derive(Debug, Error)]
pub enum ReleaseDeletionError {
    #[error("{kind} deletion path is not a safe relative directory: {path}")]
    InvalidPath { kind: &'static str, path: String },
    #[error("failed to remove {kind} Release directory {path}")]
    Remove {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Release deletion task did not complete")]
    Task(#[from] tokio::task::JoinError),
}

/// Removes the canonical bundle and optional retained staging tree.
///
/// Missing trees are idempotent success so a retry can finish the durable
/// database commit after a process or connection failure. Any symlink or
/// special file fails closed and leaves the deletion authorization pending.
pub async fn delete_release_storage(
    canonical_root: &Path,
    canonical_relative_path: &str,
    staging_root: Option<&Path>,
    staging_relative_path: Option<&str>,
) -> Result<DeletedReleasePaths, ReleaseDeletionError> {
    let canonical = owned_path(canonical_root, canonical_relative_path, "canonical")?;
    let staging = match (staging_root, staging_relative_path) {
        (Some(root), Some(relative)) => {
            Some((owned_path(root, relative, "staging")?, relative.to_owned()))
        }
        (None, None) | (Some(_), None) => None,
        (None, Some(relative)) => {
            return Err(ReleaseDeletionError::InvalidPath {
                kind: "staging",
                path: relative.to_owned(),
            });
        }
    };
    let canonical_relative_path = canonical_relative_path.to_owned();
    let staging_relative_path = staging.as_ref().map(|(_, relative)| relative.clone());

    tokio::task::spawn_blocking(move || {
        remove_owned_tree(&canonical, "canonical")?;
        if let Some((path, _)) = staging {
            remove_owned_tree(&path, "staging")?;
        }
        Ok::<(), ReleaseDeletionError>(())
    })
    .await??;

    Ok(DeletedReleasePaths {
        canonical_relative_path,
        staging_relative_path,
    })
}

fn owned_path(
    root: &Path,
    relative: &str,
    kind: &'static str,
) -> Result<PathBuf, ReleaseDeletionError> {
    let path = Path::new(relative);
    let safe = !relative.trim().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !safe {
        return Err(ReleaseDeletionError::InvalidPath {
            kind,
            path: relative.to_owned(),
        });
    }
    Ok(root.join(path))
}

fn remove_owned_tree(path: &Path, kind: &'static str) -> Result<(), ReleaseDeletionError> {
    match remove_directory_tree(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ReleaseDeletionError::Remove {
            kind,
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn removes_only_the_selected_owned_release_trees_and_is_idempotent() {
        let root = tempfile::tempdir().expect("root");
        let canonical_root = root.path().join("library");
        let staging_root = root.path().join("staging");
        let canonical = canonical_root.join("releases/aa/bb/rel_test");
        let staged = staging_root.join("releases/work-item");
        std::fs::create_dir_all(&canonical).expect("canonical tree");
        std::fs::create_dir_all(&staged).expect("staging tree");
        std::fs::write(canonical.join("track.mp3"), b"canonical").expect("canonical file");
        std::fs::write(staged.join("track.flac"), b"source").expect("staged file");

        for _ in 0..2 {
            delete_release_storage(
                &canonical_root,
                "releases/aa/bb/rel_test",
                Some(&staging_root),
                Some("releases/work-item"),
            )
            .await
            .expect("delete owned trees");
        }
        assert!(!canonical.exists());
        assert!(!staged.exists());
        assert!(canonical_root.exists());
        assert!(staging_root.exists());
    }

    #[tokio::test]
    async fn rejects_traversal_before_touching_storage() {
        let root = tempfile::tempdir().expect("root");
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).expect("outside");

        let result = delete_release_storage(root.path(), "../outside", None, None).await;
        assert!(matches!(
            result,
            Err(ReleaseDeletionError::InvalidPath { .. })
        ));
        assert!(outside.exists());
    }
}

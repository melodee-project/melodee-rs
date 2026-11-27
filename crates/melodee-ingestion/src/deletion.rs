//! Explicit administrator-authorized cleanup of one Melodee-owned Release.
//!
//! This module removes only directory trees derived from durable relative
//! paths below the configured staging and canonical roots. The underlying
//! descriptor-relative operation refuses symlinks and special files.

use std::{io, path::{Component, Path, PathBuf}};

use thiserror::Error;

use crate::secure_fs;

// Match the persistence reservation's maximum work-item scope.
const MAX_LIFECYCLE_HOLDS: usize = 128;

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
    #[error("Release deletion admission failed")]
    Admission(#[from] tokio::sync::AcquireError),
    #[error("too many protected lifecycle holds for one deletion")]
    TooManyLifecycleHolds,
    #[error("the inbound source occurrence {path} is not empty; nothing was removed")]
    InboundNotEmpty { path: PathBuf },
    #[error("the inbound source occurrence {path} changed identity during removal; moved bytes retained")]
    InboundIdentityChanged { path: PathBuf },
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
    delete_release_storage_with_holds(
        canonical_root,
        canonical_relative_path,
        staging_root,
        staging_relative_path,
        Vec::new(),
    )
    .await
}

/// Retain caller-validated lifecycle holds in the actual blocking worker.
/// The caller must independently authorize deletion and keep its own holds
/// until the corresponding fenced database completion commits.
#[doc(hidden)]
pub async fn delete_release_storage_with_holds(
    canonical_root: &Path,
    canonical_relative_path: &str,
    staging_root: Option<&Path>,
    staging_relative_path: Option<&str>,
    holds: Vec<crate::StageLifetimeHold>,
) -> Result<DeletedReleasePaths, ReleaseDeletionError> {
    if holds.len() > MAX_LIFECYCLE_HOLDS {
        return Err(ReleaseDeletionError::TooManyLifecycleHolds);
    }
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

    run_with_lifecycle_holds(holds, move || {
        remove_owned_tree(&canonical, "canonical")?;
        if let Some((path, _)) = staging {
            remove_owned_tree(&path, "staging")?;
        }
        Ok::<(), ReleaseDeletionError>(())
    })
    .await?;

    Ok(DeletedReleasePaths {
        canonical_relative_path,
        staging_relative_path,
    })
}

/// Removes individual media files below a configured root after validating
/// every relative path. Missing files are idempotent success. Only regular
/// file leaves are unlinked; symlinks and special files fail closed.
pub async fn delete_storage_files(
    root: &Path,
    relative_paths: &[String],
) -> Result<Vec<String>, ReleaseDeletionError> {
    delete_storage_files_with_holds(root, relative_paths, Vec::new()).await
}

/// Retain caller-validated lifecycle holds through blocking file removal.
/// Holds are not deletion authorization; callers keep originals until commit.
#[doc(hidden)]
pub async fn delete_storage_files_with_holds(
    root: &Path,
    relative_paths: &[String],
    holds: Vec<crate::StageLifetimeHold>,
) -> Result<Vec<String>, ReleaseDeletionError> {
    if holds.len() > MAX_LIFECYCLE_HOLDS {
        return Err(ReleaseDeletionError::TooManyLifecycleHolds);
    }
    let mut owned = Vec::with_capacity(relative_paths.len());
    for relative in relative_paths {
        owned.push((relative.clone(), owned_path(root, relative, "media")?));
    }
    if owned.is_empty() {
        return Ok(Vec::new());
    }
    let paths = owned
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    run_with_lifecycle_holds(holds, move || {
        for path in paths {
            match crate::secure_fs::unlink_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(ReleaseDeletionError::Remove {
                        kind: "media",
                        path,
                        source,
                    });
                }
            }
        }
        Ok(())
    })
    .await?;
    Ok(owned.into_iter().map(|(relative, _)| relative).collect())
}

/// Remove only the directories pinned by a pre-catalog authorization and this work item's scratch.
pub async fn delete_unpublished_storage(
    id: uuid::Uuid,
    work_root: &Path,
    staging_root: Option<&Path>,
    staging_path: Option<&str>,
    canonical_root: Option<&Path>,
    canonical_path: Option<&str>,
) -> Result<(), ReleaseDeletionError> {
    delete_unpublished_storage_with_holds(
        id,
        work_root,
        staging_root,
        staging_path,
        canonical_root,
        canonical_path,
        Vec::new(),
    )
    .await
}

/// Retain caller-validated lifecycle holds through unpublished cleanup.
/// The caller must keep originals through the fenced database completion.
#[doc(hidden)]
pub async fn delete_unpublished_storage_with_holds(
    id: uuid::Uuid,
    work_root: &Path,
    staging_root: Option<&Path>,
    staging_path: Option<&str>,
    canonical_root: Option<&Path>,
    canonical_path: Option<&str>,
    holds: Vec<crate::StageLifetimeHold>,
) -> Result<(), ReleaseDeletionError> {
    if holds.len() > MAX_LIFECYCLE_HOLDS {
        return Err(ReleaseDeletionError::TooManyLifecycleHolds);
    }
    let scratch = id.simple().to_string();
    let expected = format!("releases/{id}/source");
    if staging_path.is_some_and(|p| p != expected) {
        return Err(ReleaseDeletionError::InvalidPath {
            kind: "staging",
            path: staging_path.unwrap_or_default().to_owned(),
        });
    }
    let mut trees = Vec::new();
    for (kind, root, path) in [
        ("work", Some(work_root), Some(scratch.as_str())),
        ("staging", staging_root, staging_path),
        ("canonical", canonical_root, canonical_path),
    ] {
        if let Some(relative) = path {
            let root = root.ok_or_else(|| ReleaseDeletionError::InvalidPath {
                kind,
                path: relative.to_owned(),
            })?;
            trees.push((kind, root.to_owned(), owned_path(root, relative, kind)?));
        }
    }
    run_with_lifecycle_holds(holds, move || {
        for (kind, root, _) in &trees {
            crate::secure_fs::open_directory(root).map_err(|source| {
                ReleaseDeletionError::Remove {
                    kind,
                    path: root.clone(),
                    source,
                }
            })?;
        }
        for (kind, _, path) in trees {
            remove_owned_tree(&path, kind)?;
        }
        Ok(())
    })
    .await
}

async fn run_with_lifecycle_holds<T: Send + 'static>(
    holds: Vec<crate::StageLifetimeHold>,
    work: impl FnOnce() -> Result<T, ReleaseDeletionError> + Send + 'static,
) -> Result<T, ReleaseDeletionError> {
    let (_, result) = run_with_owned_operation_scope(holds, move |_| work()).await?;
    Ok(result)
}

// The operation scope moves into the actual worker and can only return after
// it terminates. A cancelled waiter therefore cannot release exclusive locks
// while a detached filesystem operation is still running. This is ownership
// plumbing only: it does not itself authorize a filesystem operation.
pub(crate) async fn run_with_owned_operation_scope<S: Send + 'static, T: Send + 'static>(
    mut scope: S,
    work: impl FnOnce(&mut S) -> Result<T, ReleaseDeletionError> + Send + 'static,
) -> Result<(S, T), ReleaseDeletionError> {
    let permit = crate::admission::heavy_ingestion().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = work(&mut scope)?;
        Ok((scope, result))
    })
    .await?
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

/// Removes one empty producer-owned inbound occurrence directory after a
/// durable (`begin_pre_catalog_purge`) reservation. The operation is
/// descriptor-relative and no-follow: the leaf must be an empty directory; it
/// is moved to a unique no-replace quarantine sibling, re-verified against
/// the pinned occurrence, re-checked for emptiness, and only then removed.
/// A producer that adds bytes in any window therefore cannot lose them: the
/// moved directory stays under its quarantine name and the error retains the
/// reservation for reviewed recovery. A missing leaf is idempotent success.
pub async fn delete_purged_inbound_occurrence(
    root: &Path,
    relative: &str,
) -> Result<(), ReleaseDeletionError> {
    let root = root.to_path_buf();
    let relative = relative.to_owned();
    run_with_lifecycle_holds(Vec::new(), move || {
        let path = owned_path(&root, &relative, "inbound")?;
        match secure_fs::remove_empty_occurrence_tree(&root, &relative) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => {
                let kind = source.kind();
                Err(match kind {
                    io::ErrorKind::DirectoryNotEmpty => {
                        ReleaseDeletionError::InboundNotEmpty { path }
                    }
                    io::ErrorKind::InvalidData => {
                        ReleaseDeletionError::InboundIdentityChanged { path }
                    }
                    _ => ReleaseDeletionError::Remove {
                        kind: "inbound",
                        path,
                        source,
                    },
                })
            }
        }
    })
    .await
}

fn remove_owned_tree(path: &Path, kind: &'static str) -> Result<(), ReleaseDeletionError> {
    match secure_fs::remove_directory_tree(path) {
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_delete_keeps_lifecycle_hold_until_worker_exits() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        struct DropProbe(Arc<AtomicBool>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let hold = crate::StageLifetimeHold::from_arc(Arc::new(DropProbe(dropped.clone())));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let task = tokio::spawn(async move {
            run_with_lifecycle_holds(vec![hold], move || {
                started_tx.send(()).expect("started");
                release_rx.recv().expect("release");
                Ok(())
            })
            .await
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("started");
        task.abort();
        assert!(!dropped.load(Ordering::SeqCst));
        release_tx.send(()).expect("release worker");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("hold released");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_worker_keeps_unique_operation_scope_until_termination() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        struct UniqueScope(Arc<AtomicBool>);
        impl Drop for UniqueScope {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let scope = UniqueScope(dropped.clone());
        let task = tokio::spawn(async move {
            run_with_owned_operation_scope(scope, move |_| {
                started_tx.send(()).expect("worker started");
                release_rx.recv().expect("release worker");
                Ok(())
            })
            .await
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("started");
        task.abort();
        assert!(!dropped.load(Ordering::SeqCst));
        release_tx.send(()).expect("release worker");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("unique scope released after worker");
    }

    #[tokio::test]
    async fn too_many_holds_refuse_before_deleting_files() {
        let root = tempfile::tempdir().expect("root");
        std::fs::write(root.path().join("track"), b"preserve").expect("fixture");
        let hold = crate::StageLifetimeHold::from_arc(std::sync::Arc::new(()));
        let result = delete_storage_files_with_holds(
            root.path(),
            &["track".to_owned()],
            vec![hold; MAX_LIFECYCLE_HOLDS + 1],
        )
        .await;
        assert!(matches!(
            result,
            Err(ReleaseDeletionError::TooManyLifecycleHolds)
        ));
        assert_eq!(
            std::fs::read(root.path().join("track")).expect("preserved"),
            b"preserve"
        );
    }

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
    async fn removes_individual_media_files_and_rejects_traversal() {
        let root = tempfile::tempdir().expect("root");
        let canonical_root = root.path().join("library");
        let media = canonical_root.join("releases/aa/bb/rel_test/01.flac");
        std::fs::create_dir_all(media.parent().expect("media parent")).expect("media tree");
        std::fs::write(&media, b"audio").expect("media file");
        let outside = root.path().join("outside.flac");
        std::fs::write(&outside, b"keep").expect("outside file");

        delete_storage_files(
            &canonical_root,
            &[
                "releases/aa/bb/rel_test/01.flac".to_owned(),
                "releases/aa/bb/rel_test/missing.flac".to_owned(),
            ],
        )
        .await
        .expect("idempotent media deletion");
        assert!(!media.exists());

        let result = delete_storage_files(&canonical_root, &["../outside.flac".to_owned()]).await;
        assert!(matches!(
            result,
            Err(ReleaseDeletionError::InvalidPath { .. })
        ));
        assert!(outside.exists());
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn removes_only_verified_empty_inbound_occurrences() {
        let root = tempfile::tempdir().expect("root");
        let occurrence = root.path().join("releases").join("never acquired");
        std::fs::create_dir_all(&occurrence).expect("empty occurrence");

        delete_purged_inbound_occurrence(root.path(), "releases/never acquired")
            .await
            .expect("removed empty occurrence");
        assert!(!occurrence.exists());
        assert!(root.path().join("releases").exists());

        // A missing leaf is idempotent success.
        delete_purged_inbound_occurrence(root.path(), "releases/never acquired")
            .await
            .expect("idempotent");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refuses_nonempty_inbound_occurrences_and_retains_bytes() {
        let root = tempfile::tempdir().expect("root");
        let occurrence = root.path().join("producer");
        std::fs::create_dir(&occurrence).expect("occurrence");
        std::fs::write(occurrence.join("track.flac"), b"producer bytes").expect("bytes");

        let result = delete_purged_inbound_occurrence(root.path(), "producer").await;
        assert!(matches!(result, Err(ReleaseDeletionError::InboundNotEmpty { .. })));
        assert_eq!(
            std::fs::read(occurrence.join("track.flac")).expect("bytes retained"),
            b"producer bytes"
        );
        // Nothing was moved or removed; the occurrence keeps its exact name.
        assert!(occurrence.is_dir());
        // After the producer clears its content, removal succeeds.
        std::fs::remove_file(occurrence.join("track.flac")).expect("clear content");
        delete_purged_inbound_occurrence(root.path(), "producer")
            .await
            .expect("removed after cleared");
        assert!(!occurrence.exists());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refuses_inbound_occurrence_leaf_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().expect("root");
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).expect("outside");
        let link = root.path().join("alias");
        symlink(&outside, &link).expect("link");

        let result = delete_purged_inbound_occurrence(root.path(), "alias").await;
        assert!(matches!(result, Err(ReleaseDeletionError::Remove { .. })));
        assert!(outside.exists());
        assert!(link.exists());
    }
}

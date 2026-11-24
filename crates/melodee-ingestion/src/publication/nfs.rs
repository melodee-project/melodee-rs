//! Manifest-last installation on Linux NFS (ADR-0202).
use super::*;

fn error(operation: &'static str, path: &Path, source: io::Error) -> PublicationError {
    PublicationError::Io {
        operation,
        path: path.to_owned(),
        source,
    }
}

fn conflict(path: &Path, reason: &str) -> PublicationError {
    PublicationError::TargetConflict {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

pub(super) fn on_nfs(lock: &File) -> io::Result<bool> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        Ok(rustix::fs::fstatfs(lock)?.f_type == rustix::fs::NFS_SUPER_MAGIC)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = lock;
        Ok(false)
    }
}

/// The lock inode authenticates a reservation across process restarts. Its
/// name inside the reserved directory binds it to exactly one immutable plan.
pub(super) struct Reservation {
    marker: PathBuf,
    pub(super) scratch: PathBuf,
    scratch_owned: std::cell::Cell<bool>,
}

impl Reservation {
    pub(super) fn new(target: &Path, plan_hash: &str) -> Self {
        Self {
            marker: target.join(format!(".melodee-publishing-{plan_hash}")),
            scratch: target.with_file_name(format!(".{}.publication.tmp", uuid::Uuid::now_v7())),
            scratch_owned: std::cell::Cell::new(false),
        }
    }

    pub(super) fn marker_name(&self) -> Result<&str, PublicationError> {
        self.marker
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| PublicationError::UnsafePath(self.marker.clone()))
    }

    pub(super) fn owned(&self, lock: &File) -> Result<bool, PublicationError> {
        if !path_exists(&self.marker)? {
            return Ok(false);
        }
        let marker = open_regular_file(&self.marker)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let identity = |file: &File| {
                file.metadata()
                    .map(|m| (m.dev(), m.ino()))
                    .map_err(|e| error("identify publication reservation", &self.marker, e))
            };
            Ok(identity(&marker)? == identity(lock)?)
        }
        #[cfg(not(unix))]
        {
            let _ = (marker, lock);
            Ok(false)
        }
    }

    pub(super) fn resume(
        &self,
        target: &Path,
        bundle: &PublicationBundle,
        manifest: &[u8],
        lock: &File,
    ) -> Result<bool, PublicationError> {
        if !self.owned(lock)? {
            return Ok(false);
        }
        if path_exists(&target.join("manifest.json"))? {
            verify_sealed_bundle(target, bundle, manifest, Some(self.marker_name()?))?;
            self.finish(target, lock)?;
            return Ok(false);
        }
        Ok(true)
    }

    pub(super) fn reserve(
        &self,
        target: &Path,
        parent: &Path,
        lock_path: &Path,
    ) -> Result<(), PublicationError> {
        crate::secure_fs::mkdir_new(target).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                conflict(target, "another entry already owns the Release directory")
            } else {
                error("reserve new Release directory", target, e)
            }
        })?;
        crate::secure_fs::link_new(lock_path, &self.marker)
            .map_err(|e| error("mark publication reservation", &self.marker, e))?;
        fsync_directory(target)?;
        fsync_directory(parent)
    }

    /// Validate every existing name before continuing an interrupted attempt.
    /// Each existing asset is also rehashed by the normal copy/verify path.
    pub(super) fn check_inventory(
        &self,
        target: &Path,
        plan: &PublicationPlan,
    ) -> Result<(), PublicationError> {
        let marker = self.marker_name()?;
        let mut allowed = std::collections::BTreeSet::from([marker, "manifest.json"]);
        for asset in &plan.assets {
            allowed.insert(&asset.canonical_path);
        }
        for name in collect_bundle_files(target)? {
            if !allowed.contains(name.as_str()) {
                return Err(conflict(
                    target,
                    "unexpected file in reserved Release directory",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn finish(&self, target: &Path, lock: &File) -> Result<(), PublicationError> {
        if !self.owned(lock)? {
            return Err(conflict(target, "publication reservation changed"));
        }
        crate::secure_fs::unlink_file(&self.marker)
            .map_err(|e| error("remove committed reservation marker", &self.marker, e))?;
        fsync_directory(target)
    }

    /// Each locked attempt gets a fresh exclusive scratch name. A crashed
    /// attempt may leave a hard link to a committed asset: never truncate it.
    pub(super) fn create_scratch(&self) -> io::Result<File> {
        let file = crate::secure_fs::open_write_new(&self.scratch)?;
        self.scratch_owned.set(true);
        Ok(file)
    }

    pub(super) fn install_file(&self, dst: &Path) -> io::Result<()> {
        let installed = crate::secure_fs::link_new(&self.scratch, dst);
        crate::secure_fs::unlink_file(&self.scratch)?;
        self.scratch_owned.set(false);
        installed
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.scratch_owned.get() {
            let _ = crate::secure_fs::unlink_file(&self.scratch);
        }
    }
}

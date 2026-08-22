//! Canonical media adoption validation (ADR-0148 §D.3). The migration
//! never moves, re-encodes, re-hashes, fingerprints, or deletes media; when
//! the operator mounts the canonical root it validates — read-only — that
//! each adopted path is contained, symlink-safe, present, and the expected
//! size.

use std::{
    io,
    path::{Component, Path, PathBuf},
};

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod secure_media {
    use std::{os::unix::ffi::OsStrExt, path::Path};

    use rustix::{
        fd::OwnedFd,
        fs::{CWD, FileType, Mode, OFlags, fstat, openat},
    };

    use super::*;

    fn io_error(error: rustix::io::Errno) -> io::Error {
        io::Error::from_raw_os_error(error.raw_os_error())
    }

    pub fn open_absolute_directory(path: &Path) -> io::Result<OwnedFd> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "canonical media root must be absolute",
            ));
        }
        let mut current = openat(
            CWD,
            b"/".as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io_error)?;
        for component in path.components() {
            let Component::Normal(name) = component else {
                if matches!(component, Component::RootDir) {
                    continue;
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "canonical media root contains traversal",
                ));
            };
            current = openat(
                &current,
                name.as_bytes(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(io_error)?;
        }
        Ok(current)
    }

    pub fn open_regular_file(directory: &OwnedFd, name: &std::ffi::OsStr) -> io::Result<u64> {
        let file = openat(
            directory,
            name.as_bytes(),
            // NONBLOCK prevents a raced FIFO from blocking validation before
            // its descriptor type can be checked.
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let stat = fstat(&file).map_err(io_error)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "canonical media path is not a regular file",
            ));
        }
        Ok(stat.st_size as u64)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
mod secure_media {
    use std::path::Path;

    use super::*;

    pub fn open_absolute_directory(_path: &Path) -> io::Result<std::fs::File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant canonical media validation is unsupported on this platform",
        ))
    }
}

/// Outcome of one adopted-file check. Missing/size-mismatch files do not
/// block catalog identity; an escape is unsafe and the track is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCheck {
    /// File present under the root and the size matches the legacy metadata.
    Ok,
    /// No file at the adopted path (recorded, imported anyway).
    Missing,
    /// File present but the byte length differs from legacy `FileSize`.
    SizeMismatch,
    /// The path is not safe to adopt, such as a traversal, symlink, or special
    /// file encountered by the no-follow validation boundary.
    Unsafe,
}

/// Precomputed canonical root for per-file validation.
pub struct CanonicalRootValidator {
    canonical_root: PathBuf,
}

impl CanonicalRootValidator {
    /// # Errors
    /// Returns an actionable message when the root does not exist.
    pub fn new(root: &Path) -> Result<Self, String> {
        let canonical_root = root
            .canonicalize()
            .map_err(|e| format!("cannot resolve --canonical-root {}: {e}", root.display()))?;
        let metadata = std::fs::metadata(&canonical_root)
            .map_err(|e| format!("cannot inspect --canonical-root {}: {e}", root.display()))?;
        if !metadata.is_dir() {
            return Err(format!(
                "--canonical-root is not a directory: {}",
                root.display()
            ));
        }
        Ok(Self { canonical_root })
    }

    /// Read-only validation of one adopted canonical-relative path.
    pub fn check(&self, relative: &str, expected_size: Option<i64>) -> MediaCheck {
        let mut components = Vec::new();
        for component in Path::new(relative).components() {
            match component {
                Component::Normal(name) => components.push(name),
                // Legacy media paths are canonical-relative. Rejecting all
                // other components keeps an absolute path or traversal from
                // replacing the configured root during validation.
                Component::CurDir
                | Component::ParentDir
                | Component::RootDir
                | Component::Prefix(_) => return MediaCheck::Unsafe,
            }
        }
        let Some(leaf) = components.pop() else {
            return MediaCheck::Unsafe;
        };

        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        let actual = {
            let directory = match secure_media::open_absolute_directory(&self.canonical_root) {
                Ok(directory) => directory,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return MediaCheck::Missing;
                }
                Err(_) => return MediaCheck::Unsafe,
            };
            let mut directory = directory;
            for component in components {
                directory = match rustix::fs::openat(
                    &directory,
                    component,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::CLOEXEC
                        | rustix::fs::OFlags::NOFOLLOW,
                    rustix::fs::Mode::empty(),
                ) {
                    Ok(directory) => directory,
                    Err(error) if error == rustix::io::Errno::NOENT => {
                        return MediaCheck::Missing;
                    }
                    Err(_) => return MediaCheck::Unsafe,
                };
            }
            match secure_media::open_regular_file(&directory, leaf) {
                Ok(size) => size,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return MediaCheck::Missing;
                }
                Err(_) => return MediaCheck::Unsafe,
            }
        };

        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
        let actual = {
            let _ = (&components, leaf);
            return MediaCheck::Unsafe;
        };

        if let Some(expected) = expected_size.filter(|e| *e >= 0)
            && i64::try_from(actual).unwrap_or(i64::MAX) != expected
        {
            return MediaCheck::SizeMismatch;
        }
        MediaCheck::Ok
    }
}

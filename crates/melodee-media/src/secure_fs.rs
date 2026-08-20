//! Descriptor-relative media staging operations.
//!
//! Normalization handles operator-provided source paths and generated output
//! paths. A metadata check followed by a path-based open is not sufficient at
//! that boundary: a concurrent replacement can turn a regular file into a
//! symlink between the two operations. Unix implementations therefore walk
//! directory components with `O_NOFOLLOW` and open the leaf with the same
//! restriction. This module deliberately exposes only the operations needed
//! by normalization; it is not a general filesystem abstraction.

use std::{fs::File, io};

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod platform {
    use std::{
        ffi::OsStr,
        os::unix::ffi::OsStrExt,
        path::{Component, Path},
    };

    use rustix::{
        fd::OwnedFd,
        fs::{CWD, FileType, Mode, OFlags, fstat, mkdirat, openat},
    };

    use super::*;

    fn io_error(error: rustix::io::Errno) -> io::Error {
        io::Error::from_raw_os_error(error.raw_os_error())
    }

    fn open_root() -> io::Result<OwnedFd> {
        openat(
            CWD,
            b"/" as &[u8],
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io_error)
    }

    fn open_absolute_directory(path: &Path, create: bool) -> io::Result<OwnedFd> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "media filesystem path must be absolute",
            ));
        }

        let mut current = open_root()?;
        for component in path.components() {
            let Component::Normal(name) = component else {
                if matches!(component, Component::RootDir) {
                    continue;
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "media filesystem path contains traversal",
                ));
            };
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
            match openat(&current, name.as_bytes(), flags, Mode::empty()) {
                Ok(next) => current = next,
                Err(error) if create && error == rustix::io::Errno::NOENT => {
                    match mkdirat(&current, name.as_bytes(), Mode::RWXU) {
                        Ok(()) => {}
                        Err(error) if error == rustix::io::Errno::EXIST => {}
                        Err(error) => return Err(io_error(error)),
                    }
                    current = openat(&current, name.as_bytes(), flags, Mode::empty())
                        .map_err(io_error)?;
                }
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(current)
    }

    fn open_parent(path: &Path) -> io::Result<(OwnedFd, &OsStr)> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "media leaf has no parent")
        })?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "media leaf has no name"))?;
        Ok((open_absolute_directory(parent, false)?, name))
    }

    pub fn create_directory_tree(path: &Path) -> io::Result<()> {
        open_absolute_directory(path, true).map(drop)
    }

    /// Create one unique directory relative to a verified parent. Unlike
    /// `std::fs::create_dir`, the parent is opened without following a
    /// symlink immediately before `mkdirat`.
    pub fn create_directory(path: &Path) -> io::Result<()> {
        let (parent, name) = open_parent(path)?;
        mkdirat(&parent, name.as_bytes(), Mode::RWXU).map_err(io_error)
    }

    pub fn open_read(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path)?;
        let file = openat(
            &parent,
            name.as_bytes(),
            // NONBLOCK prevents a raced FIFO from making the worker wait
            // before its descriptor type can be checked.
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let stat = fstat(&file).map_err(io_error)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "media source is not a regular file",
            ));
        }
        Ok(File::from(file))
    }

    /// Copy one source file into a newly-created destination. Both leaves are
    /// opened without following symlinks; the destination is never replaced.
    pub fn copy_new(source: &Path, destination: &Path) -> io::Result<u64> {
        let mut source = open_read(source)?;
        let mut destination = open_write_new(destination)?;
        let copied = io::copy(&mut source, &mut destination)?;
        destination.sync_all()?;
        Ok(copied)
    }

    pub fn open_write_new(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path)?;
        openat(
            &parent,
            name.as_bytes(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(io_error)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
mod platform {
    use std::path::Path;

    use super::*;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant media filesystem operations are unsupported on this platform",
        )
    }

    pub fn create_directory_tree(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn create_directory(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn open_read(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn copy_new(_source: &Path, _destination: &Path) -> io::Result<u64> {
        Err(unsupported())
    }

    pub fn open_write_new(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }
}

pub(crate) use platform::{copy_new, create_directory, create_directory_tree, open_read};

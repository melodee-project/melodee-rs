//! Race-resistant filesystem operations rooted in verified directory handles.
//!
//! Configured roots are operator-managed directories. Every operation below
//! still walks from the filesystem root one component at a time with
//! `O_DIRECTORY|O_NOFOLLOW`, then opens or renames the leaf relative to the
//! verified parent handle. A concurrent symlink swap therefore fails instead
//! of redirecting publication outside the configured tree.

use std::{ffi::OsStr, fs::File, io, path::Component};

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod platform {
    use std::{os::unix::ffi::OsStrExt, path::Path};

    use rustix::{
        fd::OwnedFd,
        fs::{CWD, Mode, OFlags, RenameFlags, mkdirat, openat, renameat_with},
    };

    use super::*;

    fn io_error(error: rustix::io::Errno) -> io::Error {
        io::Error::from_raw_os_error(error.raw_os_error())
    }

    fn open_root() -> io::Result<OwnedFd> {
        openat(
            CWD,
            b"/".as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io_error)
    }

    fn open_absolute_directory(path: &Path, create: bool) -> io::Result<OwnedFd> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "trusted filesystem path must be absolute",
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
                    "trusted filesystem path contains traversal",
                ));
            };
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
            match openat(&current, name.as_bytes(), flags, Mode::empty()) {
                Ok(next) => current = next,
                Err(error) if create && error == rustix::io::Errno::NOENT => {
                    match mkdirat(
                        &current,
                        name.as_bytes(),
                        Mode::RWXU | Mode::RGRP | Mode::XGRP,
                    ) {
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

    fn open_parent(path: &Path, create: bool) -> io::Result<(OwnedFd, &OsStr)> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "filesystem leaf has no parent")
        })?;
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "filesystem leaf has no name")
        })?;
        Ok((open_absolute_directory(parent, create)?, name))
    }

    pub fn create_directory_tree(path: &Path) -> io::Result<()> {
        open_absolute_directory(path, true).map(drop)
    }

    pub fn open_read(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path, false)?;
        openat(
            &parent,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io_error)
    }

    pub fn open_write(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path, false)?;
        openat(
            &parent,
            name.as_bytes(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(io_error)
    }

    pub fn open_lock(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path, false)?;
        openat(
            &parent,
            name.as_bytes(),
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(io_error)
    }

    pub fn open_directory(path: &Path) -> io::Result<File> {
        open_absolute_directory(path, false).map(File::from)
    }

    pub fn rename_noreplace(source: &Path, destination: &Path) -> io::Result<()> {
        let (source_parent, source_name) = open_parent(source, false)?;
        let (destination_parent, destination_name) = open_parent(destination, false)?;
        renameat_with(
            &source_parent,
            source_name.as_bytes(),
            &destination_parent,
            destination_name.as_bytes(),
            RenameFlags::NOREPLACE,
        )
        .map_err(io_error)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
mod platform {
    use std::{
        fs::{self, OpenOptions},
        path::Path,
    };

    use super::*;

    pub fn create_directory_tree(path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    pub fn open_read(path: &Path) -> io::Result<File> {
        File::open(path)
    }

    pub fn open_write(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
    }

    pub fn open_lock(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
    }

    pub fn open_directory(path: &Path) -> io::Result<File> {
        File::open(path)
    }

    pub fn rename_noreplace(source: &Path, destination: &Path) -> io::Result<()> {
        #[cfg(windows)]
        {
            fs::rename(source, destination)
        }
        #[cfg(not(windows))]
        {
            let _ = (source, destination);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic no-replace rename is unsupported on this platform",
            ))
        }
    }
}

pub(crate) use platform::{
    create_directory_tree, open_directory, open_lock, open_read, open_write, rename_noreplace,
};

//! Descriptor-relative operations for the owned Tantivy generation root.
//!
//! Search generations are rebuildable, but their manifests and retirement
//! markers still participate in ownership and activation decisions. Keep the
//! path checks and the filesystem operation in one no-follow boundary so a
//! concurrent replacement cannot redirect a read or write through an
//! operator-managed symlink.

use std::{fs::File, io, path::Component};

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod platform {
    use std::{os::unix::ffi::OsStrExt, path::Path};

    use rustix::{
        fd::OwnedFd,
        fs::{
            AtFlags, CWD, FileType, Mode, OFlags, fstat, mkdirat, openat, renameat, statat,
            unlinkat,
        },
    };

    use super::*;

    fn io_error(error: rustix::io::Errno) -> io::Error {
        io::Error::from_raw_os_error(error.raw_os_error())
    }

    fn open_absolute_directory(path: &Path) -> io::Result<OwnedFd> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "search filesystem path must be absolute",
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
                    "search filesystem path contains traversal",
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

    fn open_parent(path: &Path) -> io::Result<(OwnedFd, &std::ffi::OsStr)> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "search leaf has no parent")
        })?;
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "search leaf has no name")
        })?;
        Ok((open_absolute_directory(parent)?, name))
    }

    pub fn create_directory(path: &Path) -> io::Result<()> {
        let (parent, name) = open_parent(path)?;
        mkdirat(&parent, name.as_bytes(), Mode::RWXU).map_err(io_error)
    }

    /// Open two absolute directories without following any component and
    /// prove that they refer to the same filesystem object.
    pub fn same_directory(first: &Path, second: &Path) -> io::Result<bool> {
        let first = open_absolute_directory(first)?;
        let second = open_absolute_directory(second)?;
        let first_stat = fstat(&first).map_err(io_error)?;
        let second_stat = fstat(&second).map_err(io_error)?;
        Ok(first_stat.st_dev == second_stat.st_dev && first_stat.st_ino == second_stat.st_ino)
    }

    pub fn open_read(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path)?;
        let file = openat(
            &parent,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let stat = fstat(&file).map_err(io_error)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "search manifest is not a regular file",
            ));
        }
        Ok(File::from(file))
    }

    pub fn open_write_new(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path)?;
        let file = openat(
            &parent,
            name.as_bytes(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io_error)?;
        Ok(File::from(file))
    }

    pub fn remove_regular_file(path: &Path) -> io::Result<()> {
        let (parent, name) = open_parent(path)?;
        let stat = statat(&parent, name.as_bytes(), AtFlags::SYMLINK_NOFOLLOW).map_err(io_error)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to remove a non-regular search file",
            ));
        }
        unlinkat(&parent, name.as_bytes(), AtFlags::empty()).map_err(io_error)
    }

    /// Replace the destination entry without resolving either leaf through a
    /// symlink. The source and destination parents are held open while the
    /// atomic rename occurs.
    pub fn rename_replace(source: &Path, destination: &Path) -> io::Result<()> {
        let (source_parent, source_name) = open_parent(source)?;
        let (destination_parent, destination_name) = open_parent(destination)?;
        renameat(
            &source_parent,
            source_name.as_bytes(),
            &destination_parent,
            destination_name.as_bytes(),
        )
        .map_err(io_error)
    }

    pub fn sync_directory(path: &Path) -> io::Result<()> {
        File::from(open_absolute_directory(path)?).sync_all()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
mod platform {
    use std::path::Path;

    use super::*;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant search filesystem operations are unsupported on this platform",
        )
    }

    pub fn create_directory(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn same_directory(_first: &Path, _second: &Path) -> io::Result<bool> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "descriptor-relative directory identity is unavailable on this platform",
        ))
    }

    pub fn open_read(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn open_write_new(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn remove_regular_file(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn rename_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn sync_directory(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }
}

pub(crate) use platform::{
    create_directory, open_read, open_write_new, remove_regular_file, rename_replace,
    same_directory, sync_directory,
};

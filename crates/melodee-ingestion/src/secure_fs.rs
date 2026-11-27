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
        fs::{
            AtFlags, CWD, Dir, FileType, Mode, OFlags, RenameFlags, fstat, linkat, mkdirat, openat,
            renameat_with, statat, unlinkat,
        },
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
        let file = openat(
            &parent,
            name.as_bytes(),
            // A raced FIFO must not block a worker before its type is checked.
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let stat = fstat(&file).map_err(io_error)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "acquisition source is not a regular file",
            ));
        }
        Ok(File::from(file))
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

    /// Create a regular file without following a leaf symlink or replacing an
    /// existing destination. This is used for first-write staging where a
    /// path check must not become an overwrite race.
    pub fn open_write_new(path: &Path) -> io::Result<File> {
        let (parent, name) = open_parent(path, false)?;
        openat(
            &parent,
            name.as_bytes(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
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

    pub fn mkdir_new(path: &Path) -> io::Result<()> {
        let (parent, name) = open_parent(path, false)?;
        mkdirat(&parent, name, Mode::RWXU | Mode::RGRP | Mode::XGRP).map_err(io_error)
    }

    pub fn link_new(source: &Path, destination: &Path) -> io::Result<()> {
        let (from, leaf) = open_parent(source, false)?;
        let (to, name) = open_parent(destination, false)?;
        linkat(&from, leaf, &to, name, AtFlags::empty()).map_err(io_error)
    }

    pub fn unlink_file(path: &Path) -> io::Result<()> {
        let (parent, name) = open_parent(path, false)?;
        unlinkat(&parent, name, AtFlags::empty()).map_err(io_error)
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

    /// Rename one child between already pinned directories without replacement.
    pub fn rename_child_noreplace(
        source_parent: &File,
        source_name: &OsStr,
        destination_parent: &File,
        destination_name: &OsStr,
    ) -> io::Result<()> {
        validate_child_name(source_name)?;
        validate_child_name(destination_name)?;
        renameat_with(
            source_parent,
            source_name.as_bytes(),
            destination_parent,
            destination_name.as_bytes(),
            RenameFlags::NOREPLACE,
        )
        .map_err(io_error)
    }

    pub fn open_child_directory(parent: &File, name: &OsStr) -> io::Result<File> {
        validate_child_name(name)?;
        openat(
            parent,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io_error)
    }

    pub fn mkdir_private_child(parent: &File, name: &OsStr) -> io::Result<()> {
        validate_child_name(name)?;
        mkdirat(
            parent,
            name.as_bytes(),
            Mode::RUSR | Mode::WUSR | Mode::XUSR,
        )
        .map_err(io_error)
    }

    fn validate_child_name(name: &OsStr) -> io::Result<()> {
        let mut parts = Path::new(name).components();
        if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected one directory entry name",
            ));
        }
        Ok(())
    }

    fn remove_contents(directory: &OwnedFd, display_path: &Path) -> io::Result<()> {
        let mut entries = Dir::read_from(directory)?;
        for entry in &mut entries {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let entry_path = display_path.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
            let stat = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io_error)?;
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => {
                    let child = openat(
                        directory,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                        Mode::empty(),
                    )
                    .map_err(io_error)?;
                    remove_contents(&child, &entry_path)?;
                    unlinkat(directory, name, AtFlags::REMOVEDIR).map_err(io_error)?;
                }
                FileType::RegularFile => {
                    unlinkat(directory, name, AtFlags::empty()).map_err(io_error)?;
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "refusing to remove symlink or special file {}",
                            entry_path.display()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Remove one owned directory without following a path component or a
    /// child symlink. The parent and every child are held or opened through
    /// directory descriptors so a concurrent replacement cannot redirect
    /// cleanup outside the configured root.
    pub fn remove_directory_tree(path: &Path) -> io::Result<()> {
        let (parent, name) = open_parent(path, false)?;
        let directory = openat(
            &parent,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io_error)?;
        remove_contents(&directory, path)?;
        drop(directory);
        unlinkat(&parent, name.as_bytes(), AtFlags::REMOVEDIR).map_err(io_error)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
mod platform {
    use std::path::Path;

    use super::*;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant ingestion filesystem operations are unsupported on this platform",
        )
    }

    pub fn create_directory_tree(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn open_read(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn open_write(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn open_write_new(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn open_lock(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn open_directory(_path: &Path) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn mkdir_new(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn link_new(_source: &Path, _destination: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn unlink_file(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn rename_noreplace(_source: &Path, _destination: &Path) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn rename_child_noreplace(
        _source_parent: &File,
        _source_name: &OsStr,
        _destination_parent: &File,
        _destination_name: &OsStr,
    ) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn open_child_directory(_parent: &File, _name: &OsStr) -> io::Result<File> {
        Err(unsupported())
    }
    pub fn mkdir_private_child(_parent: &File, _name: &OsStr) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn remove_directory_tree(_path: &Path) -> io::Result<()> {
        Err(unsupported())
    }
}

pub(crate) use platform::{
    create_directory_tree, link_new, mkdir_new, mkdir_private_child, open_child_directory,
    open_directory, open_lock, open_read, open_write, open_write_new, remove_directory_tree,
    rename_child_noreplace, rename_noreplace, unlink_file,
};

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
mod custody_tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::{fs, os::unix::fs::MetadataExt, path::PathBuf};

    #[test]
    fn pinned_parent_noreplace_preserves_replacement_for_verification() {
        let root = std::env::temp_dir().join(format!(
            "melodee-custody-primitives-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(root.join("inbound/candidate")).expect("candidate");
        fs::create_dir_all(root.join("private")).expect("private");
        fs::write(root.join("inbound/candidate/file"), b"original").expect("original");
        let inbound = open_directory(&root.join("inbound")).expect("pinned parent");
        let private = open_directory(&root.join("private")).expect("private parent");
        let original =
            open_child_directory(&inbound, OsStr::new("candidate")).expect("pinned original");
        fs::rename(
            root.join("inbound/candidate"),
            root.join("inbound/original"),
        )
        .expect("park original");
        fs::create_dir(root.join("inbound/candidate")).expect("replacement");
        fs::write(root.join("inbound/candidate/file"), b"replacement").expect("replacement file");
        rename_child_noreplace(
            &inbound,
            OsStr::new("candidate"),
            &private,
            OsStr::new("held"),
        )
        .expect("atomic move");
        let moved = open_child_directory(&private, OsStr::new("held")).expect("moved");
        assert_ne!(
            original.metadata().expect("original stat").ino(),
            moved.metadata().expect("moved stat").ino()
        );
        rename_child_noreplace(
            &private,
            OsStr::new("held"),
            &inbound,
            OsStr::new("candidate"),
        )
        .expect("restore replacement");
        assert_eq!(
            fs::read(root.join("inbound/candidate/file")).expect("replacement"),
            b"replacement"
        );
        assert_eq!(
            fs::read(root.join("inbound/original/file")).expect("original"),
            b"original"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn pinned_parent_operations_reject_traversal_and_existing_destinations() {
        let root: PathBuf = std::env::temp_dir().join(format!(
            "melodee-custody-noreplace-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(root.join("source")).expect("source");
        fs::create_dir(root.join("existing")).expect("existing");
        let parent = open_directory(&root).expect("parent");
        assert!(
            rename_child_noreplace(
                &parent,
                OsStr::new("source"),
                &parent,
                OsStr::new("existing")
            )
            .is_err()
        );
        assert!(open_child_directory(&parent, OsStr::new("../outside")).is_err());
        assert!(mkdir_private_child(&parent, OsStr::new("nested/path")).is_err());
        assert!(root.join("source").is_dir() && root.join("existing").is_dir());
        fs::remove_dir_all(root).expect("cleanup");
    }
}

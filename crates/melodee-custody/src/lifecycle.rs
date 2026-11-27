//! Descriptor-pinned lifecycle lock acquisition. No filesystem deletion occurs.

use std::{
    fs::File,
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf},
};

use rustix::fs::{Mode, OFlags, openat};
use uuid::Uuid;

const MAX_LOCKS: usize = 128;
const MAX_ANCESTORS: usize = 256;

/// Immutable registry identity to compare against the actual held inode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleLockIdentity {
    pub work_item_id: Uuid,
    pub lock_root_generation: String,
    pub lock_generation: String,
}

/// A non-cloneable holder of exclusive kernel locks. This is not a database
/// reservation, a complete topology proof, or a deletion authorization.
pub struct HeldLifecycleLocks {
    root_path: PathBuf,
    // Keep every walked directory open so its inode cannot be recycled while
    // this scope relies on its identity, including the lock root itself.
    ancestry: Vec<File>,
    root_generation: String,
    locks: Vec<(LifecycleLockIdentity, File)>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn os_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

fn generation(file: &File) -> io::Result<String> {
    let metadata = file.metadata()?;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

fn validate_acl(file: &File) -> io::Result<()> {
    let fs = rustix::fs::fstatfs(file).map_err(os_error)?;
    const SUPPORTED: [u32; 4] = [0x9123_683e, 0xef53, 0x5846_5342, 0x0102_1994];
    if !SUPPORTED.contains(&(fs.f_type as u32)) {
        return Err(invalid("unsupported filesystem"));
    }
    for name in ["system.posix_acl_access", "system.posix_acl_default"] {
        match rustix::fs::fgetxattr_query(file, name) {
            Err(rustix::io::Errno::NODATA) => {}
            Ok(_) => return Err(invalid("nontrivial ACL")),
            Err(error) => return Err(os_error(error)),
        }
    }
    Ok(())
}

pub(crate) fn open_private_root(path: &Path) -> io::Result<(File, Vec<File>)> {
    if !path.is_absolute()
        || path
            .as_os_str()
            .as_bytes()
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
    {
        return Err(invalid("invalid root path"));
    }
    let mut current = File::from(
        openat(
            rustix::fs::CWD,
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(os_error)?,
    );
    let mut ancestry = Vec::new();
    let uid = rustix::process::geteuid().as_raw();
    let mut check = |file: &File| -> io::Result<()> {
        if ancestry.len() == MAX_ANCESTORS {
            return Err(invalid("root too deep"));
        }
        let metadata = file.metadata()?;
        if (metadata.uid() != 0 && metadata.uid() != uid) || metadata.mode() & 0o022 != 0 {
            return Err(invalid("unprotected root ancestry"));
        }
        validate_acl(file)?;
        ancestry.push(file.try_clone()?);
        Ok(())
    };
    check(&current)?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = File::from(
                    openat(
                        &current,
                        name.as_bytes(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(os_error)?,
                );
                check(&current)?;
            }
            _ => return Err(invalid("invalid root component")),
        }
    }
    let metadata = current.metadata()?;
    if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(invalid("root must be service-owned and private"));
    }
    Ok((current, ancestry))
}

fn open_lock(root: &File, identity: &LifecycleLockIdentity) -> io::Result<File> {
    let name = format!("work-{}.lock", identity.work_item_id.simple());
    let file = File::from(
        openat(
            root,
            name.as_str(),
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(os_error)?,
    );
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || generation(&file)? != identity.lock_generation
    {
        return Err(invalid("lock identity changed"));
    }
    validate_acl(&file)?;
    Ok(file)
}

// Reopen one component at a time, retaining only the previous and next
// descriptors. The original ancestry remains pinned by the scope.
pub(crate) fn reopen_root_check(path: &Path, held: &[File]) -> io::Result<File> {
    let mut current = File::from(
        openat(
            rustix::fs::CWD,
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(os_error)?,
    );
    let mut index = 0;
    let uid = rustix::process::geteuid().as_raw();
    let check = |file: &File, index: usize| -> io::Result<()> {
        let old = held
            .get(index)
            .ok_or_else(|| invalid("lock ancestry changed"))?;
        let metadata = file.metadata()?;
        if generation(old)? != generation(file)?
            || (metadata.uid() != 0 && metadata.uid() != uid)
            || metadata.mode() & 0o022 != 0
        {
            return Err(invalid("lock ancestry changed"));
        }
        validate_acl(file)?;
        Ok(())
    };
    check(&current, index)?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = File::from(
                    openat(
                        &current,
                        name.as_bytes(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(os_error)?,
                );
                index += 1;
                check(&current, index)?;
            }
            _ => return Err(invalid("invalid root component")),
        }
    }
    if index + 1 != held.len() {
        return Err(invalid("lock ancestry changed"));
    }
    let metadata = current.metadata()?;
    if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(invalid("root must be service-owned and private"));
    }
    Ok(current)
}

impl HeldLifecycleLocks {
    /// Adopt an already-open lock descriptor without releasing its flock. The
    /// descriptor and its current name are independently compared with the
    /// registry identity before it is retained. This does not establish DB
    /// authority, root topology, or permission to mutate media.
    pub fn acquire_from_descriptor(
        root_path: &Path,
        identity: LifecycleLockIdentity,
        file: File,
    ) -> io::Result<Self> {
        let (root, ancestry) = open_private_root(root_path)?;
        let root_generation = generation(&root)?;
        if identity.lock_root_generation != root_generation {
            return Err(invalid("lock root changed"));
        }
        let current = open_lock(&root, &identity)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || generation(&file)? != generation(&current)?
        {
            return Err(invalid("transferred lock identity changed"));
        }
        validate_acl(&file)?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(os_error)?;
        let held = Self {
            root_path: root_path.to_path_buf(),
            ancestry,
            root_generation,
            locks: vec![(identity, file)],
        };
        held.revalidate()?;
        Ok(held)
    }

    /// Acquire a sorted bounded set without waiting. The caller must freshly
    /// revalidate its database reservation after acquiring all locks. This
    /// primitive grants no authority to mutate filesystem content.
    pub fn acquire(root_path: &Path, identities: &[LifecycleLockIdentity]) -> io::Result<Self> {
        if identities.is_empty()
            || identities.len() > MAX_LOCKS
            || identities
                .windows(2)
                .any(|pair| pair[0].work_item_id >= pair[1].work_item_id)
        {
            return Err(invalid("invalid lifecycle lock scope"));
        }
        let (root, ancestry) = open_private_root(root_path)?;
        let root_generation = generation(&root)?;
        let mut locks = Vec::with_capacity(identities.len());
        for identity in identities {
            if identity.lock_root_generation != root_generation {
                return Err(invalid("lock root changed"));
            }
            let file = open_lock(&root, identity)?;
            rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .map_err(os_error)?;
            locks.push((identity.clone(), file));
        }
        let held = Self {
            root_path: root_path.to_path_buf(),
            ancestry,
            root_generation,
            locks,
        };
        held.revalidate()?;
        Ok(held)
    }

    /// Reopen the root and each lock name, comparing them with held descriptors.
    pub fn revalidate(&self) -> io::Result<()> {
        let root = reopen_root_check(&self.root_path, &self.ancestry)?;
        if generation(&root)? != self.root_generation {
            return Err(invalid("lock root changed"));
        }
        for (identity, held) in &self.locks {
            let current = open_lock(&root, identity)?;
            if generation(held)? != generation(&current)? {
                return Err(invalid("held lock changed"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn exact_inode_contention_and_root_replacement_fail_closed() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let root = fixture.path().join("locks");
        fs::create_dir(&root).expect("lock root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root");
        let item = Uuid::now_v7();
        let path = root.join(format!("work-{}.lock", item.simple()));
        fs::write(&path, b"owned lock").expect("lock file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private lock");
        let root_file = File::open(&root).expect("root handle");
        let lock_file = File::open(&path).expect("lock handle");
        let identity = LifecycleLockIdentity {
            work_item_id: item,
            lock_root_generation: generation(&root_file).expect("root identity"),
            lock_generation: generation(&lock_file).expect("lock identity"),
        };
        let held =
            HeldLifecycleLocks::acquire(&root, std::slice::from_ref(&identity)).expect("acquire");
        assert!(HeldLifecycleLocks::acquire(&root, std::slice::from_ref(&identity)).is_err());
        let moved = fixture.path().join("moved");
        fs::rename(&root, &moved).expect("move root");
        fs::create_dir(&root).expect("replacement root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("replacement mode");
        assert!(held.revalidate().is_err());
        assert_eq!(
            fs::read(moved.join(path.file_name().expect("lock name"))).expect("original bytes"),
            b"owned lock"
        );
        fs::remove_dir(&root).expect("remove owned empty replacement");
        fs::rename(&moved, &root).expect("restore root");
        held.revalidate().expect("same root restored");
        drop(held);
        HeldLifecycleLocks::acquire(&root, std::slice::from_ref(&identity))
            .expect("acquire after release");
        let mut changed = identity;
        changed.lock_generation.push('x');
        assert!(HeldLifecycleLocks::acquire(&root, &[changed]).is_err());
    }

    #[test]
    fn rejects_empty_unsorted_and_over_quota_scopes_before_open() {
        let identity = |id| LifecycleLockIdentity {
            work_item_id: Uuid::from_u128(id),
            lock_root_generation: "root".into(),
            lock_generation: "lock".into(),
        };
        let absent = Path::new("/not-an-existing-custody-root");
        assert_eq!(
            HeldLifecycleLocks::acquire(absent, &[])
                .err()
                .expect("reject empty")
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            HeldLifecycleLocks::acquire(absent, &[identity(2), identity(1)])
                .err()
                .expect("reject order")
                .kind(),
            io::ErrorKind::InvalidData
        );
        let too_many: Vec<_> = (1..=129).map(identity).collect();
        assert_eq!(
            HeldLifecycleLocks::acquire(absent, &too_many)
                .err()
                .expect("reject quota")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn unlinked_ancestor_and_root_remain_pinned_and_replacement_is_rejected() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let parent = fixture.path().join("parent");
        let root = parent.join("locks");
        for directory in [&parent, &root] {
            fs::create_dir(directory).expect("owned directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("private directory");
        }
        let item = Uuid::now_v7();
        let name = format!("work-{}.lock", item.simple());
        let lock_path = root.join(&name);
        fs::write(&lock_path, b"owned").expect("owned lock");
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).expect("private lock");
        let identity = LifecycleLockIdentity {
            work_item_id: item,
            lock_root_generation: generation(&File::open(&root).expect("root"))
                .expect("root identity"),
            lock_generation: generation(&File::open(&lock_path).expect("lock"))
                .expect("lock identity"),
        };
        let held = HeldLifecycleLocks::acquire(&root, &[identity]).expect("acquire scope");
        let held_root = held.ancestry.last().expect("held root");
        let held_parent = &held.ancestry[held.ancestry.len() - 2];
        fs::remove_file(&lock_path).expect("remove owned lock name");
        fs::remove_dir(&root).expect("unlink owned root");
        fs::remove_dir(&parent).expect("unlink owned parent");
        assert_eq!(held_root.metadata().expect("pinned root").nlink(), 0);
        assert_eq!(held_parent.metadata().expect("pinned parent").nlink(), 0);
        for directory in [&parent, &root] {
            fs::create_dir(directory).expect("replacement directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("private replacement");
        }
        fs::write(&lock_path, b"replacement").expect("replacement lock");
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600))
            .expect("replacement lock mode");
        assert_ne!(
            generation(held_root).expect("held root identity"),
            generation(&File::open(&root).expect("new root")).expect("new root identity")
        );
        assert_ne!(
            generation(held_parent).expect("held parent identity"),
            generation(&File::open(&parent).expect("new parent")).expect("new parent identity")
        );
        assert!(held.revalidate().is_err());
        assert_eq!(
            fs::read(&lock_path).expect("replacement preserved"),
            b"replacement"
        );
    }

    #[test]
    fn provisioned_descriptor_transfers_without_unlocking() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let root = fixture.path().join("locks");
        fs::create_dir(&root).expect("owned root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root");
        let item = Uuid::now_v7();
        let path = root.join(format!("work-{}.lock", item.simple()));
        fs::write(&path, b"owned").expect("owned lock");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private lock");
        let original = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("original descriptor");
        rustix::fs::flock(
            &original,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .expect("original flock");
        let identity = LifecycleLockIdentity {
            work_item_id: item,
            lock_root_generation: generation(&File::open(&root).expect("root"))
                .expect("root identity"),
            lock_generation: generation(&original).expect("lock identity"),
        };
        assert!(HeldLifecycleLocks::acquire(&root, std::slice::from_ref(&identity)).is_err());
        let transferred =
            HeldLifecycleLocks::acquire_from_descriptor(&root, identity.clone(), original)
                .expect("transfer exact descriptor");
        assert!(HeldLifecycleLocks::acquire(&root, std::slice::from_ref(&identity)).is_err());
        transferred.revalidate().expect("transferred scope intact");
        drop(transferred);
        HeldLifecycleLocks::acquire(&root, std::slice::from_ref(&identity))
            .expect("release only after transfer scope ends");
        let wrong_path = root.join("other-owned-file");
        fs::write(&wrong_path, b"preserve other").expect("other owned file");
        fs::set_permissions(&wrong_path, fs::Permissions::from_mode(0o600))
            .expect("other file mode");
        let wrong = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&wrong_path)
            .expect("wrong descriptor");
        assert!(HeldLifecycleLocks::acquire_from_descriptor(&root, identity, wrong).is_err());
        assert_eq!(fs::read(&path).expect("original preserved"), b"owned");
        assert_eq!(
            fs::read(&wrong_path).expect("other preserved"),
            b"preserve other"
        );
    }
}

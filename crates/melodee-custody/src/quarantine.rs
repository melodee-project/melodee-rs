//! Test-only no-replace quarantine experiment. It never unlinks media and does
//! not bind DB intent, configured roots, producer exclusion or deletion proof.

use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags, StatxFlags, openat, renameat_with, statx};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path},
};
use uuid::Uuid;

const MAX_TEST_FILE: u64 = 8 * 1024 * 1024;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "quarantine observation changed")
}
fn os_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}
fn inode(file: &File) -> io::Result<(u64, u64)> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn mount(file: &File) -> io::Result<u64> {
    let s = statx(file, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID).map_err(os_error)?;
    if s.stx_mask & StatxFlags::MNT_ID.bits() == 0 {
        return Err(invalid());
    }
    Ok(s.stx_mnt_id)
}
fn open_file(parent: &File, name: &OsStr) -> io::Result<File> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(invalid());
    }
    Ok(File::from(
        openat(
            parent,
            name.as_bytes(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(os_error)?,
    ))
}
fn verify_file(file: &mut File, size: u64, hash: [u8; 32]) -> io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let before = file.metadata()?;
    if !before.is_file() || before.nlink() != 1 || before.len() != size || size > MAX_TEST_FILE {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut digest = blake3::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    let mut count = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count = count.checked_add(read as u64).ok_or_else(invalid)?;
        if count > size {
            return Err(invalid());
        }
        digest.update(&buffer[..read]);
    }
    let after = file.metadata()?;
    if count != size
        || digest.finalize().as_bytes() != &hash
        || (
            before.dev(),
            before.ino(),
            before.len(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (
            after.dev(),
            after.ino(),
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        )
    {
        return Err(invalid());
    }
    Ok(())
}

// Retains all file/parent FDs and the unique lock scope after a successful
// move. No method here unlinks the held occurrence or acknowledges a page.
struct QuarantinedObservation {
    _scope: crate::HeldLifecycleLocks,
    _source_parent: File,
    _source_ancestry: Vec<File>,
    _quarantine_parent: File,
    _quarantine_ancestry: Vec<File>,
    _file: File,
}

fn move_observed(
    scope: crate::HeldLifecycleLocks,
    source_path: &Path,
    quarantine_path: &Path,
    source_name: &OsStr,
    intent_id: Uuid,
    size: u64,
    hash: [u8; 32],
    after_name_check: impl FnOnce() -> io::Result<()>,
) -> io::Result<QuarantinedObservation> {
    if intent_id.is_nil()
        || source_path.starts_with(quarantine_path)
        || quarantine_path.starts_with(source_path)
    {
        return Err(invalid());
    }
    scope.revalidate()?;
    // This narrow experiment requires both parents private; actual supported
    // production roots and exact DB root-set identity remain unresolved.
    let (source_parent, source_ancestry) = crate::lifecycle::open_private_root(source_path)?;
    let (quarantine_parent, quarantine_ancestry) =
        crate::lifecycle::open_private_root(quarantine_path)?;
    if mount(&source_parent)? != mount(&quarantine_parent)?
        || inode(&source_parent)?.0 != inode(&quarantine_parent)?.0
    {
        return Err(invalid());
    }
    let mut file = open_file(&source_parent, source_name)?;
    verify_file(&mut file, size, hash)?;
    let named = open_file(&source_parent, source_name)?;
    if inode(&named)? != inode(&file)? {
        return Err(invalid());
    }
    scope.revalidate()?;
    if inode(&crate::lifecycle::reopen_root_check(
        source_path,
        &source_ancestry,
    )?)? != inode(&source_parent)?
        || inode(&crate::lifecycle::reopen_root_check(
            quarantine_path,
            &quarantine_ancestry,
        )?)? != inode(&quarantine_parent)?
    {
        return Err(invalid());
    }
    after_name_check()?;
    let target = format!("quarantine-{}.entry", intent_id.simple());
    renameat_with(
        &source_parent,
        source_name.as_bytes(),
        &quarantine_parent,
        target.as_str(),
        RenameFlags::NOREPLACE,
    )
    .map_err(os_error)?;
    source_parent.sync_all()?;
    quarantine_parent.sync_all()?;
    let mut moved = open_file(&quarantine_parent, OsStr::new(&target))?;
    if inode(&moved)? != inode(&file)? {
        return Err(invalid());
    }
    verify_file(&mut moved, size, hash)?;
    scope.revalidate()?;
    if inode(&crate::lifecycle::reopen_root_check(
        source_path,
        &source_ancestry,
    )?)? != inode(&source_parent)?
        || inode(&crate::lifecycle::reopen_root_check(
            quarantine_path,
            &quarantine_ancestry,
        )?)? != inode(&quarantine_parent)?
    {
        return Err(invalid());
    }
    Ok(QuarantinedObservation {
        _scope: scope,
        _source_parent: source_parent,
        _source_ancestry: source_ancestry,
        _quarantine_parent: quarantine_parent,
        _quarantine_ancestry: quarantine_ancestry,
        _file: moved,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn replacement_between_check_and_move_is_preserved_and_rejected() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let locks = fixture.path().join("locks");
        let source = fixture.path().join("source");
        let quarantine = fixture.path().join("quarantine");
        for dir in [&locks, &source, &quarantine] {
            fs::create_dir(dir).expect("owned directory");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("private mode");
        }
        let item = Uuid::now_v7();
        let lock = locks.join(format!("work-{}.lock", item.simple()));
        fs::write(&lock, b"lock").expect("owned lock");
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
        let generation = |path: &Path| {
            let m = fs::metadata(path).expect("metadata");
            format!("unix:{}:{}", m.dev(), m.ino())
        };
        let identity = crate::LifecycleLockIdentity {
            work_item_id: item,
            lock_root_generation: generation(&locks),
            lock_generation: generation(&lock),
        };
        fs::write(source.join("file"), b"original").expect("owned source");
        let intent = Uuid::now_v7();
        let scope = crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
            .expect("scope");
        let result = move_observed(
            scope,
            &source,
            &quarantine,
            OsStr::new("file"),
            intent,
            8,
            *blake3::hash(b"original").as_bytes(),
            || {
                fs::rename(source.join("file"), source.join("parked"))?;
                fs::write(source.join("file"), b"replaced")?;
                Ok(())
            },
        );
        assert!(
            result.is_err(),
            "replacement inode must never become a verified observation"
        );
        assert_eq!(
            fs::read(source.join("parked")).expect("original retained"),
            b"original"
        );
        assert_eq!(
            fs::read(quarantine.join(format!("quarantine-{}.entry", intent.simple())))
                .expect("replacement retained"),
            b"replaced"
        );
        fs::write(source.join("second"), b"original").expect("second owned source");
        let scope = crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
            .expect("conflict scope");
        assert!(
            move_observed(
                scope,
                &source,
                &quarantine,
                OsStr::new("second"),
                intent,
                8,
                *blake3::hash(b"original").as_bytes(),
                || Ok(())
            )
            .is_err(),
            "existing quarantine name must not be replaced"
        );
        assert_eq!(
            fs::read(source.join("second")).expect("second retained"),
            b"original"
        );
        assert_eq!(
            fs::read(quarantine.join(format!("quarantine-{}.entry", intent.simple())))
                .expect("existing quarantine retained"),
            b"replaced"
        );
        let fresh = Uuid::now_v7();
        let scope = crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity))
            .expect("fresh scope");
        let moved = move_observed(
            scope,
            &source,
            &quarantine,
            OsStr::new("second"),
            fresh,
            8,
            *blake3::hash(b"original").as_bytes(),
            || Ok(()),
        )
        .expect("exact moved observation");
        assert!(
            crate::HeldLifecycleLocks::acquire(&locks, std::slice::from_ref(&identity)).is_err()
        );
        assert_eq!(
            fs::read(quarantine.join(format!("quarantine-{}.entry", fresh.simple())))
                .expect("exact bytes retained"),
            b"original"
        );
        drop(moved);
    }
}

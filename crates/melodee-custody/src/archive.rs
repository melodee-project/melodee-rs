//! Test-only filesystem half of an archive; it grants no DB authority.

use rustix::fs::{Mode, OFlags, openat};
use sha2::Digest;
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const DOMAIN: &[u8] = b"melodee-admin-deletion-tombstone-v1\0";
const MAX_BYTES: usize = 8192;

#[derive(Clone, Copy)]
struct Tombstone<'a> {
    reservation: Uuid,
    completion: Uuid,
    snapshot: Uuid,
    roots: Uuid,
    completed_micros: i64,
    kind: &'a str,
    actor: &'a str,
    reason: &'a str,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid archive metadata")
}
fn os_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

fn canonical(record: &Tombstone<'_>) -> io::Result<Vec<u8>> {
    if [
        record.reservation,
        record.completion,
        record.snapshot,
        record.roots,
    ]
    .iter()
    .any(Uuid::is_nil)
        || !matches!(record.kind, "release" | "track" | "artist")
        || record.actor.trim().is_empty()
        || record.reason.trim().is_empty()
    {
        return Err(invalid());
    }
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(DOMAIN);
    for value in [
        record.reservation,
        record.completion,
        record.snapshot,
        record.roots,
    ] {
        out.extend_from_slice(value.as_bytes());
    }
    out.extend_from_slice(&record.completed_micros.to_be_bytes());
    for value in [record.kind, record.actor, record.reason] {
        let len = u32::try_from(value.len()).map_err(|_| invalid())?;
        if out
            .len()
            .checked_add(4 + value.len())
            .is_none_or(|size| size > MAX_BYTES)
        {
            return Err(invalid());
        }
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    Ok(out)
}

// Owns the real lock scope throughout create/fsync/readback. Its verified
// local bytes are not a database completion proof or authority to clear quota.
struct ProvisionalArchiveRoot {
    path: PathBuf,
    root: File,
    ancestry: Vec<File>,
    root_generation: String,
    lock: ProvisionalArchiveLock,
}

struct ProvisionalArchive {
    _root: ProvisionalArchiveRoot,
    _file: File,
    digest: [u8; 32],
}

// A distinct per-completion lock is needed after catalog work-item locks have
// been removed. DB intent/epoch checks and acknowledgement are not implemented
// here, so neither constructor grants archive or cleanup authority.
struct ProvisionalArchiveLock {
    path: PathBuf,
    root: File,
    ancestry: Vec<File>,
    root_generation: String,
    name: String,
    file: File,
    lock_generation: String,
}

impl ProvisionalArchiveLock {
    fn create_reserved(path: &Path, root_generation: &str, completion: Uuid) -> io::Result<Self> {
        if completion.is_nil() {
            return Err(invalid());
        }
        let (root, ancestry) = crate::lifecycle::open_private_root(path)?;
        if identity(&root)? != root_generation {
            return Err(invalid());
        }
        let name = format!("archive-{}.lock", completion.simple());
        let file = File::from(
            openat(
                &root,
                name.as_str(),
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(os_error)?,
        );
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(os_error)?;
        file.sync_all()?;
        root.sync_all()?;
        let lock_generation = identity(&file)?;
        let lock = Self {
            path: path.to_path_buf(),
            root,
            ancestry,
            root_generation: root_generation.to_owned(),
            name,
            file,
            lock_generation,
        };
        lock.revalidate()?;
        Ok(lock)
    }

    fn reopen_acknowledged(
        path: &Path,
        root_generation: &str,
        completion: Uuid,
        lock_generation: &str,
    ) -> io::Result<Self> {
        if completion.is_nil() {
            return Err(invalid());
        }
        let (root, ancestry) = crate::lifecycle::open_private_root(path)?;
        if identity(&root)? != root_generation {
            return Err(invalid());
        }
        let name = format!("archive-{}.lock", completion.simple());
        let file = File::from(
            openat(
                &root,
                name.as_str(),
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(os_error)?,
        );
        if identity(&file)? != lock_generation {
            return Err(invalid());
        }
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(os_error)?;
        let lock = Self {
            path: path.to_path_buf(),
            root,
            ancestry,
            root_generation: root_generation.to_owned(),
            name,
            file,
            lock_generation: lock_generation.to_owned(),
        };
        lock.revalidate()?;
        Ok(lock)
    }

    fn revalidate(&self) -> io::Result<()> {
        let root = crate::lifecycle::reopen_root_check(&self.path, &self.ancestry)?;
        if identity(&root)? != self.root_generation || identity(&self.root)? != self.root_generation
        {
            return Err(invalid());
        }
        let current = File::from(
            openat(
                &root,
                self.name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(os_error)?,
        );
        let metadata = self.file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || identity(&self.file)? != self.lock_generation
            || identity(&current)? != self.lock_generation
        {
            return Err(invalid());
        }
        for name in ["system.posix_acl_access", "system.posix_acl_default"] {
            match rustix::fs::fgetxattr_query(&self.file, name) {
                Err(rustix::io::Errno::NODATA) => {}
                Ok(_) => return Err(invalid()),
                Err(error) => return Err(os_error(error)),
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArchiveCheckpoint {
    Created,
    Synced,
}

fn identity(file: &File) -> io::Result<String> {
    let m = file.metadata()?;
    Ok(format!("unix:{}:{}", m.dev(), m.ino()))
}

impl ProvisionalArchiveRoot {
    fn open(path: &Path, expected_root: &str, lock: ProvisionalArchiveLock) -> io::Result<Self> {
        lock.revalidate()?;
        if lock.path != path || lock.root_generation != expected_root {
            return Err(invalid());
        }
        let (root, ancestry) = crate::lifecycle::open_private_root(path)?;
        if identity(&root)? != expected_root {
            return Err(invalid());
        }
        let result = Self {
            path: path.to_path_buf(),
            root,
            ancestry,
            root_generation: expected_root.to_owned(),
            lock,
        };
        result.revalidate()?;
        Ok(result)
    }
    fn revalidate(&self) -> io::Result<()> {
        self.lock.revalidate()?;
        let current = crate::lifecycle::reopen_root_check(&self.path, &self.ancestry)?;
        if identity(&self.root)? != self.root_generation
            || identity(&current)? != self.root_generation
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn write(self, record: &Tombstone<'_>) -> io::Result<ProvisionalArchive> {
        self.write_with_hook(record, |_| Ok(()))
    }
    fn write_with_hook(
        self,
        record: &Tombstone<'_>,
        mut hook: impl FnMut(ArchiveCheckpoint) -> io::Result<()>,
    ) -> io::Result<ProvisionalArchive> {
        let bytes = canonical(record)?;
        if self.lock.name != format!("archive-{}.lock", record.completion.simple()) {
            return Err(invalid());
        }
        self.revalidate()?;
        let name = format!("admin-tombstone-{}.v1", record.completion.simple());
        let mut file = File::from(
            openat(
                &self.root,
                name.as_str(),
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(os_error)?,
        );
        hook(ArchiveCheckpoint::Created)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.root.sync_all()?;
        hook(ArchiveCheckpoint::Synced)?;
        self.revalidate()?;
        let mut reopened = File::from(
            openat(
                &self.root,
                name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(os_error)?,
        );
        let metadata = reopened.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || identity(&reopened)? != identity(&file)?
            || metadata.len() != bytes.len() as u64
        {
            return Err(invalid());
        }
        let mut checked = Vec::new();
        (&mut reopened)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut checked)?;
        if checked != bytes {
            return Err(invalid());
        }
        let digest = sha2::Sha256::digest(&checked).into();
        self.revalidate()?;
        Ok(ProvisionalArchive {
            _root: self,
            _file: file,
            digest,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn canonical_encoding_is_bounded_and_exact() {
        let id = Uuid::from_u128(1);
        let mut record = Tombstone {
            reservation: id,
            completion: id,
            snapshot: id,
            roots: id,
            completed_micros: -3,
            kind: "track",
            actor: "actor",
            reason: "reason",
        };
        let bytes = canonical(&record).expect("canonical");
        assert_eq!(&bytes[..DOMAIN.len()], DOMAIN);
        assert_eq!(&bytes[DOMAIN.len()..DOMAIN.len() + 16], id.as_bytes());
        let offset = DOMAIN.len() + 64;
        assert_eq!(&bytes[offset..offset + 8], &(-3_i64).to_be_bytes());
        assert_eq!(&bytes[offset + 8..offset + 12], &5_u32.to_be_bytes());
        assert_eq!(&bytes[offset + 12..offset + 17], b"track");
        record.actor = " ";
        assert!(canonical(&record).is_err());
        record.actor = "actor";
        let huge = "x".repeat(MAX_BYTES);
        record.reason = &huge;
        assert!(canonical(&record).is_err());
    }

    #[test]
    fn per_completion_lock_excludes_writers_and_refuses_replacement() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let parent = fixture.path().join("archive");
        fs::create_dir(&parent).expect("owned archive parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("private mode");
        let root_generation = identity(&File::open(&parent).expect("root")).expect("root identity");
        let completion = Uuid::now_v7();
        let lock = ProvisionalArchiveLock::create_reserved(&parent, &root_generation, completion)
            .expect("exclusive reserved lock");
        let lock_generation = lock.lock_generation.clone();
        assert!(
            ProvisionalArchiveLock::create_reserved(&parent, &root_generation, completion).is_err()
        );
        assert!(
            ProvisionalArchiveLock::reopen_acknowledged(
                &parent,
                &root_generation,
                completion,
                &lock_generation
            )
            .is_err()
        );
        drop(lock);
        let reopened = ProvisionalArchiveLock::reopen_acknowledged(
            &parent,
            &root_generation,
            completion,
            &lock_generation,
        )
        .expect("exact acknowledged lock");
        let path = parent.join(format!("archive-{}.lock", completion.simple()));
        let parked = parent.join("parked-owned-lock");
        fs::rename(&path, &parked).expect("park owned lock");
        fs::write(&path, b"replacement").expect("replacement bytes");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("replacement mode");
        assert!(reopened.revalidate().is_err());
        assert_eq!(
            fs::read(&path).expect("replacement retained"),
            b"replacement"
        );
        assert!(parked.exists());
    }
    #[test]
    fn archive_is_exact_no_replace_and_keeps_scope() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let archive = fixture.path().join("archive");
        fs::create_dir(&archive).expect("owned root");
        fs::set_permissions(&archive, fs::Permissions::from_mode(0o700)).expect("private mode");
        let root_id =
            identity(&File::open(&archive).expect("archive root")).expect("root identity");
        let record = Tombstone {
            reservation: Uuid::now_v7(),
            completion: Uuid::now_v7(),
            snapshot: Uuid::now_v7(),
            roots: Uuid::now_v7(),
            completed_micros: 123,
            kind: "release",
            actor: "operator",
            reason: "owned fixture",
        };
        let lock = ProvisionalArchiveLock::create_reserved(&archive, &root_id, record.completion)
            .expect("reserved archive lock");
        let lock_generation = lock.lock_generation.clone();
        let root = ProvisionalArchiveRoot::open(&archive, &root_id, lock).expect("archive root");
        let expected = canonical(&record).expect("canonical bytes");
        let proof = root.write(&record).expect("write and verify");
        assert_eq!(
            proof.digest,
            <[u8; 32]>::from(sha2::Sha256::digest(&expected))
        );
        assert!(
            ProvisionalArchiveLock::reopen_acknowledged(
                &archive,
                &root_id,
                record.completion,
                &lock_generation
            )
            .is_err()
        );
        let name = format!("admin-tombstone-{}.v1", record.completion.simple());
        assert_eq!(
            fs::read(archive.join(&name)).expect("archive bytes"),
            expected
        );
        drop(proof);
        let lock = ProvisionalArchiveLock::reopen_acknowledged(
            &archive,
            &root_id,
            record.completion,
            &lock_generation,
        )
        .expect("released archive lock");
        let root = ProvisionalArchiveRoot::open(&archive, &root_id, lock).expect("archive root");
        assert!(
            root.write(&record).is_err(),
            "unacknowledged existing name must not be adopted"
        );
        assert_eq!(
            fs::read(archive.join(name)).expect("archive retained"),
            expected
        );

        let interrupted = Tombstone {
            completion: Uuid::now_v7(),
            ..record
        };
        let interrupted_path = archive.join(format!(
            "admin-tombstone-{}.v1",
            interrupted.completion.simple()
        ));
        let lock =
            ProvisionalArchiveLock::create_reserved(&archive, &root_id, interrupted.completion)
                .expect("interrupted lock");
        let interrupted_lock_generation = lock.lock_generation.clone();
        let root = ProvisionalArchiveRoot::open(&archive, &root_id, lock).expect("archive root");
        assert!(
            root.write_with_hook(&interrupted, |checkpoint| {
                if checkpoint == ArchiveCheckpoint::Created {
                    Err(io::Error::from(io::ErrorKind::Interrupted))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(
            fs::metadata(&interrupted_path)
                .expect("interrupted file retained")
                .len(),
            0
        );
        let lock = ProvisionalArchiveLock::reopen_acknowledged(
            &archive,
            &root_id,
            interrupted.completion,
            &interrupted_lock_generation,
        )
        .expect("retry lock");
        let root = ProvisionalArchiveRoot::open(&archive, &root_id, lock).expect("archive root");
        assert!(
            root.write(&interrupted).is_err(),
            "interrupted existing file must remain quarantined"
        );

        let changed = Tombstone {
            completion: Uuid::now_v7(),
            ..record
        };
        let changed_path = archive.join(format!(
            "admin-tombstone-{}.v1",
            changed.completion.simple()
        ));
        let changed_bytes = vec![0x78; canonical(&changed).expect("expected size").len()];
        let lock = ProvisionalArchiveLock::create_reserved(&archive, &root_id, changed.completion)
            .expect("mutation lock");
        let root = ProvisionalArchiveRoot::open(&archive, &root_id, lock).expect("archive root");
        assert!(
            root.write_with_hook(&changed, |checkpoint| {
                if checkpoint == ArchiveCheckpoint::Synced {
                    fs::write(&changed_path, &changed_bytes)?;
                }
                Ok(())
            })
            .is_err(),
            "same-size archive mutation must fail readback"
        );
        assert_eq!(
            fs::read(changed_path).expect("changed fixture retained"),
            changed_bytes
        );

        let cancelled = Tombstone {
            completion: Uuid::now_v7(),
            ..record
        };
        let lock =
            ProvisionalArchiveLock::create_reserved(&archive, &root_id, cancelled.completion)
                .expect("cancellation lock");
        let cancelled_lock_generation = lock.lock_generation.clone();
        let root = ProvisionalArchiveRoot::open(&archive, &root_id, lock).expect("archive root");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = root.write_with_hook(&cancelled, |checkpoint| {
                if checkpoint == ArchiveCheckpoint::Created {
                    started_tx.send(()).map_err(io::Error::other)?;
                    release_rx.recv().map_err(io::Error::other)?;
                }
                Ok(())
            });
            drop(result);
            let _ = done_tx.send(());
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker started");
        drop(worker); // A cancelled waiter does not stop its blocking worker.
        assert!(
            ProvisionalArchiveLock::reopen_acknowledged(
                &archive,
                &root_id,
                cancelled.completion,
                &cancelled_lock_generation
            )
            .is_err()
        );
        release_tx.send(()).expect("release worker");
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker done");
        ProvisionalArchiveLock::reopen_acknowledged(
            &archive,
            &root_id,
            cancelled.completion,
            &cancelled_lock_generation,
        )
        .expect("lock released only after worker termination");
    }
}

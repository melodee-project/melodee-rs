//! Linux-only, fail-closed sealing for newly created stage-file inodes.
//!
//! This test-only helper is not wired into acquisition. Its caller must separately
//! validate the protected namespace, fence the lease, verify source and stage
//! content, and retain/reobserve the original source. It never opens an
//! existing file for writing or enables verity on a caller-supplied file.

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    sync::Arc,
};

use composefs_ioctls::fsverity::{
    EnableVerityError, MeasureVerityError, fs_ioc_enable_verity, fs_ioc_measure_verity,
};
use rustix::fs::{Mode, OFlags, openat};

use crate::{acquisition::AcquisitionError, custody::PrivateStageParent};

#[derive(Debug, thiserror::Error)]
pub(crate) enum StageVerityError {
    #[error("stage filesystem does not support fs-verity")]
    Unsupported,
    #[error("stage file has a writable descriptor or mapping")]
    WriterOpen,
    #[error("stage file was unexpectedly already sealed")]
    AlreadyEnabled,
    #[error("stage file identity or verity digest changed")]
    Replaced,
    #[error("source file changed during protected copy")]
    SourceChanged,
    #[error("protected copy was cancelled")]
    Cancelled,
    #[error("stage namespace validation failed: {0}")]
    Namespace(#[from] AcquisitionError),
    #[error("stage filesystem operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("stage verity operation failed: {0}")]
    Verity(String),
}

/// The sole constructor creates a fresh file, so no existing source inode can
/// be passed to the sealing API. Its capability revalidates the private
/// namespace and directory ancestry at every stage boundary.
pub(crate) struct CreatedStageFile {
    parent: Arc<PrivateStageParent>,
    name: OsString,
    writer: File,
    reader: File,
}

/// Immutable file bytes and their kernel-enforced digest. Directory entries
/// remain mutable; read from this descriptor and revalidate the namespace.
pub(crate) struct SealedStageFile {
    parent: Arc<PrivateStageParent>,
    name: OsString,
    reader: File,
    verity_digest: [u8; 32],
}

fn validate_name(name: &OsStr) -> io::Result<()> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected one stage entry name",
        ));
    }
    Ok(())
}

fn as_io(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

fn open_reader(parent: &File, name: &OsStr) -> io::Result<File> {
    openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(as_io)
}

fn same_regular_file(left: &File, right: &File) -> io::Result<bool> {
    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.is_file() && right.is_file() && (left.dev(), left.ino()) == (right.dev(), right.ino()))
}

impl CreatedStageFile {
    pub(crate) fn create(
        parent: &Arc<PrivateStageParent>,
        name: &OsStr,
    ) -> Result<Self, StageVerityError> {
        validate_name(name)?;
        let directory = parent.checked_file()?;
        let writer = File::from(
            openat(
                directory,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(as_io)?,
        );
        let reader = open_reader(directory, name)?;
        if !same_regular_file(&writer, &reader)? {
            return Err(StageVerityError::Replaced);
        }
        parent.checked_file()?;
        Ok(Self {
            parent: Arc::clone(parent),
            name: name.to_owned(),
            writer,
            reader,
        })
    }

    /// Seal after all writes terminate. The caller must hold blocking-work
    /// admission until this function actually returns, even on cancellation.
    /// Errors leave the uniquely created entry for fenced recovery; they do not
    /// delete a potentially replaced entry or authorize a weaker fallback.
    pub(crate) fn seal(self) -> Result<SealedStageFile, StageVerityError> {
        self.seal_with(|reader| fs_ioc_enable_verity(reader, 1, 4096))
    }

    fn seal_with(
        self,
        enable: impl FnOnce(&File) -> Result<(), EnableVerityError>,
    ) -> Result<SealedStageFile, StageVerityError> {
        let Self {
            parent,
            name,
            mut writer,
            reader,
        } = self;
        writer.flush()?;
        writer.sync_all()?;
        let current = open_reader(parent.checked_file()?, &name)?;
        if !same_regular_file(&writer, &reader)? || !same_regular_file(&reader, &current)? {
            return Err(StageVerityError::Replaced);
        }
        drop(current);
        drop(writer);
        enable(&reader).map_err(|error| match error {
            EnableVerityError::FilesystemNotSupported => StageVerityError::Unsupported,
            EnableVerityError::FileOpenedForWrite => StageVerityError::WriterOpen,
            EnableVerityError::AlreadyEnabled => StageVerityError::AlreadyEnabled,
            other => StageVerityError::Verity(other.to_string()),
        })?;
        let verity_digest = measure(&reader)?;
        reader.sync_all()?;
        parent.checked_file()?.sync_all()?;
        let sealed = SealedStageFile {
            parent,
            name,
            reader,
            verity_digest,
        };
        sealed.verify_current()?;
        Ok(sealed)
    }
}

impl Write for CreatedStageFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writer.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// Copy one already-open source file into a fresh protected inode and seal it.
/// The callback observes the exact copied chunks for provisional v3/v4
/// evidence; neither this result nor unchanged metadata proves the source was
/// stable during copying. Callers must hold admission until termination and
/// perform the independent source/stage/source protocol before publication.
pub(crate) fn copy_opened_file_and_seal(
    mut source: File,
    parent: &Arc<PrivateStageParent>,
    name: &OsStr,
    cancelled: impl Fn() -> bool,
    mut observe: impl FnMut(u64, &[u8]) -> Result<(), StageVerityError>,
) -> Result<SealedStageFile, StageVerityError> {
    let before = source.metadata()?;
    if !before.is_file() {
        return Err(StageVerityError::SourceChanged);
    }
    source.seek(SeekFrom::Start(0))?;
    let mut destination = CreatedStageFile::create(parent, name)?;
    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::AcquisitionCopy,
    );
    let mut remaining = before.len();
    let mut index = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining != 0 {
        if cancelled() {
            return Err(StageVerityError::Cancelled);
        }
        let count = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| StageVerityError::SourceChanged)?;
        source.read_exact(&mut buffer[..count]).map_err(|error| {
            operation.mark_bytes_incomplete();
            if error.kind() == io::ErrorKind::UnexpectedEof {
                StageVerityError::SourceChanged
            } else {
                StageVerityError::Io(error)
            }
        })?;
        operation.add_read(count as u64);
        destination.write_all(&buffer[..count]).map_err(|error| {
            operation.mark_bytes_incomplete();
            StageVerityError::Io(error)
        })?;
        operation.add_written(count as u64);
        observe(index, &buffer[..count])?;
        remaining -= count as u64;
        index = index
            .checked_add(1)
            .ok_or(StageVerityError::SourceChanged)?;
    }
    if cancelled() {
        return Err(StageVerityError::Cancelled);
    }
    let mut extra = [0_u8; 1];
    let trailing = source.read(&mut extra).map_err(|error| {
        operation.mark_bytes_incomplete();
        StageVerityError::Io(error)
    })?;
    operation.add_read(trailing as u64);
    if trailing != 0 || source.metadata()?.len() != before.len() {
        return Err(StageVerityError::SourceChanged);
    }
    let sealed = destination.seal()?;
    operation.finish();
    Ok(sealed)
}

fn measure(reader: &File) -> Result<[u8; 32], StageVerityError> {
    fs_ioc_measure_verity::<32>(reader, 1).map_err(|error| match error {
        MeasureVerityError::FilesystemNotSupported => StageVerityError::Unsupported,
        other => StageVerityError::Verity(other.to_string()),
    })
}

impl SealedStageFile {
    /// Kernel measurement and inode identity for provisional durable evidence.
    /// This tuple alone never authorizes a receipt or source removal.
    pub(crate) fn evidence(&self) -> Result<([u8; 32], u64, u64), StageVerityError> {
        self.verify_current()?;
        let metadata = self.reader.metadata()?;
        Ok((self.verity_digest, metadata.dev(), metadata.ino()))
    }

    /// Reopen a previously created sealed file under the protected parent.
    /// The caller must obtain the expected values from fenced durable evidence;
    /// this operation never enables verity on an existing file.
    pub(crate) fn reopen(
        parent: &Arc<PrivateStageParent>,
        name: &OsStr,
        expected_digest: [u8; 32],
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<Self, StageVerityError> {
        validate_name(name)?;
        let reader = open_reader(parent.checked_file()?, name)?;
        let metadata = reader.metadata()?;
        if !metadata.is_file()
            || (metadata.dev(), metadata.ino()) != (expected_device, expected_inode)
        {
            return Err(StageVerityError::Replaced);
        }
        let sealed = Self {
            parent: Arc::clone(parent),
            name: name.to_owned(),
            reader,
            verity_digest: expected_digest,
        };
        sealed.verify_current()?;
        Ok(sealed)
    }

    pub(crate) fn reader(&self) -> &File {
        &self.reader
    }

    pub(crate) fn verify_current(&self) -> Result<(), StageVerityError> {
        let current = open_reader(self.parent.checked_file()?, &self.name)?;
        if !same_regular_file(&self.reader, &current)? || measure(&current)? != self.verity_digest {
            return Err(StageVerityError::Replaced);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::{config::InboundSource, custody::validate_custody_namespace, source::GuardedRoot};
    use std::{
        io::{Read, Write},
        os::unix::fs::PermissionsExt,
        path::PathBuf,
    };

    struct Fixture {
        _root: tempfile::TempDir,
        stage: PathBuf,
    }

    impl Fixture {
        fn path(&self) -> &Path {
            &self.stage
        }
    }

    fn directory() -> (Fixture, Arc<PrivateStageParent>) {
        let candidates = [
            std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            std::env::current_dir().ok(),
            std::env::var_os("HOME").map(PathBuf::from),
        ];
        let service_uid = rustix::process::geteuid().as_raw();
        let other_uid = if service_uid == 1 { 2 } else { 1 };
        for base in candidates.into_iter().flatten() {
            let Ok(root) = tempfile::tempdir_in(base) else {
                continue;
            };
            let source = root.path().join("source");
            let private = root.path().join("private");
            let stage = private.join("nested");
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir(&private).unwrap();
            std::fs::create_dir(&stage).unwrap();
            std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o700)).unwrap();
            let Ok(namespace) = validate_custody_namespace(&private, &source, other_uid) else {
                continue;
            };
            let namespace = Arc::new(namespace);
            let guarded = GuardedRoot::open(
                InboundSource::from_root(source, None, Some("test".into())).unwrap(),
            )
            .unwrap();
            let parent = Arc::new(
                PrivateStageParent::open(namespace, guarded, Path::new("nested")).unwrap(),
            );
            return (Fixture { _root: root, stage }, parent);
        }
        panic!("no writable ACL-free private stage parent available for positive capability tests");
    }

    fn replace_parent(root: &Fixture) -> PathBuf {
        let original = root.path().with_file_name("preserved-nested");
        std::fs::rename(root.path(), &original).unwrap();
        std::fs::create_dir(root.path()).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        original
    }

    #[test]
    fn replaced_private_parent_cannot_create_or_seal_a_stage_file() {
        let (root, parent) = directory();
        let original = replace_parent(&root);
        assert!(matches!(
            CreatedStageFile::create(&parent, OsStr::new("new")),
            Err(StageVerityError::Namespace(_))
        ));
        assert!(!root.path().join("new").exists());
        assert!(!original.join("new").exists());

        let (root, parent) = directory();
        let mut stage = CreatedStageFile::create(&parent, OsStr::new("file")).unwrap();
        stage.write_all(b"original").unwrap();
        let original = replace_parent(&root);
        std::fs::write(root.path().join("file"), b"replacement").unwrap();
        let mut called = false;
        let result = stage.seal_with(|_| {
            called = true;
            Err(EnableVerityError::FilesystemNotSupported)
        });
        assert!(matches!(result, Err(StageVerityError::Namespace(_))));
        assert!(!called, "detached parent must be rejected before the ioctl");
        assert_eq!(std::fs::read(original.join("file")).unwrap(), b"original");
        assert_eq!(
            std::fs::read(root.path().join("file")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn replaced_parent_blocks_verification_before_measurement_on_any_filesystem() {
        // This synthetic token tests only parent-check ordering, not fs-verity.
        let (root, parent) = directory();
        let mut stage = CreatedStageFile::create(&parent, OsStr::new("file")).unwrap();
        stage.write_all(b"original").unwrap();
        let CreatedStageFile {
            parent,
            name,
            writer,
            reader,
        } = stage;
        drop(writer);
        let synthetic = SealedStageFile {
            parent,
            name,
            reader,
            verity_digest: [0; 32],
        };
        let original = replace_parent(&root);
        std::fs::write(root.path().join("file"), b"replacement").unwrap();
        assert!(matches!(
            synthetic.verify_current(),
            Err(StageVerityError::Namespace(_))
        ));
        assert_eq!(std::fs::read(original.join("file")).unwrap(), b"original");
        assert_eq!(
            std::fs::read(root.path().join("file")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn replaced_private_parent_invalidates_a_previously_sealed_file() {
        let (root, parent) = directory();
        let mut stage = CreatedStageFile::create(&parent, OsStr::new("file")).unwrap();
        stage.write_all(b"original").unwrap();
        let sealed = match stage.seal() {
            Ok(sealed) => sealed,
            Err(StageVerityError::Unsupported) => {
                eprintln!("fs-verity unsupported on this test filesystem");
                return;
            }
            Err(error) => panic!("unexpected stage seal error: {error}"),
        };
        let original = replace_parent(&root);
        std::fs::write(root.path().join("file"), b"replacement").unwrap();
        assert!(matches!(
            sealed.verify_current(),
            Err(StageVerityError::Namespace(_))
        ));
        assert_eq!(std::fs::read(original.join("file")).unwrap(), b"original");
        assert_eq!(
            std::fs::read(root.path().join("file")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn creation_rejects_existing_and_traversing_names_without_touching_them() {
        let (root, parent) = directory();
        let original = root.path().join("existing");
        std::fs::write(&original, b"producer data").unwrap();
        assert!(CreatedStageFile::create(&parent, OsStr::new("existing")).is_err());
        assert!(CreatedStageFile::create(&parent, OsStr::new("../outside")).is_err());
        std::os::unix::fs::symlink(&original, root.path().join("link")).unwrap();
        assert!(CreatedStageFile::create(&parent, OsStr::new("link")).is_err());
        assert_eq!(std::fs::read(original).unwrap(), b"producer data");
    }

    #[test]
    fn unsupported_filesystem_and_open_writer_are_never_accepted() {
        let (root, parent) = directory();
        let mut stage = CreatedStageFile::create(&parent, OsStr::new("unsupported")).unwrap();
        stage.write_all(b"retained").unwrap();
        assert!(matches!(
            stage.seal_with(|_| Err(EnableVerityError::FilesystemNotSupported)),
            Err(StageVerityError::Unsupported)
        ));
        assert_eq!(
            std::fs::read(root.path().join("unsupported")).unwrap(),
            b"retained"
        );
        let mut stage = CreatedStageFile::create(&parent, OsStr::new("writer")).unwrap();
        stage.write_all(b"retained").unwrap();
        assert!(matches!(
            stage.seal_with(|_| Err(EnableVerityError::FileOpenedForWrite)),
            Err(StageVerityError::WriterOpen)
        ));
        assert_eq!(
            std::fs::read(root.path().join("writer")).unwrap(),
            b"retained"
        );
    }

    #[test]
    fn existing_writer_and_replaced_stage_do_not_issue_a_seal() {
        let (root, parent) = directory();
        let mut stage = CreatedStageFile::create(&parent, OsStr::new("stage")).unwrap();
        stage.write_all(b"stage-data").unwrap();
        let extra_writer = std::fs::OpenOptions::new()
            .write(true)
            .open(root.path().join("stage"))
            .unwrap();
        match stage.seal() {
            Err(StageVerityError::WriterOpen) => {}
            Err(StageVerityError::Unsupported) => {
                eprintln!(
                    "fs-verity unsupported on this test filesystem; deterministic failure coverage still ran"
                );
            }
            _ => panic!("expected active-writer refusal or explicit unsupported filesystem"),
        }
        drop(extra_writer);

        let mut stage = CreatedStageFile::create(&parent, OsStr::new("replacement")).unwrap();
        stage.write_all(b"original").unwrap();
        std::fs::rename(
            root.path().join("replacement"),
            root.path().join("preserved"),
        )
        .unwrap();
        std::fs::write(root.path().join("replacement"), b"other").unwrap();
        assert!(matches!(stage.seal(), Err(StageVerityError::Replaced)));
        assert_eq!(
            std::fs::read(root.path().join("replacement")).unwrap(),
            b"other"
        );
        assert_eq!(
            std::fs::read(root.path().join("preserved")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn sealed_stage_rejects_writes_and_detects_name_replacement() {
        let (root, parent) = directory();
        let mut stage = CreatedStageFile::create(&parent, OsStr::new("file")).unwrap();
        stage.write_all(b"immutable").unwrap();
        let sealed = match stage.seal() {
            Ok(sealed) => sealed,
            Err(StageVerityError::Unsupported) => {
                eprintln!("fs-verity unsupported on this test filesystem");
                return;
            }
            Err(error) => panic!("unexpected stage seal error: {error}"),
        };
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(root.path().join("file"))
                .is_err()
        );
        sealed.verify_current().unwrap();
        let (digest, device, inode) = sealed.evidence().unwrap();
        let reopened = SealedStageFile::reopen(&parent, OsStr::new("file"), digest, device, inode)
            .expect("reopen sealed stage");
        reopened.verify_current().unwrap();
        assert!(
            SealedStageFile::reopen(&parent, OsStr::new("file"), [0; 32], device, inode).is_err()
        );
        assert!(
            SealedStageFile::reopen(
                &parent,
                OsStr::new("file"),
                digest,
                device,
                inode.wrapping_add(1)
            )
            .is_err()
        );
        let mut content = Vec::new();
        sealed
            .reader()
            .try_clone()
            .unwrap()
            .read_to_end(&mut content)
            .unwrap();
        assert_eq!(content, b"immutable");
        std::fs::rename(
            root.path().join("file"),
            root.path().join("sealed-original"),
        )
        .unwrap();
        std::fs::write(root.path().join("file"), b"replacement").unwrap();
        assert!(matches!(
            sealed.verify_current(),
            Err(StageVerityError::Replaced)
        ));
        assert!(matches!(
            SealedStageFile::reopen(&parent, OsStr::new("file"), digest, device, inode),
            Err(StageVerityError::Replaced)
        ));
        assert_eq!(
            std::fs::read(root.path().join("sealed-original")).unwrap(),
            b"immutable"
        );
    }

    #[test]
    fn cancelled_copy_preserves_source_and_unsealed_partial_stage() {
        let (root, parent) = directory();
        let source = tempfile::tempdir().unwrap();
        let bytes = vec![0x5a; 2 * 64 * 1024];
        std::fs::write(source.path().join("source"), &bytes).unwrap();
        let cancel = std::cell::Cell::new(false);
        let result = copy_opened_file_and_seal(
            File::open(source.path().join("source")).unwrap(),
            &parent,
            OsStr::new("partial"),
            || cancel.get(),
            |index, chunk| {
                assert_eq!(index, 0);
                assert_eq!(chunk.len(), 64 * 1024);
                cancel.set(true);
                Ok(())
            },
        );
        assert!(matches!(result, Err(StageVerityError::Cancelled)));
        assert_eq!(std::fs::read(source.path().join("source")).unwrap(), bytes);
        assert_eq!(
            std::fs::read(root.path().join("partial")).unwrap(),
            bytes[..64 * 1024]
        );
    }

    #[test]
    fn protected_copy_observes_exact_chunks_and_preserves_source_when_unsupported() {
        let (root, parent) = directory();
        let source = tempfile::tempdir().unwrap();
        let bytes = vec![0x63; 64 * 1024 + 17];
        std::fs::write(source.path().join("source"), &bytes).unwrap();
        let mut observed = Vec::new();
        let result = copy_opened_file_and_seal(
            File::open(source.path().join("source")).unwrap(),
            &parent,
            OsStr::new("copy"),
            || false,
            |index, chunk| {
                observed.push((index, chunk.len(), blake3::hash(chunk)));
                Ok(())
            },
        );
        assert_eq!(
            observed,
            vec![
                (0, 64 * 1024, blake3::hash(&bytes[..64 * 1024])),
                (1, 17, blake3::hash(&bytes[64 * 1024..]))
            ]
        );
        assert_eq!(std::fs::read(source.path().join("source")).unwrap(), bytes);
        assert_eq!(std::fs::read(root.path().join("copy")).unwrap(), bytes);
        match result {
            Ok(sealed) => {
                let (digest, device, inode) = sealed.evidence().unwrap();
                SealedStageFile::reopen(&parent, OsStr::new("copy"), digest, device, inode)
                    .expect("reopen sealed copy");
            }
            Err(StageVerityError::Unsupported) => {
                eprintln!("fs-verity unsupported; source and partial stage retained")
            }
            Err(error) => panic!("unexpected protected copy result: {error}"),
        }
    }
}

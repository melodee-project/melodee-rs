//! Descriptor-relative quarantine primitives for a future durable handoff.
//!
//! These operations are not yet wired into production acquisition or purge.

#[cfg(not(unix))]
use std::io;
use std::{ffi::OsStr, fs::File};

use crate::{
    acquisition::{
        AcquisitionError, source_generation_from_directory, tree_hash_from_pinned_directory,
    },
    secure_fs::{open_child_directory, rename_child_noreplace},
    source::GuardedRoot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuarantineOutcome {
    Held,
    ReplacementRestored,
    ReplacementPreserved,
}

/// An intent replay must inspect the protected custody entry before deciding
/// whether the source should be revisited. Neither result authorizes removal.
#[cfg(unix)]
pub(crate) enum PendingCustodyObservation {
    Missing,
    Matching { held: File, legacy_hash: String },
    Different { held: File },
}

/// Inspect the deterministic custody name through an already validated private
/// parent. A changed root aborts replay; callers must revalidate again at the
/// staging boundary. Unexpected bytes remain held intact for review.
#[cfg(unix)]
pub(crate) fn inspect_pending_custody(
    guarded_root: &GuardedRoot,
    private_parent: &File,
    private_name: &OsStr,
    expected_generation: &str,
    expected_tree_hash: &str,
) -> Result<PendingCustodyObservation, AcquisitionError> {
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let held = match open_child_directory(private_parent, private_name) {
        Ok(held) => held,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            guarded_root
                .verify_current()
                .map_err(|error| AcquisitionError::Task(error.to_string()))?;
            return Ok(PendingCustodyObservation::Missing);
        }
        Err(error) => return Err(AcquisitionError::Task(error.to_string())),
    };
    let generation = source_generation_from_directory(&held, std::path::Path::new(private_name))?;
    let (hash, legacy_hash) = crate::acquisition::tree_hashes_from_pinned_directory(&held)?;
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if generation == expected_generation && hash == expected_tree_hash {
        Ok(PendingCustodyObservation::Matching { held, legacy_hash })
    } else {
        Ok(PendingCustodyObservation::Different { held })
    }
}

/// Validate the namespace protecting a proposed custody directory. This does
/// not prove that producers have relinquished writable file handles, and is
/// therefore insufficient on its own to authorize draining any source.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(crate) fn validate_custody_namespace(
    private_path: &std::path::Path,
    source_path: &std::path::Path,
    producer_uid: u32,
) -> Result<File, AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    let service_uid = rustix::process::geteuid().as_raw();
    if producer_uid == service_uid || producer_uid == 0 {
        return Err(AcquisitionError::Task(
            "custody requires a distinct, unprivileged producer identity".to_owned(),
        ));
    }
    let normalized_absolute = |path: &std::path::Path| {
        path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    };
    if !normalized_absolute(private_path)
        || !normalized_absolute(source_path)
        || private_path.starts_with(source_path)
        || source_path.starts_with(private_path)
    {
        return Err(AcquisitionError::Task(
            "custody and source roots must be distinct absolute paths".to_owned(),
        ));
    }
    let source = crate::secure_fs::open_directory(source_path)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let source_metadata = source
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let private = crate::secure_fs::open_directory(private_path)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let private_metadata = private
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if source_metadata.dev() != private_metadata.dev()
        || private_metadata.uid() != service_uid
        || private_metadata.mode() & 0o077 != 0
    {
        return Err(AcquisitionError::Task(
            "custody root must be private, service-owned and on the source filesystem".to_owned(),
        ));
    }
    for ancestor in private_path.ancestors() {
        let directory = crate::secure_fs::open_directory(ancestor)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let metadata = directory
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        verify_acl_free(&directory)?;
        if (metadata.dev(), metadata.ino()) == (source_metadata.dev(), source_metadata.ino())
            || (metadata.uid() != service_uid && metadata.uid() != 0)
            || metadata.mode() & 0o022 != 0
        {
            return Err(AcquisitionError::Task(
                "custody ancestry is not protected from the producer".to_owned(),
            ));
        }
    }
    for ancestor in source_path.ancestors() {
        let directory = crate::secure_fs::open_directory(ancestor)
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        let metadata = directory
            .metadata()
            .map_err(|error| AcquisitionError::Task(error.to_string()))?;
        if (metadata.dev(), metadata.ino()) == (private_metadata.dev(), private_metadata.ino()) {
            return Err(AcquisitionError::Task(
                "custody root contains the source root".to_owned(),
            ));
        }
    }
    Ok(private)
}

// Whitelist only local Linux filesystem types whose POSIX ACLs are exposed
// through these xattrs. Unknown filesystems and unsupported ACL inspection
// fail closed. This remains a namespace check, not an open-writer proof.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_acl_free(directory: &File) -> Result<(), AcquisitionError> {
    let fs = rustix::fs::fstatfs(directory)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    const LOCAL_POSIX_ACL_FILESYSTEMS: [u32; 4] = [0x9123_683e, 0xef53, 0x5846_5342, 0x0102_1994];
    if !LOCAL_POSIX_ACL_FILESYSTEMS.contains(&(fs.f_type as u32)) {
        return Err(AcquisitionError::Task(
            "custody ACL inspection is unsupported for this filesystem".to_owned(),
        ));
    }
    for name in ["system.posix_acl_access", "system.posix_acl_default"] {
        match rustix::fs::fgetxattr_query(directory, name) {
            Err(rustix::io::Errno::NODATA) => {}
            Ok(_) => {
                return Err(AcquisitionError::Task(
                    "custody ancestry has an ACL".to_owned(),
                ));
            }
            Err(error) => {
                return Err(AcquisitionError::Task(format!(
                    "custody ACL inspection failed: {error}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn verify_acl_free(_directory: &File) -> Result<(), AcquisitionError> {
    Err(AcquisitionError::Task(
        "custody ACL inspection is unavailable on this platform".to_owned(),
    ))
}

/// Move a candidate using pinned parents, then compare the held directory
/// with the still-open original occurrence and its preclaim content hash.
/// Mismatches are restored only with NOREPLACE or retained intact for recovery.
#[cfg(unix)]
pub(crate) fn quarantine_verified_occurrence(
    guarded_root: &GuardedRoot,
    source_parent: &File,
    source_name: &OsStr,
    original: &File,
    private_parent: &File,
    private_name: &OsStr,
    expected_generation: &str,
    expected_tree_hash: &str,
) -> Result<QuarantineOutcome, AcquisitionError> {
    quarantine_verified_occurrence_with_hook(
        guarded_root,
        source_parent,
        source_name,
        original,
        private_parent,
        private_name,
        expected_generation,
        expected_tree_hash,
        || Ok(()),
    )
}

#[cfg(unix)]
fn quarantine_verified_occurrence_with_hook(
    guarded_root: &GuardedRoot,
    source_parent: &File,
    source_name: &OsStr,
    original: &File,
    private_parent: &File,
    private_name: &OsStr,
    expected_generation: &str,
    expected_tree_hash: &str,
    after_move: impl FnOnce() -> Result<(), AcquisitionError>,
) -> Result<QuarantineOutcome, AcquisitionError> {
    use std::os::unix::fs::MetadataExt;
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let source_meta = source_parent
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let private_meta = private_parent
        .metadata()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    if source_meta.dev() != private_meta.dev() {
        return Err(AcquisitionError::Task(
            "custody parent must be on the source filesystem".to_owned(),
        ));
    }
    if !private_meta.is_dir() || private_meta.mode() & 0o077 != 0 {
        return Err(AcquisitionError::Task(
            "custody parent is not private".to_owned(),
        ));
    }
    let original_generation =
        source_generation_from_directory(original, std::path::Path::new(source_name))?;
    if original_generation != expected_generation {
        return Err(AcquisitionError::SourceChanged);
    }
    rename_child_noreplace(source_parent, source_name, private_parent, private_name)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    after_move()?;
    source_parent
        .sync_all()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    private_parent
        .sync_all()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let held = open_child_directory(private_parent, private_name)
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let held_generation =
        source_generation_from_directory(&held, std::path::Path::new(private_name))?;
    let held_hash = tree_hash_from_pinned_directory(&held);
    if held_generation != original_generation
        || !matches!(held_hash, Ok(ref hash) if hash == expected_tree_hash)
    {
        return match rename_child_noreplace(
            private_parent,
            private_name,
            source_parent,
            source_name,
        ) {
            Ok(()) => {
                source_parent
                    .sync_all()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                private_parent
                    .sync_all()
                    .map_err(|error| AcquisitionError::Task(error.to_string()))?;
                Ok(QuarantineOutcome::ReplacementRestored)
            }
            Err(_) => Ok(QuarantineOutcome::ReplacementPreserved),
        };
    }
    // An error here leaves the object in custody for intent recovery; it must
    // never authorize staging bytes from a detached source root.
    guarded_root
        .verify_current()
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    Ok(QuarantineOutcome::Held)
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use crate::config::InboundSource;
    use crate::secure_fs::open_directory;
    use std::{fs, path::Path};

    #[test]
    fn byte_identical_replacement_is_restored_or_preserved_without_deletion() {
        use std::os::unix::fs::PermissionsExt;
        for block_restore in [false, true] {
            let temp = tempfile::tempdir().expect("temp");
            let inbound = temp.path().join("inbound");
            let private = temp.path().join("private");
            fs::create_dir_all(inbound.join("candidate")).expect("original");
            fs::create_dir(&private).expect("private");
            fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private mode");
            fs::write(inbound.join("candidate/file"), b"identical").expect("file");
            let guarded_root = GuardedRoot::open(
                InboundSource::from_root(inbound.clone(), None, Some("test".into()))
                    .expect("source"),
            )
            .expect("root");
            let source_parent = open_directory(&inbound).expect("source parent");
            let private_parent = open_directory(&private).expect("private parent");
            let original = open_child_directory(&source_parent, OsStr::new("candidate"))
                .expect("original handle");
            let generation = source_generation_from_directory(&original, Path::new("candidate"))
                .expect("generation");
            let hash = tree_hash_from_pinned_directory(&original).expect("hash");
            fs::rename(inbound.join("candidate"), inbound.join("parked-original"))
                .expect("park original");
            fs::create_dir(inbound.join("candidate")).expect("replacement");
            fs::write(inbound.join("candidate/file"), b"identical").expect("replacement bytes");
            let outcome = quarantine_verified_occurrence_with_hook(
                &guarded_root,
                &source_parent,
                OsStr::new("candidate"),
                &original,
                &private_parent,
                OsStr::new("held"),
                &generation,
                &hash,
                || {
                    if block_restore {
                        fs::create_dir(inbound.join("candidate")).expect("occupy original name");
                    }
                    Ok(())
                },
            )
            .expect("quarantine attempt");
            let replacement = if block_restore {
                assert_eq!(outcome, QuarantineOutcome::ReplacementPreserved);
                private.join("held/file")
            } else {
                assert_eq!(outcome, QuarantineOutcome::ReplacementRestored);
                inbound.join("candidate/file")
            };
            assert_eq!(
                fs::read(replacement).expect("replacement retained"),
                b"identical"
            );
            assert_eq!(
                fs::read(inbound.join("parked-original/file")).expect("original retained"),
                b"identical"
            );
        }
    }

    #[test]
    fn non_private_custody_parent_is_rejected_before_move() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let inbound = temp.path().join("inbound");
        let private = temp.path().join("not-private");
        fs::create_dir_all(inbound.join("candidate")).expect("candidate");
        fs::write(inbound.join("candidate/file"), b"keep").expect("file");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).expect("permissions");
        let guarded_root = GuardedRoot::open(
            InboundSource::from_root(inbound.clone(), None, Some("test".into())).expect("source"),
        )
        .expect("root");
        let source_parent = open_directory(&inbound).expect("source");
        let private_parent = open_directory(&private).expect("private");
        let original =
            open_child_directory(&source_parent, OsStr::new("candidate")).expect("original");
        let generation = source_generation_from_directory(&original, Path::new("candidate"))
            .expect("generation");
        let hash = tree_hash_from_pinned_directory(&original).expect("hash");
        assert!(
            quarantine_verified_occurrence(
                &guarded_root,
                &source_parent,
                OsStr::new("candidate"),
                &original,
                &private_parent,
                OsStr::new("held"),
                &generation,
                &hash
            )
            .is_err()
        );
        assert_eq!(
            fs::read(inbound.join("candidate/file")).expect("source preserved"),
            b"keep"
        );
        assert!(!private.join("held").exists());
    }

    #[test]
    fn source_root_replacement_after_move_retains_custody_and_rejects_handoff() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let inbound = temp.path().join("inbound");
        let old_root = temp.path().join("old-root");
        let private = temp.path().join("private");
        fs::create_dir_all(inbound.join("candidate")).expect("candidate");
        fs::write(inbound.join("candidate/file"), b"original").expect("file");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let guarded_root = GuardedRoot::open(
            InboundSource::from_root(inbound.clone(), None, Some("test".into())).expect("source"),
        )
        .expect("root");
        let source_parent = open_directory(&inbound).expect("source parent");
        let private_parent = open_directory(&private).expect("private parent");
        let original = open_child_directory(&source_parent, OsStr::new("candidate"))
            .expect("candidate handle");
        let generation = source_generation_from_directory(&original, Path::new("candidate"))
            .expect("generation");
        let hash = tree_hash_from_pinned_directory(&original).expect("hash");
        let result = quarantine_verified_occurrence_with_hook(
            &guarded_root,
            &source_parent,
            OsStr::new("candidate"),
            &original,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
            || {
                fs::rename(&inbound, &old_root).expect("replace pinned root");
                fs::create_dir_all(inbound.join("candidate")).expect("replacement");
                fs::write(inbound.join("candidate/file"), b"replacement").expect("new file");
                Ok(())
            },
        );
        assert!(result.is_err(), "detached root cannot authorize handoff");
        assert_eq!(
            fs::read(private.join("held/file")).expect("held"),
            b"original"
        );
        assert_eq!(
            fs::read(inbound.join("candidate/file")).expect("replacement"),
            b"replacement"
        );
    }

    #[test]
    fn custody_namespace_rejects_same_identity_overlaps_and_writable_ancestors() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let untrusted = temp.path().join("writable");
        let private = untrusted.join("private");
        fs::create_dir(&source).expect("source");
        fs::create_dir_all(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        fs::set_permissions(&untrusted, fs::Permissions::from_mode(0o777)).expect("mode");
        let service_uid = rustix::process::geteuid().as_raw();
        let other_uid = if service_uid == 1 { 2 } else { 1 };
        assert!(validate_custody_namespace(&private, &source, service_uid).is_err());
        assert!(validate_custody_namespace(&source, &source, other_uid).is_err());
        assert!(validate_custody_namespace(&private, &source, other_uid).is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn protected_service_owned_custody_namespace_is_accepted() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let service_uid = rustix::process::geteuid().as_raw();
        let other_uid = if service_uid == 1 { 2 } else { 1 };
        let candidates = [
            std::env::var_os("XDG_RUNTIME_DIR").map(Into::into),
            std::env::current_dir().ok(),
            std::env::var_os("HOME").map(Into::into),
        ];
        let base = candidates.into_iter().flatten().find(|path| {
            path.is_absolute()
                && path.ancestors().all(|ancestor| {
                    crate::secure_fs::open_directory(ancestor).is_ok_and(|directory| {
                        verify_acl_free(&directory).is_ok()
                            && directory.metadata().is_ok_and(|metadata| {
                                (metadata.uid() == service_uid || metadata.uid() == 0)
                                    && metadata.mode() & 0o022 == 0
                            })
                    })
                })
        });
        let Some(base) = base else {
            eprintln!("no protected writable test parent available");
            return;
        };
        let temp = tempfile::tempdir_in(base).expect("protected temp");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        fs::create_dir(&source).expect("source");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let validated =
            validate_custody_namespace(&private, &source, other_uid).expect("protected namespace");
        let actual = validated.metadata().expect("validated metadata");
        let expected = fs::metadata(&private).expect("path metadata");
        assert_eq!(
            (actual.dev(), actual.ino()),
            (expected.dev(), expected.ino())
        );
        eprintln!("protected namespace positive assertion executed");

        // Linux POSIX ACL xattr format: version 2 followed by fixed-width
        // little-endian tag, permissions and ID entries. This named user has
        // no effective rights; even a benign nontrivial ACL must fail closed.
        let mut acl = 2_u32.to_le_bytes().to_vec();
        for (tag, permission, id) in [
            (1_u16, 7_u16, u32::MAX),
            (2, 0, other_uid),
            (4, 0, u32::MAX),
            (16, 0, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend_from_slice(&tag.to_le_bytes());
            acl.extend_from_slice(&permission.to_le_bytes());
            acl.extend_from_slice(&id.to_le_bytes());
        }
        rustix::fs::fsetxattr(
            &validated,
            "system.posix_acl_access",
            &acl,
            rustix::fs::XattrFlags::empty(),
        )
        .expect("set test ACL");
        assert!(validate_custody_namespace(&private, &source, other_uid).is_err());
        eprintln!("named-user ACL rejection assertion executed");
    }

    #[test]
    fn private_namespace_does_not_revoke_existing_writer_descriptor() {
        use std::{io::Write, os::unix::fs::PermissionsExt};
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        fs::create_dir_all(source.join("candidate")).expect("candidate");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let mut writer = fs::File::create(source.join("candidate/file")).expect("writer");
        writer.write_all(b"before").expect("write");
        fs::rename(source.join("candidate"), private.join("held")).expect("move");
        writer.write_all(b"after").expect("writer remains open");
        assert_eq!(
            fs::read(private.join("held/file")).expect("held"),
            b"beforeafter"
        );
    }

    #[test]
    fn interrupted_move_can_be_inspected_from_the_deterministic_custody_name() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        let private = temp.path().join("private");
        fs::create_dir_all(source.join("candidate")).expect("candidate");
        fs::write(source.join("candidate/file"), b"held bytes").expect("file");
        fs::create_dir(&private).expect("private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("mode");
        let guarded = GuardedRoot::open(
            InboundSource::from_root(source.clone(), None, Some("test".into())).expect("source"),
        )
        .expect("root");
        let source_parent = open_directory(&source).expect("source parent");
        let private_parent = open_directory(&private).expect("private parent");
        let original = open_child_directory(&source_parent, OsStr::new("candidate"))
            .expect("candidate handle");
        let generation = source_generation_from_directory(&original, Path::new("candidate"))
            .expect("generation");
        let hash = tree_hash_from_pinned_directory(&original).expect("hash");
        let error = quarantine_verified_occurrence_with_hook(
            &guarded,
            &source_parent,
            OsStr::new("candidate"),
            &original,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
            || {
                Err(AcquisitionError::Task(
                    "injected post-move interruption".to_owned(),
                ))
            },
        );
        assert!(error.is_err());
        assert!(!source.join("candidate").exists());
        match inspect_pending_custody(
            &guarded,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
        )
        .expect("inspect held entry")
        {
            PendingCustodyObservation::Matching { held, legacy_hash } => {
                assert!(!legacy_hash.is_empty());
                assert_eq!(
                    source_generation_from_directory(&held, Path::new("held")).expect("generation"),
                    generation
                );
            }
            _ => unreachable!("held occurrence must be recovered for further verification"),
        }
        assert_eq!(
            fs::read(private.join("held/file")).expect("held bytes"),
            b"held bytes"
        );
        fs::write(private.join("held/file"), b"new  bytes").expect("mutation");
        match inspect_pending_custody(
            &guarded,
            &private_parent,
            OsStr::new("held"),
            &generation,
            &hash,
        )
        .expect("inspect changed entry")
        {
            PendingCustodyObservation::Different { held } => {
                assert!(held.metadata().expect("held metadata").is_dir());
            }
            _ => unreachable!("changed custody must remain held for review"),
        }
        assert_eq!(
            fs::read(private.join("held/file")).expect("changed bytes"),
            b"new  bytes"
        );
    }
}

#[cfg(not(unix))]
pub(crate) fn quarantine_verified_occurrence(
    _guarded_root: &GuardedRoot,
    _source_parent: &File,
    _source_name: &OsStr,
    _original: &File,
    _private_parent: &File,
    _private_name: &OsStr,
    _expected_generation: &str,
    _expected_tree_hash: &str,
) -> Result<QuarantineOutcome, AcquisitionError> {
    Err(AcquisitionError::Task(
        io::Error::new(
            io::ErrorKind::Unsupported,
            "descriptor-relative custody unavailable",
        )
        .to_string(),
    ))
}

//! Pinned, fail-closed filesystem root-set validation. A caller's root list is
//! not an administrative intent and this type does not authorize deletion.

use rustix::fs::{AtFlags, Mode, OFlags, StatxFlags, openat, statx};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::File,
    io::{self, Read},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

const MAX_ROOTS: usize = 132;
const MAX_HANDLES: usize = 512;
const MAX_MOUNTINFO: u64 = 8 * 1024 * 1024;

/// Expected registry metadata. It is untrusted until opened and checked.
#[derive(Clone, PartialEq, Eq)]
pub struct RootExpectation {
    pub role: String,
    pub source_library_id: Option<Uuid>,
    pub path_hex: String,
    pub generation: String,
    pub mount_id: String,
    pub filesystem_magic: String,
}

impl RootExpectation {
    /// Observe a root's current metadata without granting any authority.
    /// Callers must still validate the complete root set and durable binding.
    pub fn observe(role: String, source_library_id: Option<Uuid>, path: &Path) -> io::Result<Self> {
        let path_hex = crate::encode_native_root(path)?;
        let root = open_directory(path)?;
        Ok(Self {
            role,
            source_library_id,
            path_hex,
            generation: generation(&root)?,
            mount_id: format!("{:016x}", mount_id(&root)?),
            filesystem_magic: format!("{:016x}", filesystem(&root)?),
        })
    }
}

/// One root from the caller's configuration. The set must contain every root.
#[derive(Clone)]
pub struct ConfiguredRoot {
    pub role: String,
    pub source_library_id: Option<Uuid>,
    pub path: PathBuf,
}

struct PinnedRoot {
    expectation: RootExpectation,
    path: PathBuf,
    ancestry: Vec<Arc<File>>,
}

/// Holds a validated supplied root set. It is non-Clone and has no operation
/// authority: trusted code must additionally prove this is the complete live
/// configuration and the exact durable intent/root revision under DB fencing.
pub struct VerifiedRootSet {
    roots: Vec<PinnedRoot>,
    mountinfo: Vec<u8>,
}

/// A supplied configuration matched exactly to supplied registry entries and
/// freshly opened filesystem roots. This is not a DB or deletion capability.
pub struct ExactConfiguredRootSet {
    roots: VerifiedRootSet,
}

impl ExactConfiguredRootSet {
    /// Compare the complete caller-provided config set with the registry set.
    /// Trusted integration must obtain that configuration from the actual
    /// service state and bind the registry revision to both live DB epochs.
    pub fn open(configured: &[ConfiguredRoot], expected: &[RootExpectation]) -> io::Result<Self> {
        if configured.len() != expected.len() || configured.len() > MAX_ROOTS {
            return Err(invalid());
        }
        let mut actual = BTreeMap::new();
        for root in configured {
            let key = (root.role.as_str(), root.source_library_id);
            if actual
                .insert(key, crate::encode_native_root(&root.path)?)
                .is_some()
            {
                return Err(invalid());
            }
        }
        for root in expected {
            if actual.get(&(root.role.as_str(), root.source_library_id)) != Some(&root.path_hex) {
                return Err(invalid());
            }
        }
        Ok(Self {
            roots: VerifiedRootSet::open(expected)?,
        })
    }

    /// Verify the held roots remain at the configured paths.
    pub fn revalidate(&self) -> io::Result<()> {
        self.roots.revalidate()
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "root set changed or is unsupported",
    )
}
fn os_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}
fn id(file: &File) -> io::Result<(u64, u64)> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn generation(file: &File) -> io::Result<String> {
    let (dev, ino) = id(file)?;
    Ok(format!("unix:{dev}:{ino}"))
}
fn mount_id(file: &File) -> io::Result<u64> {
    let s = statx(file, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID).map_err(os_error)?;
    if s.stx_mask & StatxFlags::MNT_ID.bits() == 0 {
        return Err(invalid());
    }
    Ok(s.stx_mnt_id)
}
fn filesystem(file: &File) -> io::Result<u64> {
    let value = rustix::fs::fstatfs(file).map_err(os_error)?.f_type as u32;
    if ![0x9123_683e, 0xef53, 0x5846_5342, 0x0102_1994].contains(&value) {
        return Err(invalid());
    }
    Ok(u64::from(value))
}
fn open_directory(path: &Path) -> io::Result<File> {
    let mut current = File::from(
        openat(
            rustix::fs::CWD,
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(os_error)?,
    );
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
                )
            }
            _ => return Err(invalid()),
        }
    }
    Ok(current)
}
fn safe_ancestry(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    let uid = rustix::process::geteuid().as_raw();
    if (m.uid() != uid && m.uid() != 0) || m.mode() & 0o022 != 0 {
        return Err(invalid());
    }
    filesystem(file)?;
    for name in ["system.posix_acl_access", "system.posix_acl_default"] {
        match rustix::fs::fgetxattr_query(file, name) {
            Err(rustix::io::Errno::NODATA) => {}
            Ok(_) => return Err(invalid()),
            Err(e) => return Err(os_error(e)),
        }
    }
    Ok(())
}
fn read_mountinfo() -> io::Result<Vec<u8>> {
    let file = File::open("/proc/self/mountinfo")?;
    if rustix::fs::fstatfs(&file).map_err(os_error)?.f_type as u32 != 0x9fa0 {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MOUNTINFO + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_MOUNTINFO {
        return Err(invalid());
    }
    Ok(bytes)
}
fn has_nested_mount(data: &[u8], root: &Path) -> io::Result<bool> {
    for line in data.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let mut fields = line.split(|b| *b == b' ');
        for _ in 0..4 {
            if fields.next().is_none_or(|v| v.is_empty()) {
                return Err(invalid());
            }
        }
        let path = fields
            .next()
            .filter(|v| !v.is_empty())
            .ok_or_else(invalid)?;
        if !fields.any(|v| v == b"-") {
            return Err(invalid());
        }
        let mut decoded = Vec::with_capacity(path.len());
        let mut index = 0;
        while index < path.len() {
            if path[index] == b'\\' {
                let escaped = path.get(index + 1..index + 4).ok_or_else(invalid)?;
                decoded.push(match escaped {
                    b"040" => b' ',
                    b"011" => b'\t',
                    b"012" => b'\n',
                    b"134" => b'\\',
                    _ => return Err(invalid()),
                });
                index += 4;
            } else {
                decoded.push(path[index]);
                index += 1;
            }
        }
        let mount = PathBuf::from(OsString::from_vec(decoded));
        if !mount.is_absolute() {
            return Err(invalid());
        }
        if mount != root && mount.starts_with(root) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl VerifiedRootSet {
    /// Open each supplied root without following links, pin distinct ancestors,
    /// check exact DB-shaped identities and reject overlap/nested mounts. This
    /// cannot establish the caller supplied the complete configured root set.
    pub fn open(expectations: &[RootExpectation]) -> io::Result<Self> {
        if !(6..=MAX_ROOTS).contains(&expectations.len()) {
            return Err(invalid());
        }
        let mut keys = BTreeSet::new();
        let mut path_bytes = 0_usize;
        let mut cache: BTreeMap<PathBuf, Arc<File>> = BTreeMap::new();
        let mut roots: Vec<PinnedRoot> = Vec::with_capacity(expectations.len());
        let topology = read_mountinfo()?;
        for expected in expectations {
            path_bytes = path_bytes
                .checked_add(expected.path_hex.len())
                .ok_or_else(invalid)?;
            if path_bytes > 1024 * 1024 {
                return Err(invalid());
            }
            if !matches!(
                expected.role.as_str(),
                "source" | "work" | "stage" | "canonical" | "asset" | "lock"
            ) || (expected.role != "source" && expected.source_library_id.is_some())
                || !keys.insert((expected.role.clone(), expected.source_library_id))
            {
                return Err(invalid());
            }
            let path = crate::decode_native_root(&expected.path_hex)?;
            let mut ancestry = Vec::new();
            for ancestor in path.ancestors() {
                let current = open_directory(ancestor)?;
                let pin = if let Some(existing) = cache.get(ancestor) {
                    if id(existing)? != id(&current)? {
                        return Err(invalid());
                    }
                    existing.clone()
                } else {
                    if cache.len() == MAX_HANDLES {
                        return Err(invalid());
                    }
                    let pin = Arc::new(current);
                    cache.insert(ancestor.to_path_buf(), pin.clone());
                    pin
                };
                if expected.role == "lock" || expected.role == "stage" {
                    safe_ancestry(&pin)?;
                }
                ancestry.push(pin);
            }
            let root = ancestry.first().ok_or_else(invalid)?;
            let root_id = id(root)?;
            if expected.role == "lock" || expected.role == "stage" {
                let m = root.metadata()?;
                if m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o077 != 0 {
                    return Err(invalid());
                }
            }
            if generation(root)? != expected.generation
                || format!("{:016x}", mount_id(root)?) != expected.mount_id
                || format!("{:016x}", filesystem(root)?) != expected.filesystem_magic
                || has_nested_mount(&topology, &path)?
            {
                return Err(invalid());
            }
            for prior in &roots {
                let prior_root = prior.ancestry.first().ok_or_else(invalid)?;
                let prior_id = id(prior_root)?;
                if path.starts_with(&prior.path)
                    || prior.path.starts_with(&path)
                    || ((root_id.0 == prior_id.0) != (mount_id(root)? == mount_id(prior_root)?))
                {
                    return Err(invalid());
                }
                for pin in &ancestry {
                    if id(pin)? == prior_id {
                        return Err(invalid());
                    }
                }
                for pin in &prior.ancestry {
                    if id(pin)? == root_id {
                        return Err(invalid());
                    }
                }
            }
            roots.push(PinnedRoot {
                expectation: expected.clone(),
                path,
                ancestry,
            });
        }
        for role in ["work", "stage", "canonical", "asset", "lock"] {
            if !keys.contains(&(role.to_owned(), None)) {
                return Err(invalid());
            }
        }
        if !roots.iter().any(|root| root.expectation.role == "source") {
            return Err(invalid());
        }
        let set = Self {
            roots,
            mountinfo: topology,
        };
        set.revalidate()?;
        Ok(set)
    }

    /// Stream a fresh no-follow walk against the pinned ancestors. It retains
    /// no second full root set and fails closed if the mount table changes.
    pub fn revalidate(&self) -> io::Result<()> {
        if read_mountinfo()? != self.mountinfo {
            return Err(invalid());
        }
        for root in &self.roots {
            if root.path.ancestors().count() != root.ancestry.len() {
                return Err(invalid());
            }
            for (path, pin) in root.path.ancestors().zip(&root.ancestry) {
                let current = open_directory(path)?;
                if id(&current)? != id(pin)? {
                    return Err(invalid());
                }
                if root.expectation.role == "lock" || root.expectation.role == "stage" {
                    safe_ancestry(&current)?;
                }
            }
            let current = open_directory(&root.path)?;
            if generation(&current)? != root.expectation.generation
                || format!("{:016x}", mount_id(&current)?) != root.expectation.mount_id
                || format!("{:016x}", filesystem(&current)?) != root.expectation.filesystem_magic
                || has_nested_mount(&self.mountinfo, &root.path)?
            {
                return Err(invalid());
            }
            if root.expectation.role == "lock" || root.expectation.role == "stage" {
                let m = current.metadata()?;
                if m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o077 != 0 {
                    return Err(invalid());
                }
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
    fn exact_native_roots_reject_overlap_and_replacement() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("owned runtime parent");
        let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
        let source = fixture
            .path()
            .join(OsString::from_vec(b"source-\xff".to_vec()));
        let paths = [
            source,
            fixture.path().join("work"),
            fixture.path().join("stage"),
            fixture.path().join("canonical"),
            fixture.path().join("asset"),
            fixture.path().join("lock"),
        ];
        let roles = ["source", "work", "stage", "canonical", "asset", "lock"];
        let mut expected = Vec::new();
        for (path, role) in paths.iter().zip(roles) {
            fs::create_dir(path).expect("owned root");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private mode");
            let fd = open_directory(path).expect("root handle");
            expected.push(RootExpectation {
                role: role.to_owned(),
                source_library_id: None,
                path_hex: crate::encode_native_root(path).expect("encode path"),
                generation: generation(&fd).expect("identity"),
                mount_id: format!("{:016x}", mount_id(&fd).expect("mount")),
                filesystem_magic: format!("{:016x}", filesystem(&fd).expect("filesystem")),
            });
        }
        fs::write(paths[3].join("original"), b"preserve").expect("canonical fixture");
        let set = VerifiedRootSet::open(&expected).expect("exact supplied roots");
        set.revalidate().expect("stable roots");
        let configured: Vec<_> = expected
            .iter()
            .map(|root| ConfiguredRoot {
                role: root.role.clone(),
                source_library_id: root.source_library_id,
                path: crate::decode_native_root(&root.path_hex).expect("decode configured path"),
            })
            .collect();
        let exact =
            ExactConfiguredRootSet::open(&configured, &expected).expect("complete configured set");
        exact.revalidate().expect("exact roots stable");
        assert!(
            ExactConfiguredRootSet::open(&configured[..configured.len() - 1], &expected).is_err(),
            "omitting a configured root must fail closed"
        );
        let mut wrong_path: Vec<_> = expected
            .iter()
            .map(|root| ConfiguredRoot {
                role: root.role.clone(),
                source_library_id: root.source_library_id,
                path: crate::decode_native_root(&root.path_hex).expect("decode configured path"),
            })
            .collect();
        wrong_path[3].path = paths[4].clone();
        assert!(
            ExactConfiguredRootSet::open(&wrong_path, &expected).is_err(),
            "different configured roots must fail closed"
        );
        let mut overlap = expected.clone();
        overlap[1].path_hex = overlap[0].path_hex.clone();
        overlap[1].generation = overlap[0].generation.clone();
        overlap[1].mount_id = overlap[0].mount_id.clone();
        overlap[1].filesystem_magic = overlap[0].filesystem_magic.clone();
        assert!(VerifiedRootSet::open(&overlap).is_err());
        let old = fixture.path().join("old-canonical");
        fs::rename(&paths[3], &old).expect("displace root");
        fs::create_dir(&paths[3]).expect("replacement root");
        fs::set_permissions(&paths[3], fs::Permissions::from_mode(0o700))
            .expect("replacement mode");
        assert!(set.revalidate().is_err());
        assert_eq!(
            fs::read(old.join("original")).expect("preserved original"),
            b"preserve"
        );
    }
}

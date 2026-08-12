//! Ownership-validated filesystem lifecycle for Tantivy generations.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::validate_search_root;
use crate::error::SearchError;

const ROOT_SENTINEL: &str = ".melodee-search-root.json";
const GENERATION_MANIFEST: &str = "generation.json";
const MAX_GENERATION_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RootSentinel {
    kind: String,
    version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GenerationManifest {
    pub generation_id: Uuid,
    pub kind: String,
    pub created_at: OffsetDateTime,
    pub document_counts: melodee_persistence::SearchDocumentCounts,
    pub schema_version: u32,
    pub source_high_water_mark: i64,
}

#[derive(Debug, Clone)]
pub struct SearchRoot {
    path: PathBuf,
}

impl SearchRoot {
    pub fn open(path: &Path) -> Result<Self, SearchError> {
        let existed = path.exists();
        let canonical = validate_search_root(path)?;
        let sentinel_path = canonical.join(ROOT_SENTINEL);
        if sentinel_path.exists() {
            let raw = fs::read(&sentinel_path).map_err(|source| SearchError::Io {
                path: sentinel_path.clone(),
                source,
            })?;
            let sentinel: RootSentinel = serde_json::from_slice(&raw)?;
            if sentinel
                != (RootSentinel {
                    kind: "melodee-search-root".to_string(),
                    version: 1,
                })
            {
                return Err(SearchError::UnownedRoot(canonical.display().to_string()));
            }
        } else {
            if existed && directory_has_entries(&canonical)? {
                return Err(SearchError::UnownedRoot(format!(
                    "existing non-empty directory {} has no ownership sentinel",
                    canonical.display()
                )));
            }
            write_new_json(
                &sentinel_path,
                &RootSentinel {
                    kind: "melodee-search-root".to_string(),
                    version: 1,
                },
            )?;
        }
        Ok(Self { path: canonical })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn generation_path(&self, generation_id: Uuid) -> PathBuf {
        self.path.join(format!("gen_{generation_id}"))
    }

    pub fn create_generation(
        &self,
        generation_id: Uuid,
        created_at: OffsetDateTime,
        schema_version: u32,
        source_high_water_mark: i64,
    ) -> Result<PathBuf, SearchError> {
        if generation_id.get_version_num() != 7 || schema_version == 0 || source_high_water_mark < 0
        {
            return Err(SearchError::InvalidRoot(
                "generation requires a UUIDv7, positive schema version, and non-negative high-water mark"
                    .to_string(),
            ));
        }
        let path = self.generation_path(generation_id);
        fs::create_dir(&path).map_err(|source| SearchError::Io {
            path: path.clone(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|source| {
                SearchError::Io {
                    path: path.clone(),
                    source,
                }
            })?;
        }
        let manifest = GenerationManifest {
            generation_id,
            kind: "melodee-search-generation".to_string(),
            created_at,
            document_counts: melodee_persistence::SearchDocumentCounts {
                artists: 0,
                releases: 0,
                tracks: 0,
            },
            schema_version,
            source_high_water_mark,
        };
        write_new_json(&path.join(GENERATION_MANIFEST), &manifest)?;
        Ok(path)
    }

    pub fn reset_generation(
        &self,
        generation_id: Uuid,
        created_at: OffsetDateTime,
        schema_version: u32,
        source_high_water_mark: i64,
    ) -> Result<PathBuf, SearchError> {
        let path = self.generation_path(generation_id);
        if path.exists() {
            self.remove_generation(generation_id)?;
        }
        self.create_generation(
            generation_id,
            created_at,
            schema_version,
            source_high_water_mark,
        )
    }

    pub fn read_manifest(&self, generation_id: Uuid) -> Result<GenerationManifest, SearchError> {
        let path = self.generation_path(generation_id);
        validate_direct_child(&self.path, &path)?;
        let manifest_path = path.join(GENERATION_MANIFEST);
        let raw = fs::read(&manifest_path).map_err(|source| SearchError::Io {
            path: manifest_path,
            source,
        })?;
        let manifest: GenerationManifest = serde_json::from_slice(&raw)?;
        if manifest.generation_id != generation_id || manifest.kind != "melodee-search-generation" {
            return Err(SearchError::UnownedRoot(path.display().to_string()));
        }
        if generation_id.get_version_num() != 7
            || manifest.schema_version == 0
            || manifest.source_high_water_mark < 0
        {
            return Err(SearchError::UnownedRoot(path.display().to_string()));
        }
        Ok(manifest)
    }

    pub fn seal_generation(
        &self,
        mut manifest: GenerationManifest,
        counts: melodee_persistence::SearchDocumentCounts,
    ) -> Result<String, SearchError> {
        let current = self.read_manifest(manifest.generation_id)?;
        if manifest != current {
            return Err(SearchError::UnownedRoot(format!(
                "generation manifest identity changed for {}",
                manifest.generation_id
            )));
        }
        manifest.document_counts = counts;
        let path = self
            .generation_path(manifest.generation_id)
            .join(GENERATION_MANIFEST);
        let bytes = serde_json::to_vec(&manifest)?;
        let temporary = path.with_extension("json.pending");
        fs::write(&temporary, &bytes).map_err(|source| SearchError::Io {
            path: temporary.clone(),
            source,
        })?;
        fs::rename(&temporary, &path).map_err(|source| SearchError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }

    pub fn remove_generation(&self, generation_id: Uuid) -> Result<(), SearchError> {
        self.remove_generation_with_hook(generation_id, || {})
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn remove_generation_with_test_hook(
        &self,
        generation_id: Uuid,
        before_generation_open: impl FnOnce(),
    ) -> Result<(), SearchError> {
        self.remove_generation_with_hook(generation_id, before_generation_open)
    }

    fn remove_generation_with_hook(
        &self,
        generation_id: Uuid,
        before_generation_open: impl FnOnce(),
    ) -> Result<(), SearchError> {
        if generation_id.get_version_num() != 7 {
            return Err(SearchError::UnownedRoot(
                self.generation_path(generation_id).display().to_string(),
            ));
        }
        secure_remove_generation(&self.path, generation_id, before_generation_open)
    }
}

fn directory_has_entries(path: &Path) -> Result<bool, SearchError> {
    let mut entries = fs::read_dir(path).map_err(|source| SearchError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    entries
        .next()
        .transpose()
        .map(|entry| entry.is_some())
        .map_err(|source| SearchError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn write_new_json<T: Serialize>(path: &Path, value: &T) -> Result<(), SearchError> {
    let bytes = serde_json::to_vec(value)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| SearchError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(&bytes).map_err(|source| SearchError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| SearchError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn validate_direct_child(root: &Path, child: &Path) -> Result<(), SearchError> {
    if child.parent() != Some(root) || child.file_name().is_none() {
        return Err(SearchError::UnownedRoot(child.display().to_string()));
    }
    let metadata = fs::symlink_metadata(child).map_err(|source| SearchError::Io {
        path: child.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(SearchError::UnownedRoot(child.display().to_string()));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn secure_remove_generation(
    root: &Path,
    generation_id: Uuid,
    before_generation_open: impl FnOnce(),
) -> Result<(), SearchError> {
    use std::os::unix::ffi::OsStrExt;

    use rustix::{
        fd::OwnedFd,
        fs::{
            AtFlags, CWD, Dir, FileType, Mode, OFlags, RenameFlags, fstat, openat, renameat_with,
            statat, unlinkat,
        },
    };

    fn io_error(error: rustix::io::Errno) -> std::io::Error {
        std::io::Error::from_raw_os_error(error.raw_os_error())
    }

    fn open_absolute_directory(path: &Path) -> Result<OwnedFd, SearchError> {
        use std::path::Component;

        if !path.is_absolute() {
            return Err(SearchError::InvalidRoot(
                "search root must be absolute before generation retirement".to_string(),
            ));
        }
        let mut current = openat(
            CWD,
            b"/".as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|error| SearchError::Io {
            path: PathBuf::from("/"),
            source: io_error(error),
        })?;
        for component in path.components() {
            let Component::Normal(name) = component else {
                if matches!(component, Component::RootDir) {
                    continue;
                }
                return Err(SearchError::InvalidRoot(
                    "search root contains traversal".to_string(),
                ));
            };
            current = openat(
                &current,
                name.as_bytes(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(|error| SearchError::Io {
                path: path.to_path_buf(),
                source: io_error(error),
            })?;
        }
        Ok(current)
    }

    fn remove_contents(directory: &OwnedFd, display_path: &Path) -> Result<(), SearchError> {
        let mut entries = Dir::read_from(directory).map_err(|error| SearchError::Io {
            path: display_path.to_path_buf(),
            source: io_error(error),
        })?;
        for entry in &mut entries {
            let entry = entry.map_err(|error| SearchError::Io {
                path: display_path.to_path_buf(),
                source: io_error(error),
            })?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let entry_path = display_path.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
            let stat = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
                SearchError::Io {
                    path: entry_path.clone(),
                    source: io_error(error),
                }
            })?;
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => {
                    let child = openat(
                        directory,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                        Mode::empty(),
                    )
                    .map_err(|error| SearchError::Io {
                        path: entry_path.clone(),
                        source: io_error(error),
                    })?;
                    remove_contents(&child, &entry_path)?;
                    unlinkat(directory, name, AtFlags::REMOVEDIR).map_err(|error| {
                        SearchError::Io {
                            path: entry_path,
                            source: io_error(error),
                        }
                    })?;
                }
                FileType::RegularFile => {
                    unlinkat(directory, name, AtFlags::empty()).map_err(|error| {
                        SearchError::Io {
                            path: entry_path,
                            source: io_error(error),
                        }
                    })?;
                }
                _ => return Err(SearchError::UnownedRoot(entry_path.display().to_string())),
            }
        }
        Ok(())
    }

    let root_fd = open_absolute_directory(root)?;
    before_generation_open();
    let generation_name = format!("gen_{generation_id}");
    let retiring_name = format!(".retiring_{generation_name}");
    let original_exists = statat(
        &root_fd,
        generation_name.as_bytes(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .is_ok();
    let retiring_exists = statat(
        &root_fd,
        retiring_name.as_bytes(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .is_ok();
    if original_exists && retiring_exists {
        return Err(SearchError::UnownedRoot(format!(
            "generation and retirement quarantine both exist for {generation_id}"
        )));
    }
    if original_exists {
        renameat_with(
            &root_fd,
            generation_name.as_bytes(),
            &root_fd,
            retiring_name.as_bytes(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| SearchError::Io {
            path: root.join(&generation_name),
            source: io_error(error),
        })?;
    } else if !retiring_exists {
        return Err(SearchError::Io {
            path: root.join(&generation_name),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        });
    }
    // The atomic in-root rename removes the public generation name before
    // traversal. Retries adopt the private quarantine after a crash, and all
    // subsequent operations remain relative to the verified root descriptor.
    let generation_path = root.join(&retiring_name);
    let generation_fd = openat(
        &root_fd,
        retiring_name.as_bytes(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| SearchError::Io {
        path: generation_path.clone(),
        source: io_error(error),
    })?;
    let manifest_fd = openat(
        &generation_fd,
        GENERATION_MANIFEST.as_bytes(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| SearchError::Io {
        path: generation_path.join(GENERATION_MANIFEST),
        source: io_error(error),
    })?;
    let manifest_stat = fstat(&manifest_fd).map_err(|error| SearchError::Io {
        path: generation_path.join(GENERATION_MANIFEST),
        source: io_error(error),
    })?;
    if FileType::from_raw_mode(manifest_stat.st_mode) != FileType::RegularFile {
        return Err(SearchError::UnownedRoot(
            generation_path
                .join(GENERATION_MANIFEST)
                .display()
                .to_string(),
        ));
    }
    let mut manifest_bytes = Vec::new();
    std::fs::File::from(manifest_fd)
        .take(MAX_GENERATION_MANIFEST_BYTES + 1)
        .read_to_end(&mut manifest_bytes)
        .map_err(|source| SearchError::Io {
            path: generation_path.join(GENERATION_MANIFEST),
            source,
        })?;
    if u64::try_from(manifest_bytes.len()).unwrap_or(u64::MAX) > MAX_GENERATION_MANIFEST_BYTES {
        return Err(SearchError::UnownedRoot(format!(
            "generation manifest exceeds {MAX_GENERATION_MANIFEST_BYTES} bytes"
        )));
    }
    let manifest: GenerationManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.generation_id != generation_id
        || manifest.kind != "melodee-search-generation"
        || manifest.schema_version == 0
        || manifest.source_high_water_mark < 0
    {
        return Err(SearchError::UnownedRoot(
            generation_path.display().to_string(),
        ));
    }
    remove_contents(&generation_fd, &generation_path)?;
    drop(generation_fd);
    unlinkat(&root_fd, retiring_name.as_bytes(), AtFlags::REMOVEDIR).map_err(|error| {
        SearchError::Io {
            path: generation_path,
            source: io_error(error),
        }
    })?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn secure_remove_generation(
    root: &Path,
    generation_id: Uuid,
    before_generation_open: impl FnOnce(),
) -> Result<(), SearchError> {
    let _ = before_generation_open;
    Err(SearchError::InvalidRoot(format!(
        "secure generation retirement is unsupported on this platform: {}",
        root.join(format!("gen_{generation_id}")).display()
    )))
}

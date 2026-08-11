//! Ownership-validated filesystem lifecycle for Tantivy generations.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::validate_search_root;
use crate::error::SearchError;

const ROOT_SENTINEL: &str = ".melodee-search-root.json";
const GENERATION_MANIFEST: &str = "generation.json";

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
        let path = self.generation_path(generation_id);
        validate_direct_child(&self.path, &path)?;
        let _ = self.read_manifest(generation_id)?;
        remove_owned_tree(&path)?;
        Ok(())
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

fn remove_owned_tree(path: &Path) -> Result<(), SearchError> {
    for entry in fs::read_dir(path).map_err(|source| SearchError::Io {
        path: path.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| SearchError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path).map_err(|source| SearchError::Io {
            path: entry_path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(SearchError::UnownedRoot(format!(
                "generation contains symlink {}",
                entry_path.display()
            )));
        }
        if metadata.is_dir() {
            remove_owned_tree(&entry_path)?;
        } else if metadata.is_file() {
            fs::remove_file(&entry_path).map_err(|source| SearchError::Io {
                path: entry_path,
                source,
            })?;
        } else {
            return Err(SearchError::UnownedRoot(entry_path.display().to_string()));
        }
    }
    fs::remove_dir(path).map_err(|source| SearchError::Io {
        path: path.to_path_buf(),
        source,
    })
}

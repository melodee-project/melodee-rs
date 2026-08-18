//! Canonical media adoption validation (ADR-0148 §D.3, R3.6). The migration
//! never moves, re-encodes, re-hashes, fingerprints, or deletes media; when
//! the operator mounts the canonical root it validates — read-only — that
//! each adopted path is contained, symlink-safe, present, and the expected
//! size.

use std::path::{Path, PathBuf};

/// Outcome of one adopted-file check. Missing/size-mismatch files do not
/// block catalog identity; an escape is unsafe and the track is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCheck {
    /// File present under the root and the size matches the legacy metadata.
    Ok,
    /// No file at the adopted path (recorded, imported anyway).
    Missing,
    /// File present but the byte length differs from legacy `FileSize`.
    SizeMismatch,
    /// The path resolves outside the canonical root through a symlinked
    /// component (recorded, track skipped).
    Escape,
}

/// Precomputed canonical root for per-file validation.
pub struct CanonicalRootValidator {
    root: PathBuf,
    canonical_root: PathBuf,
}

impl CanonicalRootValidator {
    /// # Errors
    /// Returns an actionable message when the root does not exist.
    pub fn new(root: &Path) -> Result<Self, String> {
        let canonical_root = root
            .canonicalize()
            .map_err(|e| format!("cannot resolve --canonical-root {}: {e}", root.display()))?;
        Ok(Self {
            root: root.to_path_buf(),
            canonical_root,
        })
    }

    /// Read-only validation of one adopted canonical-relative path.
    pub fn check(&self, relative: &str, expected_size: Option<i64>) -> MediaCheck {
        let candidate = self.root.join(relative);
        let metadata = match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(_) => return MediaCheck::Missing,
        };
        // Containment proof: resolve every symlink component and require the
        // real path to stay under the canonical root. Metadata above proves
        // existence, so canonicalize cannot race into a different answer
        // except under operator mutation, which migration never performs.
        let resolved = match candidate.canonicalize() {
            Ok(resolved) => resolved,
            Err(_) => return MediaCheck::Missing,
        };
        if !resolved.starts_with(&self.canonical_root) {
            return MediaCheck::Escape;
        }
        if let Some(expected) = expected_size.filter(|e| *e >= 0) {
            let actual = if metadata.is_symlink() {
                resolved.metadata().map(|m| m.len()).unwrap_or(0)
            } else {
                metadata.len()
            };
            if i64::try_from(actual).unwrap_or(i64::MAX) != expected {
                return MediaCheck::SizeMismatch;
            }
        }
        MediaCheck::Ok
    }
}

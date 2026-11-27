//! Structural validation of normalized output.
//!
//! This boundary treats the stored manifest and work directory as untrusted.
//! No canonical target path is touched until every planned input has passed
//! path, file-type, size, and streamed BLAKE3 verification.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use melodee_domain::{
    CURRENT_MANIFEST_VERSION, CanonicalAsset, CanonicalAssetKind, Finding, FindingSeverity,
    ObservedFileKind, ReleaseManifest, StructuralValidation, ValidationOutcome,
};
use thiserror::Error;

/// Stable structural-validation profile recorded in manifest v3.
pub const STRUCTURAL_VALIDATION_PROFILE: &str = "phase3.structural-validation";
/// Current structural-validation profile version. Version 2 adds the ADR-0146
/// segment-aware inventory rules for CUE-governed sources; ungoverned releases
/// validate under the unchanged version-1 rules.
pub const STRUCTURAL_VALIDATION_PROFILE_VERSION: u16 = 2;

/// A normalized bundle whose complete asset inventory is safe to plan.
#[derive(Debug, Clone)]
pub struct ValidatedNormalization {
    pub work_root: PathBuf,
    pub validation: StructuralValidation,
    pub assets: Vec<CanonicalAsset>,
}

/// Fatal inconsistency in a normalized manifest or its work assets.
#[derive(Debug, Error)]
pub enum StructuralValidationError {
    #[error(
        "manifest schema version {actual} is not the Phase 3.2 normalization version {expected}"
    )]
    UnsupportedManifestVersion { actual: u16, expected: u16 },

    #[error("normalized manifest is incomplete: {0}")]
    IncompleteManifest(String),

    #[error("structural validation was canceled")]
    Canceled,

    #[error("unsafe normalized asset path {path:?}: {reason}")]
    UnsafePath { path: String, reason: &'static str },

    #[error("normalization work root {path:?} is outside configured work area {base:?}")]
    WorkRootOutsideBase { path: PathBuf, base: PathBuf },

    #[error("symlinks are forbidden in normalized publication inputs: {0:?}")]
    Symlink(PathBuf),

    #[error("normalized publication input is not a regular file: {0:?}")]
    NotRegularFile(PathBuf),

    #[error("normalized asset size mismatch for {path:?}: expected {expected}, observed {actual}")]
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },

    #[error(
        "normalized asset BLAKE3 mismatch for {path:?}: expected {expected}, observed {actual}"
    )]
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("failed to {operation} normalized publication input {path:?}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Validates the normalized v2 manifest and every planned work-area asset.
///
/// Missing presentation or ordering observations produce `NeedsReview` with
/// explicit findings. Missing, mismatched, or untrusted bytes are fatal and
/// must not be routed to canonical storage.
pub fn validate_normalized_manifest(
    manifest: &ReleaseManifest,
    configured_work_area: &Path,
) -> Result<ValidatedNormalization, StructuralValidationError> {
    validate_normalized_manifest_with_cancellation(
        manifest,
        configured_work_area,
        &AtomicBool::new(false),
    )
}

pub(crate) fn validate_normalized_manifest_with_cancellation(
    manifest: &ReleaseManifest,
    configured_work_area: &Path,
    cancellation: &AtomicBool,
) -> Result<ValidatedNormalization, StructuralValidationError> {
    if manifest.schema_version != CURRENT_MANIFEST_VERSION {
        return Err(StructuralValidationError::UnsupportedManifestVersion {
            actual: manifest.schema_version,
            expected: CURRENT_MANIFEST_VERSION,
        });
    }
    if manifest.publication.is_some() {
        return Err(StructuralValidationError::IncompleteManifest(
            "normalization v2 unexpectedly contains publication facts".to_owned(),
        ));
    }
    if !manifest.inspection.source_stable {
        return Err(StructuralValidationError::IncompleteManifest(
            "inspection did not record a stable source".to_owned(),
        ));
    }
    let normalization = manifest.normalization.as_ref().ok_or_else(|| {
        StructuralValidationError::IncompleteManifest(
            "normalization observations are missing".to_owned(),
        )
    })?;
    if normalization.profile_version.trim().is_empty() {
        return Err(StructuralValidationError::IncompleteManifest(
            "normalization profile version is missing".to_owned(),
        ));
    }
    if normalization.outputs.is_empty() {
        return Err(StructuralValidationError::IncompleteManifest(
            "at least one playable normalized media asset is required".to_owned(),
        ));
    }

    let work_root = derive_work_root(manifest, configured_work_area)?;

    let audio_sources = manifest
        .files
        .iter()
        .filter(|file| file.kind == ObservedFileKind::Audio)
        .map(|file| file.relative_path.as_str())
        .collect::<BTreeSet<_>>();

    // ADR-0146: Tracks and outputs of a CUE-governed source are keyed by the
    // deterministic segment identity `<source>#cue-track-NN`, not the plain
    // source path. Join Tracks to outputs on the logical track key first,
    // then compare the governing source inventory against observed audio.
    let mut output_by_key = BTreeMap::new();
    for output in &normalization.outputs {
        if output_by_key.insert(output.track_key(), output).is_some() {
            return Err(StructuralValidationError::IncompleteManifest(
                "normalized output track identities are not unique".to_owned(),
            ));
        }
    }
    let output_keys = output_by_key
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let track_keys = manifest
        .tracks
        .iter()
        .map(|track| track.file_path.as_str())
        .collect::<BTreeSet<_>>();
    if track_keys.len() != manifest.tracks.len() {
        return Err(StructuralValidationError::IncompleteManifest(
            "manifest Track identities are not unique".to_owned(),
        ));
    }
    if track_keys != output_keys {
        return Err(StructuralValidationError::IncompleteManifest(
            "manifest Track and normalized output inventories do not match".to_owned(),
        ));
    }
    let track_sources = track_keys
        .iter()
        .map(|key| {
            output_by_key
                .get(*key)
                .map_or(*key, |output| output.source_file.as_str())
        })
        .collect::<BTreeSet<_>>();
    if audio_sources.is_empty() || audio_sources != track_sources {
        return Err(StructuralValidationError::IncompleteManifest(
            "audio file and Track inventories do not match".to_owned(),
        ));
    }

    let mut fingerprint_sources = BTreeSet::new();
    for fingerprint in &normalization.fingerprints {
        validate_relative_path(&fingerprint.source_file)?;
        if !audio_sources.contains(fingerprint.source_file.as_str()) {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "fingerprint references unknown audio source {:?}",
                fingerprint.source_file
            )));
        }
        if !fingerprint_sources.insert(fingerprint.source_file.as_str()) {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "audio source {:?} has multiple fingerprints",
                fingerprint.source_file
            )));
        }
        if fingerprint.fingerprint.trim().is_empty() || fingerprint.duration_ms == 0 {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "fingerprint for {:?} is incomplete",
                fingerprint.source_file
            )));
        }
    }
    if fingerprint_sources != audio_sources {
        return Err(StructuralValidationError::IncompleteManifest(
            "every normalized audio source requires exactly one acoustic fingerprint".to_owned(),
        ));
    }

    let mut assets =
        Vec::with_capacity(normalization.outputs.len() + normalization.normalized_artwork.len());
    let mut normalized_paths = BTreeSet::new();
    let mut output_sources = BTreeSet::new();
    // Per-source governance shape: ungoverned sources have exactly one
    // whole-file output; ADR-0146 governed sources have only segment outputs
    // whose identities are keyed by `<source>#cue-track-NN`. Whole-file and
    // segment outputs must never mix for one source (whole-file suppression).
    let mut source_governance = BTreeMap::<&str, (usize, usize)>::new();
    for output in &normalization.outputs {
        validate_relative_path(&output.source_file)?;
        validate_relative_path(&output.output_path)?;
        if !audio_sources.contains(output.source_file.as_str()) {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "normalized output references unknown audio source {:?}",
                output.source_file
            )));
        }
        output_sources.insert(output.source_file.as_str());
        let entry = source_governance
            .entry(output.source_file.as_str())
            .or_insert((0, 0));
        entry.0 += 1;
        if let Some(segment) = &output.segment {
            entry.1 += 1;
            validate_relative_path(&segment.cue)?;
            let cue_parsed = normalization.parsed_sidecars.iter().any(|sidecar| {
                sidecar.sidecar_type == "cue" && sidecar.relative_path == segment.cue
            });
            if !cue_parsed {
                return Err(StructuralValidationError::IncompleteManifest(format!(
                    "CUE segment output {:?} has no parsed CUE sidecar observation for {:?}",
                    output.output_path, segment.cue
                )));
            }
            if segment.track == 0 {
                return Err(StructuralValidationError::IncompleteManifest(format!(
                    "CUE segment output {:?} has an invalid track number",
                    output.output_path
                )));
            }
        }
        if !normalized_paths.insert(output.output_path.as_str()) {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "normalized output path {:?} is duplicated",
                output.output_path
            )));
        }
        if output.container != "mp3"
            || output.codec != "mp3"
            || output.sample_rate_hz == 0
            || output.channels == 0
            || output.duration_ms == 0
        {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "normalized audio {:?} is not a complete playable MP3 observation",
                output.output_path
            )));
        }
        let size_bytes = verify_asset(
            &work_root,
            &output.output_path,
            &output.byte_hash,
            cancellation,
        )?;
        assets.push(CanonicalAsset {
            kind: CanonicalAssetKind::Media,
            // ADR-0146: the logical track key, not the raw source path, joins
            // canonical media assets to Tracks; segment keys keep segment
            // assets distinct for one governed source.
            source_path: output.track_key(),
            normalized_path: output.output_path.clone(),
            canonical_path: format!("media/{}.mp3", output.byte_hash),
            size_bytes,
            byte_hash: output.byte_hash.clone(),
        });
    }
    for (source, (total, segmented)) in &source_governance {
        let governed = *segmented > 0;
        if (governed && segmented != total) || (!governed && *total != 1) {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "audio source {source:?} mixes whole-file and CUE segment outputs or has duplicate whole-file outputs"
            )));
        }
    }
    if output_sources != audio_sources {
        return Err(StructuralValidationError::IncompleteManifest(
            "not every observed audio file has a normalized output".to_owned(),
        ));
    }

    for artwork in &normalization.normalized_artwork {
        validate_logical_source(&artwork.source_file)?;
        validate_relative_path(&artwork.output_path)?;
        if !normalized_paths.insert(artwork.output_path.as_str()) {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "normalized output path {:?} is duplicated",
                artwork.output_path
            )));
        }
        if artwork.mime_type != "image/jpeg" || artwork.width == 0 || artwork.height == 0 {
            return Err(StructuralValidationError::IncompleteManifest(format!(
                "normalized artwork {:?} is not a complete JPEG observation",
                artwork.output_path
            )));
        }
        let size_bytes = verify_asset(
            &work_root,
            &artwork.output_path,
            &artwork.byte_hash,
            cancellation,
        )?;
        assets.push(CanonicalAsset {
            kind: CanonicalAssetKind::Artwork,
            source_path: artwork.source_file.clone(),
            normalized_path: artwork.output_path.clone(),
            canonical_path: format!("artwork/{}.jpg", artwork.byte_hash),
            size_bytes,
            byte_hash: artwork.byte_hash.clone(),
        });
    }
    assets.sort_by(|left, right| {
        (
            &left.canonical_path,
            left.kind,
            &left.source_path,
            &left.normalized_path,
        )
            .cmp(&(
                &right.canonical_path,
                right.kind,
                &right.source_path,
                &right.normalized_path,
            ))
    });

    let findings = presentation_findings(manifest);
    let outcome = if findings.is_empty() {
        ValidationOutcome::Passed
    } else {
        ValidationOutcome::NeedsReview
    };
    Ok(ValidatedNormalization {
        work_root,
        validation: StructuralValidation {
            profile_name: STRUCTURAL_VALIDATION_PROFILE.to_owned(),
            profile_version: STRUCTURAL_VALIDATION_PROFILE_VERSION,
            outcome,
            findings,
        },
        assets,
    })
}

pub(crate) fn derive_work_root(
    manifest: &ReleaseManifest,
    configured_work_area: &Path,
) -> Result<PathBuf, StructuralValidationError> {
    let normalization = manifest.normalization.as_ref().ok_or_else(|| {
        StructuralValidationError::IncompleteManifest(
            "normalization observations are missing".to_owned(),
        )
    })?;
    let attempt_id = normalization.provenance.attempt_id.ok_or_else(|| {
        StructuralValidationError::IncompleteManifest(
            "normalization attempt identity is missing".to_owned(),
        )
    })?;
    let derived_work_root = configured_work_area
        .join(manifest.work_item_id.simple().to_string())
        .join("attempts")
        .join(attempt_id.simple().to_string());
    if let Some(claimed) = normalization.provenance.work_area.as_deref()
        && Path::new(claimed) != derived_work_root
    {
        return Err(StructuralValidationError::IncompleteManifest(
            "normalization work-area provenance does not match work-item and attempt identities"
                .to_owned(),
        ));
    }
    trusted_work_root(configured_work_area, &derived_work_root)
}

fn trusted_work_root(
    configured_work_area: &Path,
    claimed_work_root: &Path,
) -> Result<PathBuf, StructuralValidationError> {
    let base = absolute_lexical_path(configured_work_area)?;
    let work_root = absolute_lexical_path(claimed_work_root)?;
    if work_root.strip_prefix(&base).is_err() {
        return Err(StructuralValidationError::WorkRootOutsideBase {
            path: work_root,
            base,
        });
    }
    reject_symlink_components(&base, &work_root)?;
    crate::secure_fs::open_directory(&base).map_err(|source| StructuralValidationError::Io {
        operation: "open configured work area without following symlinks",
        path: base.clone(),
        source,
    })?;
    let claimed_metadata =
        fs::symlink_metadata(&work_root).map_err(|source| StructuralValidationError::Io {
            operation: "inspect normalization work root",
            path: work_root.clone(),
            source,
        })?;
    if claimed_metadata.file_type().is_symlink() {
        return Err(StructuralValidationError::Symlink(work_root));
    }
    if !claimed_metadata.is_dir() {
        return Err(StructuralValidationError::NotRegularFile(work_root));
    }
    crate::secure_fs::open_directory(&work_root).map_err(|source| {
        StructuralValidationError::Io {
            operation: "open normalization work root without following symlinks",
            path: work_root.clone(),
            source,
        }
    })?;
    Ok(work_root)
}

fn absolute_lexical_path(path: &Path) -> Result<PathBuf, StructuralValidationError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| StructuralValidationError::Io {
                operation: "resolve relative work path",
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(StructuralValidationError::WorkRootOutsideBase {
                        path: absolute,
                        base: path.to_path_buf(),
                    });
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

fn verify_asset(
    work_root: &Path,
    relative_path: &str,
    expected_hash: &str,
    cancellation: &AtomicBool,
) -> Result<u64, StructuralValidationError> {
    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::ValidationHash,
    );
    validate_hash(expected_hash)?;
    let path = work_root.join(relative_path);
    reject_symlink_components(work_root, &path)?;
    let link_metadata =
        fs::symlink_metadata(&path).map_err(|source| StructuralValidationError::Io {
            operation: "inspect",
            path: path.clone(),
            source,
        })?;
    if link_metadata.file_type().is_symlink() {
        return Err(StructuralValidationError::Symlink(path));
    }
    if !link_metadata.is_file() {
        return Err(StructuralValidationError::NotRegularFile(path));
    }
    let mut file = open_read_nofollow(&path).map_err(|source| StructuralValidationError::Io {
        operation: "open",
        path: path.clone(),
        source,
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| StructuralValidationError::Io {
            operation: "read metadata for",
            path: path.clone(),
            source,
        })?;
    if !metadata.is_file() {
        return Err(StructuralValidationError::NotRegularFile(path));
    }
    let (actual_hash, actual_size) = hash_reader(&mut file, &path, cancellation, &mut operation)?;
    if actual_size != metadata.len() {
        return Err(StructuralValidationError::SizeMismatch {
            path,
            expected: metadata.len(),
            actual: actual_size,
        });
    }
    if actual_hash != expected_hash {
        return Err(StructuralValidationError::HashMismatch {
            path,
            expected: expected_hash.to_owned(),
            actual: actual_hash,
        });
    }
    operation.finish();
    Ok(actual_size)
}

fn hash_reader(
    reader: &mut File,
    path: &Path,
    cancellation: &AtomicBool,
    operation: &mut melodee_media::operation_metrics::OperationGuard,
) -> Result<(String, u64), StructuralValidationError> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        if cancellation.load(Ordering::SeqCst) {
            return Err(StructuralValidationError::Canceled);
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|source| StructuralValidationError::Io {
                operation: "hash",
                path: path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        operation.add_read(u64::try_from(count).unwrap_or(u64::MAX));
        hasher.update(&buffer[..count]);
        size = size.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

fn reject_symlink_components(
    trusted_root: &Path,
    path: &Path,
) -> Result<(), StructuralValidationError> {
    let relative = path.strip_prefix(trusted_root).map_err(|_| {
        StructuralValidationError::WorkRootOutsideBase {
            path: path.to_path_buf(),
            base: trusted_root.to_path_buf(),
        }
    })?;
    let mut current = trusted_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(StructuralValidationError::Symlink(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(StructuralValidationError::Io {
                    operation: "inspect path component for",
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), StructuralValidationError> {
    let parsed = Path::new(path);
    let valid = !path.is_empty()
        && !path.contains('\\')
        && !parsed.is_absolute()
        && parsed
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if valid {
        Ok(())
    } else {
        Err(StructuralValidationError::UnsafePath {
            path: path.to_owned(),
            reason: "expected a portable relative path with no traversal",
        })
    }
}

fn validate_logical_source(path: &str) -> Result<(), StructuralValidationError> {
    let filesystem_part = path.split('#').next().unwrap_or(path);
    validate_relative_path(filesystem_part)
}

fn validate_hash(hash: &str) -> Result<(), StructuralValidationError> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(StructuralValidationError::IncompleteManifest(format!(
            "BLAKE3 hash is not 64 lowercase hexadecimal characters: {hash:?}"
        )))
    }
}

fn presentation_findings(manifest: &ReleaseManifest) -> Vec<Finding> {
    let mut findings = Vec::new();
    if manifest
        .release_observation
        .title
        .as_deref()
        .is_none_or(|title| title.trim().is_empty())
    {
        findings.push(review_finding(
            "validation.release_title_missing",
            "Release title is missing; operator review is required",
            None,
        ));
    }
    if manifest.release_observation.artists.is_empty()
        || manifest
            .release_observation
            .artists
            .iter()
            .any(|artist| artist.trim().is_empty())
    {
        findings.push(review_finding(
            "validation.release_artist_missing",
            "Release Artist display observation is missing; operator review is required",
            None,
        ));
    }

    let mut ordering = BTreeMap::new();
    for track in &manifest.tracks {
        if track
            .track_title
            .as_deref()
            .is_none_or(|title| title.trim().is_empty())
        {
            findings.push(review_finding(
                "validation.track_title_missing",
                "Track title is missing; operator review is required",
                Some(track.file_path.clone()),
            ));
        }
        let Some(track_number) = track.track_number else {
            findings.push(review_finding(
                "validation.track_number_missing",
                "Track ordering is missing; operator review is required",
                Some(track.file_path.clone()),
            ));
            continue;
        };
        let key = (track.media_number.unwrap_or(1), track_number);
        if let Some(first) = ordering.insert(key, &track.file_path) {
            findings.push(Finding {
                code: "validation.track_order_ambiguous".to_owned(),
                message: "Multiple Tracks have the same disc/track ordering".to_owned(),
                severity: FindingSeverity::Warning,
                file: Some(track.file_path.clone()),
                evidence: serde_json::json!({ "first_file": first, "ordering": key }),
            });
        }
    }
    findings.sort_by(|left, right| {
        (&left.code, &left.file, &left.message).cmp(&(&right.code, &right.file, &right.message))
    });
    findings
}

fn review_finding(code: &str, message: &str, file: Option<String>) -> Finding {
    Finding {
        code: code.to_owned(),
        message: message.to_owned(),
        severity: FindingSeverity::Warning,
        file,
        evidence: serde_json::json!({ "action": "operator_review" }),
    }
}

fn open_read_nofollow(path: &Path) -> io::Result<File> {
    crate::secure_fs::open_read(path)
}

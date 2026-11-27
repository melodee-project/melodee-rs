//! Media normalization, acoustic fingerprinting, and sidecar parsing.
//!
//! Produces canonical MP3 audio in an isolated work-area staging directory,
//! Chromaprint fingerprints via `fpcalc`, normalized artwork, and parsed sidecar
//! semantics. Source media is never modified. All output paths are constrained
//! to the work-area directory to prevent path traversal.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration as StdDuration, Instant},
};

use melodee_domain::{
    CANONICAL_MP3_PROFILE_VERSION, CURRENT_MANIFEST_VERSION, CueSegmentIdentity, Finding,
    FindingSeverity, FingerprintObservation, INSPECTION_MANIFEST_VERSION,
    NormalizationExecutionProvenance, NormalizationObservations, NormalizedArtwork,
    NormalizedOutput, ObservedArtwork, ObservedFile, ObservedFileKind, ObservedMetadata,
    ObservedTrack, ParsedSidecar, ReleaseManifest,
};
use quick_xml::{Reader, events::Event};
use serde::Deserialize;
use thiserror::Error;
use time::OffsetDateTime;
use tracing::info;
use uuid::Uuid;

use crate::secure_fs;
use crate::{
    InspectionOptions, ProbeResult, hash_file, image_dimensions, path_to_posix, read_limited,
    run_ffprobe_cancellable, tool_version_with_cancellation,
};

/// Converted MP3 profile parameters (PRD-MEDIA-002). The legacy-seeded FFmpeg
/// arguments combine VBR quality 4 with `-b:a 384k`; libmp3lame's VBR path
/// ignores the bitrate argument, so it is not a maximum. Conversion forces
/// 48 kHz, drops video, preserves mono/stereo, and downmixes wider layouts to
/// libmp3lame's stereo limit.
const CANONICAL_SAMPLE_RATE_HZ: u32 = 48_000;
const CANONICAL_ARTWORK_MAX_DIMENSION: u32 = 1600;
const MAX_ARTWORK_SOURCE_DIMENSION: u32 = 16_384;
const MAX_ARTWORK_SOURCE_PIXELS: u64 = 100_000_000;
const FINGERPRINT_LENGTH_SECONDS: u32 = 120;
const OUTPUT_DURATION_TOLERANCE_MS: u64 = 250;
const DEFAULT_MAX_TOOL_OUTPUT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_ARTWORK_BYTES: usize = 16 * 1024 * 1024;
const MAX_SIDECAR_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_TIMEOUT_FFMPEG: StdDuration = StdDuration::from_secs(300);
const DEFAULT_TIMEOUT_FPCALC: StdDuration = StdDuration::from_secs(60);
const DEFAULT_TIMEOUT_FFPROBE: StdDuration = StdDuration::from_secs(30);

/// Configuration for the normalization stage. All paths and timeouts are
/// explicit configuration; never inferred from ambient environment.
#[derive(Debug, Clone)]
pub struct NormalizationOptions {
    pub ffmpeg_path: PathBuf,
    pub ffmpeg_timeout: StdDuration,
    pub fpcalc_path: PathBuf,
    pub fpcalc_timeout: StdDuration,
    pub ffprobe_path: PathBuf,
    pub ffprobe_timeout: StdDuration,
    pub max_tool_output_bytes: usize,
    pub max_artwork_bytes: usize,
    pub work_area: PathBuf,
    /// Unique physical execution attempt. It is provenance, never semantic
    /// output identity.
    pub attempt_id: Uuid,
    pub profile_version: String,
    pub cancellation: Arc<AtomicBool>,
    /// Optional process-wide graceful-shutdown signal. Lease cancellation is
    /// kept separate so one lost item never cancels sibling work.
    pub shutdown: Option<Arc<AtomicBool>>,
}

impl Default for NormalizationOptions {
    fn default() -> Self {
        Self {
            ffmpeg_path: PathBuf::from("ffmpeg"),
            ffmpeg_timeout: DEFAULT_TIMEOUT_FFMPEG,
            fpcalc_path: PathBuf::from("fpcalc"),
            fpcalc_timeout: DEFAULT_TIMEOUT_FPCALC,
            ffprobe_path: PathBuf::from("ffprobe"),
            ffprobe_timeout: DEFAULT_TIMEOUT_FFPROBE,
            max_tool_output_bytes: DEFAULT_MAX_TOOL_OUTPUT_BYTES,
            max_artwork_bytes: DEFAULT_MAX_ARTWORK_BYTES,
            work_area: PathBuf::from("/tmp/melodee-work"),
            attempt_id: Uuid::now_v7(),
            profile_version: CANONICAL_MP3_PROFILE_VERSION.to_owned(),
            cancellation: Arc::new(AtomicBool::new(false)),
            shutdown: None,
        }
    }
}

/// Result of normalizing one Release manifest.
#[derive(Debug)]
pub struct NormalizationReport {
    pub manifest: ReleaseManifest,
    pub manifest_hash: String,
    pub outputs_produced: u64,
    pub fingerprints_produced: u64,
    pub artwork_produced: u64,
    pub sidecars_parsed: u64,
    pub bytes_processed: u64,
    pub duration_ms: u64,
    pub findings: Vec<Finding>,
    pub provenance: NormalizationExecutionProvenance,
}

#[derive(Debug, Error)]
pub enum NormalizationError {
    #[error("source directory is missing: {0}")]
    SourceMissing(PathBuf),
    #[error("source directory cannot be read: {path}: {message}")]
    SourceUnreadable { path: PathBuf, message: String },
    #[error("source changed since inspection: {0}")]
    SourceChanged(String),
    #[error("work area could not be created or cleaned: {path}: {message}")]
    WorkAreaCreation { path: PathBuf, message: String },
    #[error("ffmpeg process failed: {0}")]
    Ffmpeg(String),
    #[error("fpcalc process failed: {0}")]
    Fpcalc(String),
    #[error("normalized output verification failed: {0}")]
    Verification(String),
    #[error("invalid sidecar text: {0}")]
    InvalidSidecarText(&'static str),
    #[error("path escaped the work-area boundary: {0}")]
    PathEscape(String),
    #[error("manifest hashing failed: {0}")]
    ManifestHash(#[from] crate::InspectionError),
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("operation was canceled")]
    Canceled,
    #[error("missing manifest field: {0}")]
    MissingField(&'static str),
    #[error("manifest schema version {0} cannot be normalized")]
    UnsupportedManifestVersion(u16),
}

/// Normalizes all audio, artwork, and sidecars described by `manifest`.
///
/// `source_root` is the canonical, absolute path to the Release directory on
/// the read-only inbound filesystem. The function writes only to the work-area
/// staging directory under `options.work_area` and never modifies `source_root`.
///
/// The manifest is mutated in place: its `normalization` field is populated
/// with observations and `provenance` is filled with execution metadata.
/// The returned report includes the recomputed manifest hash.
pub fn normalize_release(
    manifest: &mut ReleaseManifest,
    source_root: &Path,
    options: &NormalizationOptions,
) -> Result<NormalizationReport, NormalizationError> {
    if !source_root.is_dir() {
        return Err(NormalizationError::SourceMissing(source_root.to_path_buf()));
    }

    if manifest.schema_version != INSPECTION_MANIFEST_VERSION
        && manifest.schema_version != CURRENT_MANIFEST_VERSION
    {
        return Err(NormalizationError::UnsupportedManifestVersion(
            manifest.schema_version,
        ));
    }

    if is_canceled(options) {
        return Err(NormalizationError::Canceled);
    }

    let started = Instant::now();
    let work_dir = create_work_area(
        &options.work_area,
        manifest.work_item_id,
        options.attempt_id,
    )?;

    let ffmpeg_version = tool_version_with_cancellation(
        &options.ffmpeg_path,
        options.max_tool_output_bytes,
        Some(&options.cancellation),
        options.shutdown.as_deref(),
    );
    let fpcalc_version = tool_version_with_cancellation(
        &options.fpcalc_path,
        options.max_tool_output_bytes,
        Some(&options.cancellation),
        options.shutdown.as_deref(),
    );
    if is_canceled(options) {
        return Err(NormalizationError::Canceled);
    }

    info!(
        work_item_id = %manifest.work_item_id,
        work_area = %work_dir.display(),
        ffmpeg_version = ?&ffmpeg_version,
        fpcalc_version = ?&fpcalc_version,
        "normalization started"
    );

    let mut outputs: Vec<NormalizedOutput> = Vec::new();
    let mut fingerprints: Vec<FingerprintObservation> = Vec::new();
    let mut normalized_artwork: Vec<NormalizedArtwork> = Vec::new();
    let mut parsed_sidecars: Vec<ParsedSidecar> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut bytes_processed: u64 = 0;
    let mut artwork_paths = HashSet::new();

    // Pass 1: parse sidecars and plan CUE governance (ADR-0146). Sidecar
    // observations are appended in manifest file order, matching the order
    // the previous interleaved loop produced.
    let mut cue_models: Vec<(String, CueModel)> = Vec::new();
    for file in &manifest.files {
        if is_canceled(options) {
            return Err(NormalizationError::Canceled);
        }
        if let Some(sidecar_kind) = parse_sidecar_for_file(file) {
            let source = resolve_source_file(source_root, &file.relative_path)?;
            match parse_sidecar_content(file, &source, sidecar_kind) {
                Ok(outcome) => {
                    findings.extend(outcome.record.findings.clone());
                    if let Some(model) = outcome.cue_model {
                        cue_models.push((file.relative_path.clone(), model));
                    }
                    parsed_sidecars.push(outcome.record);
                }
                Err(error) => findings.push(Finding {
                    code: "sidecar.parse_failed".to_owned(),
                    message: truncate_message(&error.to_string()),
                    severity: FindingSeverity::Warning,
                    file: Some(file.relative_path.clone()),
                    evidence: serde_json::json!({ "error": error.to_string() }),
                }),
            }
        }
    }

    // NFO and Blackbeard are active legacy metadata sources. They are kept as
    // parsed sidecars for auditability, while filling only missing release
    // observations so embedded/audio tags remain the authoritative value when
    // both sources provide the same field.
    apply_release_sidecar_overrides(manifest, &parsed_sidecars);

    // A CUE governs at most one source; each source has at most one
    // governing CUE. Plans are keyed by the source's relative path.
    let mut governance: HashMap<String, CueGovernance> = HashMap::new();
    for (cue_path, model) in &cue_models {
        if let Some(plan) =
            plan_cue_governance(cue_path, model, &manifest.files, source_root, &mut findings)
        {
            governance.insert(plan.source_relative.clone(), plan);
        }
    }

    for file in &manifest.files {
        if is_canceled(options) {
            return Err(NormalizationError::Canceled);
        }
        match file.kind {
            ObservedFileKind::Audio => {
                let source = resolve_source_file(source_root, &file.relative_path)?;
                if let Some(plan) = governance.get(&file.relative_path) {
                    let result =
                        process_cue_governed_source(file, &source, plan, &work_dir, options)?;
                    outputs.extend(result.outputs);
                    if let Some(fp) = result.fingerprint {
                        fingerprints.push(fp);
                    }
                    bytes_processed = bytes_processed.saturating_add(result.bytes);
                } else {
                    let result = process_audio_file(file, &source, &work_dir, options)?;
                    outputs.push(result.output);
                    if let Some(fp) = result.fingerprint {
                        fingerprints.push(fp);
                    }
                    bytes_processed = bytes_processed.saturating_add(result.bytes);
                }
            }
            ObservedFileKind::Artwork => {
                let source = resolve_source_file(source_root, &file.relative_path)?;
                let result = preflight_artwork(file, &source, options)
                    .and_then(|()| process_external_artwork(file, &source, &work_dir, options));
                match result {
                    Ok(Some(art)) if artwork_paths.insert(art.output_path.clone()) => {
                        normalized_artwork.push(art);
                    }
                    Ok(_) => {}
                    Err(NormalizationError::Canceled) => {
                        return Err(NormalizationError::Canceled);
                    }
                    Err(
                        error @ (NormalizationError::SourceChanged(_)
                        | NormalizationError::SourceMissing(_)
                        | NormalizationError::SourceUnreadable { .. }),
                    ) => return Err(error),
                    Err(error) => findings.push(artwork_failure(file, &error)),
                }
                verify_observed_source(file, &source)?;
            }
            _ => {}
        }

        // Both audio paths just checked the source hash after processing. A
        // further check is needed only if artwork extraction reads it again.
        if file.kind == ObservedFileKind::Audio && !file.embedded_artwork.is_empty() {
            // Embedded artwork extraction from audio files.
            for (picture_index, embedded) in file.embedded_artwork.iter().enumerate() {
                if is_canceled(options) {
                    return Err(NormalizationError::Canceled);
                }
                let source = resolve_source_file(source_root, &file.relative_path)?;
                match process_embedded_artwork(
                    embedded,
                    &file.relative_path,
                    picture_index,
                    &source,
                    &work_dir,
                    options,
                ) {
                    Ok(Some(art)) if artwork_paths.insert(art.output_path.clone()) => {
                        normalized_artwork.push(art);
                    }
                    Ok(_) => {}
                    Err(NormalizationError::Canceled) => {
                        return Err(NormalizationError::Canceled);
                    }
                    Err(error) => findings.push(artwork_failure(file, &error)),
                }
            }
            let source = resolve_source_file(source_root, &file.relative_path)?;
            verify_observed_source(file, &source)?;
        }
    }

    if !governance.is_empty() {
        // ADR-0146: a governed source's playable Tracks are the CUE segments,
        // not the whole file. Replace the inspection-time whole-file Track
        // entries and fill Release observation gaps from the governing sheet
        // (legacy: the album exists only through the sidecar path).
        let mut plans: Vec<&CueGovernance> = governance.values().collect();
        plans.sort_by(|left, right| left.source_relative.cmp(&right.source_relative));
        for plan in &plans {
            manifest
                .tracks
                .retain(|track| track.file_path != plan.source_relative);
            for segment in &plan.segments {
                let Some(output) = outputs.iter().find(|output| {
                    output.segment.as_ref().is_some_and(|identity| {
                        identity.cue == plan.cue_path && identity.track == segment.number
                    })
                }) else {
                    return Err(NormalizationError::Verification(format!(
                        "planned CUE segment {} track {} has no normalized output",
                        plan.cue_path, segment.number
                    )));
                };
                manifest.tracks.push(ObservedTrack {
                    file_path: output.track_key(),
                    track_number: Some(segment.number),
                    track_title: segment.title.clone(),
                    media_number: None,
                    sort_order: Some(segment.number),
                    byte_hash: Some(output.byte_hash.clone()),
                    duration_ms: Some(output.duration_ms),
                    observed_metadata: Some(ObservedMetadata {
                        album: plan.album_title.clone(),
                        album_artist: plan.album_performer.clone(),
                        track_title: segment.title.clone(),
                        artist: segment.performer.clone(),
                        track_number: Some(segment.number),
                        track_total: Some(u32::try_from(plan.segments.len()).unwrap_or(u32::MAX)),
                        disc_number: None,
                        disc_total: None,
                        year: None,
                        date: None,
                        genre: None,
                        raw: serde_json::json!({
                            "source": "cue",
                            "cue": plan.cue_path,
                            "track": segment.number,
                        }),
                    }),
                });
            }
        }
        manifest
            .tracks
            .sort_unstable_by(|left, right| left.file_path.cmp(&right.file_path));
        let observation = &mut manifest.release_observation;
        if observation
            .title
            .as_deref()
            .is_none_or(|title| title.trim().is_empty())
            && let Some(title) = plans.iter().find_map(|plan| plan.album_title.clone())
        {
            observation.title = Some(title);
        }
        if observation.artists.is_empty()
            && let Some(performer) = plans.iter().find_map(|plan| plan.album_performer.clone())
        {
            observation.artists.push(performer);
        }
    }

    manifest.schema_version = CURRENT_MANIFEST_VERSION;
    manifest.normalization = Some(NormalizationObservations {
        profile_version: options.profile_version.clone(),
        outputs,
        fingerprints,
        normalized_artwork,
        parsed_sidecars,
        provenance: NormalizationExecutionProvenance {
            attempt_id: Some(options.attempt_id),
            worker_id: None,
            normalized_at: Some(OffsetDateTime::now_utc()),
            elapsed_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
            ffmpeg_path: ffmpeg_version
                .as_ref()
                .and_then(|v| extract_tool_path(v, &options.ffmpeg_path)),
            ffmpeg_version,
            ffmpeg_exit_code: Some(0),
            fpcalc_path: fpcalc_version
                .as_ref()
                .and_then(|v| extract_tool_path(v, &options.fpcalc_path)),
            fpcalc_version,
            fpcalc_exit_code: Some(0),
            work_area: Some(path_to_posix(&work_dir)),
        },
        findings: findings.clone(),
    });

    let manifest_hash = crate::hash_manifest(manifest).map_err(NormalizationError::ManifestHash)?;

    let duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    info!(
        work_item_id = %manifest.work_item_id,
        outputs = manifest.normalization.as_ref().map(|n| n.outputs.len()).unwrap_or(0),
        fingerprints = manifest.normalization.as_ref().map(|n| n.fingerprints.len()).unwrap_or(0),
        elapsed_ms = duration_ms,
        "normalization completed"
    );

    let provenance = manifest
        .normalization
        .as_ref()
        .map(|n| n.provenance.clone())
        .unwrap_or_default();

    let all_findings: Vec<Finding> = manifest
        .normalization
        .as_ref()
        .map(|n| n.findings.clone())
        .unwrap_or_default();

    Ok(NormalizationReport {
        manifest: manifest.clone(),
        manifest_hash,
        outputs_produced: u64::try_from(
            manifest
                .normalization
                .as_ref()
                .map(|n| n.outputs.len())
                .unwrap_or(0),
        )
        .unwrap_or(u64::MAX),
        fingerprints_produced: u64::try_from(
            manifest
                .normalization
                .as_ref()
                .map(|n| n.fingerprints.len())
                .unwrap_or(0),
        )
        .unwrap_or(u64::MAX),
        artwork_produced: u64::try_from(
            manifest
                .normalization
                .as_ref()
                .map(|n| n.normalized_artwork.len())
                .unwrap_or(0),
        )
        .unwrap_or(u64::MAX),
        sidecars_parsed: u64::try_from(
            manifest
                .normalization
                .as_ref()
                .map(|n| n.parsed_sidecars.len())
                .unwrap_or(0),
        )
        .unwrap_or(u64::MAX),
        bytes_processed,
        duration_ms,
        findings: all_findings,
        provenance,
    })
}

fn apply_release_sidecar_overrides(manifest: &mut ReleaseManifest, sidecars: &[ParsedSidecar]) {
    for sidecar in sidecars {
        let (title, artist, date) = match sidecar.sidecar_type.as_str() {
            "nfo" => {
                let fields = sidecar.parsed.get("fields");
                (
                    fields
                        .and_then(|value| value.get("title"))
                        .and_then(value_string),
                    fields
                        .and_then(|value| value.get("album_artist"))
                        .and_then(value_string)
                        .or_else(|| {
                            fields
                                .and_then(|value| value.get("artist"))
                                .and_then(value_string)
                        }),
                    fields
                        .and_then(|value| value.get("date"))
                        .and_then(value_string)
                        .or_else(|| {
                            fields
                                .and_then(|value| value.get("year"))
                                .and_then(value_string)
                        }),
                )
            }
            "blackbeard" => {
                let release = sidecar
                    .parsed
                    .get("document")
                    .and_then(|value| value.get("release"));
                (
                    release
                        .and_then(|value| value.get("album_title"))
                        .and_then(value_string),
                    release
                        .and_then(|value| value.get("album_artist"))
                        .and_then(value_string),
                    release
                        .and_then(|value| value.get("release_year"))
                        .and_then(|value| {
                            value
                                .as_i64()
                                .map(|year| year.to_string())
                                .or_else(|| value_string(value))
                        }),
                )
            }
            _ => (None, None, None),
        };
        if manifest
            .release_observation
            .title
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            manifest.release_observation.title = title;
        }
        if manifest.release_observation.artists.is_empty()
            && let Some(artist) = artist
        {
            manifest.release_observation.artists.push(artist);
        }
        if manifest
            .release_observation
            .date
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            manifest.release_observation.date = date;
        }
    }
}

fn value_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Per-file processing result bundling the normalized output and its fingerprint.
struct AudioProcessResult {
    output: NormalizedOutput,
    fingerprint: Option<FingerprintObservation>,
    bytes: u64,
}

/// Segmentation plan for one governing CUE sheet (ADR-0146).
#[derive(Debug)]
pub(crate) struct CueGovernance {
    cue_path: String,
    source_relative: String,
    album_title: Option<String>,
    album_performer: Option<String>,
    segments: Vec<CueSegmentPlan>,
}

#[derive(Debug)]
pub(crate) struct CueSegmentPlan {
    number: u32,
    title: Option<String>,
    performer: Option<String>,
    start_ms: u64,
    /// End of the segment; `None` runs to end of stream (the last track).
    end_ms: Option<u64>,
}

/// Decide whether a parsed CUE sheet governs a staged source audio file, and
/// if so compute segment boundaries at full frame precision. Governance
/// requires: exactly one FILE reference (legacy parity: multi-FILE sheets are
/// rejected), the reference resolving inside the staged tree to a classified
/// audio file, at least one track, a first-index offset for every track,
/// strictly increasing boundaries, and a known probed source duration with
/// every start in range. Failures record actionable findings on the CUE and
/// govern nothing — the referenced audio then normalizes whole-file.
pub(crate) fn plan_cue_governance(
    cue_path: &str,
    model: &CueModel,
    files: &[ObservedFile],
    source_root: &Path,
    findings: &mut Vec<Finding>,
) -> Option<CueGovernance> {
    let mut reject = |code: &str, message: String, evidence: serde_json::Value| {
        findings.push(Finding {
            code: code.to_owned(),
            message,
            severity: FindingSeverity::Warning,
            file: Some(cue_path.to_owned()),
            evidence,
        });
    };

    if model.files.len() > 1 {
        reject(
            "sidecar.cue.multiple_files",
            format!(
                "CUE sheet declares {} FILE references; exactly one is supported",
                model.files.len()
            ),
            serde_json::json!({ "files": model.files }),
        );
        return None;
    }
    let Some(reference) = model.files.first() else {
        reject(
            "sidecar.cue.missing_source",
            "CUE sheet declares no FILE reference".to_owned(),
            serde_json::json!({}),
        );
        return None;
    };
    let reference_path = Path::new(reference);
    if reference_path.is_absolute()
        || reference_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        reject(
            "sidecar.cue.unsafe_file",
            format!("CUE FILE reference {reference:?} escapes the staged source tree"),
            serde_json::json!({ "reference": reference }),
        );
        return None;
    }
    if resolve_source_file(source_root, reference).is_err() {
        reject(
            "sidecar.cue.missing_source",
            format!("CUE FILE reference {reference:?} does not resolve inside the staged source"),
            serde_json::json!({ "reference": reference }),
        );
        return None;
    }
    let observed = files.iter().find(|file| file.relative_path == *reference);
    let Some(observed) = observed.filter(|file| file.kind == ObservedFileKind::Audio) else {
        reject(
            "sidecar.cue.missing_source",
            format!("CUE FILE reference {reference:?} is not a classified audio file"),
            serde_json::json!({ "reference": reference }),
        );
        return None;
    };
    if model.tracks.is_empty() {
        // parse_cue already recorded sidecar.cue.no_tracks.
        return None;
    }
    let Some(source_duration_ms) = observed.audio.as_ref().and_then(|audio| audio.duration_ms)
    else {
        reject(
            "sidecar.cue.invalid_boundaries",
            format!("probed duration of CUE source {reference:?} is unknown"),
            serde_json::json!({ "reference": reference }),
        );
        return None;
    };

    let mut segments: Vec<CueSegmentPlan> = Vec::with_capacity(model.tracks.len());
    for track in &model.tracks {
        let Some(start_ms) = track.first_index_ms else {
            reject(
                "sidecar.cue.invalid_boundaries",
                format!("CUE TRACK {:02} has no valid INDEX timestamp", track.number),
                serde_json::json!({ "track": track.number }),
            );
            return None;
        };
        if let Some(previous) = segments.last()
            && start_ms <= previous.start_ms
        {
            reject(
                "sidecar.cue.invalid_boundaries",
                "CUE INDEX boundaries are not strictly increasing".to_owned(),
                serde_json::json!({ "track": track.number, "start_ms": start_ms }),
            );
            return None;
        }
        if start_ms >= source_duration_ms {
            reject(
                "sidecar.cue.invalid_boundaries",
                format!(
                    "CUE TRACK {:02} starts at {start_ms} ms, beyond the source duration {source_duration_ms} ms",
                    track.number
                ),
                serde_json::json!({ "track": track.number, "start_ms": start_ms }),
            );
            return None;
        }
        segments.push(CueSegmentPlan {
            number: track.number,
            title: track.title.clone(),
            performer: track.performer.clone(),
            start_ms,
            end_ms: None,
        });
    }
    for index in 0..segments.len().saturating_sub(1) {
        segments[index].end_ms = Some(segments[index + 1].start_ms);
    }
    Some(CueGovernance {
        cue_path: cue_path.to_owned(),
        source_relative: reference.clone(),
        album_title: model.album_title.clone(),
        album_performer: model.album_performer.clone(),
        segments,
    })
}

/// Process one CUE-governed source: generate, verify, and fingerprint per
/// ADR-0146. The source-safety spine (hash before/after, probe, fingerprint,
/// cancellation) mirrors `process_audio_file`; the staged original is never
/// modified and segment bytes exist only in the attempt work area.
fn process_cue_governed_source(
    file: &ObservedFile,
    source: &Path,
    plan: &CueGovernance,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<CueSourceResult, NormalizationError> {
    let relative = &file.relative_path;
    let (source_hash_before, source_bytes) = hash_source(source)?;
    if file
        .content_hash
        .as_deref()
        .is_none_or(|expected| expected != source_hash_before)
    {
        return Err(NormalizationError::SourceChanged(relative.clone()));
    }

    let source_probe = run_ffprobe_safe(source, options)?;
    if source_probe.audio_stream_count != 1 {
        return Err(NormalizationError::Verification(format!(
            "CUE source {relative} contains {} audio streams; exactly one is required",
            source_probe.audio_stream_count
        )));
    }
    let source_duration = source_probe
        .audio
        .duration_ms
        .ok_or(NormalizationError::MissingField("CUE source duration"))?;
    let source_channels = source_probe
        .audio
        .channels
        .ok_or(NormalizationError::MissingField("CUE source channels"))?;

    let mut outputs = Vec::with_capacity(plan.segments.len());
    for segment in &plan.segments {
        if is_canceled(options) {
            return Err(NormalizationError::Canceled);
        }
        let segment_relative = format!("{}.track-{:02}", plan.cue_path, segment.number);
        let output_path = output_path_for(work_dir, "audio", &segment_relative, "mp3")?;
        ensure_output_absent(&output_path)?;
        run_ffmpeg_segment(
            source,
            &output_path,
            segment.start_ms,
            segment.end_ms,
            segment,
            plan,
            options,
        )?;
        let (byte_hash, output_bytes) = hash_file(&output_path).map_err(|error| {
            NormalizationError::Verification(format!(
                "CUE segment {segment_relative} could not be hashed: {error}"
            ))
        })?;
        if output_bytes == 0 {
            return Err(NormalizationError::Verification(format!(
                "CUE segment {segment_relative} is empty"
            )));
        }
        let probe = run_ffprobe_safe(&output_path, options)?;
        let expected_duration = segment
            .end_ms
            .map_or(source_duration.saturating_sub(segment.start_ms), |end_ms| {
                end_ms.saturating_sub(segment.start_ms)
            });
        verify_segment_audio(
            &segment_relative,
            &probe,
            source_channels,
            expected_duration,
        )?;
        let output_relative =
            path_to_posix(output_path.strip_prefix(work_dir).unwrap_or(&output_path));
        outputs.push(NormalizedOutput {
            source_file: relative.clone(),
            output_path: output_relative,
            byte_hash,
            container: probe
                .audio
                .container
                .clone()
                .ok_or(NormalizationError::MissingField("segment container"))?,
            codec: probe
                .audio
                .codec
                .clone()
                .ok_or(NormalizationError::MissingField("segment codec"))?,
            sample_rate_hz: probe
                .audio
                .sample_rate_hz
                .ok_or(NormalizationError::MissingField("segment sample rate"))?,
            channels: probe
                .audio
                .channels
                .ok_or(NormalizationError::MissingField("segment channels"))?,
            bitrate_bps: probe.audio.bitrate_bps,
            duration_ms: probe
                .audio
                .duration_ms
                .ok_or(NormalizationError::MissingField("segment duration"))?,
            retained: false,
            segment: Some(CueSegmentIdentity {
                cue: plan.cue_path.clone(),
                track: segment.number,
            }),
            findings: Vec::new(),
        });
    }

    // One acoustic fingerprint per source file (ADR-0146): the whole source
    // remains the fingerprinted evidence; segments are not re-fingerprinted.
    let fingerprint = if !is_canceled(options) {
        Some(fingerprint_file(source, relative, options)?)
    } else {
        return Err(NormalizationError::Canceled);
    };

    let (source_hash_after, _) = hash_source(source)?;
    if source_hash_after != source_hash_before {
        return Err(NormalizationError::SourceChanged(relative.clone()));
    }

    Ok(CueSourceResult {
        outputs,
        fingerprint,
        bytes: source_bytes,
    })
}

struct CueSourceResult {
    outputs: Vec<NormalizedOutput>,
    fingerprint: Option<FingerprintObservation>,
    bytes: u64,
}

/// Encode one CUE segment with the canonical MP3 profile. Seek and duration
/// are passed at full millisecond precision (`-ss` before `-i`, frame-accurate
/// when re-encoding); the last track omits `-t` and runs to end of stream.
/// CUE title/performer/album observations are written as output tags —
/// generated segment bytes, never the staged original (ADR-0146).
fn run_ffmpeg_segment(
    input: &Path,
    output: &Path,
    start_ms: u64,
    end_ms: Option<u64>,
    segment: &CueSegmentPlan,
    plan: &CueGovernance,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let timestamp = |ms: u64| format!("{}.{:03}", ms / 1000, ms % 1000);
    let mut args: Vec<String> = vec![
        // The attempt directory is private and the destination must not be
        // replaced. `-n` makes ffmpeg fail closed if a destination appears
        // between planning and the tool opening it.
        "-n".to_owned(),
        "-v".to_owned(),
        "error".to_owned(),
        "-xerror".to_owned(),
        "-ss".to_owned(),
        timestamp(start_ms),
        "-i".to_owned(),
        input.to_string_lossy().to_string(),
    ];
    if let Some(end_ms) = end_ms {
        args.push("-t".to_owned());
        args.push(timestamp(end_ms.saturating_sub(start_ms)));
    }
    args.extend([
        "-vn".to_owned(),
        "-map".to_owned(),
        "0:a:0".to_owned(),
        "-c:a".to_owned(),
        "libmp3lame".to_owned(),
        "-q:a".to_owned(),
        "4".to_owned(),
        "-b:a".to_owned(),
        "384k".to_owned(),
        "-ar".to_owned(),
        "48000".to_owned(),
        "-map_metadata".to_owned(),
        "0".to_owned(),
    ]);
    if let Some(title) = &segment.title {
        args.push("-metadata".to_owned());
        args.push(format!("title={title}"));
    }
    if let Some(performer) = &segment.performer {
        args.push("-metadata".to_owned());
        args.push(format!("artist={performer}"));
    }
    if let Some(album) = &plan.album_title {
        args.push("-metadata".to_owned());
        args.push(format!("album={album}"));
    }
    if let Some(performer) = &plan.album_performer {
        args.push("-metadata".to_owned());
        args.push(format!("album_artist={performer}"));
    }
    args.push("-metadata".to_owned());
    args.push(format!("track={}", segment.number));
    args.push(output.to_string_lossy().to_string());

    let (exit_code, stderr) = run_ffmpeg_process(
        &options.ffmpeg_path,
        &args,
        options.ffmpeg_timeout,
        options.max_tool_output_bytes,
        &options.cancellation,
        options.shutdown.as_deref(),
    )?;
    if exit_code != 0 {
        return Err(NormalizationError::Ffmpeg(format!(
            "ffmpeg CUE segment encoding failed (exit {exit_code}): {stderr}"
        )));
    }
    Ok(())
}

/// Verify one encoded CUE segment against the canonical profile and its
/// declared boundary span.
fn verify_segment_audio(
    relative: &str,
    output: &ProbeResult,
    source_channels: u32,
    expected_duration_ms: u64,
) -> Result<(), NormalizationError> {
    if output.audio_stream_count != 1 {
        return Err(NormalizationError::Verification(format!(
            "CUE segment {relative} contains {} audio streams",
            output.audio_stream_count
        )));
    }
    if output.audio.codec.as_deref() != Some("mp3") {
        return Err(NormalizationError::Verification(format!(
            "CUE segment {relative} is not MP3"
        )));
    }
    let container_is_mp3 = output
        .audio
        .container
        .as_deref()
        .is_some_and(|container| container.split(',').any(|name| name == "mp3"));
    if !container_is_mp3 {
        return Err(NormalizationError::Verification(format!(
            "CUE segment {relative} is not in an MP3 container"
        )));
    }
    let output_rate = output
        .audio
        .sample_rate_hz
        .ok_or(NormalizationError::MissingField("segment sample rate"))?;
    if output_rate != CANONICAL_SAMPLE_RATE_HZ {
        return Err(NormalizationError::Verification(format!(
            "CUE segment {relative} has sample rate {output_rate}, expected {CANONICAL_SAMPLE_RATE_HZ}"
        )));
    }
    let output_channels = output
        .audio
        .channels
        .ok_or(NormalizationError::MissingField("segment channels"))?;
    let expected_channels = source_channels.min(2);
    if output_channels != expected_channels {
        return Err(NormalizationError::Verification(format!(
            "CUE segment {relative} has {output_channels} channels, expected {expected_channels}"
        )));
    }
    let output_duration = output
        .audio
        .duration_ms
        .ok_or(NormalizationError::MissingField("segment duration"))?;
    let tolerance = OUTPUT_DURATION_TOLERANCE_MS.max(expected_duration_ms / 50);
    if expected_duration_ms.abs_diff(output_duration) > tolerance {
        return Err(NormalizationError::Verification(format!(
            "CUE segment {relative} duration differs by {} ms (allowed {tolerance} ms)",
            expected_duration_ms.abs_diff(output_duration)
        )));
    }
    Ok(())
}

fn process_audio_file(
    file: &ObservedFile,
    source: &Path,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<AudioProcessResult, NormalizationError> {
    let relative = &file.relative_path;
    let (source_hash_before, source_bytes) = hash_source(source)?;
    if file
        .content_hash
        .as_deref()
        .is_none_or(|expected| expected != source_hash_before)
    {
        return Err(NormalizationError::SourceChanged(relative.clone()));
    }

    let source_probe = run_ffprobe_safe(source, options)?;
    if source_probe.audio_stream_count != 1 {
        return Err(NormalizationError::Verification(format!(
            "source {relative} contains {} audio streams; exactly one is required",
            source_probe.audio_stream_count
        )));
    }

    let output_path = output_path_for(work_dir, "audio", relative, "mp3")?;
    ensure_within(&output_path, work_dir)?;
    ensure_output_absent(&output_path)?;

    let retained = is_retainable_mp3(&source_probe);

    if retained {
        verify_audio_decodes(source, options)?;
        secure_fs::copy_new(source, &output_path).map_err(|error| {
            NormalizationError::Ffmpeg(format!(
                "failed to copy canonical source {relative}: {error}"
            ))
        })?;
    } else {
        run_ffmpeg_encode(source, &output_path, options)?;
    }

    let (byte_hash, output_bytes) = hash_file(&output_path).map_err(|error| {
        NormalizationError::Verification(format!(
            "normalized output for {relative} could not be hashed: {error}"
        ))
    })?;
    if output_bytes == 0 {
        return Err(NormalizationError::Verification(format!(
            "normalized output for {relative} is empty"
        )));
    }

    let probe = run_ffprobe_safe(&output_path, options)?;
    verify_normalized_audio(relative, &source_probe, &probe, retained)?;

    let output_relative = path_to_posix(output_path.strip_prefix(work_dir).unwrap_or(&output_path));

    let output = NormalizedOutput {
        source_file: relative.clone(),
        output_path: output_relative,
        byte_hash,
        container: probe
            .audio
            .container
            .clone()
            .ok_or(NormalizationError::MissingField("output container"))?,
        codec: probe
            .audio
            .codec
            .clone()
            .ok_or(NormalizationError::MissingField("output codec"))?,
        sample_rate_hz: probe
            .audio
            .sample_rate_hz
            .ok_or(NormalizationError::MissingField("output sample rate"))?,
        channels: probe
            .audio
            .channels
            .ok_or(NormalizationError::MissingField("output channels"))?,
        bitrate_bps: probe.audio.bitrate_bps,
        duration_ms: probe
            .audio
            .duration_ms
            .ok_or(NormalizationError::MissingField("output duration"))?,
        retained,
        segment: None,
        findings: Vec::new(),
    };

    let fingerprint = if !is_canceled(options) {
        Some(fingerprint_file(source, relative, options)?)
    } else {
        return Err(NormalizationError::Canceled);
    };

    let (source_hash_after, _) = hash_source(source)?;
    if source_hash_after != source_hash_before {
        return Err(NormalizationError::SourceChanged(relative.clone()));
    }

    Ok(AudioProcessResult {
        output,
        fingerprint,
        bytes: source_bytes,
    })
}

fn hash_observed_buffer(bytes: &[u8]) -> blake3::Hash {
    let mut operation = crate::operation_metrics::OperationGuard::start(
        crate::operation_metrics::Operation::MediaBufferHash,
    );
    let hash = blake3::hash(bytes);
    operation.add_read(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    operation.finish();
    hash
}

fn hash_source(source: &Path) -> Result<(String, u64), NormalizationError> {
    hash_file(source).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NormalizationError::SourceMissing(source.to_path_buf())
        } else {
            NormalizationError::SourceUnreadable {
                path: source.to_path_buf(),
                message: error.to_string(),
            }
        }
    })
}

fn verify_observed_source(file: &ObservedFile, source: &Path) -> Result<(), NormalizationError> {
    let (actual, _) = hash_source(source)?;
    if file.content_hash.as_deref() != Some(actual.as_str()) {
        return Err(NormalizationError::SourceChanged(
            file.relative_path.clone(),
        ));
    }
    Ok(())
}

fn process_external_artwork(
    file: &ObservedFile,
    source: &Path,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<Option<NormalizedArtwork>, NormalizationError> {
    let output_path = output_path_for(work_dir, "artwork/external", &file.relative_path, "jpg")?;
    ensure_output_absent(&output_path)?;

    run_ffmpeg_image_normalize(source, &output_path, None, options)?;
    let verified = verify_normalized_artwork(&output_path, work_dir, options)?;

    Ok(Some(NormalizedArtwork {
        source: "external".to_owned(),
        source_file: file.relative_path.clone(),
        output_path: verified.relative_path,
        mime_type: "image/jpeg".to_owned(),
        width: verified.width,
        height: verified.height,
        byte_hash: verified.byte_hash,
    }))
}

fn process_embedded_artwork(
    embedded: &ObservedArtwork,
    source_relative: &str,
    picture_index: usize,
    source: &Path,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<Option<NormalizedArtwork>, NormalizationError> {
    if embedded.size_bytes > options.max_artwork_bytes as u64 {
        return Err(NormalizationError::Verification(format!(
            "embedded artwork {picture_index} in {source_relative} exceeds the {} byte limit",
            options.max_artwork_bytes
        )));
    }
    let (Some(width), Some(height)) = (embedded.width, embedded.height) else {
        return Err(NormalizationError::Verification(format!(
            "embedded artwork {picture_index} in {source_relative} has no valid dimensions"
        )));
    };
    if width == 0
        || height == 0
        || width > MAX_ARTWORK_SOURCE_DIMENSION
        || height > MAX_ARTWORK_SOURCE_DIMENSION
        || u64::from(width).saturating_mul(u64::from(height)) > MAX_ARTWORK_SOURCE_PIXELS
    {
        return Err(NormalizationError::Verification(format!(
            "embedded artwork {picture_index} in {source_relative} has unsafe dimensions {width}x{height}"
        )));
    }
    let stable_name = embedded.content_hash.clone().unwrap_or_else(|| {
        blake3::hash(format!("{source_relative}#{picture_index}").as_bytes())
            .to_hex()
            .to_string()
    });
    let logical_source = format!("{source_relative}#embedded-{picture_index}");
    let output_path = output_path_for(
        work_dir,
        "artwork/embedded",
        &format!("{stable_name}.embedded"),
        "jpg",
    )?;
    ensure_output_absent(&output_path)?;

    run_ffmpeg_image_normalize(source, &output_path, Some(picture_index), options)?;
    let verified = verify_normalized_artwork(&output_path, work_dir, options)?;

    Ok(Some(NormalizedArtwork {
        source: "embedded".to_owned(),
        source_file: logical_source,
        output_path: verified.relative_path,
        mime_type: "image/jpeg".to_owned(),
        width: verified.width,
        height: verified.height,
        byte_hash: verified.byte_hash,
    }))
}

struct VerifiedArtwork {
    relative_path: String,
    width: u32,
    height: u32,
    byte_hash: String,
}

fn preflight_artwork(
    observed: &ObservedFile,
    source: &Path,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let file =
        secure_fs::open_read(source).map_err(|error| NormalizationError::SourceUnreadable {
            path: source.to_path_buf(),
            message: error.to_string(),
        })?;
    let size = file
        .metadata()
        .map_err(|error| NormalizationError::SourceUnreadable {
            path: source.to_path_buf(),
            message: error.to_string(),
        })?
        .len();
    if size == 0 || size > options.max_artwork_bytes as u64 {
        // Even rejected artwork must be checked against the inspection
        // evidence before its failure is treated as a nonfatal finding.
        verify_observed_source(observed, source)?;
        return Err(NormalizationError::Verification(format!(
            "artwork byte size {} is outside 1..={} bytes",
            size, options.max_artwork_bytes
        )));
    }
    let limit = options.max_artwork_bytes.saturating_add(1);
    let bytes = read_limited(file.take(u64::try_from(limit).unwrap_or(u64::MAX)), limit).map_err(
        |error| NormalizationError::SourceUnreadable {
            path: source.to_path_buf(),
            message: error.to_string(),
        },
    )?;
    if bytes.len() > options.max_artwork_bytes {
        verify_observed_source(observed, source)?;
        return Err(NormalizationError::Verification(format!(
            "artwork grew beyond the {} byte limit",
            options.max_artwork_bytes
        )));
    }
    let actual = hash_observed_buffer(&bytes).to_hex().to_string();
    if observed.content_hash.as_deref() != Some(actual.as_str()) {
        return Err(NormalizationError::SourceChanged(
            observed.relative_path.clone(),
        ));
    }
    let (width, height) = image_dimensions(&bytes).ok_or_else(|| {
        NormalizationError::Verification("unsupported or malformed artwork header".to_owned())
    })?;
    if width == 0
        || height == 0
        || width > MAX_ARTWORK_SOURCE_DIMENSION
        || height > MAX_ARTWORK_SOURCE_DIMENSION
        || u64::from(width).saturating_mul(u64::from(height)) > MAX_ARTWORK_SOURCE_PIXELS
    {
        return Err(NormalizationError::Verification(format!(
            "artwork dimensions {width}x{height} exceed resource limits"
        )));
    }
    Ok(())
}

fn verify_normalized_artwork(
    output: &Path,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<VerifiedArtwork, NormalizationError> {
    let file = secure_fs::open_read(output).map_err(|error| {
        NormalizationError::Verification(format!("normalized artwork could not be opened: {error}"))
    })?;
    // Hash the exact bytes inspected below, rather than opening the output a
    // second time: an output may change between two independent reads.
    let limit = options.max_artwork_bytes.saturating_add(1);
    let bytes = read_limited(file.take(u64::try_from(limit).unwrap_or(u64::MAX)), limit).map_err(
        |error| {
            NormalizationError::Verification(format!(
                "normalized artwork could not be read: {error}"
            ))
        },
    )?;
    if bytes.is_empty() || bytes.len() > options.max_artwork_bytes {
        return Err(NormalizationError::Verification(format!(
            "normalized artwork byte size {} is outside 1..={} bytes",
            bytes.len(),
            options.max_artwork_bytes
        )));
    }
    let byte_hash = hash_observed_buffer(&bytes).to_hex().to_string();
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return Err(NormalizationError::Verification(
            "normalized artwork is not JPEG".to_owned(),
        ));
    }
    let (width, height) = image_dimensions(&bytes).ok_or_else(|| {
        NormalizationError::Verification("normalized artwork has no valid dimensions".to_owned())
    })?;
    if width != CANONICAL_ARTWORK_MAX_DIMENSION || height != CANONICAL_ARTWORK_MAX_DIMENSION {
        return Err(NormalizationError::Verification(format!(
            "normalized artwork is {width}x{height}, expected {0}x{0}",
            CANONICAL_ARTWORK_MAX_DIMENSION
        )));
    }
    let relative_path = output
        .strip_prefix(work_dir)
        .map(path_to_posix)
        .map_err(|_| NormalizationError::PathEscape(output.display().to_string()))?;
    Ok(VerifiedArtwork {
        relative_path,
        width,
        height,
        byte_hash,
    })
}

fn artwork_failure(file: &ObservedFile, error: &NormalizationError) -> Finding {
    Finding {
        code: "artwork.normalization_failed".to_owned(),
        message: truncate_message(&error.to_string()),
        severity: FindingSeverity::Warning,
        file: Some(file.relative_path.clone()),
        evidence: serde_json::json!({ "error": error.to_string() }),
    }
}

fn fingerprint_file(
    source: &Path,
    relative_path: &str,
    options: &NormalizationOptions,
) -> Result<FingerprintObservation, NormalizationError> {
    let output = run_fpcalc(source, options)?;

    let parsed: FpcalcResult = serde_json::from_str(&output).map_err(|error| {
        NormalizationError::Fpcalc(format!("invalid fpcalc JSON output: {error}"))
    })?;

    let duration = parsed
        .duration
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| NormalizationError::Fpcalc("fpcalc produced invalid duration".to_owned()))?;
    let duration_ms = (duration * 1000.0).round();
    if !(1.0..=(u64::MAX as f64)).contains(&duration_ms) {
        return Err(NormalizationError::Fpcalc(
            "fpcalc duration is outside the supported range".to_owned(),
        ));
    }

    let fingerprint = parsed.fingerprint.trim();
    if fingerprint.is_empty() {
        return Err(NormalizationError::Fpcalc(
            "fpcalc produced empty fingerprint".to_owned(),
        ));
    }
    Ok(FingerprintObservation {
        source_file: relative_path.to_owned(),
        fingerprint: fingerprint.to_owned(),
        duration_ms: duration_ms as u64,
        findings: Vec::new(),
    })
}

/// Legacy-compatible retention policy: an existing MP3 is not re-encoded
/// merely to force the conversion profile's sample rate or VBR mode. It must
/// still have exactly one audio stream and pass a complete decode check before
/// its bytes are copied. Attached artwork and other non-audio streams remain
/// intact because retention is byte-for-byte.
fn is_retainable_mp3(probe: &ProbeResult) -> bool {
    probe.audio.codec.as_deref() == Some("mp3")
        && probe
            .audio
            .container
            .as_deref()
            .is_some_and(|container| container.split(',').any(|name| name == "mp3"))
        && probe.audio_stream_count == 1
}

fn verify_normalized_audio(
    relative: &str,
    source: &ProbeResult,
    output: &ProbeResult,
    retained: bool,
) -> Result<(), NormalizationError> {
    if output.audio_stream_count != 1 {
        return Err(NormalizationError::Verification(format!(
            "normalized output for {relative} contains {} audio streams",
            output.audio_stream_count
        )));
    }
    if output.audio.codec.as_deref() != Some("mp3") {
        return Err(NormalizationError::Verification(format!(
            "normalized output for {relative} is not MP3"
        )));
    }
    let container_is_mp3 = output
        .audio
        .container
        .as_deref()
        .is_some_and(|container| container.split(',').any(|name| name == "mp3"));
    if !container_is_mp3 {
        return Err(NormalizationError::Verification(format!(
            "normalized output for {relative} is not in an MP3 container"
        )));
    }

    let output_rate = output
        .audio
        .sample_rate_hz
        .ok_or(NormalizationError::MissingField("output sample rate"))?;
    if retained {
        if source.audio.sample_rate_hz != Some(output_rate) {
            return Err(NormalizationError::Verification(format!(
                "retained MP3 sample rate changed for {relative}"
            )));
        }
    } else if output_rate != CANONICAL_SAMPLE_RATE_HZ {
        return Err(NormalizationError::Verification(format!(
            "encoded output for {relative} has sample rate {output_rate}, expected {CANONICAL_SAMPLE_RATE_HZ}"
        )));
    }

    let source_channels = source
        .audio
        .channels
        .ok_or(NormalizationError::MissingField("source channels"))?;
    let output_channels = output
        .audio
        .channels
        .ok_or(NormalizationError::MissingField("output channels"))?;
    let expected_channels = source_channels.min(2);
    if output_channels != expected_channels {
        return Err(NormalizationError::Verification(format!(
            "normalized output for {relative} has {output_channels} channels, expected {expected_channels}"
        )));
    }

    let source_duration = source
        .audio
        .duration_ms
        .ok_or(NormalizationError::MissingField("source duration"))?;
    let output_duration = output
        .audio
        .duration_ms
        .ok_or(NormalizationError::MissingField("output duration"))?;
    let tolerance = OUTPUT_DURATION_TOLERANCE_MS.max(source_duration / 50);
    if source_duration.abs_diff(output_duration) > tolerance {
        return Err(NormalizationError::Verification(format!(
            "normalized output duration for {relative} differs by {} ms (allowed {tolerance} ms)",
            source_duration.abs_diff(output_duration)
        )));
    }
    Ok(())
}

fn run_ffmpeg_encode(
    input: &Path,
    output: &Path,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let mut invocation = crate::ToolInvocation::start("ffmpeg", input);
    let args: Vec<String> = vec![
        "-n".to_string(),
        "-v".to_string(),
        "error".to_string(),
        "-xerror".to_string(),
        "-i".to_string(),
        input.to_string_lossy().to_string(),
        "-vn".to_string(),
        "-map".to_string(),
        "0:a:0".to_string(),
        "-c:a".to_string(),
        "libmp3lame".to_string(),
        "-q:a".to_string(),
        "4".to_string(),
        "-b:a".to_string(),
        "384k".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-map_metadata".to_string(),
        "0".to_string(),
        output.to_string_lossy().to_string(),
    ];

    let (exit_code, stderr) = run_ffmpeg_process(
        &options.ffmpeg_path,
        &args,
        options.ffmpeg_timeout,
        options.max_tool_output_bytes,
        &options.cancellation,
        options.shutdown.as_deref(),
    )?;

    if exit_code != 0 {
        invocation.set_outcome("exit_failure");
        return Err(NormalizationError::Ffmpeg(format!(
            "ffmpeg encoding failed (exit {exit_code}): {stderr}"
        )));
    }
    invocation.set_outcome("ok");
    Ok(())
}

fn run_ffmpeg_process(
    ffmpeg_path: &Path,
    args: &[String],
    timeout: StdDuration,
    max_output: usize,
    cancellation: &AtomicBool,
    shutdown: Option<&AtomicBool>,
) -> Result<(i32, String), NormalizationError> {
    let mut command = Command::new(ffmpeg_path);
    command.args(args);
    let output = crate::run_supervised_tool(
        command,
        timeout,
        Some(cancellation),
        shutdown,
        crate::ToolStdout::None,
        Some(max_output),
    )
    .map_err(|error| match error {
        crate::SupervisedToolError::Canceled => NormalizationError::Canceled,
        other => NormalizationError::Ffmpeg(format!("ffmpeg {other}")),
    })?;
    Ok((
        output.status.code().unwrap_or(-1),
        crate::truncate_bytes(&output.stderr),
    ))
}

fn run_fpcalc(input: &Path, options: &NormalizationOptions) -> Result<String, NormalizationError> {
    let mut invocation = crate::ToolInvocation::start("fpcalc", input);
    let mut command = Command::new(&options.fpcalc_path);
    command
        .arg("-json")
        .arg("-length")
        .arg(FINGERPRINT_LENGTH_SECONDS.to_string())
        .arg("--")
        .arg(input);
    let output = crate::run_supervised_tool(
        command,
        options.fpcalc_timeout,
        Some(&options.cancellation),
        options.shutdown.as_deref(),
        crate::ToolStdout::Capture(options.max_tool_output_bytes),
        Some(options.max_tool_output_bytes),
    )
    .map_err(|error| match error {
        crate::SupervisedToolError::Canceled => NormalizationError::Canceled,
        other => NormalizationError::Fpcalc(format!("fpcalc {other}")),
    })?;
    let status = output.status;
    // The complete bounded capture is required for long Chromaprint JSON.
    let stdout_output = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr_output = crate::truncate_bytes(&output.stderr);

    if !status.success() {
        invocation.set_outcome("exit_failure");
        return Err(NormalizationError::Fpcalc(format!(
            "fpcalc exited with {}: {}",
            status, stderr_output
        )));
    }

    if stdout_output.trim().is_empty() {
        invocation.set_outcome("empty_output");
        return Err(NormalizationError::Fpcalc(
            "fpcalc produced no output".to_owned(),
        ));
    }

    invocation.set_outcome("ok");
    Ok(stdout_output)
}

pub(crate) fn retry_executable_busy<T>(
    mut spawn: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    const EXECUTABLE_BUSY_RETRIES: u8 = 4;
    let mut retry_count = 0;
    loop {
        match spawn() {
            Err(error)
                if error.raw_os_error() == Some(26) && retry_count < EXECUTABLE_BUSY_RETRIES =>
            {
                // Linux can briefly return ETXTBSY while a just-created executable is still
                // being released by the filesystem. Bound the retry so genuine tool failures
                // remain immediate and actionable.
                retry_count += 1;
                thread::sleep(StdDuration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn verify_audio_decodes(
    input: &Path,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let args = vec![
        "-v".to_owned(),
        "error".to_owned(),
        "-xerror".to_owned(),
        "-i".to_owned(),
        input.to_string_lossy().into_owned(),
        "-map".to_owned(),
        "0:a:0".to_owned(),
        "-f".to_owned(),
        "null".to_owned(),
        "-".to_owned(),
    ];
    let (exit_code, stderr) = run_ffmpeg_process(
        &options.ffmpeg_path,
        &args,
        options.ffmpeg_timeout,
        options.max_tool_output_bytes,
        &options.cancellation,
        options.shutdown.as_deref(),
    )?;
    if exit_code != 0 {
        return Err(NormalizationError::Verification(format!(
            "source MP3 failed complete decode (exit {exit_code}): {stderr}"
        )));
    }
    Ok(())
}

fn run_ffmpeg_image_normalize(
    input: &Path,
    output: &Path,
    embedded_picture_index: Option<usize>,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let scale_arg = format!(
        "scale={dim}:{dim}:force_original_aspect_ratio=decrease,pad={dim}:{dim}:(ow-iw)/2:(oh-ih)/2:color=black",
        dim = CANONICAL_ARTWORK_MAX_DIMENSION
    );
    let args: Vec<String> = vec![
        "-n".to_string(),
        "-v".to_string(),
        "error".to_string(),
        "-xerror".to_string(),
        "-i".to_string(),
        input.to_string_lossy().to_string(),
        "-map".to_string(),
        embedded_picture_index
            .map(|index| format!("0:v:{index}"))
            .unwrap_or_else(|| "0:v:0".to_owned()),
        "-frames:v".to_string(),
        "1".to_string(),
        "-vf".to_string(),
        scale_arg,
        "-c:v".to_string(),
        "mjpeg".to_string(),
        "-q:v".to_string(),
        "2".to_string(),
        "-map_metadata".to_string(),
        "-1".to_string(),
        output.to_string_lossy().to_string(),
    ];

    let (exit_code, stderr) = run_ffmpeg_process(
        &options.ffmpeg_path,
        &args,
        options.ffmpeg_timeout,
        options.max_tool_output_bytes,
        &options.cancellation,
        options.shutdown.as_deref(),
    )?;

    if exit_code != 0 {
        return Err(NormalizationError::Ffmpeg(format!(
            "ffmpeg image normalize failed (exit {exit_code}): {stderr}"
        )));
    }
    Ok(())
}

fn run_ffprobe_safe(
    path: &Path,
    options: &NormalizationOptions,
) -> Result<ProbeResult, NormalizationError> {
    let probe_options = InspectionOptions {
        ffprobe_path: options.ffprobe_path.clone(),
        ffprobe_timeout: options.ffprobe_timeout,
        max_tool_output_bytes: options.max_tool_output_bytes,
        max_artwork_bytes: options.max_artwork_bytes,
    };
    run_ffprobe_cancellable(
        path,
        &probe_options,
        &options.cancellation,
        options.shutdown.as_deref(),
    )
    .map_err(|message| {
        if is_canceled(options) || message == "ffprobe canceled" {
            NormalizationError::Canceled
        } else {
            NormalizationError::Verification(format!(
                "ffprobe verification failed for {}: {message}",
                path.display()
            ))
        }
    })
}

fn create_work_area(
    base: &Path,
    work_item_id: Uuid,
    attempt_id: Uuid,
) -> Result<PathBuf, NormalizationError> {
    secure_fs::create_directory_tree(base).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: base.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    let attempts_dir = base
        .join(work_item_id.simple().to_string())
        .join("attempts");
    secure_fs::create_directory_tree(&attempts_dir).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: attempts_dir.clone(),
            message: error.to_string(),
        }
    })?;
    let work_dir = attempts_dir.join(attempt_id.simple().to_string());
    secure_fs::create_directory(&work_dir).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: work_dir.clone(),
            message: error.to_string(),
        }
    })?;
    secure_fs::create_directory_tree(&work_dir.join("audio")).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: work_dir.join("audio"),
            message: error.to_string(),
        }
    })?;
    secure_fs::create_directory_tree(&work_dir.join("artwork")).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: work_dir.join("artwork"),
            message: error.to_string(),
        }
    })?;
    Ok(work_dir)
}

fn resolve_source_file(source_root: &Path, relative: &str) -> Result<PathBuf, NormalizationError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(NormalizationError::PathEscape(relative.to_owned()));
    }
    let root =
        source_root
            .canonicalize()
            .map_err(|error| NormalizationError::SourceUnreadable {
                path: source_root.to_path_buf(),
                message: error.to_string(),
            })?;
    let candidate = source_root.join(relative_path);
    let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NormalizationError::SourceMissing(candidate.clone())
        } else {
            NormalizationError::SourceUnreadable {
                path: candidate.clone(),
                message: error.to_string(),
            }
        }
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(NormalizationError::PathEscape(format!(
            "source is not a regular non-symlink file: {}",
            candidate.display()
        )));
    }
    let canonical =
        candidate
            .canonicalize()
            .map_err(|error| NormalizationError::SourceUnreadable {
                path: candidate.clone(),
                message: error.to_string(),
            })?;
    canonical
        .strip_prefix(&root)
        .map_err(|_| NormalizationError::PathEscape(relative.to_owned()))?;
    Ok(canonical)
}

fn output_path_for(
    work_dir: &Path,
    category: &str,
    relative: &str,
    extension: &str,
) -> Result<PathBuf, NormalizationError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(NormalizationError::PathEscape(relative.to_owned()));
    }
    let mut output_relative = relative_path.to_path_buf();
    let file_name = relative_path
        .file_name()
        .ok_or_else(|| NormalizationError::PathEscape(relative.to_owned()))?;
    let mut output_name = file_name.to_os_string();
    output_name.push(format!(".{extension}"));
    output_relative.set_file_name(output_name);
    let output = work_dir.join(category).join(output_relative);
    ensure_within(&output, work_dir)?;
    let parent = output.parent().ok_or_else(|| {
        NormalizationError::PathEscape(format!("output has no parent: {}", output.display()))
    })?;
    secure_fs::create_directory_tree(parent).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: parent.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    Ok(output)
}

fn is_canceled(options: &NormalizationOptions) -> bool {
    options.cancellation.load(Ordering::SeqCst)
        || options
            .shutdown
            .as_ref()
            .is_some_and(|signal| signal.load(Ordering::SeqCst))
}

/// Reject an existing output before an external encoder is invoked. This is
/// intentionally a fail-closed preflight; ffmpeg also receives `-n` so a
/// concurrent create cannot turn the output operation into an overwrite.
fn ensure_output_absent(path: &Path) -> Result<(), NormalizationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(NormalizationError::Verification(format!(
            "normalization output already exists: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(NormalizationError::WorkAreaCreation {
            path: path.to_path_buf(),
            message: error.to_string(),
        }),
    }
}

fn ensure_within(path: &Path, base: &Path) -> Result<(), NormalizationError> {
    let resolved = normalize_path(path);
    let base_resolved = normalize_path(base);
    if resolved.strip_prefix(&base_resolved).is_err() {
        return Err(NormalizationError::PathEscape(format!(
            "path escapes work area: {}",
            resolved.display()
        )));
    }
    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut resolved: PathBuf = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !resolved.pop() {
                    resolved = PathBuf::from("/");
                }
            }
            Component::CurDir => {}
            Component::RootDir => {
                resolved = PathBuf::from("/");
            }
            _ => resolved.push(component.as_os_str()),
        }
    }
    resolved
}

fn parse_sidecar_for_file(file: &ObservedFile) -> Option<ObservedFileKind> {
    if file.sidecar_type.is_some() {
        match file.sidecar_type.as_deref() {
            Some("cuesheet") => Some(ObservedFileKind::CueSheet),
            Some("playlist") => Some(ObservedFileKind::Playlist),
            Some("nfo") => Some(ObservedFileKind::Nfo),
            Some("checksum") => Some(ObservedFileKind::Checksum),
            Some("lyrics") => Some(ObservedFileKind::Lyrics),
            Some("sidecar") => Some(ObservedFileKind::Sidecar),
            _ => None,
        }
    } else {
        matches!(
            file.kind,
            ObservedFileKind::CueSheet
                | ObservedFileKind::Playlist
                | ObservedFileKind::Nfo
                | ObservedFileKind::Checksum
                | ObservedFileKind::Lyrics
        )
        .then_some(file.kind.clone())
    }
}

/// Sidecar parse outcome: the domain observation plus the typed CUE model
/// when the sidecar is a CUE sheet (needed for segmentation planning,
/// ADR-0146).
pub(crate) struct ParsedSidecarOutcome {
    record: ParsedSidecar,
    cue_model: Option<CueModel>,
}

fn parse_sidecar_content(
    file: &ObservedFile,
    source: &Path,
    kind: ObservedFileKind,
) -> Result<ParsedSidecarOutcome, NormalizationError> {
    let sidecar_type = match kind {
        ObservedFileKind::CueSheet => "cue",
        ObservedFileKind::Playlist => "m3u",
        ObservedFileKind::Nfo => "nfo",
        ObservedFileKind::Checksum => "sfv",
        ObservedFileKind::Lyrics if file.relative_path.to_ascii_lowercase().ends_with(".txt") => {
            "txt"
        }
        ObservedFileKind::Lyrics => "lrc",
        ObservedFileKind::Sidecar
            if file.relative_path.ends_with(".blackbeard.provenance.json") =>
        {
            "blackbeard"
        }
        _ => "sidecar",
    };

    let file_handle =
        secure_fs::open_read(source).map_err(|error| NormalizationError::SourceUnreadable {
            path: source.to_path_buf(),
            message: error.to_string(),
        })?;
    let size = file_handle
        .metadata()
        .map_err(|error| NormalizationError::SourceUnreadable {
            path: source.to_path_buf(),
            message: error.to_string(),
        })?
        .len();
    if size > MAX_SIDECAR_BYTES {
        return Err(NormalizationError::Verification(format!(
            "sidecar exceeds the {MAX_SIDECAR_BYTES} byte limit"
        )));
    }
    let limit = usize::try_from(MAX_SIDECAR_BYTES).unwrap_or(usize::MAX);
    let bytes = read_limited(file_handle, limit.saturating_add(1)).map_err(|error| {
        NormalizationError::SourceUnreadable {
            path: source.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    if bytes.len() > limit {
        return Err(NormalizationError::Verification(format!(
            "sidecar exceeds the {MAX_SIDECAR_BYTES} byte limit"
        )));
    }
    let actual_hash = hash_observed_buffer(&bytes).to_hex().to_string();
    if file.content_hash.as_deref() != Some(actual_hash.as_str()) {
        return Err(NormalizationError::SourceChanged(
            file.relative_path.clone(),
        ));
    }
    let (content, mut findings) = decode_sidecar(&bytes, &file.relative_path)?;

    let mut cue_model = None;
    let (parsed, parser_findings) = match sidecar_type {
        "cue" => {
            let (parsed, parser_findings, model) = parse_cue(&content);
            cue_model = Some(model);
            (parsed, parser_findings)
        }
        "m3u" => parse_m3u(&content),
        "sfv" => parse_sfv(&content),
        "nfo" => parse_nfo(&content),
        "lrc" => parse_lrc(&content),
        "txt" => parse_plain_lyrics(&content),
        "blackbeard" => parse_blackbeard(&content),
        "sidecar" if file.relative_path.to_ascii_lowercase().ends_with(".txt") => {
            parse_plain_lyrics(&content)
        }
        _ => (serde_json::json!({ "raw": content }), Vec::new()),
    };
    // A structured sidecar can also express NUL through a JSON escape.
    if sidecar_contains_nul(&parsed) {
        return Err(NormalizationError::InvalidSidecarText(
            "embedded NUL character in parsed metadata",
        ));
    }
    findings.extend(parser_findings.into_iter().map(|mut finding| {
        finding.file = Some(file.relative_path.clone());
        finding
    }));

    Ok(ParsedSidecarOutcome {
        record: ParsedSidecar {
            sidecar_type: sidecar_type.to_owned(),
            relative_path: file.relative_path.clone(),
            parsed,
            findings,
        },
        cue_model,
    })
}

fn sidecar_contains_nul(value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match value {
        Value::String(text) => text.contains('\0'),
        Value::Array(items) => items.iter().any(sidecar_contains_nul),
        Value::Object(fields) => fields
            .iter()
            .any(|(key, item)| key.contains('\0') || sidecar_contains_nul(item)),
        _ => false,
    }
}

fn decode_sidecar(
    bytes: &[u8],
    relative: &str,
) -> Result<(String, Vec<Finding>), NormalizationError> {
    let (text, findings) = match bytes {
        [0xff, 0xfe, rest @ ..] => (decode_utf16_sidecar(rest, u16::from_le_bytes)?, Vec::new()),
        [0xfe, 0xff, rest @ ..] => (decode_utf16_sidecar(rest, u16::from_be_bytes)?, Vec::new()),
        _ => decode_legacy_sidecar(
            bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes),
            relative,
        ),
    };
    // Neither file paths nor PostgreSQL text can contain NUL. Preserve the
    // source file and let the caller record a finding for the unparseable text.
    if text.contains('\0') {
        return Err(NormalizationError::InvalidSidecarText(
            "embedded NUL character",
        ));
    }
    Ok((text, findings))
}

fn decode_utf16_sidecar(
    bytes: &[u8],
    read_unit: fn([u8; 2]) -> u16,
) -> Result<String, NormalizationError> {
    let (pairs, trailing) = bytes.as_chunks::<2>();
    if !trailing.is_empty() {
        return Err(NormalizationError::InvalidSidecarText(
            "truncated UTF-16 code unit",
        ));
    }
    char::decode_utf16(pairs.iter().copied().map(read_unit))
        .collect::<Result<_, _>>()
        .map_err(|_| NormalizationError::InvalidSidecarText("invalid UTF-16 surrogate pair"))
}

fn decode_legacy_sidecar(bytes: &[u8], relative: &str) -> (String, Vec<Finding>) {
    match String::from_utf8(bytes.to_vec()) {
        Ok(content) => (content, Vec::new()),
        Err(_) => (
            bytes.iter().map(|byte| windows_1252_char(*byte)).collect(),
            vec![Finding {
                code: "sidecar.encoding_fallback".to_owned(),
                message: "sidecar was decoded as Windows-1252 after UTF-8 decoding failed"
                    .to_owned(),
                severity: FindingSeverity::Warning,
                file: Some(relative.to_owned()),
                evidence: serde_json::json!({ "encoding": "windows-1252" }),
            }],
        ),
    }
}

fn windows_1252_char(byte: u8) -> char {
    match byte {
        0x80 => '\u{20ac}',
        0x82 => '\u{201a}',
        0x83 => '\u{0192}',
        0x84 => '\u{201e}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02c6}',
        0x89 => '\u{2030}',
        0x8a => '\u{0160}',
        0x8b => '\u{2039}',
        0x8c => '\u{0152}',
        0x8e => '\u{017d}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201c}',
        0x94 => '\u{201d}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02dc}',
        0x99 => '\u{2122}',
        0x9a => '\u{0161}',
        0x9b => '\u{203a}',
        0x9c => '\u{0153}',
        0x9e => '\u{017e}',
        0x9f => '\u{0178}',
        _ => char::from(byte),
    }
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 {
        let first = s.chars().next();
        let last = s.chars().last();
        if let (Some(first), Some(last)) = (first, last)
            && (first == '"' || first == '\'')
            && first == last
        {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Typed CUE sheet model used for segmentation planning (ADR-0146). The
/// observation JSON emitted alongside it retains raw timestamp strings; the
/// model carries parsed millisecond offsets at full 75-fps frame precision.
#[derive(Debug, Default)]
pub(crate) struct CueModel {
    album_title: Option<String>,
    album_performer: Option<String>,
    /// Every FILE reference in authored order; exactly one is required for
    /// the sheet to govern (legacy parity: multi-FILE sheets are rejected).
    files: Vec<String>,
    tracks: Vec<CueTrackModel>,
}

#[derive(Debug, Default)]
pub(crate) struct CueTrackModel {
    number: u32,
    title: Option<String>,
    performer: Option<String>,
    /// First INDEX offset (any index number — an INDEX 00 pregap start is
    /// the split point when present, matching the legacy first-index rule).
    first_index_ms: Option<u64>,
    /// PREGAP/POSTGAP are observed but never applied to boundaries
    /// (ADR-0146, legacy parity).
    pregap_ms: Option<u64>,
    postgap_ms: Option<u64>,
}

/// Parse a CUE `MM:SS:FF` timestamp to milliseconds (75 frames per second).
fn parse_cue_timestamp_ms(value: &str) -> Option<u64> {
    let (minutes, rest) = value.split_once(':')?;
    let (seconds, frames) = rest.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    let seconds: u64 = seconds.trim().parse().ok()?;
    let frames: u64 = frames.trim().parse().ok()?;
    if seconds >= 60 || frames >= 75 {
        return None;
    }
    let total_frames = (minutes * 60 + seconds)
        .checked_mul(75)?
        .checked_add(frames)?;
    Some(total_frames * 1000 / 75)
}

fn parse_cue(content: &str) -> (serde_json::Value, Vec<Finding>, CueModel) {
    let mut tracks = Vec::new();
    let mut current: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut current_model: Option<CueTrackModel> = None;
    let mut current_file: Option<String> = None;
    let mut album = serde_json::Map::new();
    let mut model = CueModel::default();
    let mut findings = Vec::new();

    fn flush_track(
        tracks: &mut Vec<serde_json::Map<String, serde_json::Value>>,
        model_tracks: &mut Vec<CueTrackModel>,
        current: Option<serde_json::Map<String, serde_json::Value>>,
        current_model: Option<CueTrackModel>,
    ) {
        if let Some(prev) = current {
            tracks.push(prev);
        }
        if let Some(prev) = current_model {
            model_tracks.push(prev);
        }
    }

    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("REM ") {
            continue;
        }
        let (command, value) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(command, value)| (command, value.trim()));
        match command.to_ascii_uppercase().as_str() {
            "FILE" => match parse_cue_file(value) {
                Some(file_name) => {
                    model.files.push(file_name.clone());
                    current_file = Some(file_name);
                }
                None => findings.push(parser_finding(
                    "sidecar.cue.invalid_file",
                    format!("invalid CUE FILE declaration on line {}", line_index + 1),
                )),
            },
            "TRACK" => {
                flush_track(
                    &mut tracks,
                    &mut model.tracks,
                    current.take(),
                    current_model.take(),
                );
                let mut fields = value.split_whitespace();
                let number = fields.next().and_then(|part| part.parse::<u32>().ok());
                let track_type = fields.next();
                if number.is_none() || track_type.is_none() {
                    findings.push(parser_finding(
                        "sidecar.cue.invalid_track",
                        format!("invalid CUE TRACK declaration on line {}", line_index + 1),
                    ));
                }
                let mut track = serde_json::Map::new();
                track.insert("number".to_owned(), serde_json::json!(number));
                track.insert("type".to_owned(), serde_json::json!(track_type));
                track.insert("file".to_owned(), serde_json::json!(current_file));
                current = Some(track);
                current_model = Some(CueTrackModel {
                    number: number.unwrap_or(0),
                    ..CueTrackModel::default()
                });
            }
            "TITLE" | "PERFORMER" => {
                let key = command.to_ascii_lowercase();
                let parsed_text = strip_quotes(value);
                let parsed = serde_json::json!(parsed_text);
                if let Some(track) = current.as_mut() {
                    track.insert(key, parsed);
                } else {
                    album.insert(key, parsed);
                }
                let owned = (!parsed_text.is_empty()).then(|| parsed_text.to_owned());
                match (command.to_ascii_uppercase().as_str(), owned) {
                    ("TITLE", Some(text)) => {
                        if let Some(track) = current_model.as_mut() {
                            track.title.get_or_insert(text);
                        } else {
                            model.album_title.get_or_insert(text);
                        }
                    }
                    ("PERFORMER", Some(text)) => {
                        if let Some(track) = current_model.as_mut() {
                            track.performer.get_or_insert(text);
                        } else {
                            model.album_performer.get_or_insert(text);
                        }
                    }
                    _ => {}
                }
            }
            "INDEX" => {
                let mut fields = value.split_whitespace();
                let number = fields.next().and_then(|part| part.parse::<u32>().ok());
                let timestamp = fields.next();
                if let (Some(track), Some(number), Some(timestamp)) =
                    (current.as_mut(), number, timestamp)
                {
                    let indexes = track
                        .entry("indexes".to_owned())
                        .or_insert_with(|| serde_json::json!([]));
                    if let Some(indexes) = indexes.as_array_mut() {
                        indexes.push(serde_json::json!({
                            "number": number,
                            "time": timestamp,
                        }));
                    }
                    match parse_cue_timestamp_ms(timestamp) {
                        Some(offset_ms) => {
                            if let Some(track_model) = current_model.as_mut() {
                                // First INDEX wins (ADR-0146 legacy parity).
                                track_model.first_index_ms.get_or_insert(offset_ms);
                            }
                        }
                        None => findings.push(parser_finding(
                            "sidecar.cue.invalid_timestamp",
                            format!(
                                "malformed CUE INDEX timestamp {timestamp:?} on line {}",
                                line_index + 1
                            ),
                        )),
                    }
                } else {
                    findings.push(parser_finding(
                        "sidecar.cue.invalid_index",
                        format!("invalid CUE INDEX declaration on line {}", line_index + 1),
                    ));
                }
            }
            "PREGAP" | "POSTGAP" => {
                let offset = parse_cue_timestamp_ms(value);
                if let Some(track_model) = current_model.as_mut() {
                    if command.eq_ignore_ascii_case("PREGAP") {
                        track_model.pregap_ms = offset;
                    } else {
                        track_model.postgap_ms = offset;
                    }
                }
            }
            _ => {}
        }
    }
    flush_track(&mut tracks, &mut model.tracks, current, current_model);
    if tracks.is_empty() {
        findings.push(parser_finding(
            "sidecar.cue.no_tracks",
            "CUE sheet contains no valid TRACK declarations".to_owned(),
        ));
    }
    (
        serde_json::json!({ "album": album, "tracks": tracks }),
        findings,
        model,
    )
}

fn parse_cue_file(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_owned());
    }
    value
        .rsplit_once(char::is_whitespace)
        .map(|(path, _kind)| path.trim().to_owned())
        .filter(|path| !path.is_empty())
}

fn parse_m3u(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let mut entries = Vec::new();
    let mut pending_extinf: Option<(Option<i64>, String)> = None;
    let mut findings = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("#EXTINF:") {
            if pending_extinf.is_some() {
                findings.push(parser_finding(
                    "sidecar.m3u.orphan_extinf",
                    format!("EXTINF on line {} replaced a prior EXTINF", line_index + 1),
                ));
            }
            let (duration, title) = value.split_once(',').unwrap_or((value, ""));
            pending_extinf = Some((duration.trim().parse::<i64>().ok(), title.trim().to_owned()));
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let (duration_seconds, title) = pending_extinf.take().unwrap_or((None, String::new()));
        entries.push(serde_json::json!({
            "path": trimmed,
            "title": title,
            "duration_seconds": duration_seconds,
        }));
    }
    if pending_extinf.is_some() {
        findings.push(parser_finding(
            "sidecar.m3u.orphan_extinf",
            "final EXTINF has no following media path".to_owned(),
        ));
    }
    (serde_json::json!({ "entries": entries }), findings)
}

fn parse_sfv(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let mut entries = Vec::new();
    let mut findings = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(";") || trimmed.starts_with("SFV") {
            continue;
        }
        if let Some((file, crc32)) = trimmed.rsplit_once(char::is_whitespace)
            && !file.trim().is_empty()
            && crc32.len() == 8
            && crc32.chars().all(|character| character.is_ascii_hexdigit())
        {
            entries.push(serde_json::json!({
                "file": file.trim(),
                "crc32": crc32.to_ascii_uppercase(),
            }));
        } else {
            findings.push(parser_finding(
                "sidecar.sfv.invalid_entry",
                format!("invalid SFV entry on line {}", line_index + 1),
            ));
        }
    }
    (serde_json::json!({ "entries": entries }), findings)
}

fn parse_nfo(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let text = content.trim();
    if text.starts_with("<?xml") || text.starts_with("<") {
        parse_nfo_xml(text)
    } else {
        parse_nfo_text(text)
    }
}

fn parse_nfo_text(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let mut fields = serde_json::Map::new();
    let mut tracks = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            let value = value.trim();
            if !value.is_empty() && is_nfo_field(&key) {
                fields.insert(nfo_field_name(&key).to_owned(), serde_json::json!(value));
            }
        }
        if let Some(track) = parse_nfo_track_line(trimmed) {
            tracks.push(track);
        }
    }
    (
        serde_json::json!({
            "type": "text",
            "content": content.trim(),
            "fields": fields,
            "tracks": tracks,
        }),
        Vec::new(),
    )
}

fn parse_nfo_xml(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);
    let mut current = Vec::new();
    let mut fields = serde_json::Map::new();
    let mut findings = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                current.push(element.name().as_ref().to_ascii_lowercase());
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::Text(value)) => {
                let Some(name) = current.last() else { continue };
                if is_nfo_field(name) {
                    let value = value.as_ref();
                    if !value.trim().is_empty() {
                        fields.insert(
                            nfo_field_name(name).to_owned(),
                            serde_json::json!(value.trim()),
                        );
                    }
                }
            }
            Ok(Event::End(_)) => {
                let _ = current.pop();
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                findings.push(parser_finding(
                    "sidecar.nfo.malformed_xml",
                    format!("NFO XML could not be parsed: {error}"),
                ));
                break;
            }
            Ok(_) => {}
        }
    }
    (
        serde_json::json!({
            "type": "xml",
            "raw": content.trim(),
            "well_formed": findings.iter().all(|finding| finding.code != "sidecar.nfo.malformed_xml"),
            "fields": fields,
        }),
        findings,
    )
}

fn is_nfo_field(key: &str) -> bool {
    matches!(
        key,
        "title"
            | "album"
            | "albumtitle"
            | "artist"
            | "albumartist"
            | "publisher"
            | "label"
            | "year"
            | "date"
            | "releasedate"
            | "retaildate"
            | "ripdate"
            | "songtotal"
            | "songs"
            | "tracktotal"
            | "genre"
            | "encoder"
            | "length"
            | "runtime"
    )
}

fn nfo_field_name(key: &str) -> &str {
    match key {
        "albumtitle" => "title",
        "albumartist" => "album_artist",
        "label" => "publisher",
        "releasedate" | "retaildate" | "ripdate" => "date",
        "songs" => "song_total",
        "tracktotal" => "track_total",
        "runtime" => "length",
        value => value,
    }
}

fn parse_nfo_track_line(line: &str) -> Option<serde_json::Value> {
    let mut words = line.split_whitespace();
    let number = words
        .next()?
        .trim_end_matches(['.', '-', ')'])
        .parse::<u32>()
        .ok()?;
    let words = words.collect::<Vec<_>>();
    let duration_index = words.iter().position(|word| {
        let mut pieces = word.split(':');
        pieces
            .next()
            .is_some_and(|part| part.chars().all(|c| c.is_ascii_digit()))
            && pieces
                .next()
                .is_some_and(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_digit()))
            && pieces.next().is_none()
    })?;
    let duration = words[duration_index];
    let title = words[..duration_index].join(" ");
    if title.is_empty() {
        return None;
    }
    Some(serde_json::json!({ "number": number, "title": title, "duration": duration }))
}

fn parse_plain_lyrics(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if let Ok(document) = serde_json::from_str::<serde_json::Value>(content.trim())
        && document.is_object()
        && let Some(value) = document
            .get("Value")
            .or_else(|| document.get("value"))
            .and_then(serde_json::Value::as_str)
    {
        let mut parsed = serde_json::Map::new();
        parsed.insert("format".to_owned(), serde_json::json!("json"));
        parsed.insert(
            "value".to_owned(),
            serde_json::json!(value.replace("\r\n", "\n").replace('\r', "\n").trim()),
        );
        for key in ["Artist", "artist", "Title", "title"] {
            if let Some(field) = document.get(key).and_then(serde_json::Value::as_str)
                && !field.trim().is_empty()
            {
                let normalized_key = key.to_ascii_lowercase();
                parsed.insert(normalized_key, serde_json::json!(field.trim()));
            }
        }
        return (serde_json::Value::Object(parsed), Vec::new());
    }
    (
        serde_json::json!({
            "format": "plain",
            "value": normalized.trim(),
        }),
        Vec::new(),
    )
}

fn parse_blackbeard(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let value = match serde_json::from_str::<serde_json::Value>(content) {
        Ok(value) => value,
        Err(error) => {
            return (
                serde_json::json!({ "type": "blackbeard", "raw": content.trim() }),
                vec![parser_finding(
                    "sidecar.blackbeard.invalid_json",
                    format!("Blackbeard provenance is not valid JSON: {error}"),
                )],
            );
        }
    };
    let mut findings = Vec::new();
    let schema_version = value
        .get("schema_version")
        .or_else(|| value.get("schemaVersion"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    if schema_version != 1 {
        findings.push(parser_finding(
            "sidecar.blackbeard.unsupported_schema",
            "Blackbeard provenance schemaVersion is not the supported version 1".to_owned(),
        ));
    }
    (
        serde_json::json!({ "type": "blackbeard", "schema_version": schema_version, "document": value }),
        findings,
    )
}

fn parse_lrc(content: &str) -> (serde_json::Value, Vec<Finding>) {
    let mut lines = Vec::new();
    let mut metadata = serde_json::Map::new();
    let mut findings = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut rest = trimmed;
        let mut timestamps = Vec::new();
        while let Some(after_open) = rest.strip_prefix('[') {
            let Some(close) = after_open.find(']') else {
                findings.push(parser_finding(
                    "sidecar.lrc.unclosed_tag",
                    format!("unclosed LRC tag on line {}", line_index + 1),
                ));
                break;
            };
            let tag = &after_open[..close];
            rest = &after_open[close + 1..];
            if is_lrc_timestamp(tag) {
                timestamps.push(tag.to_owned());
            } else if let Some((key, value)) = tag.split_once(':') {
                metadata.insert(key.to_owned(), serde_json::json!(value));
            } else {
                findings.push(parser_finding(
                    "sidecar.lrc.invalid_tag",
                    format!("invalid LRC tag on line {}", line_index + 1),
                ));
            }
        }
        if timestamps.is_empty() && rest == trimmed {
            findings.push(parser_finding(
                "sidecar.lrc.untimed_line",
                format!("untimed LRC line {}", line_index + 1),
            ));
        }
        for timestamp in timestamps {
            lines.push(serde_json::json!({
                "time": timestamp,
                "text": rest.trim(),
            }));
        }
    }
    (
        serde_json::json!({ "metadata": metadata, "lines": lines }),
        findings,
    )
}

fn is_lrc_timestamp(value: &str) -> bool {
    let Some((minutes, seconds)) = value.split_once(':') else {
        return false;
    };
    minutes.parse::<u32>().is_ok()
        && seconds
            .parse::<f64>()
            .is_ok_and(|seconds| (0.0..60.0).contains(&seconds))
}

fn parser_finding(code: &str, message: String) -> Finding {
    Finding {
        code: code.to_owned(),
        message,
        severity: FindingSeverity::Warning,
        file: None,
        evidence: serde_json::json!({}),
    }
}

fn extract_tool_path(_version_output: &str, configured_path: &Path) -> Option<String> {
    configured_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn truncate_message(message: &str) -> String {
    if message.len() > 512 {
        format!("{}…", &message[..512])
    } else {
        message.to_owned()
    }
}

#[derive(Debug, Deserialize)]
struct FpcalcResult {
    duration: Option<f64>,
    fingerprint: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
    use super::*;
    use melodee_domain::AudioTechnicalObservation;

    fn probe(container: &str, codec: &str, rate: u32, channels: u32) -> ProbeResult {
        ProbeResult {
            audio: AudioTechnicalObservation {
                container: Some(container.to_owned()),
                codec: Some(codec.to_owned()),
                sample_rate_hz: Some(rate),
                channels: Some(channels),
                channel_layout: None,
                bit_depth: None,
                bitrate_bps: None,
                stream_count: Some(1),
                duration_ms: Some(10_000),
                stream_index: Some(0),
            },
            mime_type: None,
            audio_stream_count: 1,
        }
    }

    fn test_temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "melodee-normalize-test-{}",
            Uuid::now_v7().simple()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_script(directory: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn canonical_mp3_profile_version_matches_domain() {
        assert_eq!(
            CANONICAL_MP3_PROFILE_VERSION,
            "mp3-libmp3lame-vbr-q4-48khz-v1"
        );
    }

    #[test]
    fn retention_accepts_valid_mp3_without_requiring_conversion_profile_rate() {
        assert!(is_retainable_mp3(&probe("mp3", "mp3", 44_100, 2)));
        assert!(is_retainable_mp3(&probe("mp3", "mp3", 48_000, 1)));
    }

    #[test]
    fn retention_rejects_non_mp3_and_multiple_audio_streams() {
        assert!(!is_retainable_mp3(&probe("flac", "flac", 48_000, 2)));
        let mut multiple = probe("mp3", "mp3", 48_000, 2);
        multiple.audio_stream_count = 2;
        assert!(!is_retainable_mp3(&multiple));
    }

    /// missing-duration row: the bundled toolchain cannot be made to
    /// report no duration for an on-disk file deterministically, so the
    /// failure mode is pinned here — a source without an observed duration
    /// must fail verification with a typed, actionable error instead of
    /// publishing unverified output.
    #[test]
    fn missing_source_duration_is_a_typed_verification_failure() {
        let mut source = probe("flac", "flac", 48_000, 2);
        source.audio.duration_ms = None;
        let output = probe("mp3", "mp3", 48_000, 2);
        assert!(matches!(
            verify_normalized_audio("track.flac", &source, &output, false),
            Err(NormalizationError::MissingField("source duration"))
        ));
    }

    #[test]
    fn output_paths_preserve_directories_and_prevent_collisions() {
        let temp = test_temp_dir();
        let first = output_path_for(&temp, "audio", "disc1/01.flac", "mp3").unwrap();
        let second = output_path_for(&temp, "audio", "disc2/01.flac", "mp3").unwrap();
        assert_ne!(first, second);
        assert!(first.ends_with("audio/disc1/01.flac.mp3"));
        assert!(second.ends_with("audio/disc2/01.flac.mp3"));
        assert!(output_path_for(&temp, "audio", "../escape.flac", "mp3").is_err());
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn media_filesystem_operations_fail_closed_on_symlink_components_and_leaves() {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir();
        let outside = test_temp_dir();
        let source = temp.join("source.flac");
        let external = outside.join("external.flac");
        fs::write(&source, b"source bytes").unwrap();
        fs::write(&external, b"external bytes").unwrap();

        let source_link = temp.join("source-link.flac");
        symlink(&external, &source_link).unwrap();
        assert!(secure_fs::open_read(&source_link).is_err());
        assert!(hash_file(&source_link).is_err());

        let fifo = temp.join("source.fifo");
        #[cfg(any(target_os = "macos", target_os = "openbsd"))]
        nix::unistd::mkfifo(
            &fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        #[cfg(not(any(target_os = "macos", target_os = "openbsd")))]
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        // The opener must reject the special file without blocking for a
        // writer. This is the raced-FIFO failure mode guarded by O_NONBLOCK
        // followed by the descriptor type check.
        assert!(secure_fs::open_read(&fifo).is_err());

        let linked_parent = temp.join("linked-parent");
        symlink(&outside, &linked_parent).unwrap();
        assert!(secure_fs::create_directory_tree(&linked_parent.join("nested")).is_err());

        let destination_link = temp.join("destination.mp3");
        symlink(&external, &destination_link).unwrap();
        assert!(secure_fs::copy_new(&source, &destination_link).is_err());
        assert_eq!(fs::read(&external).unwrap(), b"external bytes");

        fs::remove_dir_all(temp).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn output_planning_rejects_symlinked_parent_and_existing_leaf() {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir();
        let outside = test_temp_dir();
        let linked_audio = temp.join("audio");
        symlink(&outside, &linked_audio).unwrap();
        assert!(output_path_for(&temp, "audio", "track.flac", "mp3").is_err());

        let output_parent = temp.join("safe");
        fs::create_dir(&output_parent).unwrap();
        let output = output_parent.join("track.flac.mp3");
        let external = outside.join("target.mp3");
        fs::write(&external, b"keep").unwrap();
        symlink(&external, &output).unwrap();
        assert!(matches!(
            ensure_output_absent(&output),
            Err(NormalizationError::Verification(_))
        ));
        assert_eq!(fs::read(&external).unwrap(), b"keep");

        fs::remove_dir_all(temp).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn parse_m3u_extracts_entries() {
        let content =
            "#EXTM3U\n#EXTINF:180,Artist - Title\ntrack01.mp3\n#EXTINF:200,Other\ntrack02.mp3\n";
        let (result, findings) = parse_m3u(content);
        assert!(findings.is_empty());
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["path"], "track01.mp3");
        assert_eq!(entries[0]["title"], "Artist - Title");
        assert_eq!(entries[0]["duration_seconds"], 180);
        assert_eq!(entries[1]["path"], "track02.mp3");
    }

    #[test]
    fn parse_sfv_preserves_filenames_with_spaces() {
        let content = "SFV next 1.0\n\nfile one.mp3 12345678\nfile2.flac ABCDEF01\n;comment\n";
        let (result, findings) = parse_sfv(content);
        assert!(findings.is_empty());
        let entries = result["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["file"], "file one.mp3");
        assert_eq!(entries[0]["crc32"], "12345678");
        assert_eq!(entries[1]["crc32"], "ABCDEF01");
    }

    #[test]
    fn parse_lrc_extracts_multiple_timestamps() {
        let content = "[ar:Artist]\n[00:12.00][00:15.50]Hello\n";
        let (result, findings) = parse_lrc(content);
        assert!(findings.is_empty());
        let lines = result["lines"].as_array().expect("lines array");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["text"], "Hello");
        assert_eq!(lines[1]["time"], "00:15.50");
        assert_eq!(result["metadata"]["ar"], "Artist");
    }

    #[test]
    fn parse_nfo_text_vs_xml() {
        let (text_result, text_findings) = parse_nfo("some description text");
        assert!(text_findings.is_empty());
        assert_eq!(text_result["type"], "text");
        let (xml_result, xml_findings) = parse_nfo("<?xml version=\"1.0\"?><root/>");
        assert!(xml_findings.is_empty());
        assert_eq!(xml_result["type"], "xml");
    }

    #[test]
    fn parse_nfo_extracts_legacy_text_fields_and_tracks() {
        let (result, findings) = parse_nfo(
            "Title: Example Release\nArtist: Example Artist\nReleaseDate: 2024-01-02\n01. First Song 03:12\n",
        );
        assert!(findings.is_empty());
        assert_eq!(result["fields"]["title"], "Example Release");
        assert_eq!(result["fields"]["artist"], "Example Artist");
        assert_eq!(result["fields"]["date"], "2024-01-02");
        assert_eq!(result["tracks"][0]["number"], 1);
        assert_eq!(result["tracks"][0]["title"], "First Song");
    }

    #[test]
    fn parse_xml_nfo_extracts_supported_fields_and_reports_malformed_input() {
        let (result, findings) =
            parse_nfo("<album><title>Example</title><artist>Ada</artist><year>2024</year></album>");
        assert!(findings.is_empty());
        assert_eq!(result["fields"]["title"], "Example");
        assert_eq!(result["fields"]["artist"], "Ada");
        assert_eq!(result["fields"]["year"], "2024");
        let (_, malformed) = parse_nfo("<album><title>broken</album>");
        assert!(
            malformed
                .iter()
                .any(|finding| finding.code == "sidecar.nfo.malformed_xml")
        );
    }

    #[test]
    fn parse_plain_txt_lyrics_normalizes_line_endings() {
        let (result, findings) = parse_plain_lyrics("line one\r\nline two\r");
        assert!(findings.is_empty());
        assert_eq!(result["format"], "plain");
        assert_eq!(result["value"], "line one\nline two");
    }

    #[test]
    fn parse_txt_lyrics_accepts_the_legacy_json_document() {
        let (result, findings) =
            parse_plain_lyrics(r#"{"Artist":"Ada","Title":"Song","Value":"one\r\ntwo"}"#);
        assert!(findings.is_empty());
        assert_eq!(result["format"], "json");
        assert_eq!(result["artist"], "Ada");
        assert_eq!(result["title"], "Song");
        assert_eq!(result["value"], "one\ntwo");
    }

    #[test]
    fn parse_blackbeard_requires_supported_schema() {
        let (result, findings) =
            parse_blackbeard(r#"{"schema_version":1,"release":{"album_title":"Example"}}"#);
        assert!(findings.is_empty());
        assert_eq!(result["type"], "blackbeard");
        assert_eq!(result["schema_version"], 1);
        let (_, unsupported) = parse_blackbeard(r#"{"schemaVersion":2}"#);
        assert!(
            unsupported
                .iter()
                .any(|finding| finding.code == "sidecar.blackbeard.unsupported_schema")
        );
    }

    #[test]
    fn parse_cue_retains_file_context_and_track_numbers() {
        let content = "PERFORMER \"Artist\"\nTITLE \"Album\"\nFILE \"track1.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Song\"\n    PERFORMER \"Artist\"\n";
        let (result, findings, model) = parse_cue(content);
        assert!(findings.is_empty());
        let tracks = result["tracks"].as_array().expect("tracks array");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["number"], 1);
        assert_eq!(tracks[0]["file"], "track1.wav");
        assert_eq!(tracks[0]["title"], "Song");
        assert_eq!(result["album"]["title"], "Album");
        assert_eq!(model.album_title.as_deref(), Some("Album"));
        assert_eq!(model.album_performer.as_deref(), Some("Artist"));
        assert_eq!(model.files, ["track1.wav".to_owned()]);
        assert_eq!(model.tracks[0].title.as_deref(), Some("Song"));
    }

    #[test]
    fn cue_timestamps_parse_at_full_frame_precision() {
        assert_eq!(parse_cue_timestamp_ms("00:00:00"), Some(0));
        assert_eq!(parse_cue_timestamp_ms("00:02:00"), Some(2_000));
        // 12 frames at 75 fps = 160 ms exactly; legacy dropped this component.
        assert_eq!(parse_cue_timestamp_ms("01:02:12"), Some(62_160));
        assert_eq!(parse_cue_timestamp_ms("00:00:74"), Some(986));
        assert_eq!(parse_cue_timestamp_ms("00:00:75"), None);
        assert_eq!(parse_cue_timestamp_ms("00:60:00"), None);
        assert_eq!(parse_cue_timestamp_ms("not-a-time"), None);
        assert_eq!(parse_cue_timestamp_ms("01:02"), None);
    }

    #[test]
    fn cue_model_first_index_wins_and_pregaps_are_unapplied() {
        let content = "FILE \"album.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"One\"\n    PREGAP 00:01:00\n    INDEX 00 00:01:00\n    INDEX 01 00:02:00\n  TRACK 02 AUDIO\n    POSTGAP 00:00:10\n    INDEX 01 00:04:00\n";
        let (_, findings, model) = parse_cue(content);
        assert!(findings.is_empty());
        // First INDEX (number 00, the pregap start) is the split point.
        assert_eq!(model.tracks[0].first_index_ms, Some(1_000));
        assert_eq!(model.tracks[0].pregap_ms, Some(1_000));
        assert_eq!(model.tracks[1].first_index_ms, Some(4_000));
        // 10 frames at 75 fps = 133 ms.
        assert_eq!(model.tracks[1].postgap_ms, Some(133));
    }

    #[test]
    fn cue_model_collects_every_file_reference() {
        let content = "FILE \"a.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\nFILE \"b.wav\" WAVE\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n";
        let (_, findings, model) = parse_cue(content);
        assert!(findings.is_empty());
        assert_eq!(model.files, ["a.wav".to_owned(), "b.wav".to_owned()]);
    }

    #[test]
    fn cue_governance_rejects_multi_file_traversal_and_missing_sources() {
        let root = test_temp_dir();
        let audio = root.join("album.wav");
        fs::write(&audio, b"not really wav").expect("audio");
        let observed = ObservedFile {
            relative_path: "album.wav".to_owned(),
            kind: ObservedFileKind::Audio,
            size_bytes: 13,
            modified_at: None,
            content_hash: None,
            mime_type: None,
            kind_code: None,
            metadata: None,
            audio: Some(melodee_domain::AudioTechnicalObservation {
                container: Some("wav".to_owned()),
                codec: Some("pcm_s16le".to_owned()),
                sample_rate_hz: Some(44_100),
                channels: Some(2),
                channel_layout: None,
                bit_depth: None,
                bitrate_bps: None,
                stream_count: Some(1),
                duration_ms: Some(6_000),
                stream_index: Some(0),
            }),
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors: false,
        };
        let files = vec![observed];

        let multi =
            "FILE \"a.wav\" WAVE\nFILE \"b.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n";
        let (_, _, model) = parse_cue(multi);
        let mut findings = Vec::new();
        assert!(plan_cue_governance("album.cue", &model, &files, &root, &mut findings).is_none());
        assert!(
            findings
                .iter()
                .any(|f| f.code == "sidecar.cue.multiple_files")
        );

        let traversal = "FILE \"../outside.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n";
        let (_, _, model) = parse_cue(traversal);
        findings.clear();
        assert!(plan_cue_governance("album.cue", &model, &files, &root, &mut findings).is_none());
        assert!(findings.iter().any(|f| f.code == "sidecar.cue.unsafe_file"));

        let missing = "FILE \"absent.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n";
        let (_, _, model) = parse_cue(missing);
        findings.clear();
        assert!(plan_cue_governance("album.cue", &model, &files, &root, &mut findings).is_none());
        assert!(
            findings
                .iter()
                .any(|f| f.code == "sidecar.cue.missing_source")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cue_governance_rejects_unordered_and_out_of_range_boundaries() {
        let root = test_temp_dir();
        fs::write(root.join("album.wav"), b"not really wav").expect("audio");
        let files = vec![ObservedFile {
            relative_path: "album.wav".to_owned(),
            kind: ObservedFileKind::Audio,
            size_bytes: 13,
            modified_at: None,
            content_hash: None,
            mime_type: None,
            kind_code: None,
            metadata: None,
            audio: Some(melodee_domain::AudioTechnicalObservation {
                container: Some("wav".to_owned()),
                codec: Some("pcm_s16le".to_owned()),
                sample_rate_hz: Some(44_100),
                channels: Some(2),
                channel_layout: None,
                bit_depth: None,
                bitrate_bps: None,
                stream_count: Some(1),
                duration_ms: Some(6_000),
                stream_index: Some(0),
            }),
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors: false,
        }];

        let unordered = "FILE \"album.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:02:00\n  TRACK 02 AUDIO\n    INDEX 01 00:01:00\n";
        let (_, _, model) = parse_cue(unordered);
        let mut findings = Vec::new();
        assert!(plan_cue_governance("album.cue", &model, &files, &root, &mut findings).is_none());
        assert!(
            findings
                .iter()
                .any(|f| f.code == "sidecar.cue.invalid_boundaries")
        );

        let out_of_range = "FILE \"album.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:09:00\n";
        let (_, _, model) = parse_cue(out_of_range);
        findings.clear();
        assert!(plan_cue_governance("album.cue", &model, &files, &root, &mut findings).is_none());
        assert!(
            findings
                .iter()
                .any(|f| f.code == "sidecar.cue.invalid_boundaries")
        );

        let valid = "FILE \"album.wav\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 00:02:00\n";
        let (_, _, model) = parse_cue(valid);
        findings.clear();
        let plan = plan_cue_governance("album.cue", &model, &files, &root, &mut findings)
            .expect("valid sheet governs");
        assert_eq!(plan.source_relative, "album.wav");
        assert_eq!(plan.segments.len(), 2);
        assert_eq!(
            (plan.segments[0].start_ms, plan.segments[0].end_ms),
            (0, Some(2_000))
        );
        assert_eq!(
            (plan.segments[1].start_ms, plan.segments[1].end_ms),
            (2_000, None)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_sidecars_produce_findings() {
        assert!(!parse_cue("FILE broken").1.is_empty());
        assert!(!parse_m3u("#EXTINF:12,title").1.is_empty());
        assert!(!parse_sfv("not a valid crc").1.is_empty());
        assert!(!parse_nfo("<root").1.is_empty());
        assert!(!parse_lrc("[broken").1.is_empty());
    }

    #[test]
    fn utf16_sidecars_honor_the_bom_and_preserve_track_names() {
        let text = "#EXTM3U\r\n03 - 音楽 🎵.flac\r\n";
        for big_endian in [false, true] {
            let mut bytes = if big_endian {
                vec![0xfe, 0xff]
            } else {
                vec![0xff, 0xfe]
            };
            for unit in text.encode_utf16() {
                bytes.extend(if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            let (content, findings) = decode_sidecar(&bytes, "disc.m3u").unwrap();
            assert!(findings.is_empty());
            assert_eq!(
                parse_m3u(&content).0["entries"][0]["path"],
                "03 - 音楽 🎵.flac"
            );
        }
    }

    #[test]
    fn invalid_sidecar_text_cannot_silently_change_file_or_track_names() {
        for bytes in [
            &b"foo\0bar"[..],
            &[0xff, 0xfe, 0, 0],
            &[0xfe, 0xff, 0x41],
            &[0xff, 0xfe, 0x00, 0xd8],
        ] {
            assert!(matches!(
                decode_sidecar(bytes, "disc.m3u"),
                Err(NormalizationError::InvalidSidecarText(_))
            ));
        }
        assert!(sidecar_contains_nul(
            &serde_json::json!({"entries":[{"title":"a\0b"}]})
        ));
        assert!(!sidecar_contains_nul(
            &serde_json::json!({"title":r"a\u0000b"})
        ));
    }

    #[test]
    fn windows_1252_sidecar_fallback_is_structured() {
        let (text, findings) = decode_sidecar(b"Beyonc\xe9", "release.nfo").unwrap();
        assert_eq!(text, "Beyonc\u{e9}");
        assert_eq!(findings[0].code, "sidecar.encoding_fallback");
    }

    #[test]
    fn attempt_work_areas_are_physically_isolated() {
        let temp = test_temp_dir();
        let work_item = Uuid::now_v7();
        let first = create_work_area(&temp, work_item, Uuid::now_v7()).unwrap();
        fs::write(first.join("audio/stale.mp3"), b"stale").unwrap();
        let second = create_work_area(&temp, work_item, Uuid::now_v7()).unwrap();
        fs::write(second.join("audio/current.mp3"), b"current").unwrap();
        assert_ne!(first, second);
        assert!(first.join("audio/stale.mp3").exists());
        assert!(second.join("audio/current.mp3").exists());
        fs::remove_dir_all(&first).unwrap();
        assert!(second.join("audio/current.mp3").exists());
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn artwork_preflight_rejects_byte_oversized_input() {
        let temp = test_temp_dir();
        let artwork = temp.join("oversized.jpg");
        let bytes = [0xff, 0xd8, 0xff, 0xd9, 0, 0, 0, 0];
        fs::write(&artwork, bytes).unwrap();
        let observed = artwork_observation(&bytes);
        let options = NormalizationOptions {
            max_artwork_bytes: 4,
            ..NormalizationOptions::default()
        };
        assert!(matches!(
            preflight_artwork(&observed, &artwork, &options),
            Err(NormalizationError::Verification(_))
        ));
        fs::remove_dir_all(temp).unwrap();
    }

    fn artwork_observation(bytes: &[u8]) -> ObservedFile {
        ObservedFile {
            relative_path: "cover.jpg".to_owned(),
            kind: ObservedFileKind::Artwork,
            size_bytes: bytes.len() as u64,
            modified_at: None,
            content_hash: Some(blake3::hash(bytes).to_hex().to_string()),
            mime_type: None,
            kind_code: None,
            metadata: None,
            audio: None,
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors: false,
        }
    }

    #[test]
    fn artwork_preflight_detects_same_size_source_replacement() {
        let temp = test_temp_dir();
        let artwork = temp.join("cover.jpg");
        let observed = artwork_observation(b"before");
        fs::write(&artwork, b"after!").unwrap();
        for max_artwork_bytes in [4, 1024] {
            let options = NormalizationOptions {
                max_artwork_bytes,
                ..NormalizationOptions::default()
            };
            assert!(matches!(
                preflight_artwork(&observed, &artwork, &options),
                Err(NormalizationError::SourceChanged(_))
            ));
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn normalized_artwork_rejects_empty_oversized_and_corrupt_bytes() {
        let temp = test_temp_dir();
        let artwork = temp.join("output.jpg");
        let options = NormalizationOptions {
            max_artwork_bytes: 4,
            ..NormalizationOptions::default()
        };
        for contents in [&b""[..], &b"12345"[..], &b"nope"[..]] {
            fs::write(&artwork, contents).unwrap();
            assert!(matches!(
                verify_normalized_artwork(&artwork, &temp, &options),
                Err(NormalizationError::Verification(_))
            ));
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_process_success_and_nonzero_are_reported() {
        let temp = test_temp_dir();
        let success = write_script(&temp, "success", "printf 'diagnostic' >&2\nexit 0");
        let failure = write_script(&temp, "failure", "printf 'failed' >&2\nexit 23");
        let cancellation = AtomicBool::new(false);
        let (status, diagnostic) = run_ffmpeg_process(
            &success,
            &[],
            StdDuration::from_secs(2),
            1024,
            &cancellation,
            None,
        )
        .unwrap();
        assert_eq!(status, 0);
        assert_eq!(diagnostic, "diagnostic");
        let (status, diagnostic) = run_ffmpeg_process(
            &failure,
            &[],
            StdDuration::from_secs(2),
            1024,
            &cancellation,
            None,
        )
        .unwrap();
        assert_eq!(status, 23);
        assert_eq!(diagnostic, "failed");
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn executable_busy_spawn_retry_is_narrow_and_bounded() {
        let mut busy_attempts = 0;
        let result = retry_executable_busy(|| {
            busy_attempts += 1;
            if busy_attempts < 5 {
                Err(std::io::Error::from_raw_os_error(26))
            } else {
                Ok("started")
            }
        })
        .unwrap();
        assert_eq!(result, "started");
        assert_eq!(busy_attempts, 5);

        let mut exhausted_attempts = 0;
        let error = retry_executable_busy::<()>(|| {
            exhausted_attempts += 1;
            Err(std::io::Error::from_raw_os_error(26))
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(26));
        assert_eq!(exhausted_attempts, 5);

        let mut other_attempts = 0;
        let error = retry_executable_busy::<()>(|| {
            other_attempts += 1;
            Err(std::io::Error::from_raw_os_error(2))
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(2));
        assert_eq!(other_attempts, 1);
    }

    #[cfg(unix)]
    #[test]
    fn all_media_wrappers_terminate_inherited_pipes_and_descendants() {
        let temp = test_temp_dir();
        let marker = temp.join("escaped");
        let script = write_script(
            &temp,
            "inherited",
            &format!(
                "(sleep 1; printf escaped >> '{}') &\nexit 0",
                marker.display()
            ),
        );
        let input = temp.join("track.flac");
        fs::write(&input, b"fixture").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(
            run_ffmpeg_process(
                &script,
                &[],
                StdDuration::from_millis(120),
                1024,
                &cancel,
                None
            )
            .unwrap_err()
            .to_string()
            .contains("timed out")
        );
        let options = NormalizationOptions {
            fpcalc_path: script.clone(),
            ffprobe_path: script.clone(),
            fpcalc_timeout: StdDuration::from_millis(120),
            ffprobe_timeout: StdDuration::from_millis(120),
            ..NormalizationOptions::default()
        };
        assert!(
            run_fpcalc(&input, &options)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(
            run_ffprobe_safe(&input, &options)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(
            crate::decoded_audio_fingerprint(&input, &script, StdDuration::from_millis(120))
                .unwrap_err()
                .contains("timed out")
        );
        thread::sleep(StdDuration::from_millis(1200));
        assert!(
            !marker.exists(),
            "no descendant may continue after a wrapper times out"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_cancellation_is_prompt_and_does_not_poison_cache() {
        let temp = test_temp_dir();
        let marker = temp.join("escaped");
        let ready = temp.join("normal");
        let script = write_script(
            &temp,
            "version",
            &format!(
                "if [ -f '{}' ]; then printf 'version 1'; exit 0; fi\n(sleep 1; printf escaped >> '{}') &\nexit 0",
                ready.display(),
                marker.display()
            ),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let canceler = thread::spawn(move || {
            thread::sleep(StdDuration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        assert!(
            crate::tool_version_with_cancellation(&script, 1024, Some(&cancel), None).is_none()
        );
        canceler.join().unwrap();
        assert!(started.elapsed() < StdDuration::from_secs(1));
        fs::write(ready, b"ready").unwrap();
        cancel.store(false, Ordering::SeqCst);
        assert_eq!(
            crate::tool_version_with_cancellation(&script, 1024, Some(&cancel), None).as_deref(),
            Some("version 1")
        );
        thread::sleep(StdDuration::from_millis(1100));
        assert!(!marker.exists());
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_timeout_terminates_and_reaps_child() {
        let temp = test_temp_dir();
        let pid_file = temp.join("pid");
        let script = write_script(
            &temp,
            "hang",
            &format!(
                "printf '%s' $$ > '{}'\nwhile :; do :; done",
                pid_file.display()
            ),
        );
        let cancellation = AtomicBool::new(false);
        let error = run_ffmpeg_process(
            &script,
            &[],
            StdDuration::from_millis(100),
            1024,
            &cancellation,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        let pid = fs::read_to_string(&pid_file).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_cancellation_terminates_and_reaps_child_promptly() {
        let temp = test_temp_dir();
        let pid_file = temp.join("pid");
        let script = write_script(
            &temp,
            "hang",
            &format!(
                "printf '%s' $$ > '{}'\nwhile :; do :; done",
                pid_file.display()
            ),
        );
        let cancellation = Arc::new(AtomicBool::new(false));
        let signal = cancellation.clone();
        let canceler = thread::spawn(move || {
            thread::sleep(StdDuration::from_millis(100));
            signal.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let error = run_ffmpeg_process(
            &script,
            &[],
            StdDuration::from_secs(30),
            1024,
            &cancellation,
            None,
        )
        .unwrap_err();
        canceler.join().unwrap();
        assert!(matches!(error, NormalizationError::Canceled));
        assert!(started.elapsed() < StdDuration::from_secs(2));
        let pid = fs::read_to_string(&pid_file).unwrap();
        assert!(!Path::new("/proc").join(pid).exists());
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fpcalc_success_failure_timeout_and_cancellation_are_bounded() {
        let temp = test_temp_dir();
        let input = temp.join("-- leading 音楽.flac");
        fs::write(&input, b"input").unwrap();
        let success = write_script(
            &temp,
            "fp-success",
            "printf '{\"duration\":12.5,\"fingerprint\":\"abc\"}'\nexit 0",
        );
        let failure = write_script(&temp, "fp-failure", "printf 'bad' >&2\nexit 9");
        let malformed = write_script(&temp, "fp-malformed", "printf 'not-json'\nexit 0");
        let empty = write_script(
            &temp,
            "fp-empty",
            "printf '{\"duration\":5,\"fingerprint\":\" \"}'\nexit 0",
        );
        let hang = write_script(&temp, "fp-hang", "while :; do :; done");

        let mut options = NormalizationOptions {
            fpcalc_path: success,
            fpcalc_timeout: StdDuration::from_secs(2),
            work_area: temp.clone(),
            ..NormalizationOptions::default()
        };
        let fingerprint = fingerprint_file(&input, "-- leading 音楽.flac", &options).unwrap();
        assert_eq!(fingerprint.fingerprint, "abc");
        assert_eq!(fingerprint.duration_ms, 12_500);

        let long_fingerprint = "1234567890,".repeat(80);
        let long_success = write_script(
            &temp,
            "fp-long-success",
            &format!(
                "printf '%s' '{{\"duration\":12.5,\"fingerprint\":\"{long_fingerprint}\"}}'\nexit 0"
            ),
        );
        options.fpcalc_path = long_success;
        let fingerprint = fingerprint_file(&input, "-- leading 音楽.flac", &options).unwrap();
        assert_eq!(fingerprint.fingerprint, long_fingerprint);
        assert_eq!(fingerprint.duration_ms, 12_500);

        options.fpcalc_path = failure;
        assert!(matches!(
            run_fpcalc(&input, &options),
            Err(NormalizationError::Fpcalc(_))
        ));

        options.fpcalc_path = malformed;
        assert!(matches!(
            fingerprint_file(&input, "track.flac", &options),
            Err(NormalizationError::Fpcalc(_))
        ));
        options.fpcalc_path = empty;
        assert!(matches!(
            fingerprint_file(&input, "track.flac", &options),
            Err(NormalizationError::Fpcalc(_))
        ));

        options.fpcalc_path = hang.clone();
        options.fpcalc_timeout = StdDuration::from_millis(100);
        assert!(
            run_fpcalc(&input, &options)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );

        options.fpcalc_timeout = StdDuration::from_secs(30);
        options.cancellation = Arc::new(AtomicBool::new(false));
        let signal = options.cancellation.clone();
        let canceler = thread::spawn(move || {
            thread::sleep(StdDuration::from_millis(100));
            signal.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        assert!(matches!(
            run_fpcalc(&input, &options),
            Err(NormalizationError::Canceled)
        ));
        canceler.join().unwrap();
        assert!(started.elapsed() < StdDuration::from_secs(2));
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn ensure_within_accepts_nested_paths() {
        let base = Path::new("/tmp/work/abc123");
        assert!(ensure_within(&base.join("audio/track.mp3"), base).is_ok());
        assert!(ensure_within(&base.join("artwork/cover.jpg"), base).is_ok());
    }

    #[test]
    fn ensure_within_rejects_escape() {
        let base = Path::new("/tmp/work/abc123");
        assert!(ensure_within(Path::new("/tmp/work/abc123/../../etc/passwd"), base).is_err());
    }
}

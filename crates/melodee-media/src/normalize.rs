//! Media normalization, acoustic fingerprinting, and sidecar parsing (Phase 3.2).
//!
//! Produces canonical MP3 audio in an isolated work-area staging directory,
//! Chromaprint fingerprints via `fpcalc`, normalized artwork, and parsed sidecar
//! semantics. Source media is never modified. All output paths are constrained
//! to the work-area directory to prevent path traversal.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration as StdDuration, Instant},
};

use melodee_domain::{
    CANONICAL_MP3_PROFILE_VERSION, Finding, FindingSeverity, FingerprintObservation,
    NormalizationExecutionProvenance, NormalizationObservations, NormalizedArtwork,
    NormalizedOutput, ObservedArtwork, ObservedFile, ObservedFileKind, ParsedSidecar,
    ReleaseManifest,
};
use serde::Deserialize;
use thiserror::Error;
use time::OffsetDateTime;
use tracing::info;
use uuid::Uuid;

use crate::{
    InspectionOptions, ProbeResult, hash_file, image_dimensions, path_to_posix, read_limited,
    run_ffprobe, tool_version,
};

/// Canonical MP3 profile parameters (PRD-MEDIA-002). Matches the legacy
/// seeded settings: VBR quality 4, 384 kbps maximum bitrate, 48 kHz sample rate,
/// no video streams, channels preserved.
const CANONICAL_SAMPLE_RATE_HZ: u32 = 48_000;
const CANONICAL_MAX_BITRATE_BPS: u64 = 384_000;
const CANONICAL_ARTWORK_MAX_DIMENSION: u32 = 1600;
const DEFAULT_MAX_TOOL_OUTPUT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_ARTWORK_BYTES: usize = 16 * 1024 * 1024;
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
    pub profile_version: String,
    pub cancellation: Arc<AtomicBool>,
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
            profile_version: CANONICAL_MP3_PROFILE_VERSION.to_owned(),
            cancellation: Arc::new(AtomicBool::new(false)),
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
    #[error("work area could not be created or cleaned: {path}: {message}")]
    WorkAreaCreation { path: PathBuf, message: String },
    #[error("ffmpeg process failed: {0}")]
    Ffmpeg(String),
    #[error("fpcalc process failed: {0}")]
    Fpcalc(String),
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

    if options.cancellation.load(Ordering::SeqCst) {
        return Err(NormalizationError::Canceled);
    }

    let started = Instant::now();
    let work_dir = create_work_area(&options.work_area, manifest.work_item_id)?;

    let ffmpeg_version = tool_version(&options.ffmpeg_path, options.max_tool_output_bytes);
    let fpcalc_version = tool_version(&options.fpcalc_path, options.max_tool_output_bytes);
    let _ffprobe_version = tool_version(&options.ffprobe_path, options.max_tool_output_bytes);

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

    for file in &manifest.files {
        if options.cancellation.load(Ordering::SeqCst) {
            return Err(NormalizationError::Canceled);
        }
        match file.kind {
            ObservedFileKind::Audio => {
                let source = source_root.join(&file.relative_path);
                let result = process_audio_file(file, &source, &work_dir, options)?;
                outputs.push(result.output);
                if let Some(fp) = result.fingerprint {
                    fingerprints.push(fp);
                }
                bytes_processed = bytes_processed.saturating_add(result.bytes);
            }
            ObservedFileKind::Artwork => {
                let source = source_root.join(&file.relative_path);
                if let Some(art) = process_external_artwork(file, &source, &work_dir, options)? {
                    normalized_artwork.push(art);
                }
            }
            _ => {}
        }

        if file.kind == ObservedFileKind::Audio {
            // Embedded artwork extraction from audio files.
            for embedded in &file.embedded_artwork {
                if options.cancellation.load(Ordering::SeqCst) {
                    return Err(NormalizationError::Canceled);
                }
                let source = source_root.join(&file.relative_path);
                if let Some(art) = process_embedded_artwork(embedded, &source, &work_dir, options)?
                {
                    normalized_artwork.push(art);
                }
            }
        }

        if let Some(_sidecar_kind) = parse_sidecar_for_file(file) {
            if options.cancellation.load(Ordering::SeqCst) {
                return Err(NormalizationError::Canceled);
            }
            let source = source_root.join(&file.relative_path);
            match parse_sidecar_content(file, &source) {
                Ok(parsed) => parsed_sidecars.push(parsed),
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

    manifest.normalization = Some(NormalizationObservations {
        profile_version: options.profile_version.clone(),
        outputs,
        fingerprints,
        normalized_artwork,
        parsed_sidecars,
        provenance: NormalizationExecutionProvenance {
            worker_id: None,
            normalized_at: Some(OffsetDateTime::now_utc()),
            elapsed_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
            ffmpeg_path: ffmpeg_version
                .as_ref()
                .and_then(|v| extract_tool_path(v, &options.ffmpeg_path)),
            ffmpeg_version,
            ffmpeg_exit_code: None,
            fpcalc_path: fpcalc_version
                .as_ref()
                .and_then(|v| extract_tool_path(v, &options.fpcalc_path)),
            fpcalc_version,
            work_area: Some(path_to_posix(&work_dir)),
        },
        findings: Vec::new(),
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

/// Per-file processing result bundling the normalized output and its fingerprint.
struct AudioProcessResult {
    output: NormalizedOutput,
    fingerprint: Option<FingerprintObservation>,
    bytes: u64,
}

fn process_audio_file(
    file: &ObservedFile,
    source: &Path,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<AudioProcessResult, NormalizationError> {
    let relative = &file.relative_path;
    let source_bytes = file.size_bytes;

    if !source.exists() {
        let finding = Finding {
            code: "normalization.source_missing".to_owned(),
            message: format!("source file disappeared: {relative}"),
            severity: FindingSeverity::Warning,
            file: Some(relative.clone()),
            evidence: serde_json::json!({}),
        };
        let output = NormalizedOutput {
            source_file: relative.clone(),
            output_path: String::new(),
            byte_hash: None,
            container: "mp3".to_owned(),
            codec: None,
            sample_rate_hz: None,
            channels: None,
            bitrate_bps: None,
            duration_ms: None,
            retained: false,
            findings: vec![finding.clone()],
        };
        return Ok(AudioProcessResult {
            output,
            fingerprint: None,
            bytes: 0,
        });
    }

    let output_name = output_name_for(relative, "mp3");
    let output_path = work_dir.join("audio").join(&output_name);
    ensure_within(&output_path, work_dir)?;
    fs::create_dir_all(output_path.parent().unwrap_or(work_dir)).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: output_path.clone(),
            message: error.to_string(),
        }
    })?;

    let retained = is_canonical_mp3(file);

    if retained {
        fs::copy(source, &output_path).map_err(|error| {
            NormalizationError::Ffmpeg(format!(
                "failed to copy canonical source {relative}: {error}"
            ))
        })?;
    } else {
        run_ffmpeg_encode(source, &output_path, options)?;
    }

    let byte_hash = hash_file(&output_path).map(|(h, _)| h).ok();

    let mut findings = Vec::new();

    let probe = run_ffprobe_safe(&output_path, options);
    let (codec, sample_rate_hz, channels, bitrate_bps, duration_ms, _stream_count, container) =
        match probe {
            Ok(result) => (
                result.audio.codec,
                result.audio.sample_rate_hz,
                result.audio.channels,
                result.audio.bitrate_bps,
                result.audio.duration_ms,
                result.audio.stream_count,
                result.audio.container,
            ),
            Err(message) => {
                findings.push(Finding {
                    code: "normalization.probe_output_failed".to_owned(),
                    message: truncate_message(&message),
                    severity: FindingSeverity::Warning,
                    file: Some(relative.clone()),
                    evidence: serde_json::json!({ "tool": "ffprobe" }),
                });
                (None, None, None, None, None, None, None)
            }
        };

    let output_relative = path_to_posix(output_path.strip_prefix(work_dir).unwrap_or(&output_path));

    if !byte_hash.is_some() {
        findings.push(Finding {
            code: "normalization.output_hash_failed".to_owned(),
            message: "could not hash normalized output".to_owned(),
            severity: FindingSeverity::Error,
            file: Some(relative.clone()),
            evidence: serde_json::json!({}),
        });
    }

    let output = NormalizedOutput {
        source_file: relative.clone(),
        output_path: output_relative,
        byte_hash,
        container: container.unwrap_or_else(|| "mp3".to_owned()),
        codec,
        sample_rate_hz,
        channels,
        bitrate_bps,
        duration_ms,
        retained,
        findings,
    };

    let fingerprint = if !options.cancellation.load(Ordering::SeqCst) {
        Some(fingerprint_file(source, relative, options)?)
    } else {
        None
    };

    Ok(AudioProcessResult {
        output,
        fingerprint,
        bytes: source_bytes,
    })
}

fn process_external_artwork(
    file: &ObservedFile,
    source: &Path,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<Option<NormalizedArtwork>, NormalizationError> {
    if file.size_bytes > options.max_artwork_bytes as u64 {
        return Ok(None);
    }
    let output_name = output_name_for(&file.relative_path, "jpg");
    let output_path = work_dir.join("artwork").join(&output_name);
    ensure_within(&output_path, work_dir)?;
    fs::create_dir_all(output_path.parent().unwrap_or(work_dir)).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: output_path.clone(),
            message: error.to_string(),
        }
    })?;

    run_ffmpeg_image_normalize(source, &output_path, options)?;

    let (width, height) =
        image_dimensions(&fs::read(&output_path).unwrap_or_default()).unwrap_or((0, 0));
    let output_hash = hash_file(&output_path).map(|(h, _)| h).ok();
    let output_relative = path_to_posix(output_path.strip_prefix(work_dir).unwrap_or(&output_path));

    Ok(Some(NormalizedArtwork {
        source: "external".to_owned(),
        source_file: file.relative_path.clone(),
        output_path: output_relative,
        mime_type: "image/jpeg".to_owned(),
        width,
        height,
        byte_hash: output_hash.unwrap_or_default(),
    }))
}

fn process_embedded_artwork(
    embedded: &ObservedArtwork,
    source: &Path,
    work_dir: &Path,
    options: &NormalizationOptions,
) -> Result<Option<NormalizedArtwork>, NormalizationError> {
    let output_name = format!(
        "embedded-{}_{}.jpg",
        embedded.picture_type.as_deref().unwrap_or("front"),
        uuid::Uuid::now_v7().simple()
    );
    let output_path = work_dir.join("artwork").join(&output_name);
    ensure_within(&output_path, work_dir)?;
    fs::create_dir_all(output_path.parent().unwrap_or(work_dir)).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: output_path.clone(),
            message: error.to_string(),
        }
    })?;

    run_ffmpeg_extract_image(source, &output_path, options)?;

    let output_hash = hash_file(&output_path).map(|(h, _)| h).ok();
    let (width, height) =
        image_dimensions(&fs::read(&output_path).unwrap_or_default()).unwrap_or((0, 0));
    let output_relative = path_to_posix(output_path.strip_prefix(work_dir).unwrap_or(&output_path));

    Ok(Some(NormalizedArtwork {
        source: "embedded".to_owned(),
        source_file: source.to_string_lossy().to_string(),
        output_path: output_relative,
        mime_type: "image/jpeg".to_owned(),
        width,
        height,
        byte_hash: output_hash.unwrap_or_default(),
    }))
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

    let duration_ms = parsed.duration.map(|d| {
        let millis = (d * 1000.0).round();
        if millis > 0.0 && millis <= f64::from(u32::MAX) {
            millis as u64
        } else {
            0
        }
    });

    if parsed.fingerprint.is_empty() {
        Err(NormalizationError::Fpcalc(
            "fpcalc produced empty fingerprint".to_owned(),
        ))
    } else {
        Ok(FingerprintObservation {
            source_file: relative_path.to_owned(),
            fingerprint: parsed.fingerprint,
            duration_ms,
            findings: Vec::new(),
        })
    }
}

/// Determines whether a source file is already in the canonical MP3 profile and
/// can be copied rather than re-encoded.
fn is_canonical_mp3(file: &ObservedFile) -> bool {
    let audio = match &file.audio {
        Some(audio) => audio,
        None => return false,
    };
    audio.codec.as_deref() == Some("mp3")
        && audio.sample_rate_hz == Some(CANONICAL_SAMPLE_RATE_HZ)
        && audio
            .bitrate_bps
            .is_some_and(|b| b <= CANONICAL_MAX_BITRATE_BPS)
        && audio.stream_count.is_some_and(|s| s <= 1)
}

fn run_ffmpeg_encode(
    input: &Path,
    output: &Path,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let args: Vec<String> = vec![
        "-y".to_string(),
        "-v".to_string(),
        "error".to_string(),
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
        output.to_string_lossy().to_string(),
    ];

    let (exit_code, stderr) = run_ffmpeg_process(
        &options.ffmpeg_path,
        &args,
        options.ffmpeg_timeout,
        options.max_tool_output_bytes,
    )?;

    if exit_code != 0 {
        return Err(NormalizationError::Ffmpeg(format!(
            "ffmpeg encoding failed (exit {exit_code}): {stderr}"
        )));
    }
    Ok(())
}

fn run_ffmpeg_process(
    ffmpeg_path: &Path,
    args: &[String],
    timeout: StdDuration,
    max_output: usize,
) -> Result<(i32, String), NormalizationError> {
    let mut command = Command::new(ffmpeg_path);
    for arg in args {
        command.arg(arg);
    }
    command.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|error| NormalizationError::Ffmpeg(format!("unable to start ffmpeg: {error}")))?;
    let stderr = child.stderr.take();
    let stderr_reader =
        stderr.map(|stderr| thread::spawn(move || read_limited(stderr, max_output)));
    let deadline = Instant::now() + timeout;

    let status = run_child_wait_ffmpeg(child, deadline)?;

    let stderr_output = stderr_reader
        .and_then(|h| h.join().ok())
        .and_then(|result| result.ok())
        .map(|b| crate::truncate_bytes(&b))
        .unwrap_or_default();

    Ok((status.code().unwrap_or(-1), stderr_output))
}

fn run_child_wait_ffmpeg(
    mut child: std::process::Child,
    deadline: Instant,
) -> Result<std::process::ExitStatus, NormalizationError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(NormalizationError::Ffmpeg("ffmpeg timed out".to_owned()));
            }
            Ok(None) => thread::sleep(StdDuration::from_millis(10)),
            Err(error) => {
                return Err(NormalizationError::Ffmpeg(format!(
                    "unable to wait for ffmpeg: {error}"
                )));
            }
        }
    }
}

fn run_fpcalc(input: &Path, options: &NormalizationOptions) -> Result<String, NormalizationError> {
    let mut child = Command::new(&options.fpcalc_path)
        .arg("-json")
        .arg("-length")
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| NormalizationError::Fpcalc(format!("unable to start fpcalc: {error}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let max_output = options.max_tool_output_bytes;
    let stdout_reader =
        stdout.map(|stdout| thread::spawn(move || read_limited(stdout, max_output)));
    let stderr_reader =
        stderr.map(|stderr| thread::spawn(move || read_limited(stderr, max_output)));

    let deadline = Instant::now() + options.fpcalc_timeout;
    let mut child_for_wait = child;
    let status = run_child_wait_generic(&mut child_for_wait, deadline, "fpcalc")?;

    let stdout_output = stdout_reader
        .and_then(|h| h.join().ok())
        .and_then(|result| result.ok())
        .map(|b| crate::truncate_bytes(&b))
        .unwrap_or_default();
    let stderr_output = stderr_reader
        .and_then(|h| h.join().ok())
        .and_then(|result| result.ok())
        .map(|b| crate::truncate_bytes(&b))
        .unwrap_or_default();

    if !status.success() {
        return Err(NormalizationError::Fpcalc(format!(
            "fpcalc exited with {}: {}",
            status, stderr_output
        )));
    }

    if stdout_output.trim().is_empty() {
        return Err(NormalizationError::Fpcalc(
            "fpcalc produced no output".to_owned(),
        ));
    }

    Ok(stdout_output)
}

fn run_child_wait_generic(
    child: &mut std::process::Child,
    deadline: Instant,
    name: &str,
) -> Result<std::process::ExitStatus, NormalizationError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(NormalizationError::Fpcalc(format!("{name} timed out")));
            }
            Ok(None) => thread::sleep(StdDuration::from_millis(10)),
            Err(error) => {
                return Err(NormalizationError::Fpcalc(format!(
                    "unable to wait for {name}: {error}"
                )));
            }
        }
    }
}

fn run_ffmpeg_image_normalize(
    input: &Path,
    output: &Path,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let scale_arg = format!(
        "scale={dim}:{dim}:force_original_aspect_ratio=decrease,pad={dim}:{dim}:(ow-iw)/2:(oh-ih)/2:color=black",
        dim = CANONICAL_ARTWORK_MAX_DIMENSION
    );
    let args: Vec<String> = vec![
        "-y".to_string(),
        "-v".to_string(),
        "error".to_string(),
        "-i".to_string(),
        input.to_string_lossy().to_string(),
        "-vf".to_string(),
        scale_arg,
        output.to_string_lossy().to_string(),
    ];

    let (exit_code, stderr) = run_ffmpeg_process(
        &options.ffmpeg_path,
        &args,
        options.ffmpeg_timeout,
        options.max_tool_output_bytes,
    )?;

    if exit_code != 0 {
        return Err(NormalizationError::Ffmpeg(format!(
            "ffmpeg image normalize failed (exit {exit_code}): {stderr}"
        )));
    }
    Ok(())
}

fn run_ffmpeg_extract_image(
    input: &Path,
    output: &Path,
    options: &NormalizationOptions,
) -> Result<(), NormalizationError> {
    let args: Vec<String> = vec![
        "-y".to_string(),
        "-v".to_string(),
        "error".to_string(),
        "-i".to_string(),
        input.to_string_lossy().to_string(),
        "-an".to_string(),
        "-vcodec".to_string(),
        "copy".to_string(),
        output.to_string_lossy().to_string(),
    ];

    let (exit_code, stderr) = run_ffmpeg_process(
        &options.ffmpeg_path,
        &args,
        options.ffmpeg_timeout,
        options.max_tool_output_bytes,
    )?;

    if exit_code != 0 {
        return Err(NormalizationError::Ffmpeg(format!(
            "ffmpeg image extract failed (exit {exit_code}): {stderr}"
        )));
    }
    Ok(())
}

fn run_ffprobe_safe(path: &Path, options: &NormalizationOptions) -> Result<ProbeResult, String> {
    let probe_options = InspectionOptions {
        ffprobe_path: options.ffprobe_path.clone(),
        ffprobe_timeout: options.ffprobe_timeout,
        max_tool_output_bytes: options.max_tool_output_bytes,
        max_artwork_bytes: options.max_artwork_bytes,
    };
    run_ffprobe(path, &probe_options)
}

fn create_work_area(base: &Path, work_item_id: Uuid) -> Result<PathBuf, NormalizationError> {
    fs::create_dir_all(base).map_err(|error| NormalizationError::WorkAreaCreation {
        path: base.to_path_buf(),
        message: error.to_string(),
    })?;
    let work_dir = base.join(work_item_id.simple().to_string());
    if work_dir.exists() {
        fs::remove_dir_all(&work_dir).map_err(|error| NormalizationError::WorkAreaCreation {
            path: work_dir.clone(),
            message: error.to_string(),
        })?;
    }
    fs::create_dir_all(&work_dir).map_err(|error| NormalizationError::WorkAreaCreation {
        path: work_dir.clone(),
        message: error.to_string(),
    })?;
    fs::create_dir_all(work_dir.join("audio")).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: work_dir.join("audio"),
            message: error.to_string(),
        }
    })?;
    fs::create_dir_all(work_dir.join("artwork")).map_err(|error| {
        NormalizationError::WorkAreaCreation {
            path: work_dir.join("artwork"),
            message: error.to_string(),
        }
    })?;
    Ok(work_dir)
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

fn output_name_for(relative: &str, extension: &str) -> String {
    let stem = Path::new(relative)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio");
    format!("{stem}.{extension}")
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

fn parse_sidecar_content(
    file: &ObservedFile,
    source: &Path,
) -> Result<ParsedSidecar, NormalizationError> {
    let sidecar_type = match file.kind {
        ObservedFileKind::CueSheet => "cue",
        ObservedFileKind::Playlist => "m3u",
        ObservedFileKind::Nfo => "nfo",
        ObservedFileKind::Checksum => "sfv",
        ObservedFileKind::Lyrics => "lrc",
        _ => "sidecar",
    };

    let content =
        fs::read_to_string(source).map_err(|error| NormalizationError::SourceUnreadable {
            path: source.to_path_buf(),
            message: error.to_string(),
        })?;

    let parsed = match sidecar_type {
        "cue" => parse_cue(&content),
        "m3u" => parse_m3u(&content),
        "sfv" => parse_sfv(&content),
        "nfo" => parse_nfo(&content),
        "lrc" => parse_lrc(&content),
        _ => serde_json::json!({ "raw": content }),
    };

    Ok(ParsedSidecar {
        sidecar_type: sidecar_type.to_owned(),
        relative_path: file.relative_path.clone(),
        parsed,
        findings: Vec::new(),
    })
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

fn parse_cue(content: &str) -> serde_json::Value {
    let mut tracks = Vec::new();
    let mut current: Option<serde_json::Map<String, serde_json::Value>> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("TRACK") {
            if let Some(prev) = current.take() {
                tracks.push(prev);
            }
            current = Some(serde_json::Map::new());
        }
        if let Some(ref mut track) = current {
            if trimmed.starts_with("TITLE") {
                track.insert(
                    "title".to_owned(),
                    serde_json::json!(strip_quotes(
                        trimmed.split_once(' ').map(|x| x.1).unwrap_or("").trim()
                    )),
                );
            } else if trimmed.starts_with("PERFORMER") {
                track.insert(
                    "performer".to_owned(),
                    serde_json::json!(strip_quotes(
                        trimmed.split_once(' ').map(|x| x.1).unwrap_or("").trim()
                    )),
                );
            } else if trimmed.starts_with("FILE") {
                track.insert(
                    "file".to_owned(),
                    serde_json::json!(strip_quotes(
                        trimmed.split_once(' ').map(|x| x.1).unwrap_or("").trim()
                    )),
                );
            } else if trimmed.starts_with("INDEX") {
                track.insert(
                    "index".to_owned(),
                    serde_json::json!(trimmed.splitn(3, ' ').nth(2).unwrap_or("").trim()),
                );
            }
        }
    }
    if let Some(prev) = current {
        tracks.push(prev);
    }
    serde_json::json!({ "tracks": tracks })
}

fn parse_m3u(content: &str) -> serde_json::Value {
    let mut entries = Vec::new();
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("#") {
            if trimmed.starts_with("#EXTINF") {
                // Next line is the path
                continue;
            }
            continue;
        }
        entries.push(serde_json::json!({
            "path": trimmed,
            "title": lines.clone().next().filter(|l| l.starts_with("#EXTINF"))
                .and_then(|ext| ext.split(',').nth(1))
                .unwrap_or(""),
        }));
    }
    serde_json::json!({ "entries": entries })
}

fn parse_sfv(content: &str) -> serde_json::Value {
    let mut entries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(";") || trimmed.starts_with("SFV") {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            entries.push(serde_json::json!({
                "file": parts[0],
                "crc32": parts[1],
            }));
        }
    }
    serde_json::json!({ "entries": entries })
}

fn parse_nfo(content: &str) -> serde_json::Value {
    let text = content.trim();
    if text.starts_with("<?xml") || text.starts_with("<") {
        serde_json::json!({ "type": "xml", "raw": text })
    } else {
        serde_json::json!({ "type": "text", "content": text })
    }
}

fn parse_lrc(content: &str) -> serde_json::Value {
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (timestamp, text) = if trimmed.starts_with('[') {
            let close = trimmed.find(']');
            if let Some(close_pos) = close {
                (
                    trimmed[1..close_pos].to_string(),
                    trimmed[close_pos + 1..].trim().to_string(),
                )
            } else {
                (String::new(), trimmed.to_string())
            }
        } else {
            (String::new(), trimmed.to_string())
        };
        lines.push(serde_json::json!({
            "time": timestamp,
            "text": text,
        }));
    }
    serde_json::json!({ "lines": lines })
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

    #[test]
    fn canonical_mp3_profile_version_matches_domain() {
        assert_eq!(CANONICAL_MP3_PROFILE_VERSION, "mp3-vbr-q4-384k-48k");
    }

    #[test]
    fn is_canonical_mp3_true_for_48k_mp3() {
        let audio = melodee_domain::AudioTechnicalObservation {
            container: Some("mpeg".to_owned()),
            codec: Some("mp3".to_owned()),
            sample_rate_hz: Some(48_000),
            channels: Some(2),
            channel_layout: Some("stereo".to_owned()),
            bit_depth: None,
            bitrate_bps: Some(320_000),
            stream_count: Some(1),
            duration_ms: Some(10_000),
            stream_index: Some(0),
        };
        let file = ObservedFile {
            relative_path: "track.mp3".to_owned(),
            kind: ObservedFileKind::Audio,
            size_bytes: 1000,
            modified_at: None,
            content_hash: None,
            mime_type: None,
            kind_code: None,
            metadata: None,
            audio: Some(audio),
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors: false,
        };
        assert!(is_canonical_mp3(&file));
    }

    #[test]
    fn is_canonical_mp3_false_for_non_mp3() {
        let audio = melodee_domain::AudioTechnicalObservation {
            container: Some("flac".to_owned()),
            codec: Some("flac".to_owned()),
            sample_rate_hz: Some(48_000),
            channels: Some(2),
            channel_layout: None,
            bit_depth: Some(16),
            bitrate_bps: None,
            stream_count: Some(1),
            duration_ms: Some(10_000),
            stream_index: Some(0),
        };
        let file = ObservedFile {
            relative_path: "track.flac".to_owned(),
            kind: ObservedFileKind::Audio,
            size_bytes: 1000,
            modified_at: None,
            content_hash: None,
            mime_type: None,
            kind_code: None,
            metadata: None,
            audio: Some(audio),
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors: false,
        };
        assert!(!is_canonical_mp3(&file));
    }

    #[test]
    fn is_canonical_mp3_false_for_wrong_sample_rate() {
        let audio = melodee_domain::AudioTechnicalObservation {
            container: Some("mpeg".to_owned()),
            codec: Some("mp3".to_owned()),
            sample_rate_hz: Some(44_100),
            channels: Some(2),
            channel_layout: Some("stereo".to_owned()),
            bit_depth: None,
            bitrate_bps: Some(320_000),
            stream_count: Some(1),
            duration_ms: Some(10_000),
            stream_index: Some(0),
        };
        let file = ObservedFile {
            relative_path: "track.mp3".to_owned(),
            kind: ObservedFileKind::Audio,
            size_bytes: 1000,
            modified_at: None,
            content_hash: None,
            mime_type: None,
            kind_code: None,
            metadata: None,
            audio: Some(audio),
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors: false,
        };
        assert!(!is_canonical_mp3(&file));
    }

    #[test]
    fn parse_m3u_extracts_entries() {
        let content =
            "#EXTM3U\n#EXTINF:180,Artist - Title\ntrack01.mp3\n#EXTINF:200,Other\ntrack02.mp3\n";
        let result = parse_m3u(content);
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["path"], "track01.mp3");
        assert_eq!(entries[1]["path"], "track02.mp3");
    }

    #[test]
    fn parse_sfv_extracts_crc32_entries() {
        let content = "SFV next 1.0\n\nfile1.mp3 12345678\nfile2.flac ABCDEF01\n;comment\n";
        let result = parse_sfv(content);
        let entries = result["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["file"], "file1.mp3");
        assert_eq!(entries[0]["crc32"], "12345678");
        assert_eq!(entries[1]["crc32"], "ABCDEF01");
    }

    #[test]
    fn parse_lrc_extracts_timed_lines() {
        let content = "[00:12.00]Hello\n[00:15.50]World\n";
        let result = parse_lrc(content);
        let lines = result["lines"].as_array().expect("lines array");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["text"], "Hello");
        assert_eq!(lines[1]["text"], "World");
    }

    #[test]
    fn parse_nfo_text_vs_xml() {
        let text_result = parse_nfo("some description text");
        assert_eq!(text_result["type"], "text");
        let xml_result = parse_nfo("<?xml version=\"1.0\"?><root/>");
        assert_eq!(xml_result["type"], "xml");
    }

    #[test]
    fn parse_cue_extracts_tracks() {
        let content = "PERFORMER \"Artist\"\nTITLE \"Album\"\nFILE \"track1.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Song\"\n    PERFORMER \"Artist\"\n";
        let result = parse_cue(content);
        let tracks = result["tracks"].as_array().expect("tracks array");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["title"], "Song");
    }

    #[test]
    fn output_name_replaces_extension() {
        assert_eq!(output_name_for("disc/track.flac", "mp3"), "track.mp3");
        assert_eq!(output_name_for("track.wav", "mp3"), "track.mp3");
        assert_eq!(output_name_for("track.mp3", "mp3"), "track.mp3");
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

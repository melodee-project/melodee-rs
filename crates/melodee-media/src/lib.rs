//! Bounded, read-only media observation for Release inspection.
//!
//! This crate owns the boundary to Lofty and `ffprobe`. It returns Melodee
//! manifest types so callers never depend on either third-party representation.

#![forbid(unsafe_code)]

use std::{
    fs::{self, DirEntry, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration as StdDuration, Instant},
};

use blake3::Hasher;
use lofty::{file::TaggedFileExt, prelude::Accessor, read_from_path, tag::ItemKey};
use melodee_domain::{
    AudioTechnicalObservation, Finding, FindingSeverity, ManifestReleaseObservation,
    ObservedArtwork, ObservedFile, ObservedFileKind, ObservedMetadata, ObservedTrack,
    ReleaseManifest,
};
use serde::Deserialize;
use thiserror::Error;
use time::{OffsetDateTime, UtcOffset};
use tracing::{info, warn};
use uuid::Uuid;

const DEFAULT_FFPROBE_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const DEFAULT_MAX_TOOL_OUTPUT: usize = 1_048_576;
const DEFAULT_MAX_ARTWORK_BYTES: usize = 16 * 1024 * 1024;
const HASH_BUFFER_SIZE: usize = 128 * 1024;

/// Input/resource limits for one inspection. Each worker receives one copy,
/// so the values also bound aggregate memory when worker concurrency is bounded.
#[derive(Debug, Clone)]
pub struct InspectionOptions {
    pub ffprobe_path: PathBuf,
    pub ffprobe_timeout: StdDuration,
    pub max_tool_output_bytes: usize,
    pub max_artwork_bytes: usize,
}

impl Default for InspectionOptions {
    fn default() -> Self {
        Self {
            ffprobe_path: PathBuf::from("ffprobe"),
            ffprobe_timeout: DEFAULT_FFPROBE_TIMEOUT,
            max_tool_output_bytes: DEFAULT_MAX_TOOL_OUTPUT,
            max_artwork_bytes: DEFAULT_MAX_ARTWORK_BYTES,
        }
    }
}

/// Result of inspecting one coherent source tree.
#[derive(Debug)]
pub struct InspectionReport {
    pub manifest: ReleaseManifest,
    pub manifest_hash: String,
    pub structurally_valid: bool,
    pub bytes_hashed: u64,
    pub ffprobe_invocations: u32,
    pub file_count: usize,
}

#[derive(Debug, Error)]
pub enum InspectionError {
    #[error("source directory is missing: {0}")]
    SourceMissing(PathBuf),
    #[error("source directory cannot be inspected: {path}: {message}")]
    SourceUnreadable { path: PathBuf, message: String },
    #[error("manifest serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("ffprobe process failed: {0}")]
    Probe(String),
}

/// Inspect all files below `release_root` without writing to the source.
/// `expected_signature` is the settling signature; callers compare it
/// before and after this operation as well, because a stable source can change
/// during a long inspection.
pub fn inspect_release(
    work_item_id: Uuid,
    source_library_id: Option<Uuid>,
    source_path: &str,
    release_root: &Path,
    source_snapshot_id: Option<i64>,
    source_signature: Option<&str>,
    options: &InspectionOptions,
) -> Result<InspectionReport, InspectionError> {
    if !release_root.is_dir() {
        return Err(InspectionError::SourceMissing(release_root.to_path_buf()));
    }

    let mut paths = Vec::new();
    collect_files(release_root, release_root, &mut paths)?;
    paths.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let mut files = Vec::with_capacity(paths.len());
    let mut tracks = Vec::new();
    let mut artwork = Vec::new();
    let mut sidecars = Vec::new();
    let mut findings = Vec::new();
    let mut release_titles = Vec::new();
    let mut release_artists = Vec::new();
    let mut bytes_hashed = 0_u64;
    let mut ffprobe_invocations = 0_u32;
    let ffprobe_version = tool_version(&options.ffprobe_path, options.max_tool_output_bytes);

    for (relative_path, path) in paths {
        let observed = inspect_file(
            &relative_path,
            &path,
            release_root,
            options,
            &mut bytes_hashed,
            &mut ffprobe_invocations,
        )?;

        if let Some(metadata) = &observed.metadata {
            push_unique(&mut release_titles, metadata.album.clone());
            push_unique(&mut release_artists, metadata.album_artist.clone());
            push_unique(&mut release_artists, metadata.artist.clone());
        }

        if observed.kind == ObservedFileKind::Audio {
            if let Some(metadata) = observed.metadata.clone() {
                tracks.push(ObservedTrack {
                    file_path: observed.relative_path.clone(),
                    track_number: metadata.track_number,
                    track_title: metadata.track_title.clone(),
                    media_number: metadata.disc_number,
                    sort_order: metadata.track_number,
                    byte_hash: observed.content_hash.clone(),
                    duration_ms: observed.audio.as_ref().and_then(|audio| audio.duration_ms),
                    observed_metadata: Some(metadata),
                });
            } else {
                tracks.push(ObservedTrack {
                    file_path: observed.relative_path.clone(),
                    track_number: None,
                    track_title: None,
                    media_number: None,
                    sort_order: None,
                    byte_hash: observed.content_hash.clone(),
                    duration_ms: observed.audio.as_ref().and_then(|audio| audio.duration_ms),
                    observed_metadata: None,
                });
            }
        }

        if observed.kind == ObservedFileKind::Artwork {
            artwork.push(artwork_from_external(&observed));
        }
        artwork.extend(observed.embedded_artwork.clone());
        if is_sidecar(observed.kind.clone()) {
            sidecars.push(observed.clone());
        }
        findings.extend(observed.findings.clone());
        files.push(observed);
    }

    tracks.sort_unstable_by(|left, right| left.file_path.cmp(&right.file_path));
    artwork.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
    sidecars.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
    release_titles.sort_unstable();
    release_artists.sort_unstable();

    let audio_count = files
        .iter()
        .filter(|file| file.kind == ObservedFileKind::Audio)
        .count();
    if audio_count == 0 {
        findings.push(Finding {
            code: "release.no_audio".to_owned(),
            message: "release contains no classified audio files".to_owned(),
            severity: FindingSeverity::Error,
            file: None,
            evidence: serde_json::json!({}),
        });
    }

    let mut track_numbers = tracks
        .iter()
        .filter_map(|track| track.track_number)
        .collect::<Vec<_>>();
    track_numbers.sort_unstable();
    let duplicate_track_numbers = track_numbers
        .windows(2)
        .any(|window| window[0] == window[1]);
    if duplicate_track_numbers {
        findings.push(Finding {
            code: "release.duplicate_track_numbers".to_owned(),
            message: "multiple audio files report the same track number".to_owned(),
            severity: FindingSeverity::Warning,
            file: None,
            evidence: serde_json::json!({ "track_numbers": track_numbers }),
        });
    }

    let manifest = ReleaseManifest::new(
        work_item_id,
        source_library_id,
        source_path.to_owned(),
        source_snapshot_id,
        source_signature.map(ToOwned::to_owned),
        ffprobe_version,
        options
            .ffprobe_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        Some(0),
        Some(
            options
                .ffprobe_timeout
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        ),
        true,
        ManifestReleaseObservation {
            title: release_titles.first().cloned(),
            artists: release_artists,
            date: None,
            source_labels: serde_json::json!({
                "observed_titles": release_titles,
            }),
        },
        files,
        tracks,
        artwork,
        sidecars,
        findings,
    );
    let manifest_hash = hash_manifest(&manifest)?;
    let structurally_valid = !manifest
        .findings
        .iter()
        .any(|finding| finding.severity == FindingSeverity::Error);

    info!(
        work_item_id = %work_item_id,
        file_count = manifest.files.len(),
        audio_count,
        structurally_valid,
        "release inspection completed"
    );

    Ok(InspectionReport {
        file_count: manifest.files.len(),
        manifest,
        manifest_hash,
        structurally_valid,
        bytes_hashed,
        ffprobe_invocations,
    })
}

/// Hashes a manifest's stable semantic representation with BLAKE3.
pub fn hash_manifest(manifest: &ReleaseManifest) -> Result<String, InspectionError> {
    let value = manifest.semantic_value()?;
    let bytes = serde_json::to_vec(&value)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), InspectionError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| InspectionError::SourceUnreadable {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| InspectionError::SourceUnreadable {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?;
    entries.sort_unstable_by_key(DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| InspectionError::SourceUnreadable {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if metadata.is_dir() {
            collect_files(root, &path, output)?;
        } else if metadata.is_file() {
            let relative =
                path.strip_prefix(root)
                    .map_err(|_| InspectionError::SourceUnreadable {
                        path: path.clone(),
                        message: "path escaped release root".to_owned(),
                    })?;
            output.push((path_to_posix(relative), path));
        } else if metadata.file_type().is_symlink() {
            warn!(relative_path = %path.display(), "ignoring symlink in release inspection");
        }
    }
    Ok(())
}

fn inspect_file(
    relative_path: &str,
    path: &Path,
    release_root: &Path,
    options: &InspectionOptions,
    bytes_hashed: &mut u64,
    ffprobe_invocations: &mut u32,
) -> Result<ObservedFile, InspectionError> {
    let metadata = fs::metadata(path).map_err(|error| InspectionError::SourceUnreadable {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let kind = classify(relative_path);
    let modified_at = metadata.modified().ok().map(|value| {
        OffsetDateTime::from(value)
            .to_offset(UtcOffset::UTC)
            .to_string()
    });
    let mut file = ObservedFile {
        relative_path: relative_path.to_owned(),
        kind: kind.clone(),
        size_bytes: metadata.len(),
        modified_at,
        content_hash: None,
        mime_type: mime_for_path(relative_path),
        kind_code: None,
        metadata: None,
        audio: None,
        findings: Vec::new(),
        embedded_artwork: Vec::new(),
        sidecar_type: sidecar_name(&kind),
        bytes_read_errors: false,
    };

    if kind == ObservedFileKind::Audio {
        match hash_file(path) {
            Ok((hash, bytes)) => {
                file.content_hash = Some(hash);
                *bytes_hashed = bytes_hashed.saturating_add(bytes);
            }
            Err(error) => {
                file.bytes_read_errors = true;
                file.findings.push(Finding {
                    code: "file.hash_failed".to_owned(),
                    message: error.to_string(),
                    severity: FindingSeverity::Error,
                    file: Some(relative_path.to_owned()),
                    evidence: serde_json::json!({}),
                });
            }
        }

        match read_native_metadata(path, options.max_artwork_bytes) {
            Ok((observed, embedded_artwork, native_findings)) => {
                file.metadata = Some(observed);
                file.embedded_artwork = embedded_artwork.clone();
                file.findings.extend(native_findings);
                // Embedded art is represented in the file findings and the
                // release artwork list by a stable pseudo-relative path.
                for (index, embedded) in embedded_artwork.into_iter().enumerate() {
                    file.findings.push(Finding {
                        code: "artwork.embedded_observed".to_owned(),
                        message: format!("embedded artwork observed at index {index}"),
                        severity: FindingSeverity::Info,
                        file: Some(relative_path.to_owned()),
                        evidence: serde_json::json!({
                            "bytes": embedded.size_bytes,
                            "mime_type": embedded.mime_type,
                        }),
                    });
                }
            }
            Err(error) => file.findings.push(Finding {
                code: "metadata.read_failed".to_owned(),
                message: truncate_message(&error),
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({ "reader": "lofty" }),
            }),
        }

        *ffprobe_invocations = ffprobe_invocations.saturating_add(1);
        match run_ffprobe(path, options) {
            Ok(probe) => {
                file.audio = Some(probe.audio);
                if file.mime_type.is_none() {
                    file.mime_type = probe.mime_type;
                }
            }
            Err(error) => {
                file.findings.push(Finding {
                    code: if error.contains("timed out") {
                        "media.ffprobe_timeout".to_owned()
                    } else {
                        "media.ffprobe_failed".to_owned()
                    },
                    message: truncate_message(&error),
                    severity: FindingSeverity::Error,
                    file: Some(relative_path.to_owned()),
                    evidence: serde_json::json!({ "tool": "ffprobe" }),
                });
            }
        }

        if file.metadata.is_none() {
            file.findings.push(Finding {
                code: "metadata.missing".to_owned(),
                message: "audio file has no readable native metadata".to_owned(),
                severity: FindingSeverity::Info,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({}),
            });
        }
        if file.audio.is_none() {
            file.kind_code = Some("unsupported_or_corrupt_audio".to_owned());
        }
    } else if kind == ObservedFileKind::Artwork {
        match hash_file(path) {
            Ok((hash, _)) => file.content_hash = Some(hash),
            Err(error) => file.findings.push(Finding {
                code: "artwork.hash_failed".to_owned(),
                message: error.to_string(),
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({}),
            }),
        }
        if file.size_bytes > options.max_artwork_bytes as u64 {
            file.findings.push(Finding {
                code: "artwork.oversized".to_owned(),
                message: "external artwork exceeds the configured observation bound".to_owned(),
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({
                    "size_bytes": file.size_bytes,
                    "max_bytes": options.max_artwork_bytes,
                }),
            });
        } else if let Err(error) = inspect_image_header(path) {
            file.findings.push(Finding {
                code: "artwork.invalid".to_owned(),
                message: error,
                severity: FindingSeverity::Warning,
                file: Some(relative_path.to_owned()),
                evidence: serde_json::json!({}),
            });
        }
    }

    let _ = release_root;
    Ok(file)
}

fn classify(relative_path: &str) -> ObservedFileKind {
    let path = Path::new(relative_path);
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if filename == ".melodee-ready" {
        return ObservedFileKind::MelodeeControl;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some(
            "aac" | "ac3" | "aif" | "aiff" | "ape" | "flac" | "m4a" | "m4b" | "m4p" | "mp2" | "mp3"
            | "mp4" | "mpc" | "ogg" | "opus" | "spx" | "wav" | "wma" | "wv",
        ) => ObservedFileKind::Audio,
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp") => ObservedFileKind::Artwork,
        Some("cue") => ObservedFileKind::CueSheet,
        Some("m3u" | "m3u8") => ObservedFileKind::Playlist,
        Some("nfo") => ObservedFileKind::Nfo,
        Some("sfv" | "md5" | "sha1" | "sha256") => ObservedFileKind::Checksum,
        Some("lrc") => ObservedFileKind::Lyrics,
        _ => ObservedFileKind::Unknown,
    }
}

fn is_sidecar(kind: ObservedFileKind) -> bool {
    matches!(
        kind,
        ObservedFileKind::CueSheet
            | ObservedFileKind::Playlist
            | ObservedFileKind::Nfo
            | ObservedFileKind::Checksum
            | ObservedFileKind::Lyrics
            | ObservedFileKind::Sidecar
    )
}

fn sidecar_name(kind: &ObservedFileKind) -> Option<String> {
    if is_sidecar(kind.clone()) {
        Some(format!("{kind:?}").to_ascii_lowercase())
    } else {
        None
    }
}

fn mime_for_path(path: &str) -> Option<String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)?;
    let mime = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "aac" => "audio/aac",
        "ac3" => "audio/ac3",
        "wav" => "audio/wav",
        "m4a" | "mp4" => "audio/mp4",
        "wma" => "audio/x-ms-wma",
        _ => return None,
    };
    Some(mime.to_owned())
}

fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes = bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

fn read_native_metadata(
    path: &Path,
    max_artwork_bytes: usize,
) -> Result<(ObservedMetadata, Vec<ObservedArtwork>, Vec<Finding>), String> {
    let tagged = read_from_path(path).map_err(|error| error.to_string())?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let Some(tag) = tag else {
        return Err("no native metadata tag".to_owned());
    };

    let text = |value: Option<std::borrow::Cow<'_, str>>| value.map(|value| value.into_owned());
    let metadata = ObservedMetadata {
        album: text(tag.album()),
        album_artist: tag.get_string(ItemKey::AlbumArtist).map(ToOwned::to_owned),
        track_title: text(tag.title()),
        artist: text(tag.artist()),
        track_number: tag.track(),
        track_total: tag.track_total(),
        disc_number: tag.disk(),
        disc_total: tag.disk_total(),
        year: None,
        date: tag.date().map(|value| value.to_string()),
        genre: text(tag.genre()),
        raw: serde_json::json!({
            "album": tag.album().as_deref(),
            "album_artist": tag.get_string(ItemKey::AlbumArtist),
            "title": tag.title().as_deref(),
            "artist": tag.artist().as_deref(),
            "track": tag.track(),
            "track_total": tag.track_total(),
            "disc": tag.disk(),
            "disc_total": tag.disk_total(),
            "date": tag.date().map(|value| value.to_string()),
            "genre": tag.genre().as_deref(),
        }),
    };
    let mut artwork = Vec::new();
    let mut findings = Vec::new();
    for (index, picture) in tag.pictures().iter().enumerate() {
        let size = picture.data().len();
        if size > max_artwork_bytes {
            findings.push(Finding {
                code: "artwork.embedded_oversized".to_owned(),
                message: "embedded artwork exceeds the configured observation bound".to_owned(),
                severity: FindingSeverity::Warning,
                file: Some(path.display().to_string()),
                evidence: serde_json::json!({
                    "index": index,
                    "size_bytes": size,
                    "max_bytes": max_artwork_bytes,
                }),
            });
            continue;
        }
        let (width, height) = image_dimensions(picture.data()).unwrap_or((0, 0));
        artwork.push(ObservedArtwork {
            relative_path: format!("{}#embedded-{index}", path.display()),
            source: "embedded".to_owned(),
            width: Some(width),
            height: Some(height),
            size_bytes: u64::try_from(size).unwrap_or(u64::MAX),
            content_hash: Some(blake3::hash(picture.data()).to_hex().to_string()),
            mime_type: picture.mime_type().map(ToString::to_string),
            is_embedded: true,
            picture_type: Some(format!("{:?}", picture.pic_type())),
        });
    }
    Ok((metadata, artwork, findings))
}

fn artwork_from_external(file: &ObservedFile) -> ObservedArtwork {
    ObservedArtwork {
        relative_path: file.relative_path.clone(),
        source: "external".to_owned(),
        width: None,
        height: None,
        size_bytes: file.size_bytes,
        content_hash: file.content_hash.clone(),
        mime_type: file.mime_type.clone(),
        is_embedded: false,
        picture_type: None,
    }
}

#[derive(Debug)]
struct ProbeResult {
    audio: AudioTechnicalObservation,
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    index: Option<u32>,
    codec_type: Option<String>,
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    bits_per_sample: Option<u32>,
    bit_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

fn run_ffprobe(path: &Path, options: &InspectionOptions) -> Result<ProbeResult, String> {
    let mut child = Command::new(&options.ffprobe_path)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("unable to start ffprobe: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffprobe stdout was not captured".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "ffprobe stderr was not captured".to_owned())?;
    let max_output = options.max_tool_output_bytes;
    let stdout_reader = thread::spawn(move || read_limited(stdout, max_output));
    let stderr_reader = thread::spawn(move || read_limited(stderr, max_output));

    let deadline = Instant::now() + options.ffprobe_timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("ffprobe timed out".to_owned());
            }
            Ok(None) => thread::sleep(StdDuration::from_millis(10)),
            Err(error) => break Err(format!("unable to wait for ffprobe: {error}")),
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "ffprobe stdout reader failed".to_owned())?
        .map_err(|error| format!("unable to read ffprobe stdout: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "ffprobe stderr reader failed".to_owned())?
        .map_err(|error| format!("unable to read ffprobe stderr: {error}"))?;
    let status = status?;
    if !status.success() {
        return Err(format!(
            "ffprobe exited with {}: {}",
            status,
            truncate_bytes(&stderr)
        ));
    }
    let document = serde_json::from_slice::<ProbeDocument>(&stdout)
        .map_err(|error| format!("invalid ffprobe JSON: {error}"))?;
    let stream = document
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"))
        .ok_or_else(|| "ffprobe reported no audio stream".to_owned())?;
    let duration = stream
        .duration
        .as_deref()
        .or(document.format.duration.as_deref())
        .and_then(parse_milliseconds);
    let bitrate = stream
        .bit_rate
        .as_deref()
        .or(document.format.bit_rate.as_deref())
        .and_then(|value| value.parse::<u64>().ok());
    Ok(ProbeResult {
        audio: AudioTechnicalObservation {
            container: document.format.format_name,
            codec: stream.codec_name.clone(),
            sample_rate_hz: stream.sample_rate.as_deref().and_then(|v| v.parse().ok()),
            channels: stream.channels,
            channel_layout: stream.channel_layout.clone(),
            bit_depth: stream.bits_per_sample,
            bitrate_bps: bitrate,
            stream_count: u32::try_from(document.streams.len()).ok(),
            duration_ms: duration,
            stream_index: stream.index,
        },
        mime_type: None,
    })
}

fn tool_version(path: &Path, max_output: usize) -> Option<String> {
    let mut child = Command::new(path)
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let stdout_reader = thread::spawn(move || read_limited(stdout, max_output));
    let stderr_reader = thread::spawn(move || read_limited(stderr, max_output));
    let deadline = Instant::now() + StdDuration::from_secs(2);
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => thread::sleep(StdDuration::from_millis(10)),
        }
    };
    let stdout = stdout_reader.join().ok()?.ok()?;
    let _stderr = stderr_reader.join().ok()?.ok()?;
    if !status.success() {
        return None;
    }
    let version_output = truncate_bytes(&stdout);
    let first_line = version_output.lines().next()?.trim();
    if first_line.is_empty() {
        None
    } else {
        Some(first_line.to_owned())
    }
}

fn read_limited<R: Read>(mut reader: R, limit: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(output);
        }
        if output.len() < limit {
            let remaining = limit - output.len();
            output.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
}

fn parse_milliseconds(value: &str) -> Option<u64> {
    let seconds = value.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let millis = (seconds * 1000.0).round();
    if millis > u64::MAX as f64 {
        return None;
    }
    Some(millis as u64)
}

fn inspect_image_header(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    image_dimensions(&bytes)
        .map(|_| ())
        .ok_or_else(|| "unsupported or malformed image header".to_owned())
}

fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((width, height));
    }
    if bytes.starts_with(b"GIF8") && bytes.len() >= 10 {
        let width = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
        let height = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
        return Some((width, height));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        return jpeg_dimensions(bytes);
    }
    None
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut index = 2_usize;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        index += 2;
        if marker == 0xd8 || marker == 0xd9 {
            continue;
        }
        let length = usize::from(u16::from_be_bytes(
            bytes.get(index..index + 2)?.try_into().ok()?,
        ));
        if length < 2 || index + length > bytes.len() {
            return None;
        }
        if (0xc0..=0xc3).contains(&marker)
            || (0xc5..=0xc7).contains(&marker)
            || (0xc9..=0xcb).contains(&marker)
            || (0xcd..=0xcf).contains(&marker)
        {
            let height = u32::from(u16::from_be_bytes(
                bytes.get(index + 3..index + 5)?.try_into().ok()?,
            ));
            let width = u32::from(u16::from_be_bytes(
                bytes.get(index + 5..index + 7)?.try_into().ok()?,
            ));
            return Some((width, height));
        }
        index += length;
    }
    None
}

fn push_unique(values: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty())
        && !values.iter().any(|existing| existing == &value)
    {
        values.push(value);
    }
}

fn path_to_posix(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn truncate_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).into_owned()
}

fn truncate_message(message: &str) -> String {
    if message.len() > 512 {
        format!("{}…", &message[..512])
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("melodee-media-{name}-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).expect("temp directory");
        path
    }

    #[test]
    fn classification_preserves_control_and_sidecars() {
        assert_eq!(classify(".melodee-ready"), ObservedFileKind::MelodeeControl);
        assert_eq!(classify("disc/cover.JPG"), ObservedFileKind::Artwork);
        assert_eq!(classify("disc/list.m3u"), ObservedFileKind::Playlist);
        assert_eq!(classify("disc/info.nfo"), ObservedFileKind::Nfo);
        assert_eq!(classify("disc/track.flac"), ObservedFileKind::Audio);
        assert_eq!(classify("disc/readme.txt"), ObservedFileKind::Unknown);
    }

    #[test]
    fn byte_hash_distinguishes_same_size_content() {
        let root = temp_dir("hash");
        let left = root.join("left.bin");
        let right = root.join("right.bin");
        fs::write(&left, b"abcd").expect("left");
        fs::write(&right, b"abce").expect("right");
        assert_ne!(hash_file(&left).unwrap().0, hash_file(&right).unwrap().0);
    }

    #[test]
    fn manifest_hash_excludes_volatile_provenance() {
        let first = ReleaseManifest::new(
            Uuid::now_v7(),
            None,
            "alpha".to_owned(),
            Some(1),
            Some("snapshot-a".to_owned()),
            Some("ffprobe".to_owned()),
            Some("/usr/bin/ffprobe".to_owned()),
            Some(0),
            Some(30_000),
            true,
            ManifestReleaseObservation {
                title: Some("Title".to_owned()),
                artists: vec!["Artist".to_owned()],
                date: None,
                source_labels: serde_json::json!({}),
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let second = ReleaseManifest::new(
            Uuid::now_v7(),
            None,
            "other".to_owned(),
            Some(2),
            Some("snapshot-b".to_owned()),
            Some("other".to_owned()),
            Some("other".to_owned()),
            Some(0),
            Some(1),
            true,
            first.release_observation.clone(),
            first.files.clone(),
            first.tracks.clone(),
            first.artwork.clone(),
            first.sidecars.clone(),
            first.findings.clone(),
        );
        assert_eq!(
            hash_manifest(&first).unwrap(),
            hash_manifest(&second).unwrap()
        );
    }

    #[test]
    fn valid_tagless_wav_is_inspected_as_audio() {
        let root = temp_dir("tagless-wav");
        let wav = root.join("track.wav");
        let sample_rate = 8_000_u32;
        let sample_count = 800_u32;
        let data_size = sample_count * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0_u8, data_size as usize));
        fs::write(&wav, bytes).expect("write wav");

        let report = inspect_release(
            Uuid::now_v7(),
            None,
            "tagless",
            &root,
            Some(1),
            Some("snapshot"),
            &InspectionOptions::default(),
        )
        .expect("inspect wav");
        assert!(report.structurally_valid);
        assert_eq!(1, report.manifest.tracks.len());
        assert!(report.manifest.tracks[0].observed_metadata.is_none());
        assert!(report.manifest.tracks[0].duration_ms.is_some());
        assert!(report.manifest.files[0].content_hash.is_some());
    }

    #[test]
    fn corrupt_audio_is_an_explicit_structural_finding_and_sidecars_are_inventory() {
        let root = temp_dir("corrupt-sidecars");
        fs::write(root.join("broken.mp3"), b"not an mp3").expect("broken audio");
        for sidecar in ["disc.cue", "disc.nfo", "disc.m3u", "disc.sfv", "disc.lrc"] {
            fs::write(root.join(sidecar), b"sidecar").expect("sidecar");
        }
        fs::write(root.join("README.txt"), b"unknown").expect("unknown");

        let report = inspect_release(
            Uuid::now_v7(),
            None,
            "corrupt",
            &root,
            None,
            None,
            &InspectionOptions::default(),
        )
        .expect("inspection report");
        assert!(!report.structurally_valid);
        assert!(
            report
                .manifest
                .findings
                .iter()
                .any(|finding| finding.code == "media.ffprobe_failed")
        );
        assert_eq!(5, report.manifest.sidecars.len());
        assert!(
            report
                .manifest
                .files
                .iter()
                .any(|file| file.kind == ObservedFileKind::Unknown)
        );
    }

    #[test]
    fn ffprobe_format_matrix_covers_mp3_flac_ogg_and_m4a() {
        let root = temp_dir("format-matrix");
        let cases = [
            ("mp3", "libmp3lame", "audio/mpeg"),
            ("flac", "flac", "audio/flac"),
            ("ogg", "libvorbis", "audio/ogg"),
            ("m4a", "aac", "audio/mp4"),
            ("opus", "libopus", "audio/ogg"),
            ("aac", "aac", "audio/aac"),
            ("ac3", "ac3", "audio/ac3"),
            ("wma", "wmav2", "audio/x-ms-wma"),
        ];
        for (extension, codec, expected_mime) in cases {
            let case_root = root.join(extension);
            fs::create_dir_all(&case_root).expect("case root");
            let mut command = std::process::Command::new("ffmpeg");
            command.args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2:sample_rate=8000",
                "-c:a",
                codec,
            ]);
            if extension == "aac" {
                command.args(["-f", "adts"]);
            } else if extension == "ac3" {
                command.args(["-f", "ac3"]);
            }
            if extension == "mp3" {
                command.args([
                    "-metadata",
                    "artist=Unicode 東京",
                    "-metadata",
                    "album=Observed Release",
                    "-metadata",
                    "title=First Light",
                    "-metadata",
                    "track=1/1",
                ]);
            }
            let output = command
                .arg(case_root.join(format!("track.{extension}")))
                .output()
                .expect("ffmpeg");
            assert!(output.status.success(), "ffmpeg failed for {extension}");
            let report = inspect_release(
                Uuid::now_v7(),
                None,
                extension,
                &case_root,
                None,
                None,
                &InspectionOptions::default(),
            )
            .expect("inspect generated media");
            assert!(report.structurally_valid, "{extension} should be valid");
            assert_eq!(
                Some(expected_mime.to_owned()),
                report.manifest.files[0].mime_type
            );
            assert!(report.manifest.files[0].audio.is_some());
            if extension == "mp3" {
                let metadata = report.manifest.files[0]
                    .metadata
                    .as_ref()
                    .expect("native metadata");
                assert_eq!(Some("Unicode 東京".to_owned()), metadata.artist);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn ffprobe_timeout_becomes_a_structured_finding() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("probe-timeout");
        let wav = root.join("track.wav");
        let sample_rate = 8_000_u32;
        let data_size = 160_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0_u8, data_size as usize));
        fs::write(&wav, bytes).expect("wav");
        let wrapper = root.join("slow-ffprobe.sh");
        fs::write(&wrapper, b"#!/bin/sh\nsleep 1\n").expect("wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("permissions");
        let options = InspectionOptions {
            ffprobe_path: wrapper,
            ffprobe_timeout: StdDuration::from_millis(20),
            ..InspectionOptions::default()
        };
        let report = inspect_release(Uuid::now_v7(), None, "timeout", &root, None, None, &options)
            .expect("timeout report");
        assert!(!report.structurally_valid);
        assert!(
            report
                .manifest
                .findings
                .iter()
                .any(|finding| finding.code == "media.ffprobe_timeout")
        );
    }

    #[test]
    #[ignore = "evidence harness; run explicitly with --ignored --nocapture"]
    fn phase1c_performance_evidence() {
        let root = temp_dir("performance");
        let source_wav = root.join("source.wav");
        let sample_rate = 8_000_u32;
        let data_size = 8_000_u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend(std::iter::repeat_n(0_u8, data_size as usize));
        fs::write(&source_wav, wav).expect("source wav");

        let cases = [
            ("small-wav", 1_usize),
            ("large-track-count", 20),
            ("artwork-heavy", 1),
        ];
        for (name, count) in cases {
            let case_root = root.join(name);
            fs::create_dir_all(&case_root).expect("case root");
            for index in 0..count {
                fs::copy(&source_wav, case_root.join(format!("track-{index:02}.wav")))
                    .expect("copy wav");
            }
            if name == "artwork-heavy" {
                let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
                png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
                png.extend_from_slice(&1_u32.to_be_bytes());
                png.extend_from_slice(&1_u32.to_be_bytes());
                png.extend_from_slice(&[8, 2, 0, 0, 0]);
                fs::write(case_root.join("cover.png"), png).expect("artwork");
            }
            let started = Instant::now();
            let report = inspect_release(
                Uuid::now_v7(),
                None,
                name,
                &case_root,
                None,
                None,
                &InspectionOptions::default(),
            )
            .expect("benchmark inspection");
            let manifest_size = serde_json::to_vec(&report.manifest)
                .expect("manifest json")
                .len();
            println!(
                "benchmark={{\"case\":\"{name}\",\"files\":{},\"bytes_hashed\":{},\"ffprobe_invocations\":{},\"elapsed_ms\":{},\"manifest_bytes\":{}}}",
                report.file_count,
                report.bytes_hashed,
                report.ffprobe_invocations,
                started.elapsed().as_millis(),
                manifest_size,
            );
        }
    }
}

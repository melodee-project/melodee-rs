//! Stable versioned manifest models used by later inspection/normalization phases.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Current manifest contract version.
pub const CURRENT_MANIFEST_VERSION: u16 = 1;

/// Canonical MP3 normalization profile version (PRD-MEDIA-002).
/// Vorbis-comment-free, schema-stable identifier for the exact FFmpeg
/// arguments that define canonical MP3 output.
pub const CANONICAL_MP3_PROFILE_VERSION: &str = "mp3-vbr-q4-384k-48k";

/// Structured finding/severity for inspection outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Error,
}

/// Canonical machine codes for stable interpretation in later stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub code: String,
    pub message: String,
    pub severity: FindingSeverity,
    pub file: Option<String>,
    pub evidence: serde_json::Value,
}

/// Classification of file contents discovered in a Release directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedFileKind {
    Audio,
    Artwork,
    CueSheet,
    Playlist,
    Nfo,
    Checksum,
    Lyrics,
    Sidecar,
    MelodeeControl,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTechnicalObservation {
    pub container: Option<String>,
    pub codec: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub bit_depth: Option<u32>,
    pub bitrate_bps: Option<u64>,
    pub stream_count: Option<u32>,
    pub duration_ms: Option<u64>,
    pub stream_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedMetadata {
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_title: Option<String>,
    pub artist: Option<String>,
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_total: Option<u32>,
    pub year: Option<String>,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedFile {
    pub relative_path: String,
    pub kind: ObservedFileKind,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
    pub content_hash: Option<String>,
    pub mime_type: Option<String>,
    pub kind_code: Option<String>,
    pub metadata: Option<ObservedMetadata>,
    pub audio: Option<AudioTechnicalObservation>,
    pub findings: Vec<Finding>,
    pub embedded_artwork: Vec<ObservedArtwork>,
    pub sidecar_type: Option<String>,
    pub bytes_read_errors: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedArtwork {
    pub relative_path: String,
    pub source: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: u64,
    pub content_hash: Option<String>,
    pub mime_type: Option<String>,
    pub is_embedded: bool,
    pub picture_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedTrack {
    pub file_path: String,
    pub track_number: Option<u32>,
    pub track_title: Option<String>,
    pub media_number: Option<u32>,
    pub sort_order: Option<u32>,
    pub byte_hash: Option<String>,
    pub duration_ms: Option<u64>,
    pub observed_metadata: Option<ObservedMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestReleaseObservation {
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub date: Option<String>,
    pub source_labels: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestInspectionContext {
    pub generated_at: OffsetDateTime,
    pub source_snapshot_id: Option<i64>,
    pub source_signature: Option<String>,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub ffprobe_version: Option<String>,
    pub ffprobe_path: Option<String>,
    pub ffprobe_exit_code: Option<i32>,
    pub timeout_ms: Option<u64>,
    pub source_stable: bool,
}

/// Per-file normalized output produced by the Phase 3.2 normalizer.
///
/// All paths are relative to the work-area staging directory; they are NOT
/// canonical storage paths (Phase 3.3 publishes canonical paths).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedOutput {
    /// Source-relative path of the original audio file.
    pub source_file: String,
    /// Work-area relative path of the normalized/canonical output.
    pub output_path: String,
    /// BLAKE3 hex digest of the normalized output bytes.
    pub byte_hash: Option<String>,
    /// Canonical container/codec label (e.g. "mp3", "lame").
    pub container: String,
    /// Canonical codec name from ffprobe of the output.
    pub codec: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub bitrate_bps: Option<u64>,
    pub duration_ms: Option<u64>,
    /// True when the source was already canonical MP3 and was copied, not re-encoded.
    pub retained: bool,
    pub findings: Vec<Finding>,
}

/// Acoustic fingerprint observation for one audio file (PRD-FP-003).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FingerprintObservation {
    /// Source-relative path of the audio file that was fingerprinted.
    pub source_file: String,
    /// Chromaprint fingerprint (Base64 variant), as emitted by `fpcalc -plain`.
    pub fingerprint: String,
    /// Duration in milliseconds from fpcalc `DURATION`.
    pub duration_ms: Option<u64>,
    pub findings: Vec<Finding>,
}

/// Normalized artwork in the work area.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedArtwork {
    /// Where the artwork originated: "embedded" or "external".
    pub source: String,
    /// Source-relative path of the original file (or embedded pseudo-path).
    pub source_file: String,
    /// Work-area relative path of the normalized image.
    pub output_path: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    /// BLAKE3 hex digest of the normalized image bytes.
    pub byte_hash: String,
}

/// Parsed semantic contents of a sidecar file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParsedSidecar {
    /// Sidecar type label: "cue", "nfo", "m3u", "sfv", "lrc".
    pub sidecar_type: String,
    /// Source-relative path.
    pub relative_path: String,
    /// Structured parse result.
    pub parsed: serde_json::Value,
    pub findings: Vec<Finding>,
}

/// Volatile execution provenance for the normalization stage.
///
/// Excluded from the manifest semantic hash: tool paths/versions, worker
/// identity, timestamps, and the work-area path. These describe HOW
/// normalization ran, not WHAT was observed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NormalizationExecutionProvenance {
    pub worker_id: Option<String>,
    pub normalized_at: Option<OffsetDateTime>,
    pub elapsed_ms: Option<u64>,
    pub ffmpeg_path: Option<String>,
    pub ffmpeg_version: Option<String>,
    pub ffmpeg_exit_code: Option<i32>,
    pub fpcalc_path: Option<String>,
    pub fpcalc_version: Option<String>,
    pub work_area: Option<String>,
}

/// Normalization and fingerprint observations for a Release (Phase 3.2).
///
/// `provenance` is volatile execution metadata excluded from the semantic
/// hash; the rest are stable media observations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NormalizationObservations {
    pub profile_version: String,
    pub outputs: Vec<NormalizedOutput>,
    pub fingerprints: Vec<FingerprintObservation>,
    pub normalized_artwork: Vec<NormalizedArtwork>,
    pub parsed_sidecars: Vec<ParsedSidecar>,
    pub provenance: NormalizationExecutionProvenance,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub schema_version: u16,
    pub manifest_id: Uuid,
    pub work_item_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub source_snapshot_id: Option<i64>,
    pub release_observation: ManifestReleaseObservation,
    pub files: Vec<ObservedFile>,
    pub tracks: Vec<ObservedTrack>,
    pub artwork: Vec<ObservedArtwork>,
    pub sidecars: Vec<ObservedFile>,
    pub findings: Vec<Finding>,
    pub inspection: ManifestInspectionContext,
    /// Phase 3.2 normalization observations. `None` until normalization
    /// completes; populated atomically with the Normalizing→Validating transition.
    #[serde(default)]
    pub normalization: Option<NormalizationObservations>,
}

impl ReleaseManifest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        work_item_id: Uuid,
        source_library_id: Option<Uuid>,
        source_path: String,
        source_snapshot_id: Option<i64>,
        source_signature: Option<String>,
        ffprobe_version: Option<String>,
        ffprobe_path: Option<String>,
        ffprobe_exit_code: Option<i32>,
        timeout_ms: Option<u64>,
        source_stable: bool,
        release_observation: ManifestReleaseObservation,
        files: Vec<ObservedFile>,
        tracks: Vec<ObservedTrack>,
        artwork: Vec<ObservedArtwork>,
        sidecars: Vec<ObservedFile>,
        findings: Vec<Finding>,
    ) -> Self {
        let source_path_for_context = source_path.clone();
        Self {
            schema_version: CURRENT_MANIFEST_VERSION,
            manifest_id: Uuid::now_v7(),
            work_item_id,
            source_library_id,
            source_path,
            source_snapshot_id,
            release_observation,
            files,
            tracks,
            artwork,
            sidecars,
            findings,
            normalization: None,
            inspection: ManifestInspectionContext {
                generated_at: OffsetDateTime::now_utc(),
                source_snapshot_id,
                source_signature,
                source_library_id,
                source_path: source_path_for_context,
                ffprobe_version,
                ffprobe_path,
                ffprobe_exit_code,
                timeout_ms,
                source_stable,
            },
        }
    }

    /// Returns the stable semantic portion used for the manifest integrity
    /// hash. Database ids, source provenance, tool versions, and generation
    /// time are deliberately excluded: they describe this observation but do
    /// not change the observed media facts. Normalization execution provenance
    /// (`normalization.provenance`) is likewise excluded because it describes
    /// how normalization ran, not what audio was observed.
    pub fn semantic_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        if let Some(object) = value.as_object_mut() {
            object.remove("manifest_id");
            object.remove("work_item_id");
            object.remove("source_library_id");
            object.remove("source_path");
            object.remove("source_snapshot_id");
            object.remove("inspection");
            if let Some(normalization) = object
                .get_mut("normalization")
                .and_then(|v| v.as_object_mut())
            {
                normalization.remove("provenance");
            }
        }
        Ok(value)
    }
}

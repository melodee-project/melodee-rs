//! Stable versioned manifest models used by later inspection/normalization phases.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Current manifest contract version.
pub const CURRENT_MANIFEST_VERSION: u16 = 1;

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
    /// not change the observed media facts.
    pub fn semantic_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        if let Some(object) = value.as_object_mut() {
            object.remove("manifest_id");
            object.remove("work_item_id");
            object.remove("source_library_id");
            object.remove("source_path");
            object.remove("source_snapshot_id");
            object.remove("inspection");
        }
        Ok(value)
    }
}

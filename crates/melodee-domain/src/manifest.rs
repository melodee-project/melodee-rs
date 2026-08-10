//! Stable versioned manifest models used by later inspection/normalization phases.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ReleaseId;

/// Normalization-stage manifest contract version.
///
/// Version 1 is the Phase 3.1 inspection-only contract. Version 2 adds the
/// semantic normalization observations produced by Phase 3.2. The optional
/// field remains deserializable on v1 payloads so stored inspection manifests
/// can be upgraded in place by the normalization worker. Normalization keeps
/// writing v2; canonical publication upgrades the durable copy to v3.
pub const NORMALIZATION_MANIFEST_VERSION: u16 = 2;

/// Backward-compatible name used by the Phase 3.2 normalizer.
pub const CURRENT_MANIFEST_VERSION: u16 = NORMALIZATION_MANIFEST_VERSION;

/// Inspection-only manifest version written by Phase 3.1.
pub const INSPECTION_MANIFEST_VERSION: u16 = 1;

/// Canonical-publication manifest version written by Phase 3.3.
pub const PUBLICATION_MANIFEST_VERSION: u16 = 3;

/// Canonical layout version fixed by ADR-0116.
pub const CANONICAL_LAYOUT_VERSION: u16 = 1;

/// Canonical MP3 normalization profile version (PRD-MEDIA-002).
/// Schema-stable identifier for the exact FFmpeg arguments that define
/// converted MP3 output.
pub const CANONICAL_MP3_PROFILE_VERSION: &str = "mp3-libmp3lame-vbr-q4-48khz-v1";

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
    pub byte_hash: String,
    /// Canonical container/codec label (e.g. "mp3", "lame").
    pub container: String,
    /// Canonical codec name from ffprobe of the output.
    pub codec: String,
    pub sample_rate_hz: u32,
    pub channels: u32,
    pub bitrate_bps: Option<u64>,
    pub duration_ms: u64,
    /// True when the source was already canonical MP3 and was copied, not re-encoded.
    pub retained: bool,
    pub findings: Vec<Finding>,
}

/// Acoustic fingerprint observation for one audio file (PRD-FP-003).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FingerprintObservation {
    /// Source-relative path of the audio file that was fingerprinted.
    pub source_file: String,
    /// Chromaprint fingerprint as emitted by `fpcalc -json`.
    pub fingerprint: String,
    /// Duration in milliseconds from fpcalc `DURATION`.
    pub duration_ms: u64,
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
    pub attempt_id: Option<Uuid>,
    pub worker_id: Option<String>,
    pub normalized_at: Option<OffsetDateTime>,
    pub elapsed_ms: Option<u64>,
    pub ffmpeg_path: Option<String>,
    pub ffmpeg_version: Option<String>,
    pub ffmpeg_exit_code: Option<i32>,
    pub fpcalc_path: Option<String>,
    pub fpcalc_version: Option<String>,
    pub fpcalc_exit_code: Option<i32>,
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

/// Result of structural/media validation before canonical publication.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationOutcome {
    Passed,
    NeedsReview,
}

/// Versioned structural validation evidence. This is deliberately narrower
/// than the Phase 5 classification and policy engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuralValidation {
    pub profile_name: String,
    pub profile_version: u16,
    pub outcome: ValidationOutcome,
    pub findings: Vec<Finding>,
}

/// Durable reference to the narrow Phase 3 publication authorization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationAuthorization {
    pub id: Uuid,
    pub name: String,
    pub version: u16,
}

impl PublicationAuthorization {
    pub const PHASE3_STRUCTURAL_NAME: &'static str = "phase3.structural-publication";
    pub const PHASE3_STRUCTURAL_VERSION: u16 = 1;

    /// Creates the non-destructive authorization fixed by ADR-0116.
    #[must_use]
    pub fn phase3_structural() -> Self {
        Self {
            id: Uuid::now_v7(),
            name: Self::PHASE3_STRUCTURAL_NAME.to_owned(),
            version: Self::PHASE3_STRUCTURAL_VERSION,
        }
    }

    #[must_use]
    pub fn is_phase3_structural(&self) -> bool {
        self.name == Self::PHASE3_STRUCTURAL_NAME && self.version == Self::PHASE3_STRUCTURAL_VERSION
    }
}

/// Disposition of the inbound source after Phase 3 publication.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceDisposition {
    Retained,
}

/// Physical role of one canonical publication asset.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalAssetKind {
    Media,
    Artwork,
}

/// Durable, idempotent milestones in the convergent publication protocol.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicationJournalStep {
    IncomingPrepared,
    AssetCopied,
    BundleSealed,
    Finalized,
    FinalAdopted,
    TargetConflict,
    Failed,
}

impl PublicationJournalStep {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IncomingPrepared => "incoming_prepared",
            Self::AssetCopied => "asset_copied",
            Self::BundleSealed => "bundle_sealed",
            Self::Finalized => "finalized",
            Self::FinalAdopted => "final_adopted",
            Self::TargetConflict => "target_conflict",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        const ALL: [PublicationJournalStep; 7] = [
            PublicationJournalStep::IncomingPrepared,
            PublicationJournalStep::AssetCopied,
            PublicationJournalStep::BundleSealed,
            PublicationJournalStep::Finalized,
            PublicationJournalStep::FinalAdopted,
            PublicationJournalStep::TargetConflict,
            PublicationJournalStep::Failed,
        ];
        ALL.into_iter().find(|step| step.as_str() == value)
    }
}

/// Mapping from an inbound/work-area logical asset to canonical Release bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CanonicalAsset {
    pub kind: CanonicalAssetKind,
    pub source_path: String,
    pub normalized_path: String,
    /// Path relative to the canonical Release directory.
    pub canonical_path: String,
    pub size_bytes: u64,
    /// Lowercase BLAKE3 hex digest of the canonical bytes.
    pub byte_hash: String,
}

/// Publication facts added to a v3 Release manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestPublication {
    pub release_id: ReleaseId,
    pub validation: StructuralValidation,
    pub authorization: PublicationAuthorization,
    pub source_disposition: SourceDisposition,
    pub layout_version: u16,
    pub plan_hash: String,
    pub canonical_assets: Vec<CanonicalAsset>,
}

/// Immutable intent persisted before any canonical target is modified.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationPlan {
    pub layout_version: u16,
    pub release_id: ReleaseId,
    pub input_manifest_hash: String,
    pub authorization: PublicationAuthorization,
    /// Storage-root-relative canonical Release directory.
    pub target_relative_path: String,
    /// Storage-root-relative path of the final v3 manifest.
    pub manifest_relative_path: String,
    pub assets: Vec<CanonicalAsset>,
}

/// Pure publication/manifest invariant failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PublicationInvariantError {
    #[error("manifest schema version {0} cannot be upgraded to publication v3")]
    UnsupportedManifestVersion(u16),
    #[error("publication v3 requires a completed normalization v2 manifest")]
    MissingNormalization,
    #[error("publication requires a passed structural validation")]
    ValidationDidNotPass,
    #[error("structural validation profile/result is internally inconsistent")]
    InvalidValidation,
    #[error("publication authorization is not phase3.structural-publication version 1")]
    InvalidAuthorization,
    #[error("canonical layout version must be 1")]
    InvalidLayoutVersion,
    #[error("canonical publication requires a UUIDv7 Release ID")]
    InvalidReleaseId,
    #[error("publication path is not a safe portable relative path: {0}")]
    UnsafePath(String),
    #[error("canonical path must contain only lowercase ASCII: {0}")]
    NonCanonicalPath(String),
    #[error("BLAKE3 hash is not 64 lowercase hexadecimal characters: {0}")]
    InvalidBlake3(String),
    #[error("canonical asset inventory is not in deterministic order")]
    UnsortedAssets,
    #[error("canonical path {0} maps to conflicting bytes")]
    ConflictingAsset(String),
    #[error("publication facts do not match their immutable plan")]
    PlanMismatch,
    #[error("publication plan could not be serialized for hashing")]
    PlanSerialization,
}

impl PublicationPlan {
    /// Computes ADR-0116's stable storage-root-relative directory from the
    /// immutable Release ID.
    #[must_use]
    pub fn canonical_target_path(release_id: ReleaseId) -> String {
        let release_uuid = release_id.into_uuid();
        let digest = blake3::hash(release_uuid.as_bytes());
        format!(
            "releases/{:02x}/{:02x}/rel_{}",
            digest.as_bytes()[0],
            digest.as_bytes()[1],
            release_uuid.simple()
        )
    }

    /// Computes the deterministic BLAKE3 identity of the complete immutable
    /// plan. The plan has no self-hash field, so no recursive exclusion is
    /// necessary.
    pub fn plan_hash(&self) -> Result<String, serde_json::Error> {
        serde_json::to_vec(self).map(|bytes| blake3::hash(&bytes).to_hex().to_string())
    }

    /// Validates the immutable plan before it crosses a persistence or
    /// filesystem boundary.
    pub fn validate(&self) -> Result<(), PublicationInvariantError> {
        if self.release_id.into_uuid().get_version_num() != 7 {
            return Err(PublicationInvariantError::InvalidReleaseId);
        }
        validate_publication_parts(self.layout_version, &self.authorization, &self.assets)?;
        validate_portable_relative_path(&self.target_relative_path)?;
        validate_portable_relative_path(&self.manifest_relative_path)?;
        if !is_canonical_ascii_path(&self.target_relative_path)
            || !is_canonical_ascii_path(&self.manifest_relative_path)
        {
            return Err(PublicationInvariantError::NonCanonicalPath(
                self.target_relative_path.clone(),
            ));
        }
        if self.target_relative_path != Self::canonical_target_path(self.release_id) {
            return Err(PublicationInvariantError::PlanMismatch);
        }
        if self.manifest_relative_path != format!("{}/manifest.json", self.target_relative_path) {
            return Err(PublicationInvariantError::PlanMismatch);
        }
        validate_blake3(&self.input_manifest_hash)?;
        Ok(())
    }
}

impl ManifestPublication {
    /// Validates v3 facts and their equality with the durable immutable plan.
    pub fn validate_against(
        &self,
        plan: &PublicationPlan,
    ) -> Result<(), PublicationInvariantError> {
        validate_publication_parts(
            self.layout_version,
            &self.authorization,
            &self.canonical_assets,
        )?;
        validate_blake3(&self.plan_hash)?;
        self.validation.validate()?;
        if self.validation.outcome != ValidationOutcome::Passed {
            return Err(PublicationInvariantError::ValidationDidNotPass);
        }
        if self.release_id != plan.release_id
            || self.layout_version != plan.layout_version
            || self.authorization != plan.authorization
            || self.canonical_assets != plan.assets
        {
            return Err(PublicationInvariantError::PlanMismatch);
        }
        plan.validate()?;
        let expected_plan_hash = plan
            .plan_hash()
            .map_err(|_| PublicationInvariantError::PlanSerialization)?;
        if self.plan_hash != expected_plan_hash {
            return Err(PublicationInvariantError::PlanMismatch);
        }
        Ok(())
    }
}

impl StructuralValidation {
    /// Ensures a passed result has a versioned profile and no error findings.
    pub fn validate(&self) -> Result<(), PublicationInvariantError> {
        if self.profile_name.is_empty()
            || self.profile_version == 0
            || (self.outcome == ValidationOutcome::Passed
                && self
                    .findings
                    .iter()
                    .any(|finding| finding.severity == FindingSeverity::Error))
        {
            Err(PublicationInvariantError::InvalidValidation)
        } else {
            Ok(())
        }
    }
}

fn validate_publication_parts(
    layout_version: u16,
    authorization: &PublicationAuthorization,
    assets: &[CanonicalAsset],
) -> Result<(), PublicationInvariantError> {
    if layout_version != CANONICAL_LAYOUT_VERSION {
        return Err(PublicationInvariantError::InvalidLayoutVersion);
    }
    if !authorization.is_phase3_structural() {
        return Err(PublicationInvariantError::InvalidAuthorization);
    }
    if !assets
        .iter()
        .any(|asset| asset.kind == CanonicalAssetKind::Media)
    {
        return Err(PublicationInvariantError::PlanMismatch);
    }
    for asset in assets {
        validate_portable_relative_path(&asset.source_path)?;
        validate_portable_relative_path(&asset.normalized_path)?;
        validate_portable_relative_path(&asset.canonical_path)?;
        if !is_canonical_ascii_path(&asset.canonical_path) {
            return Err(PublicationInvariantError::NonCanonicalPath(
                asset.canonical_path.clone(),
            ));
        }
        validate_blake3(&asset.byte_hash)?;
        let required_path = match asset.kind {
            CanonicalAssetKind::Media => format!("media/{}.mp3", asset.byte_hash),
            CanonicalAssetKind::Artwork => format!("artwork/{}.jpg", asset.byte_hash),
        };
        if asset.canonical_path != required_path {
            return Err(PublicationInvariantError::PlanMismatch);
        }
    }
    if assets
        .windows(2)
        .any(|pair| asset_sort_key(&pair[0]) > asset_sort_key(&pair[1]))
    {
        return Err(PublicationInvariantError::UnsortedAssets);
    }
    for pair in assets.windows(2) {
        if pair[0].canonical_path == pair[1].canonical_path
            && (pair[0].kind != pair[1].kind
                || pair[0].size_bytes != pair[1].size_bytes
                || pair[0].byte_hash != pair[1].byte_hash)
        {
            return Err(PublicationInvariantError::ConflictingAsset(
                pair[0].canonical_path.clone(),
            ));
        }
    }
    Ok(())
}

fn asset_sort_key(asset: &CanonicalAsset) -> (&str, CanonicalAssetKind, &str, &str) {
    (
        &asset.canonical_path,
        asset.kind,
        &asset.source_path,
        &asset.normalized_path,
    )
}

fn validate_portable_relative_path(path: &str) -> Result<(), PublicationInvariantError> {
    let safe = !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..");
    if safe {
        Ok(())
    } else {
        Err(PublicationInvariantError::UnsafePath(path.to_owned()))
    }
}

fn is_canonical_ascii_path(path: &str) -> bool {
    path.bytes().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'/' | b'_' | b'-' | b'.')
    })
}

fn validate_blake3(value: &str) -> Result<(), PublicationInvariantError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(PublicationInvariantError::InvalidBlake3(value.to_owned()))
    }
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
    /// Phase 3.3 canonical publication facts. Absent from readable v1/v2
    /// payloads and populated only when the durable publication plan is staged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<ManifestPublication>,
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
            schema_version: INSPECTION_MANIFEST_VERSION,
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
            publication: None,
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
            // Publication is operational intent/receipt, not a newly observed
            // media fact. Preserve the v2 semantic identity after v3 upgrade.
            if self.schema_version == PUBLICATION_MANIFEST_VERSION {
                object.insert(
                    "schema_version".to_owned(),
                    serde_json::Value::from(CURRENT_MANIFEST_VERSION),
                );
            }
            object.remove("manifest_id");
            object.remove("work_item_id");
            object.remove("source_library_id");
            object.remove("source_path");
            object.remove("source_snapshot_id");
            object.remove("inspection");
            object.remove("publication");
            if let Some(normalization) = object
                .get_mut("normalization")
                .and_then(|v| v.as_object_mut())
            {
                normalization.remove("provenance");
            }
        }
        Ok(value)
    }

    /// Explicitly upgrades a completed normalization v2 manifest to the
    /// publication v3 contract. Reapplying identical v3 facts is idempotent.
    pub fn upgrade_for_publication(
        &mut self,
        publication: ManifestPublication,
        plan: &PublicationPlan,
    ) -> Result<(), PublicationInvariantError> {
        publication.validate_against(plan)?;
        match self.schema_version {
            PUBLICATION_MANIFEST_VERSION => {
                if self.publication.as_ref() == Some(&publication) {
                    Ok(())
                } else {
                    Err(PublicationInvariantError::PlanMismatch)
                }
            }
            NORMALIZATION_MANIFEST_VERSION if self.normalization.is_some() => {
                if self.publication.is_some() {
                    return Err(PublicationInvariantError::PlanMismatch);
                }
                self.schema_version = PUBLICATION_MANIFEST_VERSION;
                self.publication = Some(publication);
                Ok(())
            }
            NORMALIZATION_MANIFEST_VERSION => Err(PublicationInvariantError::MissingNormalization),
            version => Err(PublicationInvariantError::UnsupportedManifestVersion(
                version,
            )),
        }
    }
}

#![allow(clippy::expect_used, clippy::unwrap_used)]

use melodee_domain::{
    CURRENT_MANIFEST_VERSION, FingerprintObservation, INSPECTION_MANIFEST_VERSION,
    NormalizationExecutionProvenance, NormalizationObservations, NormalizedOutput, ReleaseManifest,
};
use time::OffsetDateTime;
use uuid::Uuid;

const PHASE_3_1_MANIFEST: &str = include_str!("fixtures/phase3_1_manifest_v1.json");

#[test]
fn phase_3_1_v1_manifest_deserializes_without_normalization() {
    let manifest: ReleaseManifest = serde_json::from_str(PHASE_3_1_MANIFEST).unwrap();
    assert_eq!(manifest.schema_version, INSPECTION_MANIFEST_VERSION);
    assert!(manifest.normalization.is_none());
}

#[test]
fn phase_3_2_v2_manifest_round_trips_and_provenance_is_not_semantic() {
    let mut manifest: ReleaseManifest = serde_json::from_str(PHASE_3_1_MANIFEST).unwrap();
    manifest.schema_version = CURRENT_MANIFEST_VERSION;
    manifest.normalization = Some(NormalizationObservations {
        profile_version: "mp3-libmp3lame-vbr-q4-48khz-v1".to_owned(),
        outputs: vec![NormalizedOutput {
            source_file: "track.flac".to_owned(),
            output_path: "audio/track.flac.mp3".to_owned(),
            byte_hash: "abc".to_owned(),
            container: "mp3".to_owned(),
            codec: "mp3".to_owned(),
            sample_rate_hz: 48_000,
            channels: 2,
            bitrate_bps: Some(192_000),
            duration_ms: 10_000,
            retained: false,
            findings: Vec::new(),
        }],
        fingerprints: vec![FingerprintObservation {
            source_file: "track.flac".to_owned(),
            fingerprint: "fingerprint".to_owned(),
            duration_ms: 10_000,
            findings: Vec::new(),
        }],
        normalized_artwork: Vec::new(),
        parsed_sidecars: Vec::new(),
        provenance: NormalizationExecutionProvenance {
            attempt_id: Some(Uuid::now_v7()),
            normalized_at: Some(OffsetDateTime::now_utc()),
            worker_id: Some("worker-a".to_owned()),
            work_area: Some("/tmp/attempt-a".to_owned()),
            ..NormalizationExecutionProvenance::default()
        },
        findings: Vec::new(),
    });

    let serialized = serde_json::to_string(&manifest).unwrap();
    let round_trip: ReleaseManifest = serde_json::from_str(&serialized).unwrap();
    assert_eq!(manifest, round_trip);

    let semantic_before = manifest.semantic_value().unwrap();
    let provenance = &mut manifest.normalization.as_mut().unwrap().provenance;
    provenance.attempt_id = Some(Uuid::now_v7());
    provenance.worker_id = Some("worker-b".to_owned());
    provenance.work_area = Some("/tmp/attempt-b".to_owned());
    assert_eq!(semantic_before, manifest.semantic_value().unwrap());
}

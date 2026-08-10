#![allow(clippy::expect_used, clippy::unwrap_used)]

use melodee_domain::{
    CANONICAL_LAYOUT_VERSION, CURRENT_MANIFEST_VERSION, CanonicalAsset, CanonicalAssetKind,
    FingerprintObservation, INSPECTION_MANIFEST_VERSION, ManifestPublication,
    NormalizationExecutionProvenance, NormalizationObservations, NormalizedOutput,
    PUBLICATION_MANIFEST_VERSION, PublicationAuthorization, PublicationPlan, ReleaseId,
    ReleaseManifest, SourceDisposition, StructuralValidation, ValidationOutcome,
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

fn publication_contract(release_id: ReleaseId) -> (PublicationPlan, ManifestPublication) {
    let byte_hash = "a".repeat(64);
    let authorization = PublicationAuthorization {
        id: Uuid::parse_str("01989f00-7000-7000-8000-000000000002").unwrap(),
        name: PublicationAuthorization::PHASE3_STRUCTURAL_NAME.to_owned(),
        version: PublicationAuthorization::PHASE3_STRUCTURAL_VERSION,
    };
    let assets = vec![CanonicalAsset {
        kind: CanonicalAssetKind::Media,
        source_path: "disc 1/track.flac".to_owned(),
        normalized_path: "audio/track.flac.mp3".to_owned(),
        canonical_path: format!("media/{byte_hash}.mp3"),
        size_bytes: 42,
        byte_hash,
    }];
    let target_relative_path = PublicationPlan::canonical_target_path(release_id);
    let plan = PublicationPlan {
        layout_version: CANONICAL_LAYOUT_VERSION,
        release_id,
        input_manifest_hash: "b".repeat(64),
        authorization: authorization.clone(),
        manifest_relative_path: format!("{target_relative_path}/manifest.json"),
        target_relative_path,
        assets: assets.clone(),
    };
    let plan_hash = plan.plan_hash().unwrap();
    let publication = ManifestPublication {
        release_id,
        validation: StructuralValidation {
            profile_name: "phase3.structural".to_owned(),
            profile_version: 1,
            outcome: ValidationOutcome::Passed,
            findings: Vec::new(),
        },
        authorization,
        source_disposition: SourceDisposition::Retained,
        layout_version: CANONICAL_LAYOUT_VERSION,
        plan_hash,
        canonical_assets: assets,
    };
    (plan, publication)
}

#[test]
fn canonical_release_layout_has_a_stable_golden_vector() {
    let release_id =
        ReleaseId::from_uuid(Uuid::parse_str("01989f00-7000-7000-8000-000000000001").unwrap());
    assert_eq!(
        PublicationPlan::canonical_target_path(release_id),
        "releases/a2/49/rel_01989f00700070008000000000000001"
    );
}

#[test]
fn v2_upgrades_to_v3_without_changing_observed_semantic_value() {
    let mut manifest: ReleaseManifest = serde_json::from_str(PHASE_3_1_MANIFEST).unwrap();
    manifest.schema_version = CURRENT_MANIFEST_VERSION;
    manifest.normalization = Some(NormalizationObservations {
        profile_version: "mp3-libmp3lame-vbr-q4-48khz-v1".to_owned(),
        ..NormalizationObservations::default()
    });
    let semantic_v2 = manifest.semantic_value().unwrap();
    let release_id =
        ReleaseId::from_uuid(Uuid::parse_str("01989f00-7000-7000-8000-000000000001").unwrap());
    let (plan, publication) = publication_contract(release_id);

    manifest
        .upgrade_for_publication(publication.clone(), &plan)
        .unwrap();
    assert_eq!(manifest.schema_version, PUBLICATION_MANIFEST_VERSION);
    assert_eq!(manifest.publication.as_ref(), Some(&publication));
    assert_eq!(semantic_v2, manifest.semantic_value().unwrap());
    manifest
        .upgrade_for_publication(publication.clone(), &plan)
        .expect("identical v3 reapplication is idempotent");

    let payload = serde_json::to_string(&manifest).unwrap();
    let decoded: ReleaseManifest = serde_json::from_str(&payload).unwrap();
    assert_eq!(decoded, manifest);
}

#[test]
fn readable_v1_manifest_is_rejected_for_direct_publication() {
    let mut manifest: ReleaseManifest = serde_json::from_str(PHASE_3_1_MANIFEST).unwrap();
    let release_id =
        ReleaseId::from_uuid(Uuid::parse_str("01989f00-7000-7000-8000-000000000001").unwrap());
    let (plan, publication) = publication_contract(release_id);
    assert!(
        manifest
            .upgrade_for_publication(publication, &plan)
            .is_err(),
        "inspection-only v1 must normalize to v2 before publication"
    );
    assert_eq!(manifest.schema_version, INSPECTION_MANIFEST_VERSION);
    assert!(manifest.publication.is_none());
}

#[test]
fn publication_plan_rejects_noncanonical_or_mismatched_paths() {
    let release_id =
        ReleaseId::from_uuid(Uuid::parse_str("01989f00-7000-7000-8000-000000000001").unwrap());
    let (mut plan, _) = publication_contract(release_id);
    assert!(plan.validate().is_ok());

    plan.assets[0].canonical_path = format!("media/{}.flac", "a".repeat(64));
    assert!(plan.validate().is_err());

    let (mut plan, _) = publication_contract(release_id);
    plan.target_relative_path = "releases/éé/00/rel_bad".to_owned();
    assert!(plan.validate().is_err());
}

//! Phase 3.3 structural-validation and canonical-filesystem safety contracts.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use melodee_domain::{
    CURRENT_MANIFEST_VERSION, FingerprintObservation, ManifestReleaseObservation,
    NormalizationExecutionProvenance, NormalizationObservations, NormalizedOutput, ObservedFile,
    ObservedFileKind, ObservedTrack, PUBLICATION_MANIFEST_VERSION, PublicationAuthorization,
    ReleaseId, ReleaseManifest, ValidationOutcome,
};
use melodee_ingestion::{
    PublicationBundle, PublicationError, PublicationOutcome, StructuralValidationError,
    build_publication_bundle, publish_canonical_bundle, validate_normalized_manifest,
};
use uuid::Uuid;

const MANIFEST_V1: &str =
    include_str!("../../melodee-domain/tests/fixtures/phase3_1_manifest_v1.json");
const SOURCE_PATH: &str = "Disc Ω/-leading track.flac";
const OUTPUT_PATH: &str = "audio/Disc Ω/-leading track.flac.mp3";
const OUTPUT_BYTES: &[u8] = b"synthetic normalized mp3 bytes\0\x01\x02";

struct Fixture {
    root: PathBuf,
    work_area: PathBuf,
    storage: PathBuf,
    source: PathBuf,
    work_root: PathBuf,
    manifest: ReleaseManifest,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "melodee-publication-{label}-{}",
            Uuid::now_v7().simple()
        ));
        let work_area = root.join("work");
        let storage = root.join("storage");
        let source = root.join("source");
        fs::create_dir_all(&work_area).expect("work area");
        fs::create_dir_all(source.join("Disc Ω")).expect("source tree");
        fs::write(source.join(SOURCE_PATH), b"immutable inbound source bytes")
            .expect("source fixture");
        fs::write(source.join("notes Ω.nfo"), b"sidecar remains inbound").expect("source sidecar");

        let mut manifest: ReleaseManifest = serde_json::from_str(MANIFEST_V1).expect("manifest v1");
        manifest.schema_version = CURRENT_MANIFEST_VERSION;
        manifest.manifest_id = Uuid::now_v7();
        manifest.work_item_id = Uuid::now_v7();
        manifest.source_path = "Observed Artist/Observed Release".to_owned();
        manifest.release_observation = ManifestReleaseObservation {
            title: Some("Observed Release".to_owned()),
            artists: vec!["Observed Artist".to_owned()],
            date: Some("2026".to_owned()),
            source_labels: serde_json::json!({"pathology": "Unicode and leading dash"}),
        };
        let source_hash = blake3::hash(b"immutable inbound source bytes")
            .to_hex()
            .to_string();
        manifest.files = vec![ObservedFile {
            relative_path: SOURCE_PATH.to_owned(),
            kind: ObservedFileKind::Audio,
            size_bytes: u64::try_from(b"immutable inbound source bytes".len()).unwrap(),
            modified_at: None,
            content_hash: Some(source_hash.clone()),
            mime_type: Some("audio/flac".to_owned()),
            kind_code: Some("audio.flac".to_owned()),
            metadata: None,
            audio: None,
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors: false,
        }];
        manifest.tracks = vec![ObservedTrack {
            file_path: SOURCE_PATH.to_owned(),
            track_number: Some(1),
            track_title: Some("Leading Track".to_owned()),
            media_number: Some(1),
            sort_order: Some(1),
            byte_hash: Some(source_hash),
            duration_ms: Some(1_000),
            observed_metadata: None,
        }];

        let attempt_id = Uuid::now_v7();
        let work_root = work_area
            .join(manifest.work_item_id.simple().to_string())
            .join("attempts")
            .join(attempt_id.simple().to_string());
        fs::create_dir_all(work_root.join("audio/Disc Ω")).expect("attempt tree");
        fs::write(work_root.join(OUTPUT_PATH), OUTPUT_BYTES).expect("normalized output");
        let output_hash = blake3::hash(OUTPUT_BYTES).to_hex().to_string();
        manifest.normalization = Some(NormalizationObservations {
            profile_version: "mp3-libmp3lame-vbr-q4-48khz-v1".to_owned(),
            outputs: vec![NormalizedOutput {
                source_file: SOURCE_PATH.to_owned(),
                output_path: OUTPUT_PATH.to_owned(),
                byte_hash: output_hash,
                container: "mp3".to_owned(),
                codec: "mp3".to_owned(),
                sample_rate_hz: 48_000,
                channels: 2,
                bitrate_bps: Some(192_000),
                duration_ms: 1_000,
                retained: false,
                findings: Vec::new(),
            }],
            fingerprints: vec![FingerprintObservation {
                source_file: SOURCE_PATH.to_owned(),
                fingerprint: "synthetic-chromaprint".to_owned(),
                duration_ms: 1_000,
                findings: Vec::new(),
            }],
            normalized_artwork: Vec::new(),
            parsed_sidecars: Vec::new(),
            provenance: NormalizationExecutionProvenance {
                attempt_id: Some(attempt_id),
                worker_id: Some("test-worker".to_owned()),
                work_area: Some(work_root.to_string_lossy().into_owned()),
                ..NormalizationExecutionProvenance::default()
            },
            findings: Vec::new(),
        });

        Self {
            root,
            work_area,
            storage,
            source,
            work_root,
            manifest,
        }
    }

    fn validated(&self) -> melodee_ingestion::ValidatedNormalization {
        validate_normalized_manifest(&self.manifest, &self.work_area).expect("valid fixture")
    }

    fn bundle(&self) -> PublicationBundle {
        let input_hash = melodee_media::hash_manifest(&self.manifest).expect("semantic hash");
        build_publication_bundle(
            &self.manifest,
            &input_hash,
            ReleaseId::new(),
            PublicationAuthorization::phase3_structural(),
            self.validated(),
        )
        .expect("publication bundle")
    }

    fn source_snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        regular_file_snapshot(&self.source)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("melodee-publication-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn regular_file_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, current: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).expect("read fixture tree") {
            let path = entry.expect("fixture entry").path();
            let metadata = fs::symlink_metadata(&path).expect("fixture metadata");
            if metadata.is_dir() {
                walk(root, &path, output);
            } else if metadata.is_file() {
                output.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(&path).expect("fixture bytes"),
                );
            }
        }
    }

    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}

fn final_path(storage: &Path, bundle: &PublicationBundle) -> PathBuf {
    storage.join(&bundle.plan.target_relative_path)
}

fn incoming_path(storage: &Path, bundle: &PublicationBundle) -> PathBuf {
    let final_path = final_path(storage, bundle);
    let release_name = final_path.file_name().unwrap().to_string_lossy();
    final_path
        .parent()
        .unwrap()
        .join(format!(".{release_name}.incoming"))
}

fn assert_no_copying_files(root: &Path) {
    if !root.exists() {
        return;
    }
    for entry in fs::read_dir(root).expect("read publication tree") {
        let path = entry.expect("publication entry").path();
        if path.is_dir() {
            assert_no_copying_files(&path);
        } else {
            assert!(
                !path.to_string_lossy().ends_with(".copying"),
                "partial copy leaked at {path:?}"
            );
        }
    }
}

#[test]
fn structurally_complete_manifest_passes_and_missing_presentation_needs_review() {
    let fixture = Fixture::new("validation-outcomes");
    let passed = fixture.validated();
    assert_eq!(passed.validation.outcome, ValidationOutcome::Passed);
    assert!(passed.validation.findings.is_empty());
    assert_eq!(passed.assets.len(), 1);
    assert_eq!(passed.work_root, fixture.work_root.canonicalize().unwrap());

    let mut needs_review = fixture.manifest.clone();
    needs_review.release_observation.title = None;
    needs_review.release_observation.artists.clear();
    needs_review.tracks[0].track_title = None;
    needs_review.tracks[0].track_number = None;
    let reviewed = validate_normalized_manifest(&needs_review, &fixture.work_area).unwrap();
    assert_eq!(reviewed.validation.outcome, ValidationOutcome::NeedsReview);
    let codes = reviewed
        .validation
        .findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"validation.release_title_missing"));
    assert!(codes.contains(&"validation.release_artist_missing"));
    assert!(codes.contains(&"validation.track_title_missing"));
    assert!(codes.contains(&"validation.track_number_missing"));
}

#[test]
fn fingerprint_and_output_inventories_fail_closed_when_missing_duplicate_or_incomplete() {
    let fixture = Fixture::new("inventory");

    let mut missing_fingerprint = fixture.manifest.clone();
    missing_fingerprint
        .normalization
        .as_mut()
        .unwrap()
        .fingerprints
        .clear();
    assert!(matches!(
        validate_normalized_manifest(&missing_fingerprint, &fixture.work_area),
        Err(StructuralValidationError::IncompleteManifest(_))
    ));

    let mut duplicate_fingerprint = fixture.manifest.clone();
    let fingerprint = duplicate_fingerprint
        .normalization
        .as_ref()
        .unwrap()
        .fingerprints[0]
        .clone();
    duplicate_fingerprint
        .normalization
        .as_mut()
        .unwrap()
        .fingerprints
        .push(fingerprint);
    assert!(matches!(
        validate_normalized_manifest(&duplicate_fingerprint, &fixture.work_area),
        Err(StructuralValidationError::IncompleteManifest(_))
    ));

    let mut incomplete_fingerprint = fixture.manifest.clone();
    incomplete_fingerprint
        .normalization
        .as_mut()
        .unwrap()
        .fingerprints[0]
        .fingerprint
        .clear();
    assert!(matches!(
        validate_normalized_manifest(&incomplete_fingerprint, &fixture.work_area),
        Err(StructuralValidationError::IncompleteManifest(_))
    ));

    let mut missing_output = fixture.manifest.clone();
    missing_output
        .normalization
        .as_mut()
        .unwrap()
        .outputs
        .clear();
    assert!(matches!(
        validate_normalized_manifest(&missing_output, &fixture.work_area),
        Err(StructuralValidationError::IncompleteManifest(_))
    ));

    let mut duplicate_output = fixture.manifest.clone();
    let output = duplicate_output.normalization.as_ref().unwrap().outputs[0].clone();
    duplicate_output
        .normalization
        .as_mut()
        .unwrap()
        .outputs
        .push(output);
    assert!(matches!(
        validate_normalized_manifest(&duplicate_output, &fixture.work_area),
        Err(StructuralValidationError::IncompleteManifest(_))
    ));

    let mut incomplete_output = fixture.manifest.clone();
    incomplete_output.normalization.as_mut().unwrap().outputs[0].sample_rate_hz = 0;
    assert!(matches!(
        validate_normalized_manifest(&incomplete_output, &fixture.work_area),
        Err(StructuralValidationError::IncompleteManifest(_))
    ));
}

#[test]
fn traversal_and_cross_attempt_provenance_are_rejected_before_target_io() {
    let fixture = Fixture::new("attempt-isolation");
    let mut traversal = fixture.manifest.clone();
    traversal.normalization.as_mut().unwrap().outputs[0].output_path = "../escape.mp3".to_owned();
    assert!(matches!(
        validate_normalized_manifest(&traversal, &fixture.work_area),
        Err(StructuralValidationError::UnsafePath { .. })
    ));
    assert!(!fixture.storage.exists());

    let sibling_attempt = fixture
        .work_root
        .parent()
        .unwrap()
        .join(Uuid::now_v7().simple().to_string());
    fs::create_dir_all(sibling_attempt.join("audio/Disc Ω")).unwrap();
    fs::copy(
        fixture.work_root.join(OUTPUT_PATH),
        sibling_attempt.join(OUTPUT_PATH),
    )
    .unwrap();
    fs::remove_file(fixture.work_root.join(OUTPUT_PATH)).unwrap();
    assert!(matches!(
        validate_normalized_manifest(&fixture.manifest, &fixture.work_area),
        Err(StructuralValidationError::Io { .. })
    ));

    let mut false_provenance = fixture.manifest.clone();
    false_provenance
        .normalization
        .as_mut()
        .unwrap()
        .provenance
        .work_area = Some(sibling_attempt.to_string_lossy().into_owned());
    assert!(matches!(
        validate_normalized_manifest(&false_provenance, &fixture.work_area),
        Err(StructuralValidationError::IncompleteManifest(_))
    ));
}

#[test]
fn normalized_byte_hash_tampering_is_rejected_without_source_mutation() {
    let fixture = Fixture::new("tamper");
    let source_before = fixture.source_snapshot();
    fs::write(
        fixture.work_root.join(OUTPUT_PATH),
        b"same-length-ish altered normalized bytes",
    )
    .unwrap();
    assert!(matches!(
        validate_normalized_manifest(&fixture.manifest, &fixture.work_area),
        Err(StructuralValidationError::HashMismatch { .. })
    ));
    assert_eq!(source_before, fixture.source_snapshot());
    assert!(!fixture.storage.exists());
}

#[cfg(unix)]
#[test]
fn symlinked_work_roots_nested_assets_and_storage_components_are_rejected() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("work-root-symlink");
    let work_item_root = fixture
        .work_root
        .parent()
        .and_then(Path::parent)
        .expect("work-item root")
        .to_path_buf();
    let outside_work_item = fixture.root.join("outside-work-item");
    fs::rename(&work_item_root, &outside_work_item).unwrap();
    symlink(&outside_work_item, &work_item_root).unwrap();
    assert!(matches!(
        validate_normalized_manifest(&fixture.manifest, &fixture.work_area),
        Err(StructuralValidationError::Symlink(_))
    ));
    fs::remove_file(&work_item_root).unwrap();
    fs::rename(&outside_work_item, &work_item_root).unwrap();

    let outside = fixture.root.join("outside-attempt");
    fs::rename(&fixture.work_root, &outside).unwrap();
    symlink(&outside, &fixture.work_root).unwrap();
    assert!(matches!(
        validate_normalized_manifest(&fixture.manifest, &fixture.work_area),
        Err(StructuralValidationError::Symlink(_))
    ));

    fs::remove_file(&fixture.work_root).unwrap();
    fs::rename(&outside, &fixture.work_root).unwrap();
    let output = fixture.work_root.join(OUTPUT_PATH);
    let outside_file = fixture.root.join("outside.mp3");
    fs::rename(&output, &outside_file).unwrap();
    symlink(&outside_file, &output).unwrap();
    assert!(matches!(
        validate_normalized_manifest(&fixture.manifest, &fixture.work_area),
        Err(StructuralValidationError::Symlink(_))
    ));

    fs::remove_file(&output).unwrap();
    fs::rename(&outside_file, &output).unwrap();
    let bundle = fixture.bundle();
    let source_before = fixture.source_snapshot();
    let outside_storage = fixture.root.join("outside-storage");
    fs::create_dir(&outside_storage).unwrap();
    symlink(&outside_storage, &fixture.storage).unwrap();
    assert!(matches!(
        publish_canonical_bundle(&fixture.storage, &bundle, &AtomicBool::new(false)),
        Err(PublicationError::Symlink(_))
    ));
    assert!(fs::read_dir(&outside_storage).unwrap().next().is_none());
    assert_eq!(source_before, fixture.source_snapshot());
}

#[cfg(unix)]
#[test]
fn symlinked_inbound_source_root_is_rejected_by_configuration() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("source-root-symlink");
    let linked_source = fixture.root.join("linked-source");
    symlink(&fixture.source, &linked_source).unwrap();
    let error = melodee_ingestion::InboundSource::from_root(
        linked_source,
        None,
        Some("untrusted source".to_owned()),
    )
    .expect_err("source root symlink must fail closed");
    assert!(error.to_string().contains("must not be a symlink"));
}

#[test]
fn canonical_publication_is_complete_v3_idempotent_and_source_retaining() {
    let fixture = Fixture::new("success-adoption");
    let source_before = fixture.source_snapshot();
    let bundle = fixture.bundle();
    let final_path = final_path(&fixture.storage, &bundle);
    assert!(!final_path.exists());

    let first = publish_canonical_bundle(&fixture.storage, &bundle, &AtomicBool::new(false))
        .expect("publish complete bundle");
    assert_eq!(first.outcome, PublicationOutcome::Published);
    assert_eq!(first.target_relative_path, bundle.plan.target_relative_path);
    assert_eq!(
        first.manifest_relative_path,
        bundle.plan.manifest_relative_path
    );
    assert_eq!(source_before, fixture.source_snapshot());
    assert!(!incoming_path(&fixture.storage, &bundle).exists());
    assert_no_copying_files(&fixture.storage);

    let manifest: ReleaseManifest = serde_json::from_slice(
        &fs::read(final_path.join("manifest.json")).expect("canonical manifest"),
    )
    .expect("manifest v3 JSON");
    assert_eq!(manifest.schema_version, PUBLICATION_MANIFEST_VERSION);
    let publication = manifest.publication.as_ref().expect("publication facts");
    assert_eq!(publication.release_id, bundle.plan.release_id);
    assert_eq!(publication.plan_hash, bundle.plan_hash);
    assert_eq!(publication.canonical_assets, bundle.plan.assets);
    for asset in &bundle.plan.assets {
        assert_eq!(
            fs::read(final_path.join(&asset.canonical_path)).unwrap(),
            OUTPUT_BYTES
        );
    }
    assert!(
        bundle
            .plan
            .target_relative_path
            .bytes()
            .all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || byte == b'/'
                || byte == b'_')
    );

    let second = publish_canonical_bundle(&fixture.storage, &bundle, &AtomicBool::new(false))
        .expect("adopt identical final");
    assert_eq!(second.outcome, PublicationOutcome::Adopted);
    assert_eq!(second.sealed_manifest_hash, first.sealed_manifest_hash);
    assert_eq!(source_before, fixture.source_snapshot());
}

#[test]
fn a_stale_incoming_bundle_is_resumed_and_atomically_finalized() {
    let fixture = Fixture::new("stale-incoming");
    let source_before = fixture.source_snapshot();
    let bundle = fixture.bundle();
    let incoming = incoming_path(&fixture.storage, &bundle);
    let asset = &bundle.plan.assets[0];
    fs::create_dir_all(incoming.join("media")).unwrap();
    fs::write(incoming.join(&asset.canonical_path), OUTPUT_BYTES).unwrap();

    let receipt = publish_canonical_bundle(&fixture.storage, &bundle, &AtomicBool::new(false))
        .expect("resume incoming bundle");
    assert_eq!(receipt.outcome, PublicationOutcome::Published);
    assert!(!incoming.exists());
    assert!(
        final_path(&fixture.storage, &bundle)
            .join("manifest.json")
            .is_file()
    );
    assert_no_copying_files(&fixture.storage);
    assert_eq!(source_before, fixture.source_snapshot());
}

#[test]
fn cancellation_leaves_no_final_and_never_mutates_the_source() {
    let fixture = Fixture::new("canceled");
    let source_before = fixture.source_snapshot();
    let bundle = fixture.bundle();
    assert!(matches!(
        publish_canonical_bundle(&fixture.storage, &bundle, &AtomicBool::new(true)),
        Err(PublicationError::Canceled)
    ));
    assert!(!final_path(&fixture.storage, &bundle).exists());
    assert_eq!(source_before, fixture.source_snapshot());
}

#[test]
fn empty_and_mismatching_finals_fail_closed_without_clobbering() {
    let empty_fixture = Fixture::new("empty-final");
    let empty_bundle = empty_fixture.bundle();
    let empty_final = final_path(&empty_fixture.storage, &empty_bundle);
    fs::create_dir_all(&empty_final).unwrap();
    assert!(matches!(
        publish_canonical_bundle(
            &empty_fixture.storage,
            &empty_bundle,
            &AtomicBool::new(false)
        ),
        Err(PublicationError::TargetConflict { .. })
    ));
    assert!(fs::read_dir(&empty_final).unwrap().next().is_none());

    let mismatch_fixture = Fixture::new("mismatch-final");
    let mismatch_bundle = mismatch_fixture.bundle();
    publish_canonical_bundle(
        &mismatch_fixture.storage,
        &mismatch_bundle,
        &AtomicBool::new(false),
    )
    .unwrap();
    let mismatch_final = final_path(&mismatch_fixture.storage, &mismatch_bundle);
    let asset_path = mismatch_final.join(&mismatch_bundle.plan.assets[0].canonical_path);
    fs::write(&asset_path, b"conflicting canonical bytes").unwrap();
    let conflicting_bytes = fs::read(&asset_path).unwrap();
    assert!(matches!(
        publish_canonical_bundle(
            &mismatch_fixture.storage,
            &mismatch_bundle,
            &AtomicBool::new(false)
        ),
        Err(PublicationError::TargetConflict { .. })
    ));
    assert_eq!(conflicting_bytes, fs::read(asset_path).unwrap());
}

#[cfg(unix)]
#[test]
fn symlink_final_is_a_no_clobber_conflict() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("symlink-final");
    let bundle = fixture.bundle();
    let final_path = final_path(&fixture.storage, &bundle);
    let outside = fixture.root.join("outside-final");
    fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"must remain").unwrap();
    symlink(&outside, &final_path).unwrap();
    assert!(matches!(
        publish_canonical_bundle(&fixture.storage, &bundle, &AtomicBool::new(false)),
        Err(PublicationError::Symlink(_))
    ));
    assert_eq!(
        b"must remain",
        fs::read(outside.join("sentinel")).unwrap().as_slice()
    );
    assert!(
        fs::symlink_metadata(final_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn plan_size_tamper_is_detected_before_any_final_is_visible() {
    let fixture = Fixture::new("size-tamper");
    let source_before = fixture.source_snapshot();
    let mut bundle = fixture.bundle();
    bundle.plan.assets[0].size_bytes += 1;
    bundle
        .manifest
        .publication
        .as_mut()
        .unwrap()
        .canonical_assets[0]
        .size_bytes += 1;
    bundle.plan_hash = bundle.plan.plan_hash().unwrap();
    bundle.manifest.publication.as_mut().unwrap().plan_hash = bundle.plan_hash.clone();
    assert!(matches!(
        publish_canonical_bundle(&fixture.storage, &bundle, &AtomicBool::new(false)),
        Err(PublicationError::SizeMismatch { .. })
    ));
    assert!(!final_path(&fixture.storage, &bundle).exists());
    assert_eq!(source_before, fixture.source_snapshot());
}

//! Shared Phase 4 catalog integration fixtures.
//!
//! These helpers construct the complete durable Phase 3 handoff through the
//! public persistence API. Catalog tests can therefore begin at `Published`
//! without bypassing the manifest, publication-plan, receipt, or lease
//! invariants that the catalog importer is required to validate.

#![allow(dead_code, clippy::expect_used)]

pub mod legacy;

use melodee_domain::{
    AudioTechnicalObservation, CANONICAL_LAYOUT_VERSION, CURRENT_MANIFEST_VERSION, CanonicalAsset,
    CanonicalAssetKind, FingerprintObservation, IngestionState, ManifestPublication,
    ManifestReleaseObservation, NormalizationObservations, NormalizedOutput, ObservedFile,
    ObservedFileKind, ObservedMetadata, ObservedTrack, PublicationAuthorization,
    PublicationJournalStep, PublicationPlan, ReleaseId, ReleaseManifest, SourceDisposition,
    StructuralValidation, TransitionKind, ValidationOutcome,
};
use melodee_persistence::{WorkItemStore, WorkerId};
use serde_json::json;

use crate::common::{T0, TestDb, seed_ready_item, test_config};

/// Immutable Phase 3 facts that Phase 4 must adopt without rewriting.
#[derive(Debug, Clone)]
pub struct PublishedCatalogFixture {
    pub work_item_id: uuid::Uuid,
    pub release_id: ReleaseId,
    pub manifest: ReleaseManifest,
    pub semantic_manifest_hash: String,
    pub plan: PublicationPlan,
    pub plan_hash: String,
    pub sealed_manifest_hash: String,
    pub source_path: String,
    pub canonical_asset_path: String,
    pub source_byte_hash: String,
    pub normalized_byte_hash: String,
}

/// Presentation observations used to create one single-Track published v3.
#[derive(Debug, Clone)]
pub struct CatalogFixturePresentation<'a> {
    pub release_title: &'a str,
    pub release_artist: &'a str,
    pub track_title: &'a str,
    pub track_artist: &'a str,
    pub release_date: &'a str,
}

impl Default for CatalogFixturePresentation<'_> {
    fn default() -> Self {
        Self {
            release_title: "Phase 4 Fixture Release",
            release_artist: "Observed Artist",
            track_title: "Fixture Track",
            track_artist: "Observed Artist",
            release_date: "2001-02-03",
        }
    }
}

/// Seeds one authoritative v3 manifest plus publication plan/receipt and
/// leaves its work item in `Published`, ready for the catalog claim path.
pub async fn seed_published_release(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
) -> PublishedCatalogFixture {
    seed_published_release_with_normalized_hash(
        db,
        ordinal,
        presentation,
        deterministic_hash('a', ordinal),
    )
    .await
}

/// Variant for synthetic duplicate-catalog truth: the complete immutable v3
/// handoff is constructed with the caller's already validated byte hash.
pub async fn seed_published_release_with_normalized_hash(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
    normalized_byte_hash: String,
) -> PublishedCatalogFixture {
    seed_published_release_with_normalized_hash_and_genre(
        db,
        ordinal,
        presentation,
        normalized_byte_hash,
        None,
    )
    .await
}

/// Variant that overrides the observed genre tag (fixtures default to
/// `"Fixture"`); genre fidelity is under test in adapter surfaces.
pub async fn seed_published_release_with_normalized_hash_and_genre(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
    normalized_byte_hash: String,
    genre: Option<&str>,
) -> PublishedCatalogFixture {
    seed_published_release_with_options_full(
        db,
        ordinal,
        presentation,
        normalized_byte_hash,
        false,
        genre,
    )
    .await
}

/// Complete immutable handoff carrying a real inspection read-error fact for
/// Phase 5 quarantine acceptance.
pub async fn seed_published_release_with_read_error(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
) -> PublishedCatalogFixture {
    seed_published_release_with_options_full(
        db,
        ordinal,
        presentation,
        deterministic_hash('a', ordinal),
        true,
        None,
    )
    .await
}

async fn seed_published_release_with_options(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
    normalized_byte_hash: String,
    bytes_read_errors: bool,
) -> PublishedCatalogFixture {
    seed_published_release_with_options_full(
        db,
        ordinal,
        presentation,
        normalized_byte_hash,
        bytes_read_errors,
        None,
    )
    .await
}

async fn seed_published_release_with_options_full(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
    normalized_byte_hash: String,
    bytes_read_errors: bool,
    genre: Option<&str>,
) -> PublishedCatalogFixture {
    let store = WorkItemStore;
    let config = test_config();
    let source_path = format!("phase4/source/release-{ordinal:04}");
    let work_item_id = seed_ready_item(&store, &db.pool, &source_path).await;

    let inspector = WorkerId::new();
    let claimed = store
        .claim(&db.pool, &inspector, &config, T0)
        .await
        .expect("claim inspection fixture");
    assert_eq!(claimed.len(), 1, "fixture setup claims exactly one item");
    store
        .transition(
            &db.pool,
            work_item_id,
            Some(&inspector),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await
        .expect("start inspection fixture");

    let source_file = format!("disc-01/{ordinal:02} - {}.flac", presentation.track_title);
    let source_byte_hash = deterministic_hash('1', ordinal);
    let observed_metadata = ObservedMetadata {
        album: Some(presentation.release_title.to_owned()),
        album_artist: Some(presentation.release_artist.to_owned()),
        track_title: Some(presentation.track_title.to_owned()),
        artist: Some(presentation.track_artist.to_owned()),
        track_number: Some(1),
        track_total: Some(1),
        disc_number: Some(1),
        disc_total: Some(1),
        year: Some(presentation.release_date[..4].to_owned()),
        date: Some(presentation.release_date.to_owned()),
        genre: Some(genre.unwrap_or("Fixture").to_owned()),
        raw: json!({
            "album_artist": presentation.release_artist,
            "artist": presentation.track_artist,
        }),
    };
    let mut manifest = ReleaseManifest::new(
        work_item_id,
        None,
        source_path.clone(),
        None,
        Some(format!("phase4-source-{ordinal:04}")),
        Some("fixture-ffprobe".to_owned()),
        Some("/nonexistent/phase4/ffprobe".to_owned()),
        Some(0),
        Some(1_000),
        true,
        ManifestReleaseObservation {
            title: Some(presentation.release_title.to_owned()),
            artists: vec![presentation.release_artist.to_owned()],
            date: Some(presentation.release_date.to_owned()),
            source_labels: json!({ "fixture_ordinal": ordinal }),
        },
        vec![ObservedFile {
            relative_path: source_file.clone(),
            kind: ObservedFileKind::Audio,
            size_bytes: 42,
            modified_at: None,
            content_hash: Some(source_byte_hash.clone()),
            mime_type: Some("audio/flac".to_owned()),
            kind_code: Some("audio".to_owned()),
            metadata: Some(observed_metadata.clone()),
            audio: Some(AudioTechnicalObservation {
                container: Some("flac".to_owned()),
                codec: Some("flac".to_owned()),
                sample_rate_hz: Some(48_000),
                channels: Some(2),
                channel_layout: Some("stereo".to_owned()),
                bit_depth: Some(24),
                bitrate_bps: Some(900_000),
                stream_count: Some(1),
                duration_ms: Some(180_000),
                stream_index: Some(0),
            }),
            findings: Vec::new(),
            embedded_artwork: Vec::new(),
            sidecar_type: None,
            bytes_read_errors,
        }],
        vec![ObservedTrack {
            file_path: source_file.clone(),
            track_number: Some(1),
            track_title: Some(presentation.track_title.to_owned()),
            media_number: Some(1),
            sort_order: Some(1),
            byte_hash: Some(source_byte_hash.clone()),
            duration_ms: Some(180_000),
            observed_metadata: Some(observed_metadata),
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let inspection_hash = deterministic_hash('2', ordinal);
    store
        .publish_manifest(
            &db.pool,
            work_item_id,
            &inspector,
            &manifest,
            &inspection_hash,
            None,
            Some(&format!("phase4-source-{ordinal:04}")),
            1,
            T0,
        )
        .await
        .expect("publish inspection fixture");

    let normalizer = WorkerId::new();
    let claimed = store
        .claim_normalizing(&db.pool, &normalizer, &config, T0)
        .await
        .expect("claim normalization fixture");
    assert_eq!(claimed.len(), 1, "fixture normalization claim is isolated");
    let normalized_path = format!("audio/{ordinal:04}.mp3");
    manifest.schema_version = CURRENT_MANIFEST_VERSION;
    manifest.normalization = Some(NormalizationObservations {
        profile_version: "phase4-fixture-mp3-v1".to_owned(),
        outputs: vec![NormalizedOutput {
            source_file: source_file.clone(),
            output_path: normalized_path.clone(),
            byte_hash: normalized_byte_hash.clone(),
            container: "mp3".to_owned(),
            codec: "mp3".to_owned(),
            sample_rate_hz: 48_000,
            channels: 2,
            bitrate_bps: Some(192_000),
            duration_ms: 180_000,
            retained: false,
            findings: Vec::new(),
        }],
        fingerprints: vec![FingerprintObservation {
            source_file: source_file.clone(),
            fingerprint: format!("fixture-fingerprint-{ordinal:04}"),
            duration_ms: 180_000,
            findings: Vec::new(),
        }],
        ..NormalizationObservations::default()
    });
    let semantic_manifest_hash = deterministic_hash('3', ordinal);
    store
        .publish_normalization(
            &db.pool,
            work_item_id,
            &normalizer,
            &manifest,
            &semantic_manifest_hash,
            &source_file,
            1,
            T0,
        )
        .await
        .expect("publish normalization fixture");

    let validator = WorkerId::new();
    let claimed = store
        .claim_validating(&db.pool, &validator, &config, T0)
        .await
        .expect("claim validation fixture");
    assert_eq!(claimed.len(), 1, "fixture validation claim is isolated");
    let release_id = ReleaseId::new();
    let authorization = PublicationAuthorization::phase3_structural();
    let canonical_asset_path = format!("media/{normalized_byte_hash}.mp3");
    let assets = vec![CanonicalAsset {
        kind: CanonicalAssetKind::Media,
        source_path: source_file,
        normalized_path,
        canonical_path: canonical_asset_path.clone(),
        size_bytes: 42,
        byte_hash: normalized_byte_hash.clone(),
    }];
    let target_relative_path = PublicationPlan::canonical_target_path(release_id);
    let plan = PublicationPlan {
        layout_version: CANONICAL_LAYOUT_VERSION,
        release_id,
        input_manifest_hash: semantic_manifest_hash.clone(),
        authorization: authorization.clone(),
        manifest_relative_path: format!("{target_relative_path}/manifest.json"),
        target_relative_path,
        assets: assets.clone(),
    };
    let plan_hash = plan.plan_hash().expect("hash fixture publication plan");
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
        plan_hash: plan_hash.clone(),
        canonical_assets: assets,
    };
    manifest
        .upgrade_for_publication(publication, &plan)
        .expect("upgrade fixture to immutable v3");
    store
        .stage_authorized_publication(&db.pool, work_item_id, &validator, &manifest, &plan, T0)
        .await
        .expect("stage fixture publication");

    let publisher = WorkerId::new();
    let claimed = store
        .claim_publication(&db.pool, &publisher, &config, T0)
        .await
        .expect("claim publication fixture");
    assert_eq!(claimed.len(), 1, "fixture publication claim is isolated");
    let asset = &plan.assets[0];
    store
        .record_publication_progress(
            &db.pool,
            work_item_id,
            &publisher,
            &format!("asset:{}", asset.canonical_path),
            PublicationJournalStep::AssetCopied,
            Some(&asset.canonical_path),
            Some(asset.size_bytes),
            Some(&asset.byte_hash),
            None,
            T0,
        )
        .await
        .expect("record fixture canonical asset");
    store
        .record_publication_progress(
            &db.pool,
            work_item_id,
            &publisher,
            &format!("bundle_sealed:{plan_hash}"),
            PublicationJournalStep::BundleSealed,
            None,
            None,
            None,
            None,
            T0,
        )
        .await
        .expect("record sealed fixture bundle");
    let sealed_manifest_hash = deterministic_hash('4', ordinal);
    store
        .record_publication_progress(
            &db.pool,
            work_item_id,
            &publisher,
            &format!("finalized:{plan_hash}"),
            PublicationJournalStep::Finalized,
            Some(&plan.manifest_relative_path),
            Some(128),
            Some(&sealed_manifest_hash),
            None,
            T0,
        )
        .await
        .expect("record finalized fixture bundle");
    let published = store
        .complete_publication(
            &db.pool,
            work_item_id,
            &publisher,
            &plan_hash,
            &sealed_manifest_hash,
            T0,
        )
        .await
        .expect("complete fixture publication");
    assert_eq!(published.state, IngestionState::Published);

    PublishedCatalogFixture {
        work_item_id,
        release_id,
        manifest,
        semantic_manifest_hash,
        plan,
        plan_hash,
        sealed_manifest_hash,
        source_path,
        canonical_asset_path,
        source_byte_hash,
        normalized_byte_hash,
    }
}

pub(crate) fn deterministic_hash(prefix: char, ordinal: u32) -> String {
    let suffix = format!("{ordinal:08x}");
    format!("{}{suffix}", prefix.to_string().repeat(64 - suffix.len()))
}

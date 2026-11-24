//! Regression coverage for unexpected per-item PostgreSQL failures in workers.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{fs, path::PathBuf};

use melodee_domain::{
    CURRENT_MANIFEST_VERSION, FingerprintObservation, IngestionState, ManifestReleaseObservation,
    NormalizationExecutionProvenance, NormalizationObservations, NormalizedOutput, ObservedFile,
    ObservedFileKind, ObservedTrack, ReleaseManifest, TransitionKind,
};
use melodee_ingestion::start_publication_loop;
use melodee_ingestion::{InboundSource, IngestionConfig, publish_once, validate_once};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use time::OffsetDateTime;
use uuid::Uuid;

const SOURCE_FILE: &str = "track.flac";
const OUTPUT_PATH: &str = "audio/track.flac.mp3";
const OUTPUT_BYTES: &[u8] = b"worker resilience normalized mp3 fixture";

struct FixtureRoots {
    root: PathBuf,
    work_area: PathBuf,
    config: IngestionConfig,
}

impl FixtureRoots {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "melodee-publication-worker-resilience-{}",
            Uuid::now_v7().simple()
        ));
        let inbound = root.join("inbound");
        let work_area = root.join("work");
        let storage = root.join("storage");
        fs::create_dir_all(inbound.join("release")).expect("inbound fixture");
        fs::create_dir_all(&work_area).expect("work-area fixture");
        fs::create_dir_all(&storage).expect("storage fixture");
        fs::write(
            inbound.join("release").join(SOURCE_FILE),
            b"retained source bytes",
        )
        .expect("source fixture");

        let source = InboundSource::from_root(inbound, None, Some("resilience".to_owned()))
            .expect("inbound source");
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.work_area_base = work_area.clone();
        config.storage_root = Some(storage);
        config.validation_concurrency = 1;
        config.publication_concurrency = 1;
        Self {
            root,
            work_area,
            config,
        }
    }
}

impl Drop for FixtureRoots {
    fn drop(&mut self) {
        if self
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("melodee-publication-worker-resilience-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

async fn seed_validating_item(database: &common::TestDb, roots: &FixtureRoots) -> Uuid {
    let store = WorkItemStore;
    let work_config = WorkEngineConfig::default();
    let id = common::seed_ready_item(&store, &database.pool, "release").await;
    let inspector = WorkerId::new();
    let claimed = store
        .claim(&database.pool, &inspector, &work_config, common::T0)
        .await
        .expect("claim inspection");
    assert_eq!(claimed.len(), 1);
    store
        .transition(
            &database.pool,
            id,
            Some(&inspector),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            Some("test.inspection"),
            common::T0,
        )
        .await
        .expect("start inspection");

    let source_hash = blake3::hash(b"retained source bytes").to_hex().to_string();
    let mut manifest = ReleaseManifest::new(
        id,
        None,
        "release".to_owned(),
        None,
        Some("stable-source".to_owned()),
        Some("test".to_owned()),
        Some("ffprobe".to_owned()),
        Some(0),
        Some(1_000),
        true,
        ManifestReleaseObservation {
            title: Some("Worker Resilience".to_owned()),
            artists: vec!["Observed Artist".to_owned()],
            date: None,
            source_labels: serde_json::Value::Null,
        },
        vec![ObservedFile {
            relative_path: SOURCE_FILE.to_owned(),
            kind: ObservedFileKind::Audio,
            size_bytes: u64::try_from(b"retained source bytes".len()).unwrap(),
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
        }],
        vec![ObservedTrack {
            file_path: SOURCE_FILE.to_owned(),
            track_number: Some(1),
            track_title: Some("Track".to_owned()),
            media_number: Some(1),
            sort_order: Some(1),
            byte_hash: Some(source_hash),
            duration_ms: Some(1_000),
            observed_metadata: None,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let inspection_hash = melodee_media::hash_manifest(&manifest).expect("inspection hash");
    store
        .publish_manifest(
            &database.pool,
            id,
            &inspector,
            &manifest,
            &inspection_hash,
            None,
            Some("stable-source"),
            1,
            common::T0,
        )
        .await
        .expect("publish inspection");

    let normalizer = WorkerId::new();
    let claimed = store
        .claim_normalizing(&database.pool, &normalizer, &work_config, common::T0)
        .await
        .expect("claim normalization");
    assert_eq!(claimed.len(), 1);
    let attempt_id = Uuid::now_v7();
    let work_root = roots
        .work_area
        .join(id.simple().to_string())
        .join("attempts")
        .join(attempt_id.simple().to_string());
    fs::create_dir_all(work_root.join("audio")).expect("normalization work root");
    fs::write(work_root.join(OUTPUT_PATH), OUTPUT_BYTES).expect("normalized output");
    let output_hash = blake3::hash(OUTPUT_BYTES).to_hex().to_string();
    manifest.schema_version = CURRENT_MANIFEST_VERSION;
    manifest.normalization = Some(NormalizationObservations {
        profile_version: "test-mp3-v1".to_owned(),
        outputs: vec![NormalizedOutput {
            source_file: SOURCE_FILE.to_owned(),
            output_path: OUTPUT_PATH.to_owned(),
            byte_hash: output_hash,
            container: "mp3".to_owned(),
            codec: "mp3".to_owned(),
            sample_rate_hz: 48_000,
            channels: 2,
            bitrate_bps: Some(192_000),
            duration_ms: 1_000,
            retained: false,
            segment: None,
            findings: Vec::new(),
        }],
        fingerprints: vec![FingerprintObservation {
            source_file: SOURCE_FILE.to_owned(),
            fingerprint: "deterministic-fingerprint".to_owned(),
            duration_ms: 1_000,
            findings: Vec::new(),
        }],
        normalized_artwork: Vec::new(),
        parsed_sidecars: Vec::new(),
        provenance: NormalizationExecutionProvenance {
            attempt_id: Some(attempt_id),
            worker_id: Some(normalizer.as_str().to_owned()),
            work_area: Some(work_root.to_string_lossy().into_owned()),
            ..NormalizationExecutionProvenance::default()
        },
        findings: Vec::new(),
    });
    let normalized_hash = melodee_media::hash_manifest(&manifest).expect("normalization hash");
    store
        .publish_normalization(
            &database.pool,
            id,
            &normalizer,
            &manifest,
            &normalized_hash,
            SOURCE_FILE,
            1,
            common::T0,
        )
        .await
        .expect("publish normalization");
    id
}

async fn install_authorization_fault(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "CREATE FUNCTION fail_publication_authorization_53100() RETURNS trigger AS $fault$
         BEGIN
           RAISE EXCEPTION 'injected authorization allocation failure' USING ERRCODE = '53100';
         END
         $fault$ LANGUAGE plpgsql;
         CREATE TRIGGER fail_publication_authorization_53100
         BEFORE INSERT ON release_publication_authorizations
         FOR EACH ROW EXECUTE FUNCTION fail_publication_authorization_53100();",
    )
    .execute(pool)
    .await
    .expect("install authorization fault");
}

async fn remove_authorization_fault(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "DROP TRIGGER fail_publication_authorization_53100
         ON release_publication_authorizations;
         DROP FUNCTION fail_publication_authorization_53100();",
    )
    .execute(pool)
    .await
    .expect("remove authorization fault");
}

async fn install_receipt_fault(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "CREATE FUNCTION fail_publication_receipt_53100() RETURNS trigger AS $fault$
         BEGIN
           RAISE EXCEPTION 'injected receipt allocation failure' USING ERRCODE = '53100';
         END
         $fault$ LANGUAGE plpgsql;
         CREATE TRIGGER fail_publication_receipt_53100
         BEFORE INSERT ON release_publication_receipts
         FOR EACH ROW EXECUTE FUNCTION fail_publication_receipt_53100();",
    )
    .execute(pool)
    .await
    .expect("install receipt fault");
}

async fn remove_receipt_fault(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "DROP TRIGGER fail_publication_receipt_53100 ON release_publication_receipts;
         DROP FUNCTION fail_publication_receipt_53100();",
    )
    .execute(pool)
    .await
    .expect("remove receipt fault");
}

async fn assert_released_without_media_failure(
    pool: &sqlx::PgPool,
    id: Uuid,
    expected_state: IngestionState,
    expected_release_events: i64,
) {
    let item = WorkItemStore.get(pool, id).await.expect("load work item");
    assert_eq!(item.state, expected_state);
    assert_eq!(item.attempt_count, 0);
    assert!(item.next_attempt_at.is_none());
    assert!(item.lease_owner.is_none());
    assert!(item.lease_acquired_at.is_none());
    assert!(item.lease_expires_at.is_none());
    assert!(item.last_error_code.is_none());
    assert!(item.last_error_message.is_none());
    let (release_events, failure_events, failed_journal): (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM release_work_item_events
            WHERE work_item_id = $1 AND event_type = 'lease_released'),
           (SELECT COUNT(*) FROM release_work_item_events
            WHERE work_item_id = $1 AND event_type = 'failed'),
           (SELECT COUNT(*) FROM release_publication_journal
            WHERE work_item_id = $1 AND step = 'failed')",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("failure evidence counts");
    assert_eq!(
        (release_events, failure_events, failed_journal),
        (expected_release_events, 0, 0)
    );
}

#[tokio::test]
async fn failed_periodic_publication_waits_before_claiming_again_then_adopts() {
    let mut roots = FixtureRoots::new();
    roots.config.reconciliation_interval = time::Duration::milliseconds(20);
    let database = common::TestDb::create("publication_cooldown").await;
    let id = seed_validating_item(&database, &roots).await;
    let mut work = WorkEngineConfig {
        claim_batch_size: 1,
        ..WorkEngineConfig::default()
    };
    work.retry_policy.base_delay = time::Duration::seconds(3);
    let validated = validate_once(
        &roots.config,
        &WorkItemStore,
        &database.pool,
        &work,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(validated.completed, 1);
    install_receipt_fault(&database.pool).await;
    let scheduler = start_publication_loop(
        roots.config.clone(),
        database.pool.clone(),
        WorkItemStore,
        work,
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM release_work_item_events
              WHERE work_item_id = $1 AND event_type = 'lease_released')",
        )
        .bind(id)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("first attempt releases its lease");
    remove_receipt_fault(&database.pool).await;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while WorkItemStore.get(&database.pool, id).await.unwrap().state
            != IngestionState::Published
        {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("publication resumed after cooldown");
    scheduler.stop().await;
    let attempts: Vec<OffsetDateTime> = sqlx::query_scalar(
        "SELECT created_at FROM release_work_item_events WHERE work_item_id = $1
          AND reason_code IN ('publication.claimed','publication.reclaimed') ORDER BY id",
    )
    .bind(id)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert!(attempts.len() >= 2);
    for pair in attempts.windows(2) {
        assert!(pair[1] - pair[0] >= work.retry_policy.base_delay);
    }
    database.drop().await;
}

#[tokio::test]
async fn disabled_one_shot_workers_skip_roots_and_database_access() {
    let mut roots = FixtureRoots::new();
    roots.config.enabled = false;
    roots.config.storage_root = Some(roots.work_area.clone());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgresql://127.0.0.1:1/unused")
        .unwrap();
    let work = WorkEngineConfig::default();
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        validate_once(&roots.config, &WorkItemStore, &pool, &work, now)
            .await
            .unwrap()
            .claimed,
        0
    );
    assert_eq!(
        publish_once(&roots.config, &WorkItemStore, &pool, &work, now)
            .await
            .unwrap()
            .claimed,
        0
    );
}

#[tokio::test]
async fn sqlstate_53100_releases_validation_and_publication_leases_for_direct_retry() {
    let roots = FixtureRoots::new();
    let database = common::TestDb::create("phase3_worker_53100").await;
    let id = seed_validating_item(&database, &roots).await;
    let store = WorkItemStore;
    let work_config = WorkEngineConfig::default();

    install_authorization_fault(&database.pool).await;
    let validation_fault = validate_once(
        &roots.config,
        &store,
        &database.pool,
        &work_config,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("validation fault batch returns stats");
    assert_eq!(validation_fault.claimed, 1);
    assert_eq!(validation_fault.completed, 0);
    assert_eq!(validation_fault.needs_review, 0);
    assert_eq!(validation_fault.failed, 0);
    assert_eq!(validation_fault.errors, 1);
    assert_released_without_media_failure(&database.pool, id, IngestionState::Validating, 1).await;

    remove_authorization_fault(&database.pool).await;
    let validation_retry = validate_once(
        &roots.config,
        &store,
        &database.pool,
        &work_config,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("validation direct retry");
    assert_eq!(validation_retry.claimed, 1);
    assert_eq!(validation_retry.completed, 1);
    assert_eq!(validation_retry.errors, 0);
    let staged = store.get(&database.pool, id).await.expect("staged item");
    assert_eq!(staged.state, IngestionState::Staged);
    assert_eq!(staged.attempt_count, 0);
    assert!(staged.lease_owner.is_none());

    install_receipt_fault(&database.pool).await;
    let publication_fault = publish_once(
        &roots.config,
        &store,
        &database.pool,
        &work_config,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("publication fault batch returns stats");
    assert_eq!(publication_fault.claimed, 1);
    assert_eq!(publication_fault.completed, 0);
    assert_eq!(publication_fault.adopted, 0);
    assert_eq!(publication_fault.failed, 0);
    assert_eq!(publication_fault.errors, 1);
    assert_released_without_media_failure(&database.pool, id, IngestionState::Publishing, 2).await;

    remove_receipt_fault(&database.pool).await;
    let publication_retry = publish_once(
        &roots.config,
        &store,
        &database.pool,
        &work_config,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("publication direct retry");
    assert_eq!(publication_retry.claimed, 1);
    assert_eq!(publication_retry.completed, 1);
    assert_eq!(publication_retry.adopted, 1);
    assert_eq!(publication_retry.failed, 0);
    assert_eq!(publication_retry.errors, 0);

    let published = store.get(&database.pool, id).await.expect("published item");
    assert_eq!(published.state, IngestionState::Published);
    assert_eq!(published.attempt_count, 0);
    assert!(published.lease_owner.is_none());
    assert!(published.last_error_code.is_none());
    assert!(published.last_error_message.is_none());
    let (receipts, published_transitions): (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM release_publication_receipts WHERE work_item_id = $1),
           (SELECT COUNT(*) FROM release_work_item_events
            WHERE work_item_id = $1 AND from_state = 'publishing' AND to_state = 'published')",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .expect("publication completion counts");
    assert_eq!((receipts, published_transitions), (1, 1));

    let no_more = publish_once(
        &roots.config,
        &store,
        &database.pool,
        &work_config,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("publication convergence check");
    assert_eq!(no_more.claimed, 0);

    database.drop().await;
}

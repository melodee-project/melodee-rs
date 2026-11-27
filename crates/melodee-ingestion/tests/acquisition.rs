//! Durable inbound-to-staging acquisition acceptance, crash-convergence, and
//! adversarial tests (ADR-0145).
//!
//! Every test runs against real PostgreSQL. Crash boundaries are simulated
//! with the one-shot `test-support` fault hook: the crashed attempt observes
//! the injected error and the retried attempt must converge with no loss, no
//! overwrite, no duplicate acquisition, and no partial authoritative tree.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use melodee_domain::{IngestionState, ReleaseWorkItem};
#[cfg(feature = "test-support")]
use melodee_ingestion::{AcquisitionFaultPoint, AcquisitionTestHook};
use melodee_ingestion::{
    InboundSource, IngestionConfig, acquire_owned_source, owned_source_root, reconcile_all_sources,
    verify_owned_source,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use time::Duration;
use uuid::Uuid;

const T0: time::OffsetDateTime = common::T0;

struct Scenario {
    root: PathBuf,
    config: IngestionConfig,
    pool: PgPool,
    database: common::TestDb,
}

struct HandoffRow {
    status: String,
    method: Option<String>,
    tree_hash: Option<String>,
    staging_relative_path: String,
}

struct FixtureFile {
    relative: &'static str,
    bytes: &'static [u8],
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn provisional_custody_capture_replays_bounded_pages_and_rejects_mutation() {
    let scenario = Scenario::new("provisional-custody-capture").await;
    let release = scenario.root.join("candidate");
    fs::create_dir(&release).expect("release");
    for index in 0..260 {
        fs::write(release.join(format!("track-{index:03}")), b"before").expect("file");
    }
    let item = scenario.settle("candidate", &[]).await;
    let worker = WorkerId::new();
    let engine = WorkEngineConfig::default();
    scenario
        .claim_one(&worker, time::OffsetDateTime::now_utc())
        .await;
    let observe = || {
        melodee_ingestion::test_capture_provisional_custody_digest(
            &scenario.pool,
            item.id,
            &worker,
            engine.lease_duration,
            scenario.config.sources[0].clone(),
            std::ffi::OsStr::new("candidate"),
        )
    };
    sqlx::query("UPDATE custody_hash_spool_budget SET max_entries=1 WHERE singleton")
        .execute(&scenario.pool)
        .await
        .expect("small quota");
    for _ in 0..2 {
        assert!(matches!(
            observe().await,
            Err(melodee_ingestion::AcquisitionError::Persistence(
                melodee_persistence::WorkStoreError::CustodyHashBudgetExhausted
            ))
        ));
    }
    sqlx::query("UPDATE custody_hash_spool_budget SET max_entries=1000 WHERE singleton")
        .execute(&scenario.pool)
        .await
        .expect("raise quota");
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), observe())
        .await
        .expect("cancelled producers were reaped")
        .expect("first capture");
    assert_eq!(first.2, 260);
    assert!(first.1.starts_with("v3:"));
    let second = observe().await.expect("bounded replay");
    assert_eq!(first, second);
    fs::write(release.join("track-100"), b"change").expect("same size mutation");
    assert!(
        observe().await.is_err(),
        "changed content must not replace prior evidence"
    );
    assert_eq!(
        fs::read(release.join("track-100")).expect("source"),
        b"change"
    );
    let handoff = WorkItemStore
        .staging_handoff(&scenario.pool, item.id)
        .await
        .expect("handoff query");
    assert!(
        handoff.is_none(),
        "provisional evidence is never a staging receipt"
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[tokio::test]
async fn provisional_source_before_staging_and_source_after_are_independent() {
    use std::os::unix::fs::PermissionsExt;
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        eprintln!("no protected runtime fixture parent available");
        return;
    };
    let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    let source_path = fixture.path().join("source");
    let staging_root = fixture.path().join("staging");
    let private = staging_root.join("releases").join(item.id.to_string());
    fs::create_dir_all(source_path.join("candidate/disc")).expect("source");
    fs::create_dir_all(private.join("source/disc")).expect("stage");
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private");
    for root in [source_path.join("candidate"), private.join("source")] {
        fs::write(root.join("disc/file"), b"before").expect("fixture bytes");
    }
    let source = InboundSource::from_root(source_path, None, Some("test".to_owned()))
        .expect("source config");
    let db = common::TestDb::create("custody_three_observations").await;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let config = WorkEngineConfig::default();
    WorkItemStore
        .claim(&db.pool, &worker, &config, now)
        .await
        .expect("claim");
    let service_uid = rustix::process::geteuid().as_raw();
    let producer_uid = if service_uid == 1 { 2 } else { 1 };
    assert!(
        melodee_ingestion::test_capture_provisional_staging_digest(
            &db.pool,
            item.id,
            &worker,
            config.lease_duration,
            source.clone(),
            &private,
            std::ffi::OsStr::new("source"),
            &fixture.path().join("wrong-root"),
            producer_uid,
        )
        .await
        .is_err(),
        "the stage observer must bind the actual receipt location"
    );
    let source_observe = || {
        melodee_ingestion::test_capture_provisional_custody_digest(
            &db.pool,
            item.id,
            &worker,
            config.lease_duration,
            source.clone(),
            std::ffi::OsStr::new("candidate"),
        )
    };
    let staging_observe = || {
        melodee_ingestion::test_capture_provisional_staging_digest(
            &db.pool,
            item.id,
            &worker,
            config.lease_duration,
            source.clone(),
            &private,
            std::ffi::OsStr::new("source"),
            &staging_root,
            producer_uid,
        )
    };
    let before = source_observe().await.expect("source before");
    let staged = staging_observe().await.expect("staging readback");
    let after = source_observe().await.expect("source after");
    assert_ne!(
        before.0, staged.0,
        "purposes must not share capture identities"
    );
    assert_eq!((&before.1, before.2), (&staged.1, staged.2));
    assert_eq!(before, after);
    fs::write(private.join("source/disc/file"), b"change").expect("mutate staging");
    assert!(staging_observe().await.is_err());
    fs::write(private.join("source/disc/file"), b"before").expect("restore staging");
    fs::write(fixture.path().join("source/candidate/disc/file"), b"change").expect("mutate source");
    assert!(source_observe().await.is_err());
    assert!(
        WorkItemStore
            .staging_handoff(&db.pool, item.id)
            .await
            .expect("handoff")
            .is_none()
    );
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[tokio::test]
async fn retained_binding_requires_ordered_source_stage_source_proofs() {
    use std::os::unix::fs::PermissionsExt;
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        eprintln!("no protected runtime fixture parent available");
        return;
    };
    let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    let source_path = fixture.path().join("source");
    let staging_root = fixture.path().join("staging");
    let private = staging_root.join("releases").join(item.id.to_string());
    fs::create_dir_all(source_path.join("candidate/disc")).expect("source");
    fs::create_dir_all(private.join("source/disc")).expect("stage");
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private");
    let source_file = source_path.join("candidate/disc/file");
    let stage_file = private.join("source/disc/file");
    fs::write(&source_file, b"before").expect("source bytes");
    fs::write(&stage_file, b"before").expect("stage bytes");
    for root in [source_path.join("candidate"), private.join("source")] {
        for index in 0..41 {
            fs::write(
                root.join(format!("track-{index:03}")),
                format!("bytes-{index}"),
            )
            .expect("sibling fixture");
        }
        for name in ["first/deep", "last/deep"] {
            fs::create_dir_all(root.join(name)).expect("nested directories");
            fs::write(root.join(name).join("track"), name.as_bytes()).expect("nested track");
        }
    }
    let source = InboundSource::from_root(source_path.clone(), None, Some("test".to_owned()))
        .expect("source config");
    let db = common::TestDb::create("custody_ordered_binding").await;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let config = WorkEngineConfig::default();
    WorkItemStore
        .claim(&db.pool, &worker, &config, now)
        .await
        .expect("claim");
    let generation =
        melodee_ingestion::source_generation(&source_path.join("candidate")).expect("generation");
    let relative = format!("releases/{}/source", item.id);
    WorkItemStore
        .begin_staging_handoff(&db.pool, item.id, &worker, &relative, &generation, now)
        .await
        .expect("intent");
    let service_uid = rustix::process::geteuid().as_raw();
    let producer_uid = if service_uid == 1 { 2 } else { 1 };
    let call = |early| {
        melodee_ingestion::test_bind_retained_receipt(
            &db.pool,
            item.id,
            &worker,
            config.lease_duration,
            source.clone(),
            std::ffi::OsStr::new("candidate"),
            &private,
            &staging_root,
            producer_uid,
            early,
        )
    };
    assert!(
        call(Some((source_file.clone(), b"change".to_vec())))
            .await
            .is_err(),
        "a source-after observation made before staging cannot be replayed"
    );
    let handoff = WorkItemStore
        .staging_handoff(&db.pool, item.id)
        .await
        .expect("handoff")
        .expect("intent");
    assert_eq!(
        handoff.status,
        melodee_persistence::StagingHandoffStatus::Intent
    );
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inbound_retained_sources WHERE work_item_id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .expect("retention count");
    assert_eq!(
        retained, 0,
        "rejected ordering must not commit a receipt or obligation"
    );
    assert_eq!(fs::read(&source_file).expect("source"), b"change");
    assert_eq!(fs::read(&stage_file).expect("stage"), b"before");
    fs::write(&source_file, b"before").expect("restore fixture");
    let legacy = call(None).await.expect("ordered bind and retry");
    let handoff = WorkItemStore
        .staging_handoff(&db.pool, item.id)
        .await
        .expect("handoff")
        .expect("receipt");
    assert!(handoff.retained_source);
    assert_eq!(handoff.tree_hash.as_deref(), Some(legacy.as_str()));
    assert!(
        WorkItemStore
            .record_inbound_removed(&db.pool, item.id, &worker, now)
            .await
            .is_err()
    );
    assert!(
        WorkItemStore
            .record_staging_receipt(
                &db.pool,
                item.id,
                &worker,
                melodee_persistence::StagingHandoffMethod::VerifiedCopy,
                &"b".repeat(64),
                now
            )
            .await
            .is_err()
    );
    let mut ingest = IngestionConfig::with_sources(vec![source]);
    ingest.staging_root = Some(staging_root);
    let owned = owned_source_root(&ingest, &WorkItemStore, &db.pool, &item)
        .await
        .expect("legacy receipt remains compatible");
    assert_eq!(owned.tree_hash, legacy);
    assert_eq!(fs::read(&source_file).expect("retained source"), b"before");
    assert_eq!(fs::read(&stage_file).expect("retained staging"), b"before");
    db.drop().await;
}

#[tokio::test]
async fn retained_receipt_retry_never_drains_the_inbound_occurrence() {
    let scenario = Scenario::new("retained-receipt-retry").await;
    let item = scenario
        .settle(
            "candidate",
            &[FixtureFile {
                relative: "track.flac",
                bytes: b"before",
            }],
        )
        .await;
    let worker = WorkerId::new();
    scenario.claim_one(&worker, T0 + Duration::seconds(2)).await;
    let inbound = scenario.root.join("candidate");
    let staged = scenario.staged_source(item.id);
    fs::create_dir_all(&staged).expect("staging");
    fs::write(staged.join("track.flac"), b"before").expect("stage file");
    let generation = melodee_ingestion::source_generation(&inbound).expect("generation");
    let relative = format!("releases/{}/source", item.id);
    WorkItemStore
        .begin_staging_handoff(
            &scenario.pool,
            item.id,
            &worker,
            &relative,
            &generation,
            T0 + Duration::seconds(2),
        )
        .await
        .expect("intent");
    let mut hash = blake3::Hasher::new();
    hash.update(b"ftrack.flac\0");
    hash.update(&6_u64.to_le_bytes());
    hash.update(b"before");
    let legacy = hash.finalize().to_hex().to_string();
    WorkItemStore
        .record_staging_receipt(
            &scenario.pool,
            item.id,
            &worker,
            melodee_persistence::StagingHandoffMethod::VerifiedCopy,
            &legacy,
            T0 + Duration::seconds(2),
        )
        .await
        .expect("receipt");
    let mut tx = scenario.pool.begin().await.expect("transaction");
    sqlx::query(
        "UPDATE release_work_item_staging_handoffs SET retained_source=true WHERE work_item_id=$1",
    )
    .bind(item.id)
    .execute(&mut *tx)
    .await
    .expect("retained marker");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate',$2,$3,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(&generation).bind(format!("v3:{}", "a".repeat(64))).execute(&mut *tx).await.expect("retained obligation");
    tx.commit().await.expect("commit");
    let owned = acquire_owned_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &worker,
        &item,
        None,
        T0 + Duration::seconds(3),
    )
    .await
    .expect("retry retained receipt");
    assert_eq!(owned.tree_hash, legacy);
    assert_eq!(
        fs::read(inbound.join("track.flac")).expect("inbound preserved"),
        b"before"
    );
    assert_eq!(
        fs::read(staged.join("track.flac")).expect("staging preserved"),
        b"before"
    );
    let active: bool =
        sqlx::query_scalar("SELECT inbound_active FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&scenario.pool)
            .await
            .expect("active");
    assert!(active);
    fs::remove_dir_all(staged.parent().expect("owned staging directory")).expect("cleanup staging");
    fs::remove_dir_all(&scenario.root).expect("cleanup inbound");
    scenario.database.drop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn custody_heartbeat_covers_many_short_steps_and_does_not_deadlock_row_holder() {
    let scenario = Scenario::new("custody-heartbeat").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("one".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore
        .create(&scenario.pool, &item)
        .await
        .expect("item");
    let worker = WorkerId::new();
    let engine = WorkEngineConfig {
        lease_duration: Duration::milliseconds(600),
        ..WorkEngineConfig::default()
    };
    assert_eq!(
        WorkItemStore
            .claim(&scenario.pool, &worker, &engine, now)
            .await
            .expect("claim")
            .len(),
        1
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        melodee_ingestion::test_custody_lease_wait(
            &scenario.pool,
            item.id,
            &worker,
            engine.lease_duration,
            false,
        ),
    )
    .await
    .expect("short-step capture completed")
    .expect("lease renewed");
    let live: bool = sqlx::query_scalar(
        "SELECT lease_expires_at > clock_timestamp() FROM release_work_items WHERE id=$1",
    )
    .bind(item.id)
    .fetch_one(&scenario.pool)
    .await
    .expect("lease status");
    assert!(
        live,
        "lease must survive more than twice its original duration"
    );
    sqlx::query("UPDATE release_work_items SET state='ignored',completed_at=clock_timestamp(),lease_owner=NULL,lease_acquired_at=NULL,lease_expires_at=NULL WHERE id=$1")
        .bind(item.id).execute(&scenario.pool).await.expect("retire test item");

    let mut second = ReleaseWorkItem::discovered(
        "two".to_owned(),
        None,
        Uuid::now_v7(),
        time::OffsetDateTime::now_utc(),
    );
    second.state = IngestionState::Ready;
    WorkItemStore
        .create(&scenario.pool, &second)
        .await
        .expect("second");
    let second_worker = WorkerId::new();
    let second_engine = WorkEngineConfig {
        lease_duration: Duration::milliseconds(1800),
        ..WorkEngineConfig::default()
    };
    assert_eq!(
        WorkItemStore
            .claim(
                &scenario.pool,
                &second_worker,
                &second_engine,
                time::OffsetDateTime::now_utc()
            )
            .await
            .expect("second claim")
            .len(),
        1
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        melodee_ingestion::test_custody_lease_wait(
            &scenario.pool,
            second.id,
            &second_worker,
            second_engine.lease_duration,
            true,
        ),
    )
    .await
    .expect("row holder committed without heartbeat deadlock")
    .expect("heartbeat succeeded");
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn cancelled_custody_heartbeat_waiting_for_pool_does_not_renew_later() {
    let scenario = Scenario::new("custody-heartbeat-pool-cancel").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("waiting".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore
        .create(&scenario.pool, &item)
        .await
        .expect("item");
    let worker = WorkerId::new();
    let engine = WorkEngineConfig {
        lease_duration: Duration::seconds(3),
        ..WorkEngineConfig::default()
    };
    WorkItemStore
        .claim(&scenario.pool, &worker, &engine, now)
        .await
        .expect("claim");
    let original_expiry: time::OffsetDateTime =
        sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&scenario.pool)
            .await
            .expect("expiry");
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&common::database_url(&scenario.database.name))
        .await
        .expect("limited pool");
    let held = limited.acquire().await.expect("occupy sole connection");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(1500),
            melodee_ingestion::test_custody_lease_pending(
                &limited,
                item.id,
                &worker,
                engine.lease_duration
            )
        )
        .await
        .is_err(),
        "pending capture must cancel while renewal waits for the pool"
    );
    drop(held);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let expiry: time::OffsetDateTime =
        sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&limited)
            .await
            .expect("expiry after cancellation");
    assert_eq!(
        expiry, original_expiry,
        "cancelled renewal must not commit later"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_item_events WHERE work_item_id=$1 AND event_type='lease_renewed'")
        .bind(item.id).fetch_one(&limited).await.expect("renewal events");
    assert_eq!(count, 0);
    limited.close().await;
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

impl Scenario {
    async fn new(name: &str) -> Self {
        let unique = Uuid::now_v7();
        let root = std::env::temp_dir().join(format!("melodee-acquisition-{name}-{unique}"));
        fs::create_dir_all(&root).expect("root");
        let database = common::TestDb::create(name).await;
        let source =
            InboundSource::from_root(root.clone(), None, Some(name.to_owned())).expect("source");
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.settling_quiet_period = Duration::ZERO;
        Self {
            root,
            config,
            pool: database.pool.clone(),
            database,
        }
    }

    fn staging_root(&self) -> PathBuf {
        self.config.staging_root.clone().expect("staging root")
    }

    fn staged_source(&self, item_id: Uuid) -> PathBuf {
        self.staging_root()
            .join("releases")
            .join(item_id.to_string())
            .join("source")
    }

    fn partial_source(&self, item_id: Uuid) -> PathBuf {
        self.staging_root()
            .join("releases")
            .join(format!(".{item_id}.acquiring"))
    }

    /// Creates a fixture tree in inbound and settles it to `ready`.
    async fn settle(&self, name: &str, files: &[FixtureFile]) -> ReleaseWorkItem {
        let release = self.root.join(name);
        for file in files {
            let path = release.join(file.relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            fs::write(path, file.bytes).expect("file");
        }
        reconcile_all_sources(&self.config, &WorkItemStore, &self.pool, T0)
            .await
            .expect("discover");
        reconcile_all_sources(
            &self.config,
            &WorkItemStore,
            &self.pool,
            T0 + Duration::seconds(1),
        )
        .await
        .expect("settle");
        WorkItemStore
            .list(&self.pool, None, 50, 0)
            .await
            .expect("list")
            .into_iter()
            .find(|item| item.source_path == name)
            .expect("ready item")
    }

    async fn claim_one(&self, worker: &WorkerId, now: time::OffsetDateTime) -> ReleaseWorkItem {
        let engine = WorkEngineConfig {
            claim_batch_size: 1,
            ..WorkEngineConfig::default()
        };
        WorkItemStore
            .claim(&self.pool, worker, &engine, now)
            .await
            .expect("claim")
            .into_iter()
            .next()
            .expect("one claimable item")
    }

    async fn acquire(
        &self,
        worker: &WorkerId,
        item: &ReleaseWorkItem,
        now: time::OffsetDateTime,
    ) -> Result<melodee_ingestion::OwnedSource, melodee_ingestion::AcquisitionError> {
        let expected = WorkItemStore
            .latest_snapshot(&self.pool, item.id)
            .await
            .expect("snapshot")
            .map(|snapshot| snapshot.signature);
        acquire_owned_source(
            &self.config,
            &WorkItemStore,
            &self.pool,
            worker,
            item,
            expected.as_deref(),
            now,
        )
        .await
    }

    /// Acquires without the settling-signature pre-check. Used only where a
    /// test deliberately mutated the source mid-copy; reconciliation owns
    /// signature refresh in production.
    #[cfg(feature = "test-support")]
    async fn acquire_unchecked(
        &self,
        worker: &WorkerId,
        item: &ReleaseWorkItem,
        now: time::OffsetDateTime,
    ) -> Result<melodee_ingestion::OwnedSource, melodee_ingestion::AcquisitionError> {
        acquire_owned_source(
            &self.config,
            &WorkItemStore,
            &self.pool,
            worker,
            item,
            None,
            now,
        )
        .await
    }

    async fn handoff(&self, item_id: Uuid) -> Option<HandoffRow> {
        sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
            "SELECT status, method, tree_hash, staging_relative_path
             FROM release_work_item_staging_handoffs WHERE work_item_id = $1",
        )
        .bind(item_id)
        .fetch_optional(&self.pool)
        .await
        .expect("handoff row")
        .map(
            |(status, method, tree_hash, staging_relative_path)| HandoffRow {
                status,
                method,
                tree_hash,
                staging_relative_path,
            },
        )
    }

    async fn handoff_count(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM release_work_item_staging_handoffs")
            .fetch_one(&self.pool)
            .await
            .expect("handoff count")
    }

    async fn inbound_active(&self, item_id: Uuid) -> bool {
        sqlx::query_scalar("SELECT inbound_active FROM release_work_items WHERE id = $1")
            .bind(item_id)
            .fetch_one(&self.pool)
            .await
            .expect("inbound_active")
    }

    fn reconnect(&self) -> PgPool {
        let url = common::database_url(&self.database.name);
        PgPool::connect_lazy(&url).expect("reconnect pool")
    }
}

/// Recursive byte-for-byte fingerprint of a tree: relative path -> BLAKE3 of
/// contents, plus explicit directory entries. Symlinks are excluded exactly
/// like the acquisition copy/hash implementation.
fn tree_fingerprint(root: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(directory) = queue.pop() {
        let mut entries: Vec<_> = fs::read_dir(&directory)
            .expect("read dir")
            .collect::<Result<_, _>>()
            .expect("entries");
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            let relative = path
                .strip_prefix(root)
                .expect("relative")
                .to_string_lossy()
                .into_owned();
            if metadata.is_dir() {
                map.insert(relative, "dir".to_owned());
                queue.push(path);
            } else if metadata.is_file() {
                let bytes = fs::read(&path).expect("read file");
                map.insert(relative, blake3::hash(&bytes).to_hex().to_string());
            }
        }
    }
    map
}

fn tree_bytes(root: &Path) -> u64 {
    let mut total = 0_u64;
    let mut queue = vec![root.to_path_buf()];
    while let Some(directory) = queue.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let path = entry.expect("entry").path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                queue.push(path);
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    total
}

fn standard_files() -> Vec<FixtureFile> {
    vec![
        FixtureFile {
            relative: "track-01.wav",
            bytes: b"r0-acquisition-audio-a",
        },
        FixtureFile {
            relative: "disc 02/track-02.wav",
            bytes: b"r0-acquisition-audio-b",
        },
        FixtureFile {
            relative: "notes.nfo",
            bytes: b"release notes",
        },
    ]
}

#[cfg(unix)]
#[tokio::test]
async fn distinct_non_utf8_occurrences_are_discovered_and_acquired_separately() {
    use std::os::unix::ffi::OsStringExt;
    let scenario = Scenario::new("acq-non-utf8").await;
    for (suffix, content) in [(0x80_u8, b"first".as_slice()), (0x81, b"second".as_slice())] {
        let name = std::ffi::OsString::from_vec(vec![b'b', b'a', b'd', b'-', suffix]);
        fs::create_dir(scenario.root.join(&name)).expect("candidate");
        fs::write(scenario.root.join(name).join("track.flac"), content).expect("file");
    }
    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    let items = WorkItemStore
        .list(&scenario.pool, None, 10, 0)
        .await
        .expect("items");
    assert_eq!(items.len(), 2);
    assert_ne!(items[0].source_path, items[1].source_path);
    let mut contents = Vec::new();
    for _ in 0..2 {
        let worker = WorkerId::new();
        let item = scenario.claim_one(&worker, T0 + Duration::seconds(2)).await;
        let owned = scenario
            .acquire(&worker, &item, T0 + Duration::seconds(2))
            .await
            .expect("acquire distinct occurrence");
        verify_owned_source(&owned).expect("content verification");
        contents.push(fs::read(owned.root.join("track.flac")).expect("staged content"));
    }
    contents.sort();
    assert_eq!(contents, vec![b"first".to_vec(), b"second".to_vec()]);
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn ambiguous_legacy_row_cannot_claim_a_distinct_literal_unicode_occurrence() {
    use std::os::unix::ffi::OsStringExt;
    let scenario = Scenario::new("acq-legacy-ambiguous").await;
    let legacy = ReleaseWorkItem::discovered("bad-�".to_owned(), None, Uuid::now_v7(), T0);
    let (legacy, _) = WorkItemStore
        .create_or_get(&scenario.pool, &legacy)
        .await
        .expect("legacy");
    sqlx::query("UPDATE release_work_items SET discovery_path_format=0,state='ready' WHERE id=$1")
        .bind(legacy.id)
        .execute(&scenario.pool)
        .await
        .expect("pre-upgrade ambiguous item");
    let raw_name = std::ffi::OsString::from_vec(vec![b'b', b'a', b'd', b'-', 0x80]);
    fs::create_dir(scenario.root.join(&raw_name)).expect("raw candidate");
    fs::write(scenario.root.join(raw_name).join("track.flac"), b"raw").expect("raw file");
    fs::create_dir(scenario.root.join("bad-�")).expect("literal candidate");
    fs::write(scenario.root.join("bad-�/track.flac"), b"literal").expect("literal file");
    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    let worker = WorkerId::new();
    let claimed = WorkItemStore
        .claim(
            &scenario.pool,
            &worker,
            &WorkEngineConfig {
                claim_batch_size: 10,
                ..WorkEngineConfig::default()
            },
            T0 + Duration::seconds(2),
        )
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 2);
    assert!(claimed.iter().all(|item| item.id != legacy.id));
    let pending = WorkItemStore
        .list_legacy_inbound_paths(&scenario.pool, None)
        .await
        .expect("review list");
    assert!(
        pending
            .iter()
            .any(|item| item.id == legacy.id && !item.has_staging_receipt)
    );
    let mut contents = Vec::new();
    let mut reviewed = None;
    for item in &claimed {
        let owned = scenario
            .acquire(&worker, item, T0 + Duration::seconds(2))
            .await
            .expect("acquire reviewed new occurrence");
        contents.push(fs::read(owned.root.join("track.flac")).expect("file"));
        if reviewed.is_none() {
            reviewed = Some((item.id, owned.tree_hash));
        }
    }
    contents.sort();
    assert_eq!(contents, vec![b"literal".to_vec(), b"raw".to_vec()]);
    let (reviewed_id, tree_hash) = reviewed.expect("reviewed receipt");
    let wrong_hash = "0".repeat(64);
    let wrong = melodee_ingestion::LegacyPathReview {
        legacy_id: legacy.id,
        reviewed_id,
        receipt_tree_hash: &wrong_hash,
        actor: "test-operator",
        reason: "checked original media and receipt",
        now: T0 + Duration::seconds(3),
    };
    assert!(
        melodee_ingestion::resolve_legacy_inbound_path(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &wrong
        )
        .await
        .is_err()
    );
    assert_eq!(
        WorkItemStore
            .get(&scenario.pool, legacy.id)
            .await
            .expect("legacy")
            .state,
        IngestionState::Ready
    );
    let review = melodee_ingestion::LegacyPathReview {
        legacy_id: legacy.id,
        reviewed_id,
        receipt_tree_hash: &tree_hash,
        actor: "test-operator",
        reason: "checked original media and receipt",
        now: T0 + Duration::seconds(3),
    };
    assert!(
        melodee_ingestion::resolve_legacy_inbound_path(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &review
        )
        .await
        .expect("reviewed binding")
    );
    assert!(
        !melodee_ingestion::resolve_legacy_inbound_path(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &review
        )
        .await
        .expect("idempotent retry")
    );
    let conflict = melodee_ingestion::LegacyPathReview {
        reason: "different review",
        ..review
    };
    assert!(
        melodee_ingestion::resolve_legacy_inbound_path(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &conflict
        )
        .await
        .is_err()
    );
    assert_eq!(
        WorkItemStore
            .get(&scenario.pool, legacy.id)
            .await
            .expect("legacy")
            .state,
        IngestionState::Ignored
    );
    assert!(
        !WorkItemStore
            .list_legacy_inbound_paths(&scenario.pool, None)
            .await
            .expect("review list")
            .iter()
            .any(|item| item.id == legacy.id)
    );
    let recorded: (Uuid, String, String) = sqlx::query_as("SELECT reviewed_work_item_id,actor,reason FROM ingest_legacy_path_resolutions WHERE legacy_work_item_id=$1")
        .bind(legacy.id).fetch_one(&scenario.pool).await.expect("audit record");
    assert_eq!(
        recorded,
        (
            reviewed_id,
            "test-operator".to_owned(),
            "checked original media and receipt".to_owned()
        )
    );
    scenario.database.drop().await;
}

#[tokio::test]
async fn ambiguous_legacy_with_verified_receipt_recovers_without_touching_inbound() {
    let scenario = Scenario::new("acq-legacy-receipt").await;
    let item = scenario
        .settle(
            "bad-�",
            &[FixtureFile {
                relative: "track.flac",
                bytes: b"owned",
            }],
        )
        .await;
    let worker = WorkerId::new();
    let claimed = scenario.claim_one(&worker, T0 + Duration::seconds(2)).await;
    let owned = scenario
        .acquire(&worker, &claimed, T0 + Duration::seconds(2))
        .await
        .expect("initial receipt");
    sqlx::query("UPDATE release_work_items SET discovery_path_format=0,state='ready',lease_owner=NULL,lease_acquired_at=NULL,lease_expires_at=NULL WHERE id=$1")
        .bind(item.id).execute(&scenario.pool).await.expect("simulate old recovery item");
    fs::create_dir(scenario.root.join("bad-�")).expect("new literal occurrence");
    fs::write(scenario.root.join("bad-�/track.flac"), b"new literal").expect("new file");
    let retry_worker = WorkerId::new();
    let retry = scenario
        .claim_one(&retry_worker, T0 + Duration::seconds(3))
        .await;
    assert_eq!(retry.id, item.id);
    let recovered = scenario
        .acquire(&retry_worker, &retry, T0 + Duration::seconds(3))
        .await
        .expect("verified staged recovery");
    assert_eq!(recovered.tree_hash, owned.tree_hash);
    assert_eq!(
        fs::read(recovered.root.join("track.flac")).expect("staged"),
        b"owned"
    );
    assert_eq!(
        fs::read(scenario.root.join("bad-�/track.flac")).expect("inbound untouched"),
        b"new literal"
    );
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn nested_in_root_alias_candidate_is_acquired() {
    use std::os::unix::fs::symlink;
    let scenario = Scenario::new("acq-nested-alias").await;
    fs::create_dir_all(scenario.root.join("holder/release")).expect("nested candidate");
    fs::write(scenario.root.join("holder/release/track.flac"), b"nested").expect("file");
    symlink(
        scenario.root.join("holder/release"),
        scenario.root.join("alias"),
    )
    .expect("alias");
    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    let worker = WorkerId::new();
    let claimed = WorkItemStore
        .claim(
            &scenario.pool,
            &worker,
            &WorkEngineConfig {
                claim_batch_size: 10,
                ..WorkEngineConfig::default()
            },
            T0 + Duration::seconds(2),
        )
        .await
        .expect("claim");
    let nested = claimed
        .iter()
        .find(|item| item.source_path == "holder/release")
        .expect("nested claim");
    let owned = scenario
        .acquire(&worker, nested, T0 + Duration::seconds(2))
        .await
        .expect("nested acquisition");
    assert_eq!(
        fs::read(owned.root.join("track.flac")).expect("staged file"),
        b"nested"
    );
    verify_owned_source(&owned).expect("content verification");
    assert!(!scenario.root.join("holder/release").exists());
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn nested_acquisition_rejects_parent_replaced_by_symlink() {
    use std::os::unix::fs::symlink;
    let scenario = Scenario::new("acq-nested-parent-swap").await;
    fs::create_dir_all(scenario.root.join("holder/release")).expect("nested candidate");
    fs::write(scenario.root.join("holder/release/track.flac"), b"original").expect("file");
    symlink(
        scenario.root.join("holder/release"),
        scenario.root.join("alias"),
    )
    .expect("alias");
    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    let worker = WorkerId::new();
    let claimed = WorkItemStore
        .claim(
            &scenario.pool,
            &worker,
            &WorkEngineConfig {
                claim_batch_size: 10,
                ..WorkEngineConfig::default()
            },
            T0 + Duration::seconds(2),
        )
        .await
        .expect("claim");
    let nested = claimed
        .iter()
        .find(|item| item.source_path == "holder/release")
        .expect("nested claim");
    let parked = scenario.root.with_extension("parked-holder");
    let outside = scenario.root.with_extension("outside-holder");
    fs::rename(scenario.root.join("holder"), &parked).expect("park original holder");
    fs::create_dir_all(outside.join("release")).expect("outside candidate");
    fs::write(outside.join("release/track.flac"), b"outside").expect("outside file");
    symlink(&outside, scenario.root.join("holder")).expect("replacement parent");
    assert!(
        scenario
            .acquire(&worker, nested, T0 + Duration::seconds(2))
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(outside.join("release/track.flac")).expect("outside preserved"),
        b"outside"
    );
    assert_eq!(
        fs::read(parked.join("release/track.flac")).expect("original preserved"),
        b"original"
    );
    fs::remove_dir_all(outside).expect("outside cleanup");
    fs::remove_dir_all(parked).expect("parked cleanup");
    scenario.database.drop().await;
}

#[cfg(feature = "test-support")]
fn arm_copy_hook(
    config: &mut IngestionConfig,
    fault: AcquisitionFaultPoint,
) -> AcquisitionTestHook {
    let hook = AcquisitionTestHook::new().forcing_copy().with_fault(fault);
    config.acquisition_test_hook = Some(hook.clone());
    hook
}

#[cfg(feature = "test-support")]
fn arm_copy_only(config: &mut IngestionConfig) {
    config.acquisition_test_hook = Some(AcquisitionTestHook::new().forcing_copy());
}

/// The forced cross-filesystem path streams, verifies, fsyncs,
/// no-replace finalizes, records a durable receipt, and only then drains
/// inbound.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn forced_cross_filesystem_acquisition_verifies_and_drains() {
    let mut scenario = Scenario::new("acq-forced-copy").await;
    let files = standard_files();
    let item = scenario.settle("release-cross", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-cross"));
    arm_copy_only(&mut scenario.config);

    let worker = WorkerId::new();
    let claimed = scenario.claim_one(&worker, T0 + Duration::seconds(2)).await;
    assert_eq!(item.id, claimed.id);
    let owned = scenario
        .acquire(&worker, &claimed, T0 + Duration::seconds(2))
        .await
        .expect("acquisition");

    assert_eq!(scenario.staged_source(item.id), owned.root);
    verify_owned_source(&owned).expect("owned source verifies");
    let handoff = scenario.handoff(item.id).await.expect("handoff row");
    assert_eq!("inbound_removed", handoff.status);
    assert_eq!(Some("verified_copy".to_owned()), handoff.method);
    assert!(handoff.tree_hash.is_some(), "durable verified receipt");
    assert_eq!(expected, tree_fingerprint(&owned.root));
    assert!(!scenario.root.join("release-cross").exists());
    assert!(!scenario.inbound_active(item.id).await);

    scenario.database.drop().await;
}

/// Crash matrix: every intent/receipt/cleanup boundary is crashed once
/// and retried. Each retry must converge with no loss, no overwrite, no
/// duplicate acquisition, no partial final tree, and eventual inbound drain.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn crash_boundaries_retry_and_converge() {
    let points = [
        ("before-intent", AcquisitionFaultPoint::BeforeIntent),
        ("after-intent", AcquisitionFaultPoint::AfterIntent),
        ("during-copy", AcquisitionFaultPoint::DuringCopy),
        (
            "after-copy",
            AcquisitionFaultPoint::AfterCopyBeforeVerification,
        ),
        (
            "after-verify",
            AcquisitionFaultPoint::AfterVerificationBeforeFinalization,
        ),
        (
            "after-finalize",
            AcquisitionFaultPoint::AfterFinalizationBeforeReceipt,
        ),
        (
            "after-receipt",
            AcquisitionFaultPoint::AfterReceiptBeforeInboundRemoval,
        ),
        (
            "after-removal",
            AcquisitionFaultPoint::AfterInboundRemovalBeforeBookkeeping,
        ),
    ];
    let mut scenario = Scenario::new("acq-crash-matrix").await;
    let files = standard_files();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let mut evidence: Vec<String> = Vec::new();

    for (label, point) in points {
        let name = format!("release-{label}");
        scenario.settle(&name, &files).await;
        let expected = tree_fingerprint(&scenario.root.join(&name));
        arm_copy_hook(&mut scenario.config, point);
        let item = scenario.claim_one(&worker, now).await;
        assert_eq!(name, item.source_path);

        let crashed = scenario.acquire(&worker, &item, now).await;
        assert!(
            crashed.is_err(),
            "injected fault at {point:?} must fail the attempt"
        );

        let handoff = scenario.handoff(item.id).await;
        let staged = scenario.staged_source(item.id);
        let partial = scenario.partial_source(item.id);
        match point {
            AcquisitionFaultPoint::BeforeIntent => {
                assert!(handoff.is_none(), "no durable intent recorded");
                assert!(!staged.exists());
            }
            AcquisitionFaultPoint::AfterIntent
            | AcquisitionFaultPoint::DuringCopy
            | AcquisitionFaultPoint::AfterCopyBeforeVerification
            | AcquisitionFaultPoint::AfterVerificationBeforeFinalization => {
                let row = handoff.expect("durable intent");
                assert_eq!("intent", row.status);
                assert!(row.tree_hash.is_none(), "no receipt recorded");
                assert!(
                    !staged.exists(),
                    "no partial authoritative final tree at {point:?}"
                );
                assert!(
                    scenario.root.join(&name).exists(),
                    "source retained for recovery"
                );
            }
            AcquisitionFaultPoint::AfterFinalizationBeforeReceipt => {
                let row = handoff.expect("durable intent");
                assert_eq!("intent", row.status);
                assert!(row.tree_hash.is_none());
                assert!(staged.is_dir(), "final tree exists but has no receipt");
                assert!(scenario.root.join(&name).exists());
            }
            AcquisitionFaultPoint::AfterReceiptBeforeInboundRemoval => {
                let row = handoff.expect("receipt");
                assert_eq!("staged", row.status);
                assert!(row.tree_hash.is_some());
                assert!(
                    scenario.root.join(&name).exists(),
                    "inbound not yet removed"
                );
            }
            AcquisitionFaultPoint::AfterInboundRemovalBeforeBookkeeping => {
                let row = handoff.expect("receipt");
                assert_eq!("staged", row.status);
                assert!(
                    !scenario.root.join(&name).exists(),
                    "inbound physically drained before bookkeeping"
                );
                assert!(
                    scenario.inbound_active(item.id).await,
                    "bookkeeping not yet recorded"
                );
            }
        }

        // Retry after the simulated restart: the one-shot fault is absent.
        arm_copy_only(&mut scenario.config);
        let owned = scenario
            .acquire(&worker, &item, now)
            .await
            .unwrap_or_else(|error| panic!("retry after {point:?} converges: {error}"));
        assert_eq!(staged, owned.root);
        verify_owned_source(&owned).expect("receipt matches tree");
        let row = scenario.handoff(item.id).await.expect("completed handoff");
        assert_eq!("inbound_removed", row.status, "{point:?}");
        assert_eq!(Some("verified_copy".to_owned()), row.method);
        assert_eq!(expected, tree_fingerprint(&staged), "no loss at {point:?}");
        assert!(!scenario.root.join(&name).exists(), "inbound drained");
        assert!(!partial.exists(), "hidden attempt scratch removed");
        assert!(!scenario.inbound_active(item.id).await);
        assert_eq!(
            1,
            scenario.handoff_count().await - i64::try_from(evidence.len()).unwrap(),
            "no duplicate acquisition at {point:?}"
        );
        evidence.push(format!("{label}:converged"));
    }

    eprintln!(
        "r0-crash-matrix={{\"boundaries\":{},\"retried\":{},\"duplicates\":0}}",
        points.len(),
        evidence.len()
    );
    scenario.database.drop().await;
}

/// Source-stability proof: additions, byte changes, and removals during the copy
/// window fail closed; the hidden partial is removed; inbound is untouched;
/// and a retry against the settled new source converges.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn source_mutation_during_copy_fails_closed_and_recovers() {
    let mut scenario = Scenario::new("acq-copy-mutation").await;
    let files = standard_files();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    for (label, mutate) in [("add", 0_u8), ("modify", 1_u8), ("remove", 2_u8)] {
        let name = format!("release-{label}");
        scenario.settle(&name, &files).await;
        let hook = AcquisitionTestHook::new()
            .forcing_copy()
            .pausing_during_copy();
        scenario.config.acquisition_test_hook = Some(hook.clone());
        let item = scenario.claim_one(&worker, now).await;

        let config = scenario.config.clone();
        let pool = scenario.pool.clone();
        let task_item = item.clone();
        let worker_for_task = worker.clone();
        let acquisition = tokio::spawn(async move {
            acquire_owned_source(
                &config,
                &WorkItemStore,
                &pool,
                &worker_for_task,
                &task_item,
                None,
                now,
            )
            .await
        });
        hook.wait_until_copy_started().await;
        match mutate {
            0 => fs::write(scenario.root.join(&name).join("late.nfo"), b"late").expect("add"),
            1 => fs::write(
                scenario.root.join(&name).join("track-01.wav"),
                b"mutated bytes",
            )
            .expect("modify"),
            _ => fs::remove_file(scenario.root.join(&name).join("notes.nfo")).expect("remove"),
        }
        hook.release_copy();
        let result = acquisition.await.expect("task");
        assert!(
            matches!(
                result,
                Err(melodee_ingestion::AcquisitionError::SourceChanged)
            ),
            "{label} mutation during copy must fail closed"
        );
        assert!(
            !scenario.partial_source(item.id).exists(),
            "partial attempt scratch removed after {label}"
        );
        assert!(
            scenario.root.join(&name).exists(),
            "inbound source retained after {label}"
        );
        let row = scenario.handoff(item.id).await.expect("intent");
        assert_eq!("intent", row.status, "no receipt from a changed tree");

        // The settled mutated tree becomes the new owned source on retry. The
        // signature pre-check belongs to the reconciliation/inspection layer
        // (it re-settles and refreshes the snapshot); the copy-window fence
        // under test is the streamed before/after tree comparison.
        arm_copy_only(&mut scenario.config);
        let expected = tree_fingerprint(&scenario.root.join(&name));
        let owned = scenario
            .acquire_unchecked(&worker, &item, now)
            .await
            .expect("retry with settled source");
        assert_eq!(expected, tree_fingerprint(&owned.root));
        assert!(!scenario.root.join(&name).exists());
    }

    scenario.database.drop().await;
}

/// A stale worker that loses its lease mid-copy cannot record the
/// receipt, cannot remove source, and its on-disk finalize is adopted only
/// after byte verification by the current owner.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn stale_worker_losing_lease_during_copy_cannot_commit() {
    let mut scenario = Scenario::new("acq-lease-loss").await;
    let files = standard_files();
    let item = scenario.settle("release-lease", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-lease"));
    let stale = WorkerId::new();
    let claimed = scenario.claim_one(&stale, T0 + Duration::seconds(2)).await;

    let hook = AcquisitionTestHook::new()
        .forcing_copy()
        .pausing_during_copy();
    scenario.config.acquisition_test_hook = Some(hook.clone());
    let config = scenario.config.clone();
    let pool = scenario.pool.clone();
    let stale_for_task = stale.clone();
    let task_item = claimed.clone();
    let attempt = tokio::spawn(async move {
        acquire_owned_source(
            &config,
            &WorkItemStore,
            &pool,
            &stale_for_task,
            &task_item,
            None,
            T0 + Duration::seconds(2),
        )
        .await
    });
    hook.wait_until_copy_started().await;

    // A second worker steals the expired lease while the copy is paused.
    let thief = WorkerId::new();
    let engine = WorkEngineConfig::default();
    let stolen_at = T0 + Duration::seconds(2) + engine.lease_duration + Duration::seconds(1);
    let stolen = WorkItemStore
        .claim(&scenario.pool, &thief, &engine, stolen_at)
        .await
        .expect("steal claim");
    assert_eq!(1, stolen.len());
    assert_eq!(item.id, stolen[0].id);
    hook.release_copy();

    let result = attempt.await.expect("task");
    assert!(
        result.is_err(),
        "stale worker must not record the receipt: {result:?}"
    );
    let row = scenario.handoff(item.id).await.expect("intent");
    assert_eq!("intent", row.status, "stale worker recorded a receipt");
    assert!(
        scenario.root.join("release-lease").exists(),
        "stale worker removed source"
    );

    // The current owner converges: it adopts the verified on-disk finalize.
    arm_copy_only(&mut scenario.config);
    let owned = scenario
        .acquire(&thief, &stolen[0], stolen_at)
        .await
        .expect("owner converges");
    assert_eq!(expected, tree_fingerprint(&owned.root));
    let row = scenario.handoff(item.id).await.expect("handoff");
    assert_eq!("inbound_removed", row.status);
    assert_eq!(1, scenario.handoff_count().await);
    assert!(!scenario.root.join("release-lease").exists());

    scenario.database.drop().await;
}

/// Delayed cleanup after occurrence A is replaced by occurrence B at
/// the same inbound path never deletes B, and B becomes a distinct
/// acquisition.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn delayed_cleanup_after_path_reuse_preserves_new_occurrence() {
    let mut scenario = Scenario::new("acq-path-reuse").await;
    let files = standard_files();
    let item_a = scenario.settle("reused", &files).await;
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    arm_copy_hook(
        &mut scenario.config,
        AcquisitionFaultPoint::AfterReceiptBeforeInboundRemoval,
    );
    let claimed = scenario.claim_one(&worker, now).await;
    let crashed = scenario.acquire(&worker, &claimed, now).await;
    assert!(crashed.is_err(), "receipt persisted before the crash");

    // Occurrence B replaces the same inbound path with different bytes while
    // A's cleanup is still delayed.
    fs::remove_dir_all(scenario.root.join("reused")).expect("replace inbound");
    fs::create_dir_all(scenario.root.join("reused")).expect("recreate");
    fs::write(
        scenario.root.join("reused").join("track-01.wav"),
        b"occurrence-b-bytes",
    )
    .expect("b bytes");
    let occurrence_b = tree_fingerprint(&scenario.root.join("reused"));

    // A's delayed cleanup observes the generation/hash mismatch and refuses.
    arm_copy_only(&mut scenario.config);
    let owned_a = scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect("A recovery completes bookkeeping");
    verify_owned_source(&owned_a).expect("A staged intact");
    let row_a = scenario.handoff(item_a.id).await.expect("A handoff");
    assert_eq!("staged", row_a.status, "A records replacement, not removal");
    assert!(!scenario.inbound_active(item_a.id).await);
    assert_eq!(
        occurrence_b,
        tree_fingerprint(&scenario.root.join("reused")),
        "A's delayed cleanup must never delete B"
    );

    // The same path now settles as a distinct acquisition.
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        now + Duration::seconds(1),
    )
    .await
    .expect("rediscover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        now + Duration::seconds(2),
    )
    .await
    .expect("resettle");
    let items = WorkItemStore
        .list(&scenario.pool, None, 10, 0)
        .await
        .expect("list");
    let item_b = items
        .iter()
        .find(|item| item.source_path == "reused" && item.id != item_a.id)
        .expect("distinct second occurrence")
        .clone();
    let worker_b = WorkerId::new();
    let claimed_b = scenario
        .claim_one(&worker_b, now + Duration::seconds(3))
        .await;
    assert_eq!(item_b.id, claimed_b.id);
    let owned_b = scenario
        .acquire(&worker_b, &claimed_b, now + Duration::seconds(3))
        .await
        .expect("B acquires");
    assert_eq!(occurrence_b, tree_fingerprint(&owned_b.root));
    assert_eq!(2, scenario.handoff_count().await);
    assert!(!scenario.root.join("reused").exists());

    scenario.database.drop().await;
}

/// A matching pre-existing final tree is adopted (never overwritten),
/// and a mismatching final tree fails closed with both trees untouched.
#[tokio::test]
async fn final_staging_tree_adoption_and_conflict_fail_closed() {
    let scenario = Scenario::new("acq-adoption").await;
    let files = standard_files();
    let item = scenario.settle("release-adopt", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-adopt"));
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let claimed = scenario.claim_one(&worker, now).await;
    let destination = scenario.staged_source(item.id);

    // Simulate a crash after finalization but before the durable intent:
    // no handoff row, complete final tree, inbound still present.
    fs::create_dir_all(destination.parent().expect("parent")).expect("dirs");
    copy_fixture_tree(&scenario.root.join("release-adopt"), &destination);
    let first = scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect("matching final adopted");
    assert_eq!(expected, tree_fingerprint(&first.root));
    let row = scenario.handoff(item.id).await.expect("receipt");
    assert_eq!("inbound_removed", row.status);
    assert!(!scenario.root.join("release-adopt").exists());

    // A mismatching final tree under durable intent fails closed.
    let conflict_item = scenario.settle("release-conflict", &files).await;
    let claimed_conflict = scenario.claim_one(&worker, now).await;
    assert_eq!(conflict_item.id, claimed_conflict.id);
    let conflict_destination = scenario.staged_source(conflict_item.id);
    fs::create_dir_all(&conflict_destination).expect("conflict dir");
    fs::write(conflict_destination.join("foreign.bin"), b"foreign").expect("foreign bytes");
    let conflicted = scenario.acquire(&worker, &claimed_conflict, now).await;
    assert!(
        matches!(
            conflicted,
            Err(melodee_ingestion::AcquisitionError::IncompleteReceipt(_))
        ),
        "conflicting final must fail closed: {conflicted:?}"
    );
    assert_eq!(
        b"foreign".as_slice(),
        fs::read(conflict_destination.join("foreign.bin")).expect("foreign intact"),
        "conflicting tree is never overwritten or deleted"
    );
    assert!(
        scenario.root.join("release-conflict").exists(),
        "source retained"
    );
    let row = scenario.handoff(conflict_item.id).await.expect("intent");
    assert_eq!("intent", row.status, "no receipt from a conflict");
    assert!(row.tree_hash.is_none());

    scenario.database.drop().await;
}

/// Quotes, apostrophes, semicolons, ampersands, parentheses, leading
/// dashes, and Unicode in candidate names acquire byte-for-byte.
#[tokio::test]
async fn adversarial_candidate_names_acquire_cleanly() {
    let scenario = Scenario::new("acq-names").await;
    let names = [
        "it's a \"quoted\"; & (live)",
        "-leading-dash",
        "café ΩΔ 日本語",
        "semi;colon & ampersand",
    ];
    let files = standard_files();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    for name in names {
        let item = scenario.settle(name, &files).await;
        let expected = tree_fingerprint(&scenario.root.join(name));
        let claimed = scenario.claim_one(&worker, now).await;
        assert_eq!(name, claimed.source_path);
        let owned = scenario
            .acquire(&worker, &claimed, now)
            .await
            .expect("acquire");
        assert_eq!(expected, tree_fingerprint(&owned.root), "{name}");
        assert!(!scenario.root.join(name).exists());
        let row = scenario.handoff(item.id).await.expect("handoff");
        assert_eq!(
            format!("releases/{}/source", item.id),
            row.staging_relative_path,
            "deterministic staging path derives from the work-item identity"
        );
    }
    assert_eq!(4, scenario.handoff_count().await);
    assert_eq!(
        0,
        fs::read_dir(&scenario.root).expect("inbound").count(),
        "inbound fully drained"
    );

    scenario.database.drop().await;
}

/// Traversal, absolute, and nested source paths recorded in durable
/// state are rejected before any filesystem mutation.
#[tokio::test]
async fn traversal_source_paths_are_rejected_before_io() {
    let scenario = Scenario::new("acq-traversal").await;
    fs::create_dir_all(scenario.root.join("innocent")).expect("inbound dir");
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    for malicious in ["../escape", "/etc/absolute"] {
        let item = ReleaseWorkItem::discovered(malicious.to_owned(), None, Uuid::now_v7(), T0);
        let id = item.id;
        WorkItemStore
            .create(&scenario.pool, &item)
            .await
            .expect("create");
        WorkItemStore
            .system_transition(
                &scenario.pool,
                id,
                IngestionState::Settling,
                Some("test"),
                T0,
            )
            .await
            .expect("settling");
        WorkItemStore
            .system_transition(&scenario.pool, id, IngestionState::Ready, Some("test"), T0)
            .await
            .expect("ready");
        let claimed = scenario.claim_one(&worker, now).await;
        assert_eq!(malicious, claimed.source_path);
        let result = scenario.acquire(&worker, &claimed, now).await;
        assert!(
            matches!(
                result,
                Err(melodee_ingestion::AcquisitionError::InvalidSourcePath(_))
            ),
            "{malicious} must be rejected: {result:?}"
        );
        assert!(
            scenario.handoff(id).await.is_none(),
            "no durable intent for a rejected path"
        );
        assert!(!scenario.staged_source(id).exists());
    }
    // A well-formed nested path is not traversal: missing bytes fail at the
    // verified no-follow directory open before any durable intent or staging
    // exists, while a present nested candidate (see the alias test above) is
    // acquired.
    let nested_missing =
        ReleaseWorkItem::discovered("nested/child".to_owned(), None, Uuid::now_v7(), T0);
    let nested_id = nested_missing.id;
    WorkItemStore
        .create(&scenario.pool, &nested_missing)
        .await
        .expect("create");
    WorkItemStore
        .system_transition(
            &scenario.pool,
            nested_id,
            IngestionState::Settling,
            Some("test"),
            T0,
        )
        .await
        .expect("settling");
    WorkItemStore
        .system_transition(
            &scenario.pool,
            nested_id,
            IngestionState::Ready,
            Some("test"),
            T0,
        )
        .await
        .expect("ready");
    let nested_claim = scenario.claim_one(&worker, now).await;
    let nested_result = scenario.acquire(&worker, &nested_claim, now).await;
    assert!(
        matches!(
            &nested_result,
            Err(melodee_ingestion::AcquisitionError::Filesystem { .. })
        ),
        "a missing well-formed nested candidate fails only on I/O: {nested_result:?}"
    );
    assert!(scenario.handoff(nested_id).await.is_none());
    assert!(!scenario.staged_source(nested_id).exists());
    assert_eq!(
        1,
        fs::read_dir(&scenario.root).expect("inbound").count(),
        "inbound untouched"
    );

    scenario.database.drop().await;
}

/// A non-directory collision at the deterministic staging path fails
/// closed without touching the collision or the source.
#[tokio::test]
async fn staging_collision_with_non_directory_fails_closed() {
    let scenario = Scenario::new("acq-collision").await;
    let files = standard_files();
    let item = scenario.settle("release-collide", &files).await;
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let claimed = scenario.claim_one(&worker, now).await;

    let destination = scenario.staged_source(item.id);
    fs::create_dir_all(destination.parent().expect("parent")).expect("dirs");
    fs::write(&destination, b"collision-file").expect("collision");

    let result = scenario.acquire(&worker, &claimed, now).await;
    assert!(result.is_err(), "collision must fail: {result:?}");
    assert_eq!(
        b"collision-file".as_slice(),
        fs::read(&destination).expect("collision intact")
    );
    assert!(scenario.root.join("release-collide").exists());
    let row = scenario.handoff(item.id).await.expect("intent");
    assert_eq!("intent", row.status);
    assert!(row.tree_hash.is_none());

    scenario.database.drop().await;
}

/// After a restart with inbound already drained, recovery adopts the
/// durable receipt and inspection continues from the staged source.
#[tokio::test]
async fn restart_with_inbound_absent_continues_from_staging() {
    let scenario = Scenario::new("acq-restart").await;
    let release = scenario.root.join("release-restart");
    fs::create_dir_all(&release).expect("release");
    write_tagless_wav(&release.join("track.wav"));
    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    let item = WorkItemStore
        .list(&scenario.pool, None, 10, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == "release-restart")
        .expect("item");
    let expected = tree_fingerprint(&release);

    // First worker acquires fully (inbound drained) and then "crashes"
    // before inspection by simply never inspecting.
    let crashed_worker = WorkerId::new();
    let claimed = scenario
        .claim_one(&crashed_worker, T0 + Duration::seconds(2))
        .await;
    let owned = scenario
        .acquire(&crashed_worker, &claimed, T0 + Duration::seconds(2))
        .await
        .expect("acquisition");
    assert!(!release.exists(), "inbound drained before the crash");
    drop(owned);

    // Restart: a fresh pool and worker reclaim the expired lease and inspect.
    let restarted = scenario.reconnect();
    let engine = WorkEngineConfig::default();
    let restart_now = T0 + Duration::seconds(2) + engine.lease_duration + Duration::seconds(1);
    let stats = melodee_ingestion::inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &restarted,
        &engine,
        restart_now,
        None,
    )
    .await
    .expect("inspection after restart");
    assert_eq!(1, stats.completed);
    let inspected = WorkItemStore.get(&restarted, item.id).await.expect("item");
    assert_eq!(IngestionState::Normalizing, inspected.state);
    let manifest = WorkItemStore
        .get_manifest(&restarted, item.id)
        .await
        .expect("manifest lookup")
        .expect("manifest");
    assert_eq!("release-restart", manifest.payload["source_path"]);
    let owned_after = owned_source_root(&scenario.config, &WorkItemStore, &restarted, &inspected)
        .await
        .expect("resolver adopts durable receipt");
    assert_eq!(expected, tree_fingerprint(&owned_after.root));
    assert_eq!(1, scenario.handoff_count().await);
    restarted.close().await;

    scenario.database.drop().await;
}

/// NeedsReview and Quarantined work retains the durable staged source
/// across a restart, and the owned-source resolver keeps serving it.
#[tokio::test]
async fn needs_review_and_quarantined_retain_staged_source_across_restart() {
    let scenario = Scenario::new("acq-retention").await;
    let files = standard_files();
    let item = scenario.settle("release-review", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-review"));
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let claimed = scenario.claim_one(&worker, now).await;
    let owned = scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect("acquisition");
    assert!(!scenario.root.join("release-review").exists());

    // Walk the legal worker path to NeedsReview, then operator-quarantine.
    for state in [
        IngestionState::Inspecting,
        IngestionState::Normalizing,
        IngestionState::Validating,
        IngestionState::NeedsReview,
    ] {
        WorkItemStore
            .transition(
                &scenario.pool,
                item.id,
                Some(&worker),
                melodee_domain::TransitionKind::Worker,
                state,
                Some("r0-retention"),
                now,
            )
            .await
            .expect("worker transition");
    }
    let restarted = scenario.reconnect();
    let item = WorkItemStore
        .get(&restarted, item.id)
        .await
        .expect("item after restart");
    assert_eq!(IngestionState::NeedsReview, item.state);
    let resolved = owned_source_root(&scenario.config, &WorkItemStore, &restarted, &item)
        .await
        .expect("NeedsReview resolves staged source");
    assert_eq!(
        expected,
        tree_fingerprint(&resolved.root),
        "NeedsReview retains staged source across restart"
    );

    WorkItemStore
        .transition(
            &restarted,
            item.id,
            None,
            melodee_domain::TransitionKind::Operator,
            IngestionState::Quarantined,
            Some("r0-retention"),
            now + Duration::seconds(1),
        )
        .await
        .expect("operator quarantine");
    let quarantined = WorkItemStore
        .get(&restarted, item.id)
        .await
        .expect("quarantined item");
    assert_eq!(IngestionState::Quarantined, quarantined.state);
    let resolved = owned_source_root(&scenario.config, &WorkItemStore, &restarted, &quarantined)
        .await
        .expect("Quarantined resolves staged source");
    assert_eq!(
        expected,
        tree_fingerprint(&resolved.root),
        "Quarantined retains staged source across restart"
    );
    restarted.close().await;
    drop(owned);

    scenario.database.drop().await;
}

/// Concurrent workers racing one occurrence produce exactly one
/// authoritative acquisition, receipt, and drain.
#[tokio::test]
async fn concurrent_acquisition_of_one_occurrence_is_single_authoritative() {
    let scenario = Scenario::new("acq-concurrent").await;
    let files = standard_files();
    let item = scenario.settle("release-race", &files).await;
    let now = T0 + Duration::seconds(2);

    let winner = WorkerId::new();
    let loser = WorkerId::new();
    let claimed = scenario.claim_one(&winner, now).await;
    // The loser's claim is fenced out by the live lease.
    let engine = WorkEngineConfig::default();
    let loser_claims = WorkItemStore
        .claim(&scenario.pool, &loser, &engine, now)
        .await
        .expect("loser claim");
    assert!(loser_claims.is_empty(), "lease exclusivity at claim");
    let loser_result = scenario.acquire(&loser, &item, now).await;
    assert!(
        loser_result.is_err(),
        "a worker without the lease cannot begin the handoff"
    );
    assert!(scenario.handoff(item.id).await.is_none());

    let owned = scenario
        .acquire(&winner, &claimed, now)
        .await
        .expect("winner acquires");
    verify_owned_source(&owned).expect("receipt");
    assert_eq!(1, scenario.handoff_count().await);
    let row = scenario.handoff(item.id).await.expect("handoff");
    assert_eq!("inbound_removed", row.status);
    assert!(!scenario.root.join("release-race").exists());

    scenario.database.drop().await;
}

/// 200 stable Releases on one filesystem acquire byte-for-byte, drain
/// inbound completely, leave 200 complete receipts with no duplicate
/// identity, rescan to zero historical candidates, and a new arrival
/// produces exactly one new acquisition.
#[tokio::test]
async fn two_hundred_release_acquisition_drains_inbound_and_stays_drained() {
    let scenario = Scenario::new("acq-200").await;
    let started = std::time::Instant::now();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    let mut expected: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut total_bytes = 0_u64;
    let mut total_files = 0_u64;
    for index in 0..200 {
        let name = format!("release-{index:03}");
        let release = scenario.root.join(&name);
        fs::create_dir_all(release.join("disc 01")).expect("dirs");
        fs::write(release.join("track-01.wav"), format!("audio-{index}")).expect("audio");
        fs::write(
            release.join("disc 01/track-02.wav"),
            format!("b-side-{index}"),
        )
        .expect("b");
        if index % 3 == 0 {
            fs::write(release.join("notes.nfo"), format!("notes-{index}")).expect("nfo");
        }
        expected.insert(name.clone(), tree_fingerprint(&release));
        total_bytes += tree_bytes(&release);
        total_files += expected[&name].len() as u64;
    }

    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    let settled = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    assert_eq!(
        200_u64,
        settled.iter().map(|r| r.stats.candidates_seen).sum::<u64>()
    );

    let engine = WorkEngineConfig {
        claim_batch_size: 25,
        ..WorkEngineConfig::default()
    };
    let mut acquired = 0_u64;
    loop {
        let items = WorkItemStore
            .claim(&scenario.pool, &worker, &engine, now)
            .await
            .expect("claim batch");
        if items.is_empty() {
            break;
        }
        for item in &items {
            let owned = scenario.acquire(&worker, item, now).await.expect("acquire");
            assert_eq!(
                expected[&item.source_path],
                tree_fingerprint(&owned.root),
                "byte-for-byte staged tree for {}",
                item.source_path
            );
            acquired += 1;
        }
    }
    assert_eq!(200, acquired);

    assert_eq!(
        0,
        fs::read_dir(&scenario.root).expect("inbound").count(),
        "all 200 inbound candidate directories drained"
    );
    let (work_items, handoffs, distinct_items, distinct_paths, removed, renames): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM release_work_items),
            (SELECT COUNT(*) FROM release_work_item_staging_handoffs),
            (SELECT COUNT(DISTINCT work_item_id) FROM release_work_item_staging_handoffs),
            (SELECT COUNT(DISTINCT staging_relative_path) FROM release_work_item_staging_handoffs),
            (SELECT COUNT(*) FROM release_work_item_staging_handoffs
             WHERE status = 'inbound_removed' AND tree_hash IS NOT NULL),
            (SELECT COUNT(*) FROM release_work_item_staging_handoffs WHERE method = 'rename')",
    )
    .fetch_one(&scenario.pool)
    .await
    .expect("counts");
    assert_eq!(
        (200, 200, 200, 200, 200, 200),
        (
            work_items,
            handoffs,
            distinct_items,
            distinct_paths,
            removed,
            renames
        ),
        "200 work items, 200 complete rename receipts, no duplicate identity"
    );

    // A second reconciliation sees zero historical candidates.
    let rescan = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(3),
    )
    .await
    .expect("second reconcile");
    let rescan_stats = rescan.iter().fold((0, 0, 0), |acc, result| {
        (
            acc.0 + result.stats.candidates_seen,
            acc.1 + result.stats.new_work_items,
            acc.2 + result.stats.missing_candidates,
        )
    });
    assert_eq!(
        (0, 0, 0),
        rescan_stats,
        "zero historical candidates after drain"
    );
    let (item_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM release_work_items")
        .fetch_one(&scenario.pool)
        .await
        .expect("count");
    assert_eq!(200, item_count);

    // Exactly one new arrival produces exactly one new acquisition.
    let arrival = scenario.root.join("release-200-new");
    fs::create_dir_all(&arrival).expect("arrival");
    fs::write(arrival.join("track.wav"), b"arrival-audio").expect("audio");
    let arrival_fingerprint = tree_fingerprint(&arrival);
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(4),
    )
    .await
    .expect("discover arrival");
    let arrival_scan = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(5),
    )
    .await
    .expect("settle arrival");
    assert_eq!(
        1_u64,
        arrival_scan
            .iter()
            .map(|r| r.stats.candidates_seen)
            .sum::<u64>()
    );
    let new_item = WorkItemStore
        .list(&scenario.pool, None, 500, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == "release-200-new")
        .expect("new item");
    let claimed = scenario.claim_one(&worker, T0 + Duration::seconds(6)).await;
    assert_eq!(new_item.id, claimed.id);
    let owned = scenario
        .acquire(&worker, &claimed, T0 + Duration::seconds(6))
        .await
        .expect("new acquisition");
    assert_eq!(arrival_fingerprint, tree_fingerprint(&owned.root));
    assert_eq!(201, scenario.handoff_count().await);
    assert_eq!(0, fs::read_dir(&scenario.root).expect("inbound").count());

    eprintln!(
        "r0-same-fs-acceptance={{\"releases\":200,\"files\":{total_files},\"bytes\":{total_bytes},\"handoff_method\":\"rename\",\"wall_ms\":{},\"second_scan_candidates\":0,\"new_arrivals_acquired\":1}}",
        started.elapsed().as_millis()
    );
    scenario.database.drop().await;
}

fn copy_fixture_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("mkdir");
    for entry in fs::read_dir(source).expect("read dir") {
        let entry = entry.expect("entry");
        let target = destination.join(entry.file_name());
        let metadata = entry.metadata().expect("metadata");
        if metadata.is_dir() {
            copy_fixture_tree(&entry.path(), &target);
        } else if metadata.is_file() {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn write_tagless_wav(path: &std::path::Path) {
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
    fs::write(path, bytes).expect("wav");
}

#[tokio::test]
async fn acquisition_accepts_legacy_settling_signature_without_rewriting_evidence() {
    let scenario = Scenario::new("legacy_signature").await;
    let item = scenario.settle("legacy-release", &standard_files()).await;
    let observed = melodee_ingestion::snapshot_release_directory(
        &scenario.root.join("legacy-release"),
        ".melodee-ready",
    )
    .expect("snapshot");
    let legacy: String = observed
        .files
        .iter()
        .map(|file| {
            format!(
                "{}\u{001f}{}\u{001f}{}\u{001f}{}\n",
                file.relative_path,
                file.file_type,
                file.file_size,
                file.modified_at.unix_timestamp_nanos()
            )
        })
        .collect();
    sqlx::query("UPDATE release_work_item_settling_snapshots SET signature = $2 WHERE id = $1")
        .bind(item.latest_settling_snapshot_id)
        .bind(&legacy)
        .execute(&scenario.pool)
        .await
        .expect("legacy evidence");
    let worker = WorkerId::new();
    let claimed = scenario.claim_one(&worker, T0 + Duration::seconds(2)).await;
    let owned = scenario
        .acquire(&worker, &claimed, T0 + Duration::seconds(2))
        .await
        .expect("legacy handoff");
    assert!(owned.root.is_dir());
    let stored: String = sqlx::query_scalar(
        "SELECT signature FROM release_work_item_settling_snapshots WHERE id = $1",
    )
    .bind(item.latest_settling_snapshot_id)
    .fetch_one(&scenario.pool)
    .await
    .expect("stored signature");
    assert_eq!(stored, legacy);
    fs::remove_dir_all(
        scenario
            .staging_root()
            .join("releases")
            .join(item.id.to_string()),
    )
    .expect("owned staged fixture");
    fs::remove_dir_all(&scenario.root).expect("owned inbound fixture");
    scenario.database.drop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn legacy_signature_survives_interrupted_finalized_handoff_recovery() {
    let mut scenario = Scenario::new("legacy_signature_recovery").await;
    let item = scenario.settle("legacy-release", &standard_files()).await;
    let expected_tree = tree_fingerprint(&scenario.root.join("legacy-release"));
    let snapshot = melodee_ingestion::snapshot_release_directory(
        &scenario.root.join("legacy-release"),
        ".melodee-ready",
    )
    .expect("snapshot");
    let legacy: String = snapshot
        .files
        .iter()
        .map(|file| {
            format!(
                "{}\u{001f}{}\u{001f}{}\u{001f}{}\n",
                file.relative_path,
                file.file_type,
                file.file_size,
                file.modified_at.unix_timestamp_nanos()
            )
        })
        .collect();
    sqlx::query("UPDATE release_work_item_settling_snapshots SET signature = $2 WHERE id = $1")
        .bind(item.latest_settling_snapshot_id)
        .bind(&legacy)
        .execute(&scenario.pool)
        .await
        .expect("legacy evidence");
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let claimed = scenario.claim_one(&worker, now).await;
    arm_copy_hook(
        &mut scenario.config,
        AcquisitionFaultPoint::AfterFinalizationBeforeReceipt,
    );
    scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect_err("injected interruption");
    arm_copy_only(&mut scenario.config);
    let owned = scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect("recover finalized tree");
    verify_owned_source(&owned).expect("verified byte hash");
    assert_eq!(tree_fingerprint(&owned.root), expected_tree);
    let stored: String = sqlx::query_scalar(
        "SELECT signature FROM release_work_item_settling_snapshots WHERE id = $1",
    )
    .bind(item.latest_settling_snapshot_id)
    .fetch_one(&scenario.pool)
    .await
    .expect("stored signature");
    assert_eq!(stored, legacy);
    fs::remove_dir_all(
        scenario
            .staging_root()
            .join("releases")
            .join(item.id.to_string()),
    )
    .expect("owned staged fixture");
    fs::remove_dir_all(&scenario.root).expect("owned inbound fixture");
    scenario.database.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_recheck_holds_changed_bytes_until_safe_intake() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(root.join("candidate")).expect("candidate");
    let source_file = root.join("candidate/file");
    fs::write(&source_file, b"changed").expect("source");
    let source = InboundSource::from_root(root, None, Some("test".to_owned())).expect("source");
    let config = IngestionConfig::with_sources(vec![source]);
    let db = common::TestDb::create("retained_recheck_changed").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate','previous',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
    let worker = WorkerId::new();
    assert!(
        melodee_ingestion::test_retained_recheck_turn(&config, &db.pool, &WorkItemStore, &worker)
            .await
            .expect("turn")
    );
    assert_eq!(fs::read(&source_file).expect("source retained"), b"changed");
    let row: (String, bool, Option<Uuid>, Option<String>) = sqlx::query_as("SELECT status,baseline_change_pending,superseded_by,last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(item.id).fetch_one(&db.pool).await.expect("obligation status");
    assert_eq!(
        row,
        (
            "active".to_owned(),
            true,
            None,
            Some("awaiting_intake".to_owned())
        )
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items")
        .fetch_one(&db.pool)
        .await
        .expect("work count");
    assert_eq!(count, 1, "no destructive ordinary successor may be created");
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_baseline_pages_yield_to_other_due_occurrences() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(&root).expect("inbound root");
    let mut config = IngestionConfig::with_sources(vec![
        InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source"),
    ]);
    config.retained_recheck_mib_per_second = 1024;
    let db = common::TestDb::create("retained_page_turn_fairness").await;
    let now = time::OffsetDateTime::now_utc();
    let mut ids = Vec::new();
    for (path, chunk_count) in [("first", 17_i64), ("second", 1_i64)] {
        fs::create_dir_all(root.join(path)).expect("source directory");
        let bytes = vec![0x5a; chunk_count as usize * 65_536];
        fs::write(root.join(path).join("track"), &bytes).expect("source file");
        let item = ReleaseWorkItem::discovered(path.to_owned(), None, Uuid::now_v7(), now);
        WorkItemStore.create(&db.pool, &item).await.expect("item");
        let baseline = Uuid::now_v7();
        // Synthetic authoritative metadata exercises only cursor scheduling;
        // a real receipt still requires the protected-stage publisher.
        sqlx::query("INSERT INTO inbound_retained_baselines(id,work_item_id,lease_owner,stage_capture_id,stage_epoch,stage_digest,stage_generation,stage_binding,state,file_count,chunk_count,logical_bytes) VALUES($1,$2,'fixture',$3,$4,$5,'stage','fixture','capturing',1,$6,128)")
            .bind(baseline).bind(item.id).bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(format!("v3:{}", "a".repeat(64))).bind(chunk_count).execute(&db.pool).await.expect("baseline fixture");
        sqlx::query("INSERT INTO inbound_retained_baseline_files(baseline_id,stage_ordinal,path_key,parent_key,file_length,stage_digest,chunk_count,finished,logical_bytes) VALUES($1,1,'track','',$2,$3,$4,true,128)")
            .bind(baseline).bind(bytes.len() as i64).bind("a".repeat(64)).bind(chunk_count).execute(&db.pool).await.expect("file fixture");
        sqlx::query("INSERT INTO inbound_retained_baseline_directories(baseline_id,stage_ordinal,path_key,parent_key,stage_digest,logical_bytes) VALUES($1,0,'','',$2,128)")
            .bind(baseline).bind("a".repeat(64)).execute(&db.pool).await.expect("root fixture");
        for index in 0..chunk_count {
            let mut digest = blake3::Hasher::new();
            digest.update(b"melodee-retained-file-chunk-v4\0");
            digest.update(&(index as u64).to_le_bytes());
            digest.update(&65_536_u64.to_le_bytes());
            digest.update(&bytes[index as usize * 65_536..(index as usize + 1) * 65_536]);
            sqlx::query("INSERT INTO inbound_retained_baseline_chunks(baseline_id,stage_ordinal,chunk_index,chunk_length,digest) VALUES($1,1,$2,65536,$3)")
                .bind(baseline).bind(index).bind(digest.finalize().to_hex().to_string()).execute(&db.pool).await.expect("chunk fixture");
        }
        sqlx::query("UPDATE inbound_retained_baseline_files SET stage_verity_sha256=repeat('c',64),stage_verity_device=repeat('1',16),stage_verity_inode=repeat('2',16) WHERE baseline_id=$1")
            .bind(baseline).execute(&db.pool).await.expect("synthetic verity evidence");
        sqlx::query("UPDATE inbound_retained_baselines SET state='frozen',protected_coverage=true WHERE id=$1")
            .bind(baseline)
            .execute(&db.pool)
            .await
            .expect("freeze fixture");
        sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at,retained_source) VALUES($1,'fixture','fixture','staged','verified_copy',$2,clock_timestamp(),clock_timestamp(),true)")
            .bind(item.id).bind("a".repeat(64)).execute(&db.pool).await.expect("synthetic retained handoff");
        let lock_name = format!("stage-{}.lock", baseline.simple());
        sqlx::query("UPDATE inbound_retained_baselines SET state='authoritative',lifecycle_lock_name=$2,lifecycle_root_generation='unix:1:1',lifecycle_lock_generation='unix:1:2' WHERE id=$1")
            .bind(baseline).bind(&lock_name)
            .execute(&db.pool)
            .await
            .expect("publish fixture");
        sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,$3,'fixture',$4,clock_timestamp()-interval '1 minute',clock_timestamp(),clock_timestamp())")
            .bind(item.id).bind(baseline).bind(path).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
        ids.push(item.id);
    }
    for _ in 0..2 {
        let worker = WorkerId::new();
        assert!(
            melodee_ingestion::test_retained_recheck_turn(
                &config,
                &db.pool,
                &WorkItemStore,
                &worker
            )
            .await
            .expect("turn")
        );
    }
    let first: (i64, bool, Option<String>) = sqlx::query_as("SELECT baseline_cursor_chunk,baseline_change_pending,lease_owner FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(ids[0]).fetch_one(&db.pool).await.expect("first cursor");
    let second: (i64, bool, Option<String>) = sqlx::query_as("SELECT baseline_cursor_chunk,baseline_change_pending,lease_owner FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(ids[1]).fetch_one(&db.pool).await.expect("second cursor");
    assert_eq!(
        first,
        (15, true, None),
        "first turn visits at most sixteen chunks"
    );
    assert_eq!(
        second,
        (0, true, None),
        "second due occurrence is not starved"
    );
    // Isolate the directory detector from the synthetic generation mismatch.
    sqlx::query(
        "UPDATE inbound_retained_sources SET baseline_change_pending=false WHERE work_item_id=$1",
    )
    .bind(ids[0])
    .execute(&db.pool)
    .await
    .expect("reset fixture signal");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        fs::write(
            root.join("first")
                .join(std::ffi::OsString::from_vec(b"new-\xff".to_vec())),
            b"new bytes",
        )
        .expect("new native-name file");
    }
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let replacement_worker = WorkerId::new();
    assert!(
        melodee_ingestion::test_retained_recheck_turn(
            &config,
            &db.pool,
            &WorkItemStore,
            &replacement_worker,
        )
        .await
        .expect("first directory turn")
    );
    let first_directory: i64 = sqlx::query_scalar("SELECT scan_revision FROM inbound_retained_baseline_directories WHERE baseline_id=(SELECT baseline_id FROM inbound_retained_sources WHERE work_item_id=$1) AND stage_ordinal=0")
        .bind(ids[0]).fetch_one(&db.pool).await.expect("first directory cursor");
    assert_eq!(first_directory, 1, "directory and file classes alternate");
    let added: bool = sqlx::query_scalar(
        "SELECT baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(ids[0])
    .fetch_one(&db.pool)
    .await
    .expect("addition signal");
    assert!(
        added,
        "an added native-name file requests fresh verification"
    );
    assert!(
        melodee_ingestion::test_retained_recheck_turn(
            &config,
            &db.pool,
            &WorkItemStore,
            &WorkerId::new()
        )
        .await
        .expect("second directory turn")
    );
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        melodee_ingestion::test_retained_recheck_turn(
            &config,
            &db.pool,
            &WorkItemStore,
            &WorkerId::new()
        )
        .await
        .expect("resumed file turn")
    );
    let resumed: i64 = sqlx::query_scalar(
        "SELECT baseline_cursor_chunk FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(ids[0])
    .fetch_one(&db.pool)
    .await
    .expect("resumed cursor");
    assert_eq!(resumed, 16, "a new worker resumes beyond the first page");
    assert_eq!(
        fs::read(root.join("first/track"))
            .expect("first intact")
            .len(),
        17 * 65_536
    );
    assert_eq!(
        fs::read(root.join("second/track"))
            .expect("second intact")
            .len(),
        65_536
    );
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_missing_baseline_file_signals_and_yields_before_later_file() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(root.join("candidate")).expect("source");
    fs::write(root.join("candidate/later"), b"later bytes").expect("later file");
    let config = IngestionConfig::with_sources(vec![
        InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source"),
    ]);
    let db = common::TestDb::create("retained_missing_page_fairness").await;
    let now = time::OffsetDateTime::now_utc();
    let item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let baseline = Uuid::now_v7();
    // This SQL-only authority is a cursor fixture, not protected-stage proof.
    sqlx::query("INSERT INTO inbound_retained_baselines(id,work_item_id,lease_owner,stage_capture_id,stage_epoch,stage_digest,stage_generation,stage_binding,state,file_count,chunk_count,logical_bytes) VALUES($1,$2,'fixture',$3,$4,$5,'stage','fixture','capturing',2,1,128)")
        .bind(baseline).bind(item.id).bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("baseline");
    for (ordinal, path, length, count) in [(1_i64, "missing", 0_i64, 0_i64), (2, "later", 11, 1)] {
        sqlx::query("INSERT INTO inbound_retained_baseline_files(baseline_id,stage_ordinal,path_key,parent_key,file_length,stage_digest,chunk_count,finished,logical_bytes) VALUES($1,$2,$3,'',$4,$5,$6,true,128)")
            .bind(baseline).bind(ordinal).bind(path).bind(length).bind("a".repeat(64)).bind(count).execute(&db.pool).await.expect("file");
    }
    sqlx::query("INSERT INTO inbound_retained_baseline_directories(baseline_id,stage_ordinal,path_key,parent_key,stage_digest,logical_bytes) VALUES($1,0,'','',$2,128)")
        .bind(baseline).bind("a".repeat(64)).execute(&db.pool).await.expect("root fixture");
    let mut digest = blake3::Hasher::new();
    digest.update(b"melodee-retained-file-chunk-v4\0");
    digest.update(&0_u64.to_le_bytes());
    digest.update(&11_u64.to_le_bytes());
    digest.update(b"later bytes");
    sqlx::query("INSERT INTO inbound_retained_baseline_chunks(baseline_id,stage_ordinal,chunk_index,chunk_length,digest) VALUES($1,2,0,11,$2)")
        .bind(baseline).bind(digest.finalize().to_hex().to_string()).execute(&db.pool).await.expect("chunk");
    sqlx::query("UPDATE inbound_retained_baseline_files SET stage_verity_sha256=repeat('c',64),stage_verity_device=repeat('1',16),stage_verity_inode=repeat('2',16) WHERE baseline_id=$1")
        .bind(baseline).execute(&db.pool).await.expect("synthetic verity evidence");
    sqlx::query(
        "UPDATE inbound_retained_baselines SET state='frozen',protected_coverage=true WHERE id=$1",
    )
    .bind(baseline)
    .execute(&db.pool)
    .await
    .expect("freeze");
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at,retained_source) VALUES($1,'fixture','fixture','staged','verified_copy',$2,clock_timestamp(),clock_timestamp(),true)")
        .bind(item.id).bind("a".repeat(64)).execute(&db.pool).await.expect("synthetic retained handoff");
    let lock_name = format!("stage-{}.lock", baseline.simple());
    sqlx::query("UPDATE inbound_retained_baselines SET state='authoritative',lifecycle_lock_name=$2,lifecycle_root_generation='unix:1:1',lifecycle_lock_generation='unix:1:2' WHERE id=$1")
        .bind(baseline).bind(&lock_name)
        .execute(&db.pool)
        .await
        .expect("publish fixture");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,'candidate','fixture',$3,clock_timestamp()-interval '1 second',clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(baseline).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
    assert!(
        melodee_ingestion::test_retained_recheck_turn(
            &config,
            &db.pool,
            &WorkItemStore,
            &WorkerId::new()
        )
        .await
        .expect("missing turn")
    );
    let missing: (i64, bool, Option<String>, Option<String>, i32) = sqlx::query_as("SELECT baseline_cursor_file,baseline_change_pending,lease_owner,baseline_unreadable_reason,baseline_unreadable_count FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(item.id).fetch_one(&db.pool).await.expect("missing cursor");
    assert_eq!(missing, (1, true, None, Some("missing".to_owned()), 1));
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        melodee_ingestion::test_retained_recheck_turn_without_directory_support(
            &config,
            &db.pool,
            &WorkItemStore,
            &WorkerId::new()
        )
        .await
        .expect("unsupported directory turn")
    );
    let unavailable: (Option<String>, bool, i64) = sqlx::query_as("SELECT baseline_directory_unavailable_reason,baseline_directory_blocked_until>clock_timestamp(),(SELECT scan_revision FROM inbound_retained_baseline_directories WHERE baseline_id=r.baseline_id AND stage_ordinal=0) FROM inbound_retained_sources r WHERE work_item_id=$1")
        .bind(item.id).fetch_one(&db.pool).await.expect("unsupported directory status");
    assert_eq!(
        unavailable,
        (Some("unsupported".to_owned()), true, 0),
        "unsupported must not acknowledge a directory scan"
    );
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        melodee_ingestion::test_retained_recheck_turn(
            &config,
            &db.pool,
            &WorkItemStore,
            &WorkerId::new()
        )
        .await
        .expect("later file turn")
    );
    let later: (i64, i64, bool) = sqlx::query_as("SELECT baseline_cursor_file,baseline_cursor_chunk,baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(item.id).fetch_one(&db.pool).await.expect("later cursor");
    assert_eq!(later, (2, 0, true));
    assert_eq!(
        fs::read(root.join("candidate/later")).expect("source intact"),
        b"later bytes"
    );
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_recheck_defers_ambiguous_source_roots_without_touching_either() {
    let fixture = tempfile::tempdir().expect("fixture");
    let mut sources = Vec::new();
    for name in ["a", "b"] {
        let root = fixture.path().join(name);
        fs::create_dir_all(root.join("candidate")).expect("candidate");
        fs::write(root.join("candidate/file"), name).expect("source");
        sources.push(InboundSource::from_root(root, None, Some(name.to_owned())).expect("source"));
    }
    let config = IngestionConfig::with_sources(sources);
    let db = common::TestDb::create("retained_recheck_ambiguous").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate','previous',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
    let worker = WorkerId::new();
    assert!(
        melodee_ingestion::test_retained_recheck_turn(&config, &db.pool, &WorkItemStore, &worker)
            .await
            .expect("turn")
    );
    for name in ["a", "b"] {
        assert_eq!(
            fs::read(fixture.path().join(name).join("candidate/file")).expect("source retained"),
            name.as_bytes()
        );
    }
    let row: (String, i32, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status,retry_count,lease_owner,last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("obligation");
    assert_eq!(
        row,
        ("active".to_owned(), 1, None, Some("ambiguous".to_owned()))
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items")
        .fetch_one(&db.pool)
        .await
        .expect("work items");
    assert_eq!(count, 1);
    fs::rename(
        fixture.path().join("a/candidate"),
        fixture.path().join("a/parked"),
    )
    .expect("temporarily unavailable occurrence");
    let one_source = IngestionConfig::with_sources(vec![config.sources[0].clone()]);
    sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp() WHERE work_item_id=$1")
        .bind(item.id).execute(&db.pool).await.expect("retry now");
    assert!(
        melodee_ingestion::test_retained_recheck_turn(
            &one_source,
            &db.pool,
            &WorkItemStore,
            &worker
        )
        .await
        .expect("missing turn")
    );
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("retry reason");
    assert_eq!(reason.as_deref(), Some("missing"));
    assert_eq!(
        fs::read(fixture.path().join("a/parked/file")).expect("parked bytes"),
        b"a"
    );
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_source_mutated_after_final_observation_is_held_on_retry() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(root.join("candidate")).expect("candidate");
    let source_file = root.join("candidate/file");
    fs::write(&source_file, b"before").expect("source");
    let source =
        InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source");
    let config = IngestionConfig::with_sources(vec![source.clone()]);
    let db = common::TestDb::create("retained_post_observation_mutation").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let engine = WorkEngineConfig::default();
    WorkItemStore
        .claim(&db.pool, &worker, &engine, now)
        .await
        .expect("claim processing lease");
    let (_, baseline, _) = melodee_ingestion::test_capture_provisional_custody_digest(
        &db.pool,
        item.id,
        &worker,
        engine.lease_duration,
        source,
        std::ffi::OsStr::new("candidate"),
    )
    .await
    .expect("baseline digest");
    let generation =
        melodee_ingestion::source_generation(&root.join("candidate")).expect("source generation");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate',$2,$3,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(generation).bind(baseline).execute(&db.pool).await.expect("obligation");
    assert!(
        melodee_ingestion::test_retained_recheck_turn_with_mutation(
            &config,
            &db.pool,
            &WorkItemStore,
            &worker,
            &source_file,
            b"change",
        )
        .await
        .expect("first turn")
    );
    assert_eq!(
        fs::read(&source_file).expect("changed source retained"),
        b"change"
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM inbound_retained_sources WHERE work_item_id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .expect("status");
    assert_eq!(
        status, "active",
        "the retained obligation survives the commit window"
    );
    for _ in 0..3 {
        sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp() WHERE work_item_id=$1 AND status='active'")
            .bind(item.id).execute(&db.pool).await.expect("make due");
        let _ = melodee_ingestion::test_retained_recheck_turn(
            &config,
            &db.pool,
            &WorkItemStore,
            &worker,
        )
        .await
        .expect("retry turn");
        let pending: bool = sqlx::query_scalar(
            "SELECT baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1",
        )
        .bind(item.id)
        .fetch_one(&db.pool)
        .await
        .expect("status");
        if pending {
            break;
        }
    }
    let row: (String, bool, Option<Uuid>, Option<String>) = sqlx::query_as("SELECT status,baseline_change_pending,superseded_by,last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(item.id).fetch_one(&db.pool).await.expect("final status");
    assert_eq!(
        row,
        (
            "active".to_owned(),
            true,
            None,
            Some("awaiting_intake".to_owned())
        ),
        "changed bytes must be signalled and held until safe intake exists"
    );
    assert_eq!(
        fs::read(&source_file).expect("source still retained"),
        b"change"
    );
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_root_replaced_after_final_check_is_held_without_deleting_either_tree() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    let parked = fixture.path().join("original-root");
    let replacement = fixture.path().join("replacement-root");
    fs::create_dir_all(root.join("candidate")).expect("original candidate");
    fs::create_dir_all(replacement.join("candidate")).expect("replacement candidate");
    fs::write(root.join("candidate/file"), b"before").expect("original bytes");
    fs::write(replacement.join("candidate/file"), b"change").expect("replacement bytes");
    let source =
        InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source");
    let config = IngestionConfig::with_sources(vec![source.clone()]);
    let db = common::TestDb::create("retained_post_check_root_swap").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let engine = WorkEngineConfig::default();
    WorkItemStore
        .claim(&db.pool, &worker, &engine, now)
        .await
        .expect("processing lease");
    let (_, baseline, _) = melodee_ingestion::test_capture_provisional_custody_digest(
        &db.pool,
        item.id,
        &worker,
        engine.lease_duration,
        source,
        std::ffi::OsStr::new("candidate"),
    )
    .await
    .expect("baseline");
    let generation =
        melodee_ingestion::source_generation(&root.join("candidate")).expect("generation");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate',$2,$3,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(generation).bind(baseline).execute(&db.pool).await.expect("obligation");
    assert!(
        melodee_ingestion::test_retained_recheck_turn_with_root_replacement(
            &config,
            &db.pool,
            &WorkItemStore,
            &worker,
            &root,
            &parked,
            &replacement,
        )
        .await
        .expect("root swap turn")
    );
    assert_eq!(
        fs::read(parked.join("candidate/file")).expect("old source retained"),
        b"before"
    );
    assert_eq!(
        fs::read(root.join("candidate/file")).expect("replacement retained"),
        b"change"
    );
    for _ in 0..3 {
        sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp() WHERE work_item_id=$1 AND status='active'")
            .bind(item.id).execute(&db.pool).await.expect("make due");
        let _ = melodee_ingestion::test_retained_recheck_turn(
            &config,
            &db.pool,
            &WorkItemStore,
            &worker,
        )
        .await
        .expect("retry");
        let pending: bool = sqlx::query_scalar(
            "SELECT baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1",
        )
        .bind(item.id)
        .fetch_one(&db.pool)
        .await
        .expect("status");
        if pending {
            break;
        }
    }
    let row: (String, bool, Option<Uuid>, Option<String>) = sqlx::query_as("SELECT status,baseline_change_pending,superseded_by,last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(item.id).fetch_one(&db.pool).await.expect("final status");
    assert_eq!(
        row,
        (
            "active".to_owned(),
            true,
            None,
            Some("awaiting_intake".to_owned())
        ),
        "changed bytes must be signalled and held until safe intake exists"
    );
    assert_eq!(
        fs::read(parked.join("candidate/file")).expect("old source retained"),
        b"before"
    );
    assert_eq!(
        fs::read(root.join("candidate/file")).expect("replacement retained"),
        b"change"
    );
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_scheduler_reobserves_unforced_after_restart_with_live_processing_lease() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(root.join("candidate")).expect("candidate");
    let source_file = root.join("candidate/file");
    fs::write(&source_file, b"before").expect("source");
    let source =
        InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source");
    let mut config = IngestionConfig::with_sources(vec![source.clone()]);
    config.reconciliation_interval = Duration::hours(1);
    let db = common::TestDb::create("retained_scheduler_restart").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let engine = WorkEngineConfig::default();
    WorkItemStore
        .claim(&db.pool, &worker, &engine, now)
        .await
        .expect("processing claim");
    let (_, baseline, _) = melodee_ingestion::test_capture_provisional_custody_digest(
        &db.pool,
        item.id,
        &worker,
        engine.lease_duration,
        source,
        std::ffi::OsStr::new("candidate"),
    )
    .await
    .expect("baseline");
    let generation =
        melodee_ingestion::source_generation(&root.join("candidate")).expect("generation");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate',$2,$3,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(generation).bind(baseline).execute(&db.pool).await.expect("obligation");
    let scheduler = melodee_ingestion::test_start_retained_scheduler(
        config.clone(),
        db.pool.clone(),
        WorkItemStore,
        Some((source_file.clone(), b"change".to_vec())),
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let completed: Option<String> = sqlx::query_scalar(
                "SELECT completed_outcome FROM inbound_retained_sources WHERE work_item_id=$1",
            )
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .expect("outcome");
            if completed.as_deref() == Some("unchanged") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("first observation commits naturally");
    tokio::time::timeout(std::time::Duration::from_secs(5), scheduler.stop())
        .await
        .expect("stop");
    assert_eq!(fs::read(&source_file).expect("source retained"), b"change");
    let restarted = melodee_ingestion::test_start_retained_scheduler(
        config,
        db.pool.clone(),
        WorkItemStore,
        None,
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let pending: bool = sqlx::query_scalar("SELECT baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1")
                .bind(item.id).fetch_one(&db.pool).await.expect("outcome");
            if pending { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }).await.expect("naturally due retry after restart");
    tokio::time::timeout(std::time::Duration::from_secs(5), restarted.stop())
        .await
        .expect("restart stop");
    let row: (String, Option<Uuid>, Option<String>) = sqlx::query_as("SELECT status,superseded_by,last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1")
        .bind(item.id).fetch_one(&db.pool).await.expect("safe hold");
    assert_eq!(
        row,
        (
            "active".to_owned(),
            None,
            Some("awaiting_intake".to_owned())
        )
    );
    let owner: Option<String> =
        sqlx::query_scalar("SELECT lease_owner FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .expect("processing owner");
    assert_eq!(
        owner.as_deref(),
        Some(worker.as_str()),
        "retained observation must not claim or clear processing lease"
    );
    assert_eq!(fs::read(&source_file).expect("source retained"), b"change");
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn retained_scheduler_holds_deleted_work_item_against_ordinary_discovery() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(root.join("candidate")).expect("candidate");
    let source_file = root.join("candidate/file");
    fs::write(&source_file, b"retained").expect("source");
    let source = InboundSource::from_root(root, None, Some("test".to_owned())).expect("source");
    let config = IngestionConfig::with_sources(vec![source]);
    let db = common::TestDb::create("retained_scheduler_deleted").await;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate','previous',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
    sqlx::query("DELETE FROM release_work_items WHERE id=$1")
        .bind(item.id)
        .execute(&db.pool)
        .await
        .expect("remove prior work item");
    let scheduler = melodee_ingestion::test_start_retained_scheduler(
        config,
        db.pool.clone(),
        WorkItemStore,
        None,
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let pending: bool = sqlx::query_scalar("SELECT baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1")
                .bind(item.id).fetch_one(&db.pool).await.expect("outcome");
            if pending { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }).await.expect("natural retained-source hold");
    tokio::time::timeout(std::time::Duration::from_secs(5), scheduler.stop())
        .await
        .expect("stop");
    let active: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM release_work_items WHERE source_library_id IS NULL AND source_path='candidate' AND inbound_active")
        .fetch_all(&db.pool).await.expect("active occurrences");
    assert!(
        active.is_empty(),
        "ordinary discovery must not create a destructive successor"
    );
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("reason");
    assert_eq!(reason.as_deref(), Some("awaiting_intake"));
    assert_eq!(fs::read(source_file).expect("source retained"), b"retained");
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn disabled_ingestion_does_not_claim_due_retained_obligations() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(root.join("candidate")).expect("candidate");
    fs::write(root.join("candidate/file"), b"retained").expect("source");
    let source =
        InboundSource::from_root(root.clone(), None, Some("test".to_owned())).expect("source");
    let mut config = IngestionConfig::with_sources(vec![source]);
    config.enabled = false;
    let db = common::TestDb::create("retained_scheduler_disabled").await;
    let now = time::OffsetDateTime::now_utc();
    let item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate','previous',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
    let scheduler =
        melodee_ingestion::start_reconciliation_loop(config, db.pool.clone(), WorkItemStore);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), scheduler.stop())
        .await
        .expect("stop");
    let row: (String, Option<String>, i32) = sqlx::query_as(
        "SELECT status,lease_owner,retry_count FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("obligation");
    assert_eq!(row, ("active".to_owned(), None, 0));
    assert_eq!(
        fs::read(root.join("candidate/file")).expect("source"),
        b"retained"
    );
    db.drop().await;
}

#[cfg(all(feature = "test-support", target_os = "linux"))]
#[tokio::test]
async fn stopping_retained_scheduler_reaps_active_paced_reader_without_publication() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("inbound");
    fs::create_dir_all(root.join("candidate")).expect("candidate");
    let source_file = root.join("candidate/file");
    let bytes = vec![0x5a; 4 * 1024 * 1024];
    fs::write(&source_file, &bytes).expect("source");
    let source = InboundSource::from_root(root, None, Some("test".to_owned())).expect("source");
    let mut config = IngestionConfig::with_sources(vec![source]);
    config.retained_recheck_mib_per_second = 1;
    let db = common::TestDb::create("retained_scheduler_stop_reader").await;
    let now = time::OffsetDateTime::now_utc();
    let item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate','previous',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
    let scheduler =
        melodee_ingestion::start_reconciliation_loop(config, db.pool.clone(), WorkItemStore);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let source_open = fs::read_dir("/proc/self/fd")
                .expect("fd directory")
                .filter_map(Result::ok)
                .filter_map(|entry| fs::read_link(entry.path()).ok())
                .any(|path| path == source_file);
            if source_open {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("retained reader reached actual file");
    tokio::time::timeout(std::time::Duration::from_secs(2), scheduler.stop())
        .await
        .expect("prompt stop");
    assert!(
        melodee_ingestion::test_custody_producers_quiescent().await,
        "reader must terminate and return its reaper capacity"
    );
    let result: (String, Option<String>) = sqlx::query_as(
        "SELECT status,completed_outcome FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("obligation");
    assert_eq!(result, ("active".to_owned(), None));
    assert_eq!(fs::read(source_file).expect("source retained"), bytes);
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[tokio::test]
async fn provisional_stage_baseline_uses_same_verified_stream_and_bounded_chunk_pages() {
    use std::os::unix::fs::PermissionsExt;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("protected runtime fixture parent");
    let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    let source_path = fixture.path().join("source");
    let staging_root = fixture.path().join("staging");
    let private = staging_root.join("releases").join(item.id.to_string());
    fs::create_dir_all(source_path.join("candidate")).expect("source");
    fs::create_dir_all(private.join("source/disc")).expect("stage");
    fs::create_dir_all(private.join("source/empty-dir")).expect("empty directory");
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private mode");
    let bytes = vec![0x5a; 128 * 64 * 1024 + 17];
    fs::write(private.join("source/disc/track"), &bytes).expect("staged file");
    fs::write(private.join("source/empty"), []).expect("empty file");
    let source =
        InboundSource::from_root(source_path, None, Some("test".to_owned())).expect("source");
    let db = common::TestDb::create("retained_staged_baseline").await;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let engine = WorkEngineConfig::default();
    WorkItemStore
        .claim(&db.pool, &worker, &engine, now)
        .await
        .expect("claim");
    let service_uid = rustix::process::geteuid().as_raw();
    let producer_uid = if service_uid == 1 { 2 } else { 1 };
    let (baseline, files, chunks) = melodee_ingestion::test_verify_staging_baseline(
        &db.pool,
        item.id,
        &worker,
        engine.lease_duration,
        source,
        &private,
        &staging_root,
        producer_uid,
    )
    .await
    .expect("provisional verified baseline");
    assert_eq!((files, chunks), (2, 129));
    let state: String =
        sqlx::query_scalar("SELECT state FROM inbound_retained_baselines WHERE id=$1")
            .bind(baseline)
            .fetch_one(&db.pool)
            .await
            .expect("baseline state");
    assert_eq!(state, "frozen", "capture must remain provisional");
    let directory_paths: Vec<String> = sqlx::query_scalar(
        "SELECT path_key FROM inbound_retained_baseline_directories WHERE baseline_id=$1 ORDER BY path_key COLLATE \"C\"",
    )
    .bind(baseline)
    .fetch_all(&db.pool)
    .await
    .expect("verified directories");
    assert_eq!(directory_paths, ["", "disc", "empty-dir"]);
    let actual: (i64, i32, String) = sqlx::query_as("SELECT c.chunk_index,c.chunk_length,c.digest FROM inbound_retained_baseline_chunks c JOIN inbound_retained_baseline_files f USING(baseline_id,stage_ordinal) WHERE c.baseline_id=$1 AND f.path_key='disc/track' ORDER BY c.chunk_index DESC LIMIT 1")
        .bind(baseline).fetch_one(&db.pool).await.expect("last chunk");
    let mut preimage = b"melodee-retained-file-chunk-v4\0".to_vec();
    preimage.extend_from_slice(&128_u64.to_le_bytes());
    preimage.extend_from_slice(&17_u64.to_le_bytes());
    preimage.extend_from_slice(&bytes[128 * 64 * 1024..]);
    assert_eq!(
        actual,
        (128, 17, blake3::hash(&preimage).to_hex().to_string())
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_staging_handoffs WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("receipt count");
    assert_eq!(receipts, 0);
    assert_eq!(
        fs::read(private.join("source/disc/track")).expect("stage retained"),
        bytes
    );
    db.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[tokio::test]
async fn provisional_baseline_budget_failure_reaps_reader_without_publishing() {
    use std::os::unix::fs::PermissionsExt;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("protected runtime fixture parent");
    let fixture = tempfile::tempdir_in(runtime).expect("owned fixture");
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    let source_path = fixture.path().join("source");
    let staging_root = fixture.path().join("staging");
    let private = staging_root.join("releases").join(item.id.to_string());
    fs::create_dir_all(source_path.join("candidate")).expect("source");
    fs::create_dir_all(private.join("source")).expect("stage");
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("private mode");
    let bytes = vec![0x5a; 130 * 64 * 1024 + 17];
    let file = private.join("source/track");
    fs::write(&file, &bytes).expect("staged file");
    let source =
        InboundSource::from_root(source_path, None, Some("test".to_owned())).expect("source");
    let db = common::TestDb::create("retained_stage_baseline_budget_failure").await;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let engine = WorkEngineConfig::default();
    WorkItemStore
        .claim(&db.pool, &worker, &engine, now)
        .await
        .expect("claim");
    sqlx::query("UPDATE inbound_retained_baseline_budget SET max_rows=1 WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("quota");
    let service_uid = rustix::process::geteuid().as_raw();
    let producer_uid = if service_uid == 1 { 2 } else { 1 };
    let outcome = melodee_ingestion::test_verify_staging_baseline(
        &db.pool,
        item.id,
        &worker,
        engine.lease_duration,
        source,
        &private,
        &staging_root,
        producer_uid,
    )
    .await;
    assert!(matches!(
        outcome,
        Err(melodee_ingestion::AcquisitionError::Persistence(
            melodee_persistence::WorkStoreError::RetainedSourceBudgetExhausted
        ))
    ));
    assert!(
        melodee_ingestion::test_custody_producers_quiescent().await,
        "failed baseline writer must reap its producer and release admission"
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM inbound_retained_baselines WHERE work_item_id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .expect("baseline state");
    assert_eq!(
        state, "capturing",
        "partial evidence cannot be frozen or authoritative"
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_staging_handoffs WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("receipts");
    assert_eq!(receipts, 0);
    assert_eq!(fs::read(file).expect("staged bytes retained"), bytes);
    db.drop().await;
}

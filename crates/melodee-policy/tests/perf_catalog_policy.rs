//! Representative production policy snapshot/evaluation and dry-run benchmark.
//! Invoked only inside scripts/perf_representative.py's owned database.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

// The shared handoff fixture uses this adapter and does not own the database.
mod common {
    use melodee_domain::{IngestionState, ReleaseWorkItem};
    use melodee_persistence::{WorkEngineConfig, WorkItemStore};
    use sqlx::PgPool;
    use time::OffsetDateTime;
    use uuid::Uuid;
    pub const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;
    pub struct TestDb {
        pub pool: PgPool,
    }
    pub fn test_config() -> WorkEngineConfig {
        WorkEngineConfig::default()
    }
    pub async fn seed_ready_item(store: &WorkItemStore, pool: &PgPool, path: &str) -> Uuid {
        let item = ReleaseWorkItem::discovered(path.to_string(), None, Uuid::now_v7(), T0);
        let id = item.id;
        store.create(pool, &item).await.unwrap();
        store
            .system_transition(pool, id, IngestionState::Settling, Some("test"), T0)
            .await
            .unwrap();
        store
            .system_transition(pool, id, IngestionState::Ready, Some("test"), T0)
            .await
            .unwrap();
        id
    }
}

use melodee_persistence::PolicyStore;
use melodee_policy::{PolicyConfig, PolicyEngine, evaluate_catalog_context};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use time::OffsetDateTime;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

const RELEASES: u32 = 8;
const REPEATS: usize = 3;

#[derive(Default)]
struct Counts {
    statements: AtomicU64,
    rows: AtomicU64,
    query_us: AtomicU64,
}
#[derive(Default)]
struct Fields {
    rows: u64,
    micros: u64,
}
impl Visit for Fields {
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "rows_returned" {
            self.rows = value;
        }
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.micros = (value * 1_000_000.0) as u64;
        }
    }
}
struct SqlCounts(Arc<Counts>);
impl<S: Subscriber> Layer<S> for SqlCounts {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        self.0.statements.fetch_add(1, Ordering::Relaxed);
        self.0.rows.fetch_add(fields.rows, Ordering::Relaxed);
        self.0.query_us.fetch_add(fields.micros, Ordering::Relaxed);
    }
}
fn snapshot(c: &Counts) -> (u64, u64, u64) {
    (
        c.statements.load(Ordering::Relaxed),
        c.rows.load(Ordering::Relaxed),
        c.query_us.load(Ordering::Relaxed),
    )
}
fn report(
    label: &str,
    iteration: usize,
    before: (u64, u64, u64),
    c: &Counts,
    start: Instant,
    contexts: &[melodee_persistence::PolicyCatalogContext],
    before_rss: Option<u64>,
) {
    let after = snapshot(c);
    let matches: usize = contexts
        .iter()
        .map(|x| x.catalog_acoustic_fingerprints.len())
        .sum();
    let digest_bytes: usize = contexts
        .iter()
        .flat_map(|x| x.catalog_acoustic_fingerprints.iter())
        .map(String::len)
        .sum();
    let tracks: usize = contexts.iter().map(|x| x.tracks.len()).sum();
    println!(
        "case={label} iteration={iteration} elapsed_ms={:.3} sql_statements={} sql_rows_returned={} sql_elapsed_us={} contexts={} track_facts={} retained_acoustic_digests={} retained_acoustic_bytes={} rss_before_kib={before_rss:?} rss_after_kib={:?} high_water_rss_kib={:?}",
        start.elapsed().as_secs_f64() * 1000.0,
        after.0 - before.0,
        after.1 - before.1,
        after.2 - before.2,
        contexts.len(),
        tracks,
        matches,
        digest_bytes,
        proc_kib("VmRSS:"),
        proc_kib("VmHWM:")
    );
}
fn proc_kib(key: &str) -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(key)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

fn plan_nodes(node: &serde_json::Value, result: &mut Vec<serde_json::Value>) {
    // Only planner metadata is copied: predicates can contain fixture UUIDs
    // and digests and should not enter shared evidence.
    let mut out = serde_json::Map::new();
    for key in [
        "Node Type",
        "Relation Name",
        "Index Name",
        "Plan Rows",
        "Actual Rows",
        "Actual Loops",
        "Shared Hit Blocks",
        "Shared Read Blocks",
        "Temp Written Blocks",
    ] {
        if let Some(value) = node.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    result.push(serde_json::Value::Object(out));
    if let Some(children) = node.get("Plans").and_then(serde_json::Value::as_array) {
        for child in children {
            plan_nodes(child, result);
        }
    }
}

async fn explain_acoustic(pool: &sqlx::PgPool, release: uuid::Uuid, digest: &str) {
    // Diagnostic only: compare the actual old and scoped normal-load SQL;
    // measurements above exercise the full production policy methods.
    let common = "SELECT DISTINCT evidence.payload->>'fingerprint_digest' \
        FROM audio_identity_evidence evidence JOIN tracks owner ON owner.id=evidence.track_id \
        WHERE evidence.evidence_kind='acoustic_fingerprint' \
          AND evidence.payload ? 'fingerprint_digest' AND owner.release_id <> $1";
    for scoped in [false, true] {
        let sql = format!(
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {common}{}",
            if scoped {
                " AND evidence.payload->>'fingerprint_digest' = ANY($2)"
            } else {
                ""
            }
        );
        let mut query =
            sqlx::query_scalar::<_, serde_json::Value>(sqlx::AssertSqlSafe(sql)).bind(release);
        if scoped {
            query = query.bind(vec![digest.to_string()]);
        }
        let plan = query
            .fetch_one(pool)
            .await
            .expect("acoustic diagnostic plan");
        let root = &plan[0]["Plan"];
        // The JSON plan contains synthetic UUIDs/digests in predicates: print only
        // aggregate fields to keep committed evidence free of identifiers.
        println!(
            "acoustic_plan={} node={} estimated_cost={} actual_rows={} elapsed_ms={} shared_hits={} shared_reads={} temp_written={}",
            if scoped { "scoped" } else { "global" },
            root["Node Type"],
            root["Total Cost"],
            root["Actual Rows"],
            plan[0]["Execution Time"],
            root["Shared Hit Blocks"],
            root["Shared Read Blocks"],
            root["Temp Written Blocks"]
        );
        let mut nodes = Vec::new();
        plan_nodes(root, &mut nodes);
        println!(
            "acoustic_plan_nodes={} {}",
            if scoped { "scoped" } else { "global" },
            serde_json::to_string(&nodes).unwrap()
        );
    }
}

#[tokio::test]
#[ignore = "requires an invocation-owned representative fixture and release build"]
async fn measure_production_policy() {
    assert!(std::env::var_os("MELODEE__STORAGE__ASSET_ROOT").is_some());
    let url = std::env::var("MELODEE__DATABASE__URL").unwrap();
    let options = PgConnectOptions::from_str(&url).unwrap();
    let counts = Arc::new(Counts::default());
    let subscriber = tracing_subscriber::registry().with(SqlCounts(counts.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        name.strip_prefix("melodee_template_")
            .is_some_and(|s| s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()))
    );
    assert_eq!(std::env::var("MELODEE_PERF_OWNED_DATABASE").unwrap(), name);
    let background: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM artists), (SELECT count(*) FROM releases), (SELECT count(*) FROM tracks)").fetch_one(&pool).await.unwrap();
    let db = common::TestDb { pool: pool.clone() };
    let mut ids = Vec::new();
    for n in 0..RELEASES {
        let title = format!("Policy Benchmark Release {n}");
        let artist = format!("Policy Benchmark Artist {n}");
        let presentation = catalog_common::CatalogFixturePresentation {
            release_title: &title,
            release_artist: &artist,
            track_title: "Policy Track",
            track_artist: &artist,
            release_date: "2025-01-01",
        };
        ids.push(
            phase5_common::seed_cataloged_release(&db, 80_000 + n, presentation)
                .await
                .release_id(),
        );
    }
    let store = PolicyStore;
    let engine = PolicyEngine::new(PolicyConfig::default()).unwrap();
    let now = OffsetDateTime::now_utc();
    // The production bounded dry-run loader (`load_test_contexts`) measures the
    // earliest `catalog_imports` rows. The representative fixture publishes ACL
    // placeholder Releases before this test runs, so the measured scope is not
    // necessarily the eight Releases seeded above. Discover the real scope once
    // and measure a subject that is provably inside it instead of assuming the
    // seeded batch is first.
    let scope = store.load_test_contexts(&pool, RELEASES).await.unwrap();
    assert_eq!(scope.len(), RELEASES as usize);
    let scope_ids = scope
        .iter()
        .map(|context| context.claim.release_id.into_uuid())
        .collect::<Vec<_>>();
    // Add one actual cross-Release acoustic match; fixture has 600K unrelated
    // digests. Pick the first in-scope Release that actually owns an acoustic
    // digest, so the benchmark fails loudly if the fixture provides none.
    let mut subject = None;
    for context in &scope {
        let snapshot = store
            .load_snapshot(&pool, context.claim.release_id)
            .await
            .unwrap()
            .unwrap();
        if let Some(digest) = snapshot
            .tracks
            .iter()
            .find_map(|track| track.audio_fingerprint_digest.clone())
        {
            subject = Some((context.claim.release_id, digest));
            break;
        }
    }
    let (subject_id, digest) = subject
        .expect("representative fixture must expose an acoustic digest inside the dry-run scope");
    drop(scope);
    sqlx::query("INSERT INTO audio_identity_evidence(id,source_audio_identity_id,track_id,evidence_kind,polarity,confidence,source,payload,created_at) SELECT uuidv7(), audio_identity_id, id, 'acoustic_fingerprint','supports',9000,'policy-benchmark',jsonb_build_object('fingerprint_digest',$1),now() FROM tracks WHERE release_id <> ALL($2) ORDER BY id LIMIT 1")
        .bind(&digest).bind(&scope_ids)
        .execute(&pool).await.unwrap();
    println!(
        "fixture_background_artists={} fixture_background_releases={} fixture_background_tracks={} seeded_policy_releases={} dry_run_scope_releases={} samples_per_case={} baseline_acoustic_rows={} sample_input_digests=1 sample_cross_release_matches=1",
        background.0,
        background.1,
        background.2,
        ids.len(),
        scope_ids.len(),
        REPEATS,
        background.2
    );
    // Bound any baseline query individually; the wrapper additionally sets a whole-workload limit.
    sqlx::query("SET statement_timeout = '30s'")
        .execute(&pool)
        .await
        .unwrap();
    let mut expected_disposition = None;
    for iteration in 0..REPEATS {
        let before = snapshot(&counts);
        let rss = proc_kib("VmRSS:");
        let start = Instant::now();
        let context = store
            .load_snapshot(&pool, subject_id)
            .await
            .unwrap()
            .unwrap();
        let result = evaluate_catalog_context(&engine, &context, now).unwrap();
        if let Some(expected) = expected_disposition {
            assert_eq!(result.decision.disposition, expected);
        }
        expected_disposition = Some(result.decision.disposition);
        assert!(context.catalog_acoustic_fingerprints.contains(&digest));
        report(
            "single_evaluate",
            iteration,
            before,
            &counts,
            start,
            &[context],
            rss,
        );
    }
    for iteration in 0..REPEATS {
        let before = snapshot(&counts);
        let rss = proc_kib("VmRSS:");
        let start = Instant::now();
        let contexts = store.load_test_contexts(&pool, RELEASES).await.unwrap();
        assert_eq!(contexts.len(), RELEASES as usize);
        for context in &contexts {
            let evaluation = evaluate_catalog_context(&engine, context, now).unwrap();
            if context.claim.release_id == subject_id {
                assert_eq!(Some(evaluation.decision.disposition), expected_disposition);
            }
        }
        assert!(
            contexts
                .iter()
                .find(|x| x.claim.release_id == subject_id)
                .unwrap()
                .catalog_acoustic_fingerprints
                .contains(&digest)
        );
        report(
            "dry_run_load_and_evaluate",
            iteration,
            before,
            &counts,
            start,
            &contexts,
            rss,
        );
    }
    explain_acoustic(&pool, subject_id.into_uuid(), &digest).await;
    pool.close().await;
}

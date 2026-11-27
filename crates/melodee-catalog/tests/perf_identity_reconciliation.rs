//! Release-build identity discovery and production drainer benchmark.
//! Run only through the invocation-owned representative fixture.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    str::FromStr,
    sync::{Arc, Mutex},
    time::Instant,
};

use melodee_catalog::reconcile_affected_identities_once;
use melodee_persistence::CatalogStore;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};
use uuid::Uuid;

#[derive(Default)]
struct StatementVisitor(Option<String>);

impl Visit for StatementVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "db.statement" && !value.is_empty() {
            self.0 = Some(value.trim().to_owned());
        }
    }
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

struct CaptureStatements(Arc<Mutex<Vec<String>>>);
impl<S: Subscriber> Layer<S> for CaptureStatements {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            let mut visitor = StatementVisitor::default();
            event.record(&mut visitor);
            if let Some(statement) = visitor.0 {
                if statement.starts_with("WITH ") {
                    let mut statements = self.0.lock().expect("statement capture lock");
                    if statements.len() == 8 {
                        statements.remove(0);
                    }
                    statements.push(statement);
                }
            }
        }
    }
}

async fn explain_captured(
    pool: &PgPool,
    captured: &Arc<Mutex<Vec<String>>>,
    name: &str,
    seeds: &[i64],
    global: bool,
) {
    let statements = std::mem::take(&mut *captured.lock().expect("statement capture lock"));
    let statement = statements
        .into_iter()
        .rev()
        .find(|sql| sql.starts_with("WITH "))
        .expect("production candidate SQL captured from SQLx tracing");
    assert!(
        !statement.contains(';'),
        "only one captured statement may be explained"
    );
    // This SQL is captured from the just-executed production query, not user
    // input. Bind the exact same seed IDs and candidate limit as that query.
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {statement}");
    let mut query = sqlx::query_scalar::<_, serde_json::Value>(sqlx::AssertSqlSafe(explain));
    if global {
        query = query.bind(64_i64);
    } else if name == "strong_release" {
        query = query.bind(seeds).bind(64_i64);
    } else {
        query = query.bind(64_i64).bind(seeds);
    }
    match query.fetch_one(pool).await {
        Ok(plan) => println!(
            "identity_plan={name} scope={} {plan}",
            if global { "global" } else { "scoped" }
        ),
        Err(error) if error.to_string().contains("statement timeout") => {
            println!(
                "identity_plan={name} scope={} status=censored_statement_timeout",
                if global { "global" } else { "scoped" }
            )
        }
        Err(error) => panic!("{name} plan: {error}"),
    }
}

async fn measure_scrobble_cursor_plans(pool: &PgPool) {
    // Extract the SELECT body from the production DECLARE statement so this
    // probe follows the exact query as it changes. It measures full SELECT
    // execution, not incremental cursor FETCH latency or lock behavior.
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../melodee-persistence/src/native_user_state.rs"
    ));
    let select = source
        .split_once("DECLARE scrobble_artists NO SCROLL CURSOR FOR ")
        .and_then(|(_, rest)| rest.split_once('"').map(|(sql, _)| sql))
        .expect("production scrobble cursor SQL");
    assert!(select.starts_with("SELECT ") && !select.contains(';'));
    let tracks: Vec<Uuid> = sqlx::query_scalar("SELECT t.id FROM tracks t JOIN releases r ON r.id=t.release_id WHERE r.title NOT IN ('Release 1','Release 2') ORDER BY t.id LIMIT 2")
        .fetch_all(pool).await.expect("select cursor benchmark Tracks");
    assert_eq!(tracks.len(), 2);
    sqlx::query("INSERT INTO track_artists (id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) SELECT uuidv7(),$1,a.id,'primary',(1 + row_number() OVER (ORDER BY a.id))::integer,a.canonical_name,'',now() FROM artists a WHERE NOT EXISTS (SELECT 1 FROM track_artists credit WHERE credit.track_id=$1 AND credit.artist_id=a.id) ORDER BY a.id LIMIT 1000")
        .bind(tracks[1]).execute(pool).await.expect("create bounded pathological credit fixture");
    for (name, track) in [("ordinary", tracks[0]), ("pathological", tracks[1])] {
        let credits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM track_artists WHERE track_id=$1 AND role='primary'",
        )
        .bind(track)
        .fetch_one(pool)
        .await
        .expect("count cursor fixture credits");
        let query = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {select}");
        let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .bind(track)
            .fetch_one(pool)
            .await
            .expect("explain scrobble cursor SELECT body");
        println!("scrobble_cursor_plan={name} primary_credit_rows={credits} {plan}");
    }
}

async fn timed<T>(
    name: &str,
    future: impl std::future::Future<Output = Result<Vec<T>, melodee_persistence::CatalogStoreError>>,
) -> Option<Vec<T>> {
    let start = Instant::now();
    match future.await {
        Ok(rows) => {
            println!(
                "identity_query={name} status=completed elapsed_ms={:.3} rows={}",
                start.elapsed().as_secs_f64() * 1000.0,
                rows.len()
            );
            Some(rows)
        }
        Err(error) if error.to_string().contains("statement timeout") => {
            println!(
                "identity_query={name} status=censored_statement_timeout elapsed_ms={:.3}",
                start.elapsed().as_secs_f64() * 1000.0
            );
            None
        }
        Err(error) => panic!("{name}: {error}"),
    }
}

#[tokio::test]
#[ignore = "requires an invocation-owned representative fixture and release build"]
async fn measure_scoped_identity_discovery_and_production_drain() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(CaptureStatements(captured.clone())),
    )
    .expect("install statement capture");
    let url = std::env::var("MELODEE__DATABASE__URL").expect("fixture URL");
    let options = PgConnectOptions::from_str(&url)
        .expect("valid fixture URL")
        .application_name("melodee-rm09-identity");
    let pool: PgPool = PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET statement_timeout='20s'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .expect("connect fixture");
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("read database");
    assert!(
        db.strip_prefix("melodee_template_")
            .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    );
    assert_eq!(
        std::env::var("MELODEE_PERF_OWNED_DATABASE").ok().as_deref(),
        Some(db.as_str())
    );
    let sizes: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM artists), (SELECT count(*) FROM releases), (SELECT count(*) FROM tracks)")
        .fetch_one(&pool).await.expect("count fixture");
    println!(
        "fixture_artists={} fixture_releases={} fixture_tracks={} statement_timeout_seconds=20",
        sizes.0, sizes.1, sizes.2
    );
    measure_scrobble_cursor_plans(&pool).await;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM artists WHERE canonical_name IN ('Artist 1', 'Artist 2') ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("load benchmark Artists");
    assert_eq!(ids.len(), 2);
    sqlx::query("UPDATE artists SET status='active' WHERE id=ANY($1)")
        .bind(&ids)
        .execute(&pool)
        .await
        .expect("prepare active identities");
    sqlx::query("INSERT INTO artist_external_identities (id,artist_id,entity_type,provider,external_id,provider_unique_scope,confidence,evidence_source,is_locked,created_at) SELECT uuidv7(), id, 'artist', 'musicbrainz', 'e8112000-0000-7000-8000-000000000001', false, 9500, 'benchmark-fixture', false, now() FROM unnest($1::uuid[]) id")
        .bind(&ids).execute(&pool).await.expect("create shared strong evidence");
    // Add one repeated, provisional billing group with actual observations so
    // the name and repeated-credit comparisons exercise non-empty results.
    let releases: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM releases WHERE title IN ('Release 1', 'Release 2') ORDER BY title",
    )
    .fetch_all(&pool)
    .await
    .expect("load fixture Releases");
    assert_eq!(releases.len(), 2);
    let mut affected_ids = ids.clone();
    for &release_id in &releases {
        let artist_id = Uuid::now_v7();
        let observed_id = Uuid::now_v7();
        let decision_id = Uuid::now_v7();
        sqlx::query("INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name, artist_type, status, created_at, updated_at) VALUES ($1,$2,'Benchmark Context Artist','benchmark context artist','Benchmark Context Artist','group','active',now(),now())")
            .bind(artist_id).bind(Uuid::now_v7()).execute(&pool).await.expect("insert contextual Artist");
        sqlx::query("INSERT INTO observed_artists (id,release_id,scope,source_ordinal,raw_value,normalized_value,display_value,source_pointer,source_payload,created_at) VALUES ($1,$2,'release',0,'Benchmark Context Artist','benchmark context artist','Benchmark Context Artist','benchmark:identity','{}'::jsonb,now())")
            .bind(observed_id).bind(release_id).execute(&pool).await.expect("insert contextual observation");
        sqlx::query("INSERT INTO artist_match_decisions (id,observed_artist_id,artist_id,outcome,authority,confidence,reason,is_locked,payload,created_at) VALUES ($1,$2,$3,'confirmed','automated',9500,'benchmark',false,'{}'::jsonb,now())")
            .bind(decision_id).bind(observed_id).bind(artist_id).execute(&pool).await.expect("insert contextual decision");
        sqlx::query("INSERT INTO artist_match_current (observed_artist_id,decision_id,updated_at) VALUES ($1,$2,now())")
            .bind(observed_id).bind(decision_id).execute(&pool).await.expect("set contextual decision");
        sqlx::query("UPDATE release_artists SET artist_id=$2,observed_artist_id=$3 WHERE release_id=$1 AND role='primary'")
            .bind(release_id).bind(artist_id).bind(observed_id).execute(&pool).await.expect("set contextual billing");
        affected_ids.push(artist_id);
    }
    // Two historical Track-only identities on the same Release exercise the
    // production same-Release predicate with a nonempty candidate, rather
    // than interpreting an empty-query plan as representative evidence.
    let track: Uuid =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1 ORDER BY id LIMIT 1")
            .bind(releases[0])
            .fetch_one(&pool)
            .await
            .expect("select same-Release Track");
    for ordinal in 0..2_i32 {
        let artist_id = Uuid::now_v7();
        let observed_id = Uuid::now_v7();
        let decision_id = Uuid::now_v7();
        sqlx::query("INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at) VALUES ($1,$2,'Benchmark Track Guest','benchmark track guest','Benchmark Track Guest','unknown','active',now(),now())")
            .bind(artist_id).bind(Uuid::now_v7()).execute(&pool).await.expect("insert Track-only Artist");
        sqlx::query("INSERT INTO observed_artists (id,release_id,track_id,scope,source_ordinal,raw_value,normalized_value,display_value,source_pointer,source_payload,created_at) VALUES ($1,$2,$3,'track',$4,'Benchmark Track Guest','benchmark track guest','Benchmark Track Guest',$5,'{}'::jsonb,now())")
            .bind(observed_id).bind(releases[0]).bind(track).bind(10_000 + ordinal)
            .bind(format!("benchmark:identity-track:{ordinal}"))
            .execute(&pool).await.expect("insert Track-only observation");
        sqlx::query("INSERT INTO artist_match_decisions (id,observed_artist_id,artist_id,outcome,authority,confidence,reason,is_locked,payload,created_at) VALUES ($1,$2,$3,'confirmed','automated',9500,'benchmark',false,'{}'::jsonb,now())")
            .bind(decision_id).bind(observed_id).bind(artist_id).execute(&pool).await.expect("insert Track-only decision");
        sqlx::query("INSERT INTO artist_match_current (observed_artist_id,decision_id,updated_at) VALUES ($1,$2,now())")
            .bind(observed_id).bind(decision_id).execute(&pool).await.expect("set Track-only decision");
        sqlx::query("INSERT INTO track_artists (id,track_id,artist_id,observed_artist_id,role,presentation_order,display_value,join_phrase,created_at) VALUES ($1,$2,$3,$4,'primary',$5,'Benchmark Track Guest','',now())")
            .bind(Uuid::now_v7()).bind(track).bind(artist_id).bind(observed_id).bind(10_000 + ordinal)
            .execute(&pool).await.expect("insert Track-only credit");
        affected_ids.push(artist_id);
    }
    sqlx::query("INSERT INTO release_external_identities (id,release_id,provider,external_id,trusted,confidence,evidence_source,is_locked,observed_at) SELECT uuidv7(),id,'musicbrainz','20000000-0000-7000-8000-000000000001',true,9500,'benchmark-fixture',false,now() FROM unnest($1::uuid[]) id")
        .bind(&releases).execute(&pool).await.expect("create shared strong Release evidence");
    let mut seeds: Vec<i64> = sqlx::query_scalar("INSERT INTO catalog_identity_reconciliation_seed (artist_id,normalized_name,created_at) SELECT id,normalized_name,now() FROM artists WHERE id=ANY($1) RETURNING id")
        .bind(&affected_ids).fetch_all(&pool).await.expect("persist affected seeds");
    seeds.extend(sqlx::query_scalar::<_, i64>("INSERT INTO catalog_identity_reconciliation_seed (release_id,created_at) SELECT id,now() FROM unnest($1::uuid[]) id RETURNING id")
        .bind(&releases).fetch_all(&pool).await.expect("persist affected Release seeds"));
    let store = CatalogStore;
    let relations: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM observed_artists), (SELECT count(*) FROM artist_match_current),
                (SELECT count(*) FROM track_artists), (SELECT count(*) FROM artist_external_identities),
                (SELECT count(*) FROM release_external_identities)",
    )
    .fetch_one(&pool)
    .await
    .expect("count identity relationship fixture");
    println!(
        "identity_fixture_relations observations={} current_decisions={} track_credits={} artist_external_ids={} release_external_ids={}",
        relations.0, relations.1, relations.2, relations.3, relations.4
    );
    macro_rules! compare {
        ($label:literal, $scoped:expr, $global:expr) => {{
            captured.lock().expect("statement capture lock").clear();
            let scoped = timed(concat!($label, "_scoped"), $scoped)
                .await
                .expect("scoped query timed out");
            if matches!(
                $label,
                "strong_artist"
                    | "same_release"
                    | "strong_release"
                    | "repeated_credit"
                    | "name_proposal"
            ) {
                assert!(
                    !scoped.is_empty(),
                    "fixture must exercise a non-empty {} candidate group",
                    $label
                );
            }
            explain_captured(&pool, &captured, $label, &seeds, false).await;
            captured.lock().expect("statement capture lock").clear();
            if let Some(global) = timed(concat!($label, "_global"), $global).await {
                assert!(global.len() < 64, "global oracle result is truncated");
                assert_eq!(scoped, global, "fixture oracle differs for {}", $label);
                println!(
                    "identity_oracle={} outcome=equivalent returned_rows={}",
                    $label,
                    global.len()
                );
                explain_captured(&pool, &captured, $label, &seeds, true).await;
            } else {
                println!("identity_oracle={} outcome=censored", $label);
            }
        }};
    }
    compare!(
        "strong_artist",
        store.seeded_strong_artist_merge_candidates(&pool, &seeds, 64),
        store.strong_artist_merge_candidates(&pool, 64)
    );
    compare!(
        "same_release",
        store.seeded_same_release_track_artist_merge_candidates(&pool, &seeds, 64),
        store.same_release_track_artist_merge_candidates(&pool, 64)
    );
    compare!(
        "repeated_credit",
        store.seeded_repeated_artist_credit_merge_candidates(&pool, &seeds, 64),
        store.repeated_artist_credit_merge_candidates(&pool, 64)
    );
    compare!(
        "name_proposal",
        store.seeded_name_only_artist_match_proposal_candidates(&pool, &seeds, 64),
        store.name_only_artist_match_proposal_candidates(&pool, 64)
    );
    compare!(
        "strong_release",
        store.seeded_strong_release_merge_candidates(&pool, &seeds, 64),
        store.strong_release_merge_candidates(&pool, 64)
    );
    let start = Instant::now();
    let ran = reconcile_affected_identities_once(&store, &pool).await;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed")
            .fetch_one(&pool)
            .await
            .expect("count queued work");
    let merged: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artists WHERE id=ANY($1) AND status='merged'")
            .bind(&ids)
            .fetch_one(&pool)
            .await
            .expect("count merged identities");
    let closure_merges: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artist_merge_events WHERE actor='catalog-reconciler'",
    )
    .fetch_one(&pool)
    .await
    .expect("count closure merges");
    let release_merges: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_merge_events WHERE actor='catalog-reconciler'",
    )
    .fetch_one(&pool)
    .await
    .expect("count Release closure merges");
    println!(
        "identity_drain_ran={ran} elapsed_ms={:.3} initial_seeds={} affected_artist_ids={} affected_release_ids={} remaining_seeds={remaining} merged_strong_artists={merged} closure_artist_merges={closure_merges} closure_release_merges={release_merges}",
        start.elapsed().as_secs_f64() * 1000.0,
        seeds.len(),
        affected_ids.len(),
        releases.len()
    );
    assert!(ran);

    // A merge may enqueue new work, and a turn may retain its captured IDs
    // when the fixed-point budget is exhausted. Record both independently;
    // neither a nonzero queue nor a single successful turn proves convergence.
    let mut queued = remaining;
    let mut turns = 0_u32;
    let closure_started = Instant::now();
    let mut outcome = "quiescent";
    loop {
        let captured_remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog_identity_reconciliation_seed WHERE id=ANY($1)",
        )
        .bind(&seeds)
        .fetch_one(&pool)
        .await
        .expect("count original captured seeds");
        println!(
            "identity_closure_turn={turns} seeds_remaining={queued} original_seeds_remaining={captured_remaining} elapsed_ms={:.3}",
            closure_started.elapsed().as_secs_f64() * 1000.0
        );
        if queued == 0 {
            break;
        }
        if turns >= 64 {
            outcome = "turn_limit_censored";
            break;
        }
        // This deadline is checked only between turns: cancelling the future
        // mid-turn could return a session advisory lock to the pool.
        if closure_started.elapsed() >= std::time::Duration::from_secs(120) {
            outcome = "deadline_censored";
            break;
        }
        if !reconcile_affected_identities_once(&store, &pool).await {
            outcome = "drain_unavailable";
            break;
        }
        turns += 1;
        queued = sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed")
            .fetch_one(&pool)
            .await
            .expect("count closure queue");
    }
    println!(
        "identity_closure_outcome={outcome} turns={turns} seeds_remaining={queued} elapsed_ms={:.3}",
        closure_started.elapsed().as_secs_f64() * 1000.0
    );
    if outcome != "quiescent" {
        return;
    }

    // Allocate the lower ID in an uncommitted transaction while a higher ID
    // is committed and drained. Only the explicitly captured IDs may be acked.
    let mut late = pool.begin().await.expect("begin late seed transaction");
    let late_id: i64 = sqlx::query_scalar("INSERT INTO catalog_identity_reconciliation_seed (normalized_name,created_at) VALUES ('benchmark late seed',now()) RETURNING id")
        .fetch_one(&mut *late).await.expect("allocate late seed");
    let visible_id: i64 = sqlx::query_scalar("INSERT INTO catalog_identity_reconciliation_seed (normalized_name,created_at) VALUES ('benchmark visible seed',now()) RETURNING id")
        .fetch_one(&pool).await.expect("commit visible seed");
    assert!(late_id < visible_id);
    let concurrent_start = Instant::now();
    assert!(reconcile_affected_identities_once(&store, &pool).await);
    late.commit()
        .await
        .expect("commit lower-ID seed after drain");
    let retained: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM catalog_identity_reconciliation_seed WHERE id=$1)",
    )
    .bind(late_id)
    .fetch_one(&pool)
    .await
    .expect("check late seed");
    let visible_retained: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM catalog_identity_reconciliation_seed WHERE id=$1)",
    )
    .bind(visible_id)
    .fetch_one(&pool)
    .await
    .expect("check visible seed");
    println!(
        "identity_late_commit_drain_ms={:.3} lower_id_retained={retained} visible_id_retained={visible_retained}",
        concurrent_start.elapsed().as_secs_f64() * 1000.0
    );
    assert!(retained, "a late lower-ID seed must remain durable");
    assert!(
        !visible_retained,
        "successfully processed visible seed must be acknowledged"
    );
}

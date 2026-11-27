//! Opt-in, release-build merge/split timing on an owned representative catalog.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    collections::HashMap,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use melodee_persistence::{CatalogStore, NativeScrobbleResult, NativeUserStateStore};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};
use uuid::Uuid;

struct QueryCounter(Arc<AtomicU64>);
impl<S: Subscriber> Layer<S> for QueryCounter {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn peak_process_rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
}

async fn measured<F, T>(
    pool: &PgPool,
    source: Uuid,
    counter: &Arc<AtomicU64>,
    operation: &'static str,
    future: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let stop = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(AtomicU64::new(0));
    let blocked = Arc::new(AtomicU64::new(0));
    let first = Arc::new(AtomicU64::new(u64::MAX));
    let last = Arc::new(AtomicU64::new(0));
    let before = counter.load(Ordering::Relaxed);
    let started = Instant::now();
    let sampler = {
        let (pool, stop, samples, blocked, first, last) = (
            pool.clone(),
            stop.clone(),
            samples.clone(),
            blocked.clone(),
            first.clone(),
            last.clone(),
        );
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let result = sqlx::query("SELECT id FROM releases WHERE id=$1 FOR UPDATE NOWAIT")
                    .bind(source)
                    .fetch_one(&pool)
                    .await;
                samples.fetch_add(1, Ordering::Relaxed);
                if let Err(error) = result {
                    assert_eq!(
                        error
                            .as_database_error()
                            .and_then(sqlx::error::DatabaseError::code)
                            .as_deref(),
                        Some("55P03"),
                        "unexpected lock probe error: {error}"
                    );
                    blocked.fetch_add(1, Ordering::Relaxed);
                    let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    first.fetch_min(micros, Ordering::Relaxed);
                    last.fetch_max(micros, Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };
    let result = future.await;
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    sampler.await.expect("join lock sampler");
    let sqlx_events = counter
        .load(Ordering::Relaxed)
        .saturating_sub(before)
        .saturating_sub(samples.load(Ordering::Relaxed));
    assert!(sqlx_events > 0, "SQLx query tracing must be enabled");
    let observed_lock_span = last
        .load(Ordering::Relaxed)
        .saturating_sub(first.load(Ordering::Relaxed));
    println!(
        "operation={operation} elapsed_ms={:.3} sqlx_query_events={sqlx_events} lock_probe_count={} lock_blocked_samples={} observed_lock_span_us={observed_lock_span} lock_sample_interval_ms=5",
        elapsed.as_secs_f64() * 1000.0,
        samples.load(Ordering::Relaxed),
        blocked.load(Ordering::Relaxed)
    );
    result
}

#[tokio::test]
#[ignore = "requires an invocation-owned representative fixture and a release build"]
async fn measure_large_release_merge_and_split() {
    let counter = Arc::new(AtomicU64::new(0));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(QueryCounter(counter.clone())),
    )
    .expect("install SQLx counter");
    let url = std::env::var("MELODEE__DATABASE__URL").expect("fixture database URL");
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .application_name("melodee-rm19-merge");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database
            .strip_prefix("melodee_template_")
            .is_some_and(|s| s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())),
        "refusing a non-fixture database"
    );
    assert_eq!(
        std::env::var("MELODEE_PERF_OWNED_DATABASE").ok().as_deref(),
        Some(database.as_str()),
        "refusing a database not named by the fixture invocation"
    );
    let n: i64 = std::env::var("MELODEE_PERF_MERGE_TRACKS")
        .unwrap_or_else(|_| "1000".to_owned())
        .parse()
        .unwrap();
    assert!(
        [1_000, 10_000].contains(&n),
        "only the bounded 1K and 10K fixtures are supported"
    );
    let source = Uuid::now_v7();
    let target = Uuid::now_v7();
    let artist: Uuid = sqlx::query_scalar("SELECT id FROM artists WHERE canonical_name='Artist 1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let user: Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE normalized_username='BENCHMARK-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    for (id, title) in [
        (source, "Merge benchmark source"),
        (target, "Merge benchmark target"),
    ] {
        sqlx::query("INSERT INTO releases(id,public_api_id,title,normalized_title,sort_title,display_artist,status,created_at,updated_at) VALUES($1,uuidv7(),$2,lower($2),$2,'Benchmark artist','active',now(),now())")
            .bind(id).bind(title).execute(&pool).await.unwrap();
    }
    // Select existing fixture Tracks and their real related asset/evidence rows;
    // all setup runs before the timed operations. The representative fixture now
    // seeds Track credits and observations for essentially every Track, so the
    // previous "no credits" filter matched zero rows. Clear the fixture's
    // Track-level artist rows for the selected Tracks and install one controlled
    // observation+credit per Track instead, so the merge/split evidence arrays
    // and counts are unambiguous.
    let selected: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tracks ORDER BY id LIMIT $1")
        .bind(n + 1)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(selected.len(), usize::try_from(n + 1).unwrap());
    for statement in [
        "DELETE FROM artist_track_associations WHERE track_id = ANY($1)",
        "DELETE FROM track_artists WHERE track_id = ANY($1)",
        "DELETE FROM observed_artists WHERE track_id = ANY($1)",
    ] {
        sqlx::query(statement)
            .bind(&selected)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE tracks t SET release_id=$1, ordinal=m.ordinal::int FROM (SELECT id, row_number() OVER (ORDER BY id) ordinal FROM unnest($2::uuid[]) id) m WHERE t.id=m.id")
        .bind(source).bind(&selected[..usize::try_from(n).unwrap()]).execute(&pool).await.unwrap();
    sqlx::query("UPDATE tracks SET release_id=$1,ordinal=1 WHERE id=$2")
        .bind(target)
        .bind(selected[usize::try_from(n).unwrap()])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO observed_artists(id,release_id,track_id,scope,source_ordinal,raw_value,normalized_value,display_value,source_pointer,source_payload,created_at) SELECT uuidv7(),release_id,id,'track',ordinal,'Benchmark artist','benchmark artist','Benchmark artist','bench/' || id::text,'{}'::jsonb,now() FROM tracks WHERE release_id=$1")
        .bind(source).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO track_artists(id,track_id,artist_id,observed_artist_id,role,presentation_order,display_value,join_phrase,created_at) SELECT uuidv7(),track_id,$2,id,'primary',1,'Benchmark artist','',now() FROM observed_artists WHERE release_id=$1 AND scope='track'")
        .bind(source).bind(artist).execute(&pool).await.unwrap();
    let original: Vec<(Uuid,i32,Uuid,Uuid)> = sqlx::query_as("SELECT id,ordinal,playable_media_asset_id,public_api_id FROM tracks WHERE release_id=$1 ORDER BY ordinal").bind(source).fetch_all(&pool).await.unwrap();
    assert_eq!(original.len(), usize::try_from(n).unwrap());
    let observation_rows: Vec<(Uuid, Uuid, i32)> = sqlx::query_as("SELECT track_id,id,source_ordinal FROM observed_artists WHERE release_id=$1 ORDER BY track_id,id")
        .bind(source).fetch_all(&pool).await.unwrap();
    let observations: HashMap<Uuid, (Uuid, i32)> = observation_rows
        .iter()
        .map(|(track, id, ordinal)| (*track, (*id, *ordinal)))
        .collect();
    assert_eq!(observations.len(), original.len());
    for (index, track) in original.iter().take(10).enumerate() {
        let now = time::OffsetDateTime::now_utc();
        assert_eq!(
            NativeUserStateStore::record_scrobble(
                &pool,
                user,
                track.3,
                &format!("rm19-play-{n}-{index}"),
                now,
                now,
                Some(30)
            )
            .await
            .unwrap(),
            NativeScrobbleResult::Applied
        );
    }
    let source_ids: Vec<Uuid> = original.iter().map(|row| row.0).collect();
    let event = measured(
        &pool,
        source,
        &counter,
        "merge",
        CatalogStore.merge_release(
            &pool,
            source,
            target,
            "benchmark",
            "reversible benchmark",
            time::OffsetDateTime::now_utc(),
        ),
    )
    .await
    .expect("merge source");
    let status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "merged");
    let moved: serde_json::Value =
        sqlx::query_scalar("SELECT moved_tracks FROM release_merge_events WHERE id=$1")
            .bind(event)
            .fetch_one(&pool)
            .await
            .unwrap();
    let moved = moved.as_array().expect("immutable moved-track array");
    assert_eq!(moved.len(), original.len());
    for (index, (actual, expected)) in moved.iter().zip(&original).enumerate() {
        assert_eq!(
            actual["track_id"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
            expected.0
        );
        assert_eq!(
            actual["original_ordinal"].as_i64().unwrap(),
            i64::from(expected.1)
        );
        assert_eq!(
            actual["merged_ordinal"].as_i64().unwrap(),
            i64::try_from(index + 2).unwrap()
        );
        assert_eq!(actual["observations"].as_array().unwrap().len(), 1);
        let observation = &actual["observations"][0];
        let expected_observation = observations.get(&expected.0).unwrap();
        assert_eq!(
            observation["observation_id"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
            expected_observation.0
        );
        assert_eq!(
            observation["original_ordinal"].as_i64().unwrap(),
            i64::from(expected_observation.1)
        );
        assert_eq!(
            observation["merged_ordinal"].as_i64().unwrap(),
            i64::try_from(index + 1).unwrap()
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, n + 1);
    measured(
        &pool,
        source,
        &counter,
        "split",
        CatalogStore.split_release_merge(
            &pool,
            event,
            "benchmark",
            "reversible benchmark",
            time::OffsetDateTime::now_utc(),
        ),
    )
    .await
    .expect("split merge");
    let retained_moved: serde_json::Value =
        sqlx::query_scalar("SELECT moved_tracks FROM release_merge_events WHERE id=$1")
            .bind(event)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        retained_moved.as_array().unwrap(),
        moved,
        "split must not rewrite immutable merge evidence"
    );
    let restored: Vec<(Uuid,i32,Uuid,Uuid)> = sqlx::query_as("SELECT id,ordinal,playable_media_asset_id,public_api_id FROM tracks WHERE release_id=$1 ORDER BY ordinal").bind(source).fetch_all(&pool).await.unwrap();
    assert_eq!(
        restored, original,
        "identity, order, and asset bindings must round-trip"
    );
    let restored_observations: Vec<(Uuid, Uuid, i32)> = sqlx::query_as("SELECT track_id,id,source_ordinal FROM observed_artists WHERE release_id=$1 ORDER BY track_id,id")
        .bind(source).fetch_all(&pool).await.unwrap();
    assert_eq!(
        restored_observations, observation_rows,
        "observation evidence must round-trip"
    );
    let facts: (i64,i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT track_count FROM release_catalog_aggregates WHERE release_id=$1), (SELECT count(*) FROM observed_artists WHERE release_id=$1 AND track_id=ANY($3)), (SELECT count(*) FROM catalog_genre_terms g JOIN tracks t ON t.id=g.track_id WHERE t.id=ANY($3) AND g.release_id<>t.release_id), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='release' AND entity_id=$1), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='release' AND entity_id=$2)")
        .bind(source).bind(target).bind(&source_ids).fetch_one(&pool).await.unwrap();
    assert_eq!(
        facts,
        (n, n, 0, 10, 0),
        "aggregates, observations, genres and recommendation popularity"
    );
    let genre_term_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_genre_terms WHERE track_id=ANY($1)")
            .bind(&source_ids)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        genre_term_count,
        n * 2,
        "both genre surfaces must retain every source Track"
    );
    let bad_genre_aggregates: i64 = sqlx::query_scalar("WITH actual AS (SELECT g.surface,g.display_value,count(*)::bigint song_count,count(DISTINCT g.release_id)::bigint release_count FROM catalog_genre_terms g JOIN releases r ON r.id=g.release_id AND r.status='active' GROUP BY g.surface,g.display_value) SELECT count(*) FROM actual FULL JOIN catalog_genre_aggregates a USING (surface,display_value) WHERE actual.song_count IS DISTINCT FROM a.song_count OR actual.release_count IS DISTINCT FROM a.release_count")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(
        bad_genre_aggregates, 0,
        "genre aggregate must match the active catalog recount"
    );
    let (outbox, splits): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM catalog_projection_outbox WHERE entity_type='track' AND entity_id=ANY($1) AND entity_revision > 1), (SELECT count(*) FROM release_split_events WHERE merge_event_id=$2)")
        .bind(&source_ids).bind(event).fetch_one(&pool).await.unwrap();
    let seeds_available: bool = sqlx::query_scalar(
        "SELECT to_regclass('catalog_identity_reconciliation_seed') IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let seeds: Option<i64> = if seeds_available {
        Some(sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed WHERE release_id=ANY($1)").bind([source,target]).fetch_one(&pool).await.unwrap())
    } else {
        None
    };
    assert_eq!(
        outbox,
        n * 2,
        "merge and split must enqueue exactly one revision per Track each"
    );
    if let Some(seeds) = seeds {
        assert!(seeds >= 2, "identity effects must remain durable");
    }
    assert_eq!(splits, 1);
    println!(
        "tracks={n} observations={n} prior_scrobbles=10 outbox_events={outbox} identity_seeds={seeds:?} sqlx_events_exclude_begin_and_trigger_internals=true lock_span=sampled_approximation benchmark_process_peak_rss_kib={:?} postgres_resources=not_measured",
        peak_process_rss_kib()
    );
}

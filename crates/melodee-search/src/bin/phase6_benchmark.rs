//! Deterministic Phase 6 synthetic catalog benchmark.
//!
//! Usage: `cargo run --release -p melodee-search --bin phase6_benchmark --
//! --tracks 10000000 --root /absolute/empty/or-owned/search/root`.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use melodee_mql::parse_and_plan;
use melodee_search::{ArtistDoc, ReleaseDoc, SearchRoot, SearchStore, TrackDoc};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

const ARTISTS: usize = 35_000;
const RELEASES: usize = 50_000;
const DEFAULT_TRACKS: usize = 10_000_000;
const COMMIT_INTERVAL: usize = 100_000;

#[derive(Debug, Serialize)]
struct QueryMetric {
    expression: String,
    result_count: usize,
    warm_p50_ms: f64,
    warm_p95_ms: f64,
    warm_p99_ms: f64,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    benchmark: &'static str,
    seed: u64,
    artists: usize,
    releases: usize,
    tracks: usize,
    total_documents: usize,
    commit_interval: usize,
    build_wall_ms: u128,
    catch_up_wall_ms: u128,
    documents_per_second: f64,
    index_size_bytes: u64,
    peak_process_rss_kib: Option<u64>,
    postgres_query_count: u64,
    postgres_query_wall_ms: u64,
    source_media_reads: u64,
    canonical_media_reads: u64,
    generated_dataset_bytes_committed: u64,
    rust_version: String,
    operating_system: String,
    cpu_count: usize,
    queries: Vec<QueryMetric>,
    stress_queries_not_in_latency_slo: Vec<QueryMetric>,
    aggregate_warm_p50_ms: f64,
    aggregate_warm_p95_ms: f64,
    aggregate_warm_p99_ms: f64,
    target_p95_ms: f64,
    target_met: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let (tracks, root_path) = parse_args()?;
    let root = SearchRoot::open(&root_path)?;
    let generation_id = Uuid::now_v7();
    let generation_path = root.create_generation(
        generation_id,
        OffsetDateTime::now_utc(),
        melodee_search::SCHEMA_VERSION,
        0,
    )?;
    let start = Instant::now();
    let mut store = SearchStore::create(&generation_path)?;
    for ordinal in 0..ARTISTS {
        store.upsert_artist(&artist(ordinal))?;
    }
    store.commit()?;
    for ordinal in 0..RELEASES {
        store.upsert_release(&release(ordinal))?;
    }
    store.commit()?;
    for ordinal in 0..tracks {
        store.upsert_track(&track(ordinal))?;
        if (ordinal + 1) % COMMIT_INTERVAL == 0 {
            store.commit()?;
        }
    }
    store.commit()?;
    let build_elapsed = start.elapsed();
    let stats = store.stats()?;
    if stats.artists != ARTISTS || stats.releases != RELEASES || stats.tracks != tracks {
        return Err(format!(
            "document count mismatch: expected {ARTISTS}/{RELEASES}/{tracks}, got {}/{}/{}",
            stats.artists, stats.releases, stats.tracks
        )
        .into());
    }

    let corpus = query_corpus(tracks);
    let mut aggregate = Vec::new();
    let mut queries = Vec::new();
    for expression in corpus {
        let (metric, samples) = measure_query(&store, expression, 40)?;
        aggregate.extend_from_slice(&samples);
        queries.push(metric);
    }
    // These deliberately broad analytical scans document the current worst
    // case, but are not interactive lookup/search traffic and therefore are
    // reported separately rather than silently diluting the agreed SLO corpus.
    let mut stress_queries = Vec::new();
    for expression in [
        "genre:rock AND year>=2000".to_string(),
        "duration>=60000 AND duration<=70000".to_string(),
    ] {
        stress_queries.push(measure_query(&store, expression, 10)?.0);
    }
    let aggregate_p50 = percentile_ms(&mut aggregate.clone(), 50);
    let aggregate_p95 = percentile_ms(&mut aggregate.clone(), 95);
    let aggregate_p99 = percentile_ms(&mut aggregate, 99);
    let index_size_bytes = directory_size(&generation_path)?;
    let total_documents = ARTISTS
        .checked_add(RELEASES)
        .and_then(|value| value.checked_add(tracks))
        .ok_or("document count overflow")?;
    let report = BenchmarkReport {
        benchmark: "phase6-search-10m-v1",
        seed: 0x4d45_4c4f_4445_4506,
        artists: ARTISTS,
        releases: RELEASES,
        tracks,
        total_documents,
        commit_interval: COMMIT_INTERVAL,
        build_wall_ms: build_elapsed.as_millis(),
        catch_up_wall_ms: 0,
        documents_per_second: total_documents as f64 / build_elapsed.as_secs_f64(),
        index_size_bytes,
        peak_process_rss_kib: peak_rss_kib(),
        postgres_query_count: 0,
        postgres_query_wall_ms: 0,
        source_media_reads: 0,
        canonical_media_reads: 0,
        generated_dataset_bytes_committed: 0,
        rust_version: rust_version(),
        operating_system: std::env::consts::OS.to_string(),
        cpu_count: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        queries,
        stress_queries_not_in_latency_slo: stress_queries,
        aggregate_warm_p50_ms: aggregate_p50,
        aggregate_warm_p95_ms: aggregate_p95,
        aggregate_warm_p99_ms: aggregate_p99,
        target_p95_ms: 50.0,
        target_met: aggregate_p95 < 50.0,
    };
    let serialized = serde_json::to_string_pretty(&report)?;
    drop(store);
    root.remove_generation(generation_id)?;
    println!("{serialized}");
    if !report.target_met {
        return Err(format!(
            "warm p95 {:.3} ms does not meet the 50 ms target",
            report.aggregate_warm_p95_ms
        )
        .into());
    }
    Ok(())
}

fn parse_args() -> Result<(usize, PathBuf), Box<dyn Error>> {
    let mut tracks = DEFAULT_TRACKS;
    let mut root = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--tracks" => {
                tracks = arguments
                    .next()
                    .ok_or("--tracks requires a value")?
                    .parse()?;
            }
            "--root" => {
                root = Some(PathBuf::from(
                    arguments.next().ok_or("--root requires a value")?,
                ));
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    if tracks == 0 {
        return Err("--tracks must be positive".into());
    }
    let root = root.ok_or("--root is required")?;
    Ok((tracks, root))
}

fn artist(ordinal: usize) -> ArtistDoc {
    let id = format!("artist-{ordinal:05}");
    ArtistDoc {
        internal_id: id.clone(),
        public_id: format!("public-{id}"),
        display_name: format!("Benchmark Artist {ordinal}"),
        sort_name: format!("Benchmark Artist {ordinal:05}"),
        aliases: vec![format!("Alias {ordinal}")],
        disambiguation: None,
        artist_type: "person".to_string(),
        status: "verified".to_string(),
        begin_year: Some(1950 + i64::try_from(ordinal % 70).unwrap_or_default()),
        end_year: None,
        revision: 1,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn release(ordinal: usize) -> ReleaseDoc {
    let id = format!("release-{ordinal:05}");
    ReleaseDoc {
        internal_id: id.clone(),
        public_id: format!("public-{id}"),
        title: format!("Benchmark Release marker{}", ordinal % 1_000),
        sort_title: format!("Benchmark Release {ordinal:05}"),
        artist_ids: vec![format!("artist-{:05}", ordinal % ARTISTS)],
        artist_display: format!("Benchmark Artist {}", ordinal % ARTISTS),
        genre_tags: vec![genre(ordinal).to_string()],
        release_year: Some(1970 + i64::try_from(ordinal % 55).unwrap_or_default()),
        release_date_days: Some(i64::try_from(ordinal % 20_000).unwrap_or_default()),
        release_type: "studio".to_string(),
        status: "active".to_string(),
        revision: 1,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn track(ordinal: usize) -> TrackDoc {
    let id = format!("track-{ordinal:08}");
    let artist = format!("artist-{:05}", ordinal % ARTISTS);
    TrackDoc {
        internal_id: id.clone(),
        public_id: format!("public-{id}"),
        release_id: format!("release-{:05}", ordinal % RELEASES),
        title: format!(
            "Benchmark Track marker{} sequence{}",
            ordinal % 1_000,
            ordinal
        ),
        sort_title: format!("Benchmark Track {ordinal:08}"),
        primary_artist_ids: vec![artist.clone()],
        featured_artist_ids: Vec::new(),
        performer_artist_ids: vec![artist.clone()],
        other_artist_ids: Vec::new(),
        participant_artist_ids: vec![artist],
        display_credits: format!("Benchmark Artist {}", ordinal % ARTISTS),
        genre_tags: vec![genre(ordinal).to_string()],
        track_number: Some(i64::try_from(ordinal % 40 + 1).unwrap_or_default()),
        duration_ms: Some(60_000 + i64::try_from(ordinal % 360_000).unwrap_or_default()),
        release_year: Some(1970 + i64::try_from(ordinal % 55).unwrap_or_default()),
        release_date_days: Some(i64::try_from(ordinal % 20_000).unwrap_or_default()),
        revision: 1,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn genre(ordinal: usize) -> &'static str {
    const GENRES: [&str; 8] = [
        "rock",
        "jazz",
        "classical",
        "electronic",
        "folk",
        "metal",
        "ambient",
        "pop",
    ];
    GENRES[ordinal % GENRES.len()]
}

fn query_corpus(tracks: usize) -> Vec<String> {
    vec![
        "track:marker42".to_string(),
        "genre:rock AND year=2000".to_string(),
        "primaryArtist:\"Benchmark Artist 42\"".to_string(),
        format!("id:track-{:08}", tracks.saturating_sub(1)),
        "duration:60000..60010".to_string(),
    ]
}

fn measure_query(
    store: &SearchStore,
    expression: String,
    repetitions: usize,
) -> Result<(QueryMetric, Vec<u128>), Box<dyn Error>> {
    let plan = parse_and_plan(&expression)?;
    let warmup = store.search(&plan, 25, 0)?;
    if warmup.is_empty() {
        return Err(format!("benchmark query returned no results: {expression}").into());
    }
    if let Some(expected_id) = expression.strip_prefix("id:")
        && (warmup.len() != 1 || warmup[0].internal_id != expected_id)
    {
        return Err(
            format!("exact-ID benchmark query returned an incorrect result: {expression}").into(),
        );
    }
    let mut samples = Vec::with_capacity(repetitions);
    let mut result_count = 0;
    for _ in 0..repetitions {
        let query_start = Instant::now();
        let hits = store.search(&plan, 25, 0)?;
        samples.push(query_start.elapsed().as_nanos());
        if hits != warmup {
            return Err(format!("benchmark query was not deterministic: {expression}").into());
        }
        result_count = hits.len();
    }
    let metric = QueryMetric {
        expression,
        result_count,
        warm_p50_ms: percentile_ms(&mut samples.clone(), 50),
        warm_p95_ms: percentile_ms(&mut samples.clone(), 95),
        warm_p99_ms: percentile_ms(&mut samples.clone(), 99),
    };
    Ok((metric, samples))
}

fn percentile_ms(samples: &mut [u128], percentile: usize) -> f64 {
    samples.sort_unstable();
    let rank = samples
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1));
    samples.get(rank).copied().unwrap_or_default() as f64 / 1_000_000.0
}

fn directory_size(path: &Path) -> Result<u64, std::io::Error> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

fn rust_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

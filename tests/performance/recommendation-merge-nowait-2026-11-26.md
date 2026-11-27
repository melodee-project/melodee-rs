# NOWAIT recommendation contention and Release merge/split, 2026-11-26

One serial diagnostic run completed on an invocation-owned disposable fixture.
The release-profile executables were built from dirty HEAD
`08de08d50cb1be6e76c66000232dae257c1a2d2f`; the source state was not a clean
commit. The [source and binary hash manifest](recommendation-merge-nowait-2026-11-26-source-hashes.json)
records 589 source/config/migration hashes taken around compilation, both release
binary hashes, and detected drift. The release executable hashes were unchanged
after the workload. The migration CLI was not rebuilt for this invocation; its
post-run SHA-256 is in the manifest, and the complete current 0065 and 0066 SQL
texts were found in that executable. This confirms the embedded migration texts,
not independently queried applied-migration checksums.

Four files changed during the run: `perf_catalog_retry.rs`,
`perf_ingestion_workload.py`, `perf_ingress_workload.py`, and
`perf_representative.py`. The last file was loaded at fixture startup; its
captured initial hash equals the version in HEAD `08de08d`. Its subsequent diff
adds optional migration-CLI and seed-timeout arguments; the default path and
fixture SQL were unchanged. The initial version can therefore be recovered from
that commit, but the dirty application source must be matched against the hash
manifest rather than inferred from HEAD.

Fixture: 35,000 Artists, 50,000 Releases and 600,000 Tracks, generated in
153.0 seconds, using the HEAD fixture script defaults. This synthetic catalog
has no physical media, complete ACLs, authentic external identity distributions,
concurrent ingestion or network transport. Each merge workload creates two
additional Releases, moves existing synthetic Tracks into the source, adds one
Track Artist observation and primary credit per Track, and records ten plays.
A single merge and split were timed for each size, in 1K-then-10K order on one
fixture; cache warmth was not controlled. Both release binaries executed with
`CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=1`; the internal scrobble workload
used eight concurrent workers and a 16-connection pool. The merge workload used
a four-connection pool.

The host reported AMD Ryzen 9 3900X, 24 logical CPUs, 65,727,588 KiB RAM,
Linux 7.3.4-300.fc45 x86-64, PostgreSQL server binary 18.3, Cargo/Rust 1.99.0,
and btrfs workspace storage. PostgreSQL settings and database storage were not
independently captured in this invocation. The same-machine earlier run
reported shared_buffers 128 MiB, work_mem 4 MiB, and max_connections 100;
those are historical observations, not verified current settings. Cache state,
PostgreSQL CPU/RSS/I/O, and backend memory were not measured. Reported RSS is
only the benchmark process peak.

Reproduction from the captured initial fixture script and matching source state:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-persistence --test perf_recommendation_contention --test perf_large_release_merge --no-run
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_representative.py -- python3 scripts/run_perf_mutation_benchmarks.py
```

The scrobble path calls `NativeUserStateStore::record_scrobble`; each scenario
uses 160 distinct events, and the final scenario also performs 20 concurrent
primary-credit SQL updates. All three returned zero errors and exact Track,
Release and Artist popularity against authoritative plays. Lock waiters are
sampled every 5 ms; NOWAIT contention can produce zero sampled waiters despite
retries. SQLx query counts exclude BEGIN and SQL inside database triggers.

```text
scenario=shared_track workers=8 samples=160 seconds=0.908 scrobbles_per_second=176.23 p50_us=4950 p95_us=13972 p99_us=686397 errors=0 relationship_updates=0 sqlx_query_events=2338 lock_waiter_samples=0 lock_max_waiters=0 sampling_interval_ms=5
scenario=separate_tracks_shared_artist workers=8 samples=160 seconds=0.696 scrobbles_per_second=230.01 p50_us=11750 p95_us=87006 p99_us=410633 errors=0 relationship_updates=0 sqlx_query_events=2080 lock_waiter_samples=559 lock_max_waiters=7 sampling_interval_ms=5
scenario=concurrent_artist_changes workers=8 samples=160 seconds=0.829 scrobbles_per_second=192.99 p50_us=3970 p95_us=104993 p99_us=609002 errors=0 relationship_updates=20 sqlx_query_events=2367 lock_waiter_samples=0 lock_max_waiters=0 sampling_interval_ms=5
benchmark_process_peak_rss_kib=6144
```

The preceding [pre-0065 run](recommendation-merge-2026-11-26.md) used the same
nominal fixture and sample counts, but different uncommitted code and an
uncontrolled cache. Its corresponding p99s were 114,223, 138,363 and 58,141
microseconds, versus 686,397, 410,633 and 609,002 here. This single-sample
comparison reveals a substantial tail-latency concern; it does not establish a
causal regression or a stable percentile. Probe/query amplification and tail
latency require further measurement under repeated mixed load.

The merge/split tests verify reversible Track/Observation evidence, public IDs,
ordinals and asset references, Release and genre aggregates, recommendation
popularity, identity seeds, search outbox rows, and immutable merge evidence.
The lock span is a sampled NOWAIT approximation, not exact server lock time.
Setup, verification and cleanup are outside the operation timings.

```text
tracks=1000 operation=merge elapsed_ms=3465.126 sqlx_query_events=36 lock_probe_count=545 lock_blocked_samples=545 observed_lock_span_us=3451637
tracks=1000 operation=split elapsed_ms=7997.916 sqlx_query_events=24 lock_probe_count=1193 lock_blocked_samples=1192 observed_lock_span_us=7979481
tracks=1000 observations=1000 prior_scrobbles=10 outbox_events=2000 identity_seeds=4004 benchmark_process_peak_rss_kib=12196
tracks=10000 operation=merge elapsed_ms=5765.436 sqlx_query_events=36 lock_probe_count=920 lock_blocked_samples=920 observed_lock_span_us=5754865
tracks=10000 operation=split elapsed_ms=8918.920 sqlx_query_events=24 lock_probe_count=1411 lock_blocked_samples=1408 observed_lock_span_us=8835434
tracks=10000 observations=10000 prior_scrobbles=10 outbox_events=20000 identity_seeds=40004 benchmark_process_peak_rss_kib=71384
```

The earlier single 10K split observation was 43.402 seconds. The 8.919-second
observation here is not a controlled before/after result: cache state and the
uncommitted implementation differ and neither side was repeated. Statement
counts omit trigger internals and transaction BEGIN. The raw concurrent-credit
writer in the contention scenario bypasses the application correction API;
this run does not prove arbitrary direct SQL or mixed multi-statement writer
ordering. It is not the end-to-end ingestion, streaming, full-resource or final
acceptance workload required by the remediation specification.

# Recommendation contention and Release merge/split diagnostic, 2026-11-26

One run on an uncommitted working tree based on
`8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`; it used the database schema
through migration 0064. Migration 0065 was not applied. This is not a
comparable baseline/final result, and cannot qualify the pending all-writer
recommendation lock protocol. The release test executables and source files
were hashed for identification below, but other uncommitted source and build
dependencies were not captured in a clean commit.

Fixture: 35,000 Artists, 50,000 Releases, 600,000 Tracks, 100 synthetic users,
600,000 synthetic acoustic evidence records and 1,200,000 genre terms. Fixture
creation took 177.2 s. No physical media, complete ACLs or authentic external
identity distributions were present. For each merge size the test creates two
additional Releases, moves existing synthetic Tracks into the source, adds one
Track Artist observation and primary credit per Track, and records ten plays.
No concurrent ingestion or network transport was involved. A single timed
merge and split per size was measured; those are not percentile estimates.

Environment: AMD Ryzen 9 3900X (12 cores / 24 threads), 62 GiB RAM, Linux
7.3.4 Fedora x86_64, NVMe-backed btrfs with zstd compression, PostgreSQL 18.3
(shared_buffers 128 MiB, work_mem 4 MiB, max_connections 100), Rust 1.99.0,
Cargo release profile. `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=1`. The database
was a generated invocation-owned UUID database and was removed on exit. Cache
warmth was not controlled; the 1K workload preceded 10K on the same fixture.

Reproduction, after building the debug migration CLI:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-persistence --test perf_recommendation_contention --test perf_large_release_merge --no-run
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_representative.py -- python3 scripts/run_perf_mutation_benchmarks.py
```

The scrobble workload calls `NativeUserStateStore::record_scrobble`; each of
three scenarios uses eight workers and 160 distinct events. The third also
performs 20 primary-artist UPDATEs while plays run. Each scenario checks exact
Track, Release, and Artist recommendation popularity against authoritative
plays. Lock waiters are sampled every 5 ms; sample counts are not durations.
SQLx query events include client-side SQLx-logged statements but omit BEGIN and
statements inside PostgreSQL triggers. Polling queries are subtracted. Peak RSS
is for the benchmark process only; PostgreSQL CPU, RSS and I/O were not measured.

```text
scenario=shared_track workers=8 samples=160 seconds=0.810 scrobbles_per_second=197.61 p50_us=34998 p95_us=73433 p99_us=114223 errors=0 relationship_updates=0 sqlx_query_events=1120 lock_waiter_samples=840 lock_max_waiters=7 sampling_interval_ms=5
scenario=separate_tracks_shared_artist workers=8 samples=160 seconds=0.693 scrobbles_per_second=230.71 p50_us=23817 p95_us=76983 p99_us=138363 errors=0 relationship_updates=0 sqlx_query_events=1120 lock_waiter_samples=667 lock_max_waiters=7 sampling_interval_ms=5
scenario=concurrent_artist_changes workers=8 samples=160 seconds=0.801 scrobbles_per_second=199.65 p50_us=38844 p95_us=49141 p99_us=58167 errors=0 relationship_updates=20 sqlx_query_events=1140 lock_waiter_samples=956 lock_max_waiters=8 sampling_interval_ms=5
recommendation_consistency=exact scenarios=3 benchmark_process_peak_rss_kib=5804
```

Before the API Track-lock fix, a separate small-fixture run failed with
PostgreSQL `40P01` from concurrent same-Track scrobbles. A focused PostgreSQL
regression reproduced the failure and passed after the fix. This does not prove
all direct database writers or mixed multi-Track operations are deadlock-free.

The merge/split workload checks immutable per-Track/per-observation evidence,
restored public IDs, ordinals and asset references, Release and genre
aggregates, recommendation popularity, identity seeds and search outbox rows.
The sampled lock span is an approximation based on repeated NOWAIT probes, not
a server-side exact lock duration. Setup, verification and cleanup are excluded
from operation timings.

```text
tracks=1000 operation=merge elapsed_ms=3853.286 sqlx_query_events=31 lock_probe_count=604 lock_blocked_samples=604 observed_lock_span_us=3842250
tracks=1000 operation=split elapsed_ms=7046.221 sqlx_query_events=23 lock_probe_count=1086 lock_blocked_samples=1084 observed_lock_span_us=7025217
tracks=1000 observations=1000 prior_scrobbles=10 outbox_events=2000 identity_seeds=4004 benchmark_process_peak_rss_kib=12120
tracks=10000 operation=merge elapsed_ms=8296.958 sqlx_query_events=31 lock_probe_count=1311 lock_blocked_samples=1311 observed_lock_span_us=8282816
tracks=10000 operation=split elapsed_ms=43401.545 sqlx_query_events=23 lock_probe_count=6701 lock_blocked_samples=6698 observed_lock_span_us=43320531
tracks=10000 observations=10000 prior_scrobbles=10 outbox_events=20000 identity_seeds=40004 benchmark_process_peak_rss_kib=69640
```

SHA-256 at measurement time:

```text
catalog.rs a0b1ddca0e268be205dbd23d1d28e6ad13e9d0e63c78f460da6dd7b786622978
native_user_state.rs 6cdbf61047c0ae7c3fc514287af8398ed8de43ecc9cca69a0d65d133401d291a
perf_recommendation_contention.rs b191aadd313ca581de12c0b738896a12997c704195feda3d59fb9055b787bccc
perf_large_release_merge.rs c5c58934b035bec19ca5523f346cc1dbc086eeaa4d7f5cbac53db0e0568fb336
perf_representative.py 37fdec8f3fe435c52ef5b93503304e0fc60baa973040fe2bddd423aaff5a0ade
run_perf_mutation_benchmarks.py 81789b1d2f6df3fba3cde14cd94c95d508e5e6c468cfd3035463dc428f09bb8c
release perf_large_release_merge executable b8d18d9ac4e57d6f79a41060cad9cf81110f9c8d37683adb7f60daf711f40495
release perf_recommendation_contention executable 207bf7141ce3e5844626d0b45049bf450bd2937a603b964048c39c7077b79d78
```

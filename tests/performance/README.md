# Performance Tests

Every benchmark records commit, hardware, OS/filesystem, dependency/tool versions, dataset generator
seed and counts, configuration, command, wall time, throughput, percentiles, CPU, peak RSS, database
queries, files/bytes touched, and errors. Baselines include 200 concurrent inbound Releases, current
35k/50k/560k scale, and synthetic multi-million Track targets.

### Disposable Rust catalog fixture (in progress)

`scripts/perf_representative.py -- <command>` creates an invocation-owned,
UUID-named migrated PostgreSQL database, seeds the Rust schema, passes its URL
in `MELODEE__DATABASE__URL`, and removes only that database on exit. The debug
`melodee-cli` binary must already be built for migrations. The default fixture
contains 35,000 artists, 50,000 releases and 600,000 tracks; the cardinalities
can be reduced with `--artists`, `--releases` and `--tracks-per-release` for a
smoke test. `MELODEE_TEST_DATABASE_URL` selects the administrator connection.

This fixture currently supplies catalog rows, referenced assets, synthetic acoustic
digests, a skewed genre distribution, and deterministic artist, release, and
track ratings for 100 synthetic users. Usernames are `benchmark-1` through
`benchmark-100` and the publicly known fixture-only password is
`benchmark-only-password`; user 1 is an administrator. These credentials are
solely for the disposable database. It does not provide physical media,
realistic identity relationships, ACLs, artwork or alternate assets,
nor does it run a workload or record benchmark metadata. It must not be cited as the representative end-to-end benchmark until
those gaps are closed.

### Identity discovery and production drain (in progress)

Build the ignored target and run it through the owned fixture:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-catalog --test perf_identity_reconciliation --no-run
python3 scripts/perf_representative.py -- python3 scripts/run_perf_identity_benchmark.py
```

Add `--augment-identities` to the workload runner to create one Artist
observation and current decision per Release and Track, Track credits, a mix of
active and verified Artists, verified same-name groups, and synthetic unique
provider keys before measuring. The augmentation accepts only an untouched
invocation-owned fixture and prints actual relationship cardinalities. It does
not simulate realistic negative assertions, manual locks, redirect histories,
multiple Track credits, or cross-catalog transitive clusters. A rerun on the
same fixture is rejected; the enclosing fixture invocation removes its database.

The test adds a shared trusted Artist external ID and a repeated Artist credit
with two explicit observations, times all five
production scoped candidate queries and their global counterparts, compares
returned candidate rows when the global query completes, and times one actual
reconciliation drain. It captures each scoped statement from SQLx tracing and
runs that exact SQL and bindings through `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`.
Queries are capped at 20 seconds and report a timeout as censored. It prints
fixture counts, seed counts and merged identities. This is
a deliberately narrow synthetic identity case: the base fixture lacks realistic
identity distributions and transitive relationships. The test does not
establish complete oracle equivalence, crash recovery, or representative import
performance. The pre-remediation revision lacks these scoped candidate methods;
its baseline needs a separate compatible harness and cannot run this target.
The benchmark also extracts the SELECT body of the phased scrobble Artist
cursor from the production source and reports plans for an ordinary Track and
one with up to 1,000 additional primary credits. Those plans execute the full
SELECT and do not measure incremental cursor fetching or lock duration.

### Bounded connection measurements

For an opt-in run set `RUST_LOG=info,melodee::performance=trace` and
`MELODEE__LOGGING__FORMAT=json` (leave `MELODEE__LOGGING__LEVEL` unset so it
does not override `RUST_LOG`). Each process with an enabled tracing subscriber emits one cumulative
fixed-cardinality snapshot at most every ten seconds, on an observation. Filter
JSON events with target `melodee::performance`; `process_id` and `process_token` identify the
emitter and `metric` is a fixed enum. Restarting a process resets counters and
generates a new token, so compare successive snapshots only within one token. Samples shorter than an interval may not contain a closing snapshot. TRACE
export adds logging and serialization overhead; record whether it was enabled
when comparing runs.

Bucket fields are disjoint durations in microseconds, at or below the named
millisecond upper bound and above the preceding bound; `bucket_overflow` is above
60 seconds. Acquisition metrics time successful pool checkout; hold metrics
include transaction start, queries and lock waits until the connection is
released. These times do not independently measure PostgreSQL CPU, I/O, lock
waits, or executor scheduling. The admin statistic metrics cover only its
headline aggregate, and the current set does not instrument every workload.

### Scrobble contention and large Release merge/split (in progress)

Compile and run these ignored persistence benchmarks through the disposable
fixture harness after the migration CLI is built:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-persistence --test perf_recommendation_contention --test perf_large_release_merge --no-run
python3 scripts/perf_representative.py -- python3 scripts/run_perf_mutation_benchmarks.py
```

The fixture invocation creates and removes its own database; the runner uses it
for all three workloads in sequence. Use `--merge-size 1000` on the runner for
a smaller smoke test. The scrobble benchmark
uses the production persistence method with eight concurrent workers and
checks recommendation counts against authoritative plays after shared-track,
shared-artist and concurrent relationship-change scenarios. The merge benchmark
uses the production catalog methods with the stated Track count, one observation
per Track and ten existing plays, and verifies the persisted evidence and restored
catalog. The sampler polls lock activity every five milliseconds; sampled lock
spans are approximations, not exact lock durations. SQLx event counts omit BEGIN
and statements executed inside PostgreSQL triggers; the sampler's own SQLx
queries are subtracted. They are not total database statement counts. Database
setup, physical media and network transport are outside timed operations. The
harness has not yet been run or validated at full scale.

Run `python3 scripts/summarize_perf_metrics.py path/to/run.jsonl` to compute
per-process, per-metric deltas and percentile bucket bounds between the first
and last captured snapshot. The tool reads the log incrementally, rejects
counter resets, and limits the number of retained series. Its results exclude
work before the first or after the last captured snapshot; `null` quantile
bounds mean either no observations or an overflow above 60 seconds.

### Media codec and worker matrix

`scripts/perf_codec_matrix.py` generates disposable mono audio fixtures and runs
`inspect_release` and `normalize_release` from `melodee-media` in a release-build
benchmark. The measured interval covers normalization only: inspections finish
before the timer starts. Each synthetic Release has one Track. The default
matrix uses MP3, FLAC, Ogg Vorbis, and WAV inputs, 1/2/4 simultaneous release
workers, eight releases per case, 60 seconds of audio per input, and two samples.
FFmpeg uses the production default options; no thread count is forced.

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_codec_matrix.py --output tests/performance/codec-matrix.json
```

The output path must not already exist. Inputs and outputs are created under a
uniquely owned temporary directory, then removed. The JSON records individual
latencies, elapsed throughput, source/output sizes, child CPU time and block
counts where available. On Linux, sampled process-tree RSS includes the test's
preparatory inspections as well as normalization and may miss brief peaks. The
separately reported child maximum RSS is a cumulative platform-dependent
high-water mark, not a per-case peak. Each codec reuses the same source and the
OS cache is warm. Build and input generation are outside each normalization
timer. A baseline checkout needs the same script and ignored benchmark test
copied into it without altering its production implementation; use identical
arguments, hardware and cache conditions for a comparison.

This is a normalization-stage measurement, not a full ingestion or interactive
load test. It does not measure PostgreSQL, lease handling, media acquisition,
publication, browser responsiveness, realistic music compression ratios, or an
FFmpeg thread-count matrix. Do not use it alone to claim end-to-end throughput
or to tune global defaults.

### PostgreSQL-backed search rebuild

Build the migration CLI and the release-mode `perf_rebuild` integration test,
then run the ignored test inside `scripts/perf_representative.py`. The test
checks the invocation-owned database marker before requesting a rebuild, uses
an automatically removed temporary search root, and verifies activated Artist,
Release and Track counts. Search tracing records the snapshot time, commit
count, per-commit latency and activation-call time; the report records elapsed
time, RSS where available and logical index bytes. Example commands and the
measured single-run limitations appear in
[search-rebuild-2026-11-26.md](search-rebuild-2026-11-26.md). This diagnostic is
not a baseline comparison, catch-up workload, or concurrent-client benchmark.

To prepare a comparable baseline, create a separate detached worktree at
`a0979cde1fd2aeff5b8027d9d59336b678f1aa12`, then run:

```text
python3 scripts/prepare_perf_search_baseline.py --worktree PATH_TO_BASELINE_WORKTREE
```

The preparation command checks the exact revision, validates the database
ownership helper, adds the benchmark files only when absent or identical,
and refuses a conflicting manifest or symlink. It does not reset, remove or
build the worktree. From the baseline worktree, build its migration CLI and
then use the same release test and fixture commands above. The report emits the
configuration available in each revision; the baseline predates the current
search trace instrumentation, so it does not emit comparable commit or snapshot
timings. UUIDs and timing-dependent cache conditions vary between generated
fixtures even though cardinalities and generation rules match.

For a bounded rebuild run, use the test executable path printed by Cargo's
`--no-run` output:

```text
python3 scripts/perf_representative.py -- python3 scripts/run_perf_search_case.py --test-binary PATH_TO_PERF_REBUILD_TEST --timeout-seconds 600
```

The wrapper returns 124 and labels the result censored if the test exceeds the
limit. It kills and reaps the test before removing the private temporary tree;
the outer fixture then removes its own database. The timeout excludes fixture
construction.

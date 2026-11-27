# Performance Tests

Every benchmark records commit, hardware, OS/filesystem, dependency/tool versions, dataset generator
seed and counts, configuration, command, wall time, throughput, percentiles, CPU, peak RSS, database
queries, files/bytes touched, and errors. Baselines include 200 concurrent inbound Releases, current
35k/50k/560k scale, and synthetic multi-million Track targets.

### Final integrated matrix (partial)

`final-matrix-2026-11-27.md` records the representative-round measurements on
the final integrated sources for all thirteen RM-00 workloads: four matched
baseline/final pairs (full rebuild, policy evaluate/dry-run, media
normalization, bounded mixed load), five measured single/repeat runs (single and
200-album import, unchanged reimport, concurrent genre imports, worker-scheduling
samples), and four final-only rows whose baseline harness cannot run (identity
reconciliation, incremental indexing, streaming during ingestion, merge/split
1K/10K). It also records the provenance records, the benchmark/fixture fixes
made to obtain the runs, and each row's limitations. The four final-only rows
are not before/after evidence; comparable-pair acceptance for them remains open.

### Disposable Rust catalog fixture (in progress)

`scripts/perf_representative.py -- <command>` creates an invocation-owned,
UUID-named migrated PostgreSQL database, seeds the Rust schema, passes its URL
in `MELODEE__DATABASE__URL`, and removes only that database on exit. The debug
`melodee-cli` binary must already be built for migrations. The default fixture
contains 35,000 artists, 50,000 releases and 600,000 tracks; the cardinalities
can be reduced with `--artists`, `--releases` and `--tracks-per-release` for a
smoke test. `MELODEE_TEST_DATABASE_URL` selects the administrator connection.

This fixture currently supplies catalog rows, referenced assets, synthetic acoustic
digests, a skewed genre distribution, deterministic artist, release, and
track ratings for 100 synthetic users, and playable and alternate asset links.
One generated PNG in the disposable asset root backs a subset of artist and
release artwork references. Two access groups and four bound library IDs cover
allowed and denied access for a subset of releases; their publication and
manifest rows are synthetic placeholders, not actual ingestion receipts.
Usernames are `benchmark-1` through
`benchmark-100` and the publicly known fixture-only password is
`benchmark-only-password`; user 1 is an administrator. These credentials are
solely for the disposable database. Aside from the shared PNG, it does not
provide physical media. It does not model realistic identity histories or
publication provenance, run a workload, or by itself record benchmark metadata.
The [full-size fixture smoke](fixture-verification-2026-11-27.md) verifies its
observed cardinalities and basic data; it is not an end-to-end benchmark.
Artwork installation currently requires POSIX descriptor-relative no-follow
directory operations; hosts without them fail closed rather than following a
possibly redirected asset path.

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

The test adds a shared trusted Artist external ID, a repeated Artist credit
with two explicit observations, and two Track-only Artists on the same Release. It times all five
production scoped candidate queries and their global counterparts, compares
returned candidate rows when the global query completes, and times bounded
production reconciliation turns until quiescence or a recorded limit. It
captures each scoped and global statement from SQLx tracing and
runs that exact SQL and bindings through `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`.
Queries are capped at 20 seconds and report a timeout as censored. It prints
fixture counts, seed counts and merged identities. This is
a deliberately narrow synthetic identity case: the base fixture lacks realistic
identity distributions and transitive relationships. The test does not
establish complete oracle equivalence, crash recovery, or representative import
performance. The pre-remediation revision lacks these scoped candidate methods;
its baseline needs a separate compatible harness and cannot run this target.
The benchmark also extracts the SELECT body of the scrobble Artist
cursor from the production source and reports plans for an ordinary Track and
one with up to 1,000 additional primary credits. Those plans execute the full
SELECT and do not measure incremental cursor fetching or lock duration.

For a controlled multi-cardinality comparison, build the `identity` target with
`python3 scripts/build_perf_test.py --target identity --output <new-build-record.json>`.
Build the native migration CLI with `python3 scripts/build_perf_cli.py --output
<new-cli-record.json>` against the same unchanged source. This wrapper rejects
`CARGO_BUILD_TARGET`; the benchmark CLI must run on the local host.
The printed executable name selects the exact `target/release/deps` binary.
Run two **sequential**, separately owned fixtures using the same executable and
record, for example 3,500 Artists / 5,000 Releases / 60,000 Tracks and then
35,000 / 50,000 / 600,000. Pass matching `--artists`, `--releases`, and
`--tracks-per-release 12` options to `perf_representative.py`, and use the
`--migration-cli <release-cli>` and `--migration-cli-provenance <new-cli-record.json>`
options before `--`. Use the following command after `--` in each invocation:

```text
python3 scripts/run_perf_identity_case.py --test-binary <release-test-binary> --build-provenance <new-build-record.json> --migration-cli-provenance <new-cli-record.json> --output <new-case.json>
```

The case collector validates the direct owned-fixture parent, binary hash,
current HEAD, fixture-validated migration CLI proof and all fingerprinted workspace sources; it bounds the child
runtime and output and keeps only synthetic query timings, oracle outcomes,
plans, and production-drain closure counts. Global-query and plan timeouts are
censored. Do not compare a censored plan as a completed result. The synthetic
affected graph has the same logical shape at each cardinality, but its UUIDs
differ. Each size uses a fresh database and uncontrolled cache state; collect
repeated trials and other distributions before generalizing scaling behavior.
The collector attempts to terminate its owned child session on SIGINT, SIGTERM,
timeout, or failure. SIGKILL or host failure can interrupt that cleanup; review
any remaining fixture processes and temporary data by exact ownership before
removing them. Do not signal or remove unrelated processes or data.

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
waits, or executor scheduling. In the current development candidate, the admin
statistic metrics cover its combined headline and format aggregation; the
current set does not instrument every workload.

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

### Bounded missing-path recovery

After building the migration CLI and a release server with
`scripts/build_perf_server.py`, run the current source against the invocation-owned
35K/50K/600K fixture:

```text
python3 scripts/perf_representative.py -- python3 scripts/perf_missing_recovery.py --server target/release/melodee-server --provenance PATH_TO_NEW_BUILD_RECORD.json
```

The workload inserts 35,000 synthetic missing work items into the disposable
database, runs the production scheduler, and records elapsed time, sampled
scan state, lock waiters, host/storage details, and server identity. It refuses
an unowned database, a non-representative or previously used fixture, and a
binary that does not match the build provenance. A timeout is reported as
censored. The empty inbound root and absence of interactive or media activity
make this a focused recovery measurement, not an end-to-end import benchmark.

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

### Real HTTP ingress contention (experimental)

After building the release server and the debug migration CLI, run this from
the repository root:

```text
python3 scripts/perf_representative.py -- python3 scripts/perf_ingress_workload.py --connection-limit 0 --connection-limit 4 --connection-limit 16 --connection-limit 32 --connection-limit 64
```

The invocation-owned fixture creates a synthetic MP3 and imports it through
the server. For each cap, a fresh loopback HTTP/1 server holds one idle
keep-alive connection and one deliberately slow direct stream while concurrent
Jellyfin stopped requests and unrelated system-info requests run against an
intentionally locked Track. The workload
checks each 204 against persisted unique play events, reports raw latency and
time to first byte, and closes the slow stream early. A small preparatory smoke
can use `perf_representative.py --artists 10 --releases 20
--tracks-per-release 2` and `--media-seconds 30 --plays 2 --probes 2` on the
workload command. The workload refuses databases not owned by its parent
fixture and creates/removes only its own temporary media tree and processes.

This is a diagnostic, not a concurrency default recommendation: it uses a
synthetic lock rather than real catalog work, one idle connection, one local
stream, loopback HTTP, no reverse proxy, no concurrent ingestion or rebuild after
setup, and temporary
storage. A short or fully buffered stream may not exercise actual socket
backpressure; track the generated media size, consumed bytes and client receive
buffer when interpreting it. It does not measure audible playback underruns.
The [600K-fixture diagnostic](./ingress-600k-2026-11-26.md) records raw samples
and demonstrates that small caps delayed unrelated requests in one scenario;
it does not establish a generally safe default.

### HTTP during real ingestion (experimental)

```text
python3 scripts/perf_representative.py -- python3 scripts/perf_ingestion_stream_workload.py --albums 200 --media-parent .
```

This separately imports a generated, playable long MP3, starts a deliberately
slow direct stream, then atomically exposes 200 prepared Releases to the real
server workers. Each Release contains one short generated MP3 or FLAC Track.
The bounded client repeatedly measures first media byte and a 64 KiB read, and
an authenticated system-info request; only samples wholly inside the observed
window bounded by polls showing unfinished work items enter the reported percentiles;
this window may contain quiet gaps between polls. The program
retains every sample up to its explicit limit and fails on errors, incomplete
ingestion, timeout, absent overlapping samples, or an exhausted sample budget.
The source media, server processes, and migrated fixture database are private to
the invocation. The media parent must be an existing known disk-backed filesystem;
the command does not flush filesystem caches. Search and several background
workers are disabled, there is one slow synthetic stream and no audible-underrun
measurement. The current runner requires the release server's bound-address
startup log and thus does not itself measure the older baseline executable.

### Policy snapshot and dry-run diagnostic

`crates/melodee-policy/tests/perf_catalog_policy.rs` creates eight cataloged
Releases through the existing manifest/publication/catalog test helpers within
an invocation-owned fixture. It measures the production `PolicyStore` normal
snapshot and dry-run context loaders followed by the production policy engine.
The test does not invoke the CLI or persist its test-run summary.
It checks the same cross-Release acoustic match and compares dispositions.
SQLx query events report returned database **rows** (not network bytes), while
retained fingerprint counts/bytes, Track facts and process RSS provide separate
bounded-result and memory indicators. Normal and dry-run results are separate;
8 one-Track Releases are not a maximum-size dry run. The diagnostic also runs
explicit `EXPLAIN (ANALYZE, BUFFERS)` for the old global and scoped acoustic
queries; those plans supplement the production-path measurements.

Compile with `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p
melodee-policy --test perf_catalog_policy --no-run` and use the printed
executable with `python3 scripts/perf_representative.py -- python3
scripts/run_perf_policy_case.py --test-binary PATH_TO_EXECUTABLE`. The wrapper
returns 124 with a censored lower bound if its explicit 600-second deadline is
reached. `--baseline-worktree` permits a prebuilt test only from the verified
original revision. Stage the test with `python3 scripts/prepare_perf_policy_baseline.py
--worktree PATH_TO_BASELINE_WORKTREE`, then build it in that worktree. Use that
revision's migration CLI with the fixture wrapper for a comparable schema. Run full-size fixture and report source/build identity,
conditions and bypasses; seeding requires actual publication evidence files in
the wrapper's private asset root. This test does not measure production worker
scheduling, media ingestion, TCP, exact wire bytes or mixed interactive traffic.

### Concurrent genre imports and scheduler samples

```text
python3 scripts/perf_representative.py -- python3 scripts/perf_genre_scheduler.py --releases 200 --temp-parent .local-dev
```

Build the release server and migration CLI first. The runner verifies the
fixture-owned database, generates one valid five-second MP3 or FLAC per Release
with the same new genre, and drives the production ingestion and catalog worker
loops with two configured workers per stage. It verifies exact authoritative and
stored Track/distinct-Release counts on both genre surfaces, observes PostgreSQL
lock/advisory waiters and lease states at 250 ms intervals, and summarizes real
batch logs and Ready-to-first-claim event latency. It stops and reaps only its
owned server process group and temporary media directory. Use an existing
owned disk-backed temporary parent when storage characteristics matter.

Sampling can miss short waits and does not directly measure active CPU, precise
lock duration, or guarantee fairness. State/lease observations include waiting.
Log pass counts include a three-second idle observation after completion. The
workload does not establish the source-code result-ring bound, exercise a broad
multi-Track corpus, or include interactive, search, policy or enrichment load.
A dirty binary without captured source identity is diagnostic evidence only.
The paired original/current policy samples and all limitations are recorded in
[policy-catalog-2026-11-26.md](policy-catalog-2026-11-26.md).

The ignored `perf_outbox` test first rebuilds the owned PostgreSQL catalog,
then executes three bounded mixed Artist/Release/Track revision and outbox
trials through the production worker. Build it with the following command and pass the printed executable to
`run_perf_search_case.py` inside the same fixture wrapper:

```text
cargo test --release -p melodee-search --test perf_outbox --no-run
```

The [mixed outbox diagnostic](search-outbox-mixed-2026-11-26.md) records actual
runs and their limitations; SQLx event counts are statement counts rather than
network round trips. The test does not simulate concurrent workloads.

The ignored `perf_outbox_concurrent` test adds four bounded in-process search
clients while the mixed outbox drains during a second rebuild. A `test-support`
barrier fixes the snapshot before the test commits the revisions; a separate
held-reader check crosses activation deliberately. See the
[concurrent search diagnostic](search-concurrent-2026-11-26.md) for commands,
raw samples and limitations; it includes no HTTP traffic or competing writer
process. Its activation call timing is not a direct measurement of exclusive
lock duration.

### Database competition workload (experimental)

`scripts/perf_db_competition.py` is designed to run as the child of the owned
fixture harness with a prebuilt release server. The `mixed` mode uses the server-started initial search rebuild when it is still active,
or submits an actual admin rebuild request otherwise. It exposes generated
Releases to the normal ingestion workers before startup, and continuously issues
authenticated admin-statistics and native Artist browse requests after login. All server activity shares the configured
PostgreSQL pool. `ingestion`, `rebuild`, and `idle` modes provide isolated
comparison runs; each requires a fresh disposable fixture. The pool size and
measurement deadline are explicit. Example:

```text
python3 scripts/perf_representative.py -- python3 scripts/perf_db_competition.py --mode mixed --albums 200 --seconds 900 --pool-size 8 --output PATH_TO_NEW_RESULT.json
```

`--output` creates a new sanitized JSON file and refuses to overwrite an
existing path. The JSON includes sampled PostgreSQL wait classes, lock waiters and heartbeat
counts, raw HTTP latency/error samples and overlap-only percentile summaries,
plus the available per-process bounded checkout/hold histograms. A mixed run
is successful only if ingestion and rebuild overlap and each HTTP probe has at
least one complete observation within that overlap. `--allow-missing-metrics`
permits an older baseline without these diagnostic events to run; the report
still explicitly names the missing measurements. A report does not establish
comparable before/after results by itself. Database samples can miss short
waits, metric snapshots cover a wider period than the overlap window, and
checkout/hold times cannot independently identify database CPU, I/O, locks, or
async scheduling. The short generated media and one Track per Release are not
a realistic music corpus. Record binary provenance, cache conditions and
unmeasured subsystems before interpreting results.

For controlled comparisons within one revision, first record a release build
and then run the cases serially against separate fixture databases:

```text
python3 scripts/build_perf_server.py --output PATH_TO_NEW_BUILD_RECORD.json
python3 scripts/perf_representative.py --seed-timeout 1200 -- python3 scripts/run_db_competition_matrix.py --provenance PATH_TO_NEW_BUILD_RECORD.json --output-dir PATH_TO_NEW_RESULT_DIRECTORY
```

The default matrix runs idle, ingestion-only, rebuild-only and mixed workloads
at pool limits 8 and 32. The fixture is seeded once, then each case uses a
sequential invocation-owned database clone that is removed after the case. The
parent fixture is never mutated and is removed by the outer harness. Repeat
`--mode` and `--pool-size` to select a smaller subset. The matrix stops on the
first nonzero result. Its fixture dimensions must match those passed to the
outer harness. The build record contains per-file
workspace hashes before and after compilation and a binary hash; the matrix
verifies these before each case and fingerprints the workload scripts and
migration CLI. Source equality at the two checkpoints cannot exclude a
transient edit during compilation; migration CLI provenance is not established
by this record. The clones share synthetic UUIDs and use uncontrolled caches.
For a baseline, build and record the detached baseline worktree separately,
then provide the matching `--migration-cli` to both the outer fixture and
matrix, and pass `--source-repo`, `--server` and its provenance record to the
matrix. `--allow-missing-metrics` retains results when the old binary lacks
telemetry. Verify the shared fixture SQL against the older schema first; matching
inputs, cache conditions and binary provenance are required for comparison.

For SIGINT or SIGTERM, the outer fixture signals and reaps only the child
session it created after checking its process-group/session identity. The
matrix and workload attempt to stop their registered server and clean registered
clone databases while unwinding; topology drift is a failure, not a license to
signal an unknown process. SIGKILL, host loss, or a child that exceeds the
45-second graceful shutdown window can prevent its own separate server session
or clone from being cleaned. A surviving process or UUID-named disposable
fixture must be inspected and handled by the operator; the harness does not
claim crash-proof cleanup.

If a clone creation loses its PostgreSQL acknowledgement, the matrix prints the
candidate UUID name as `uncertain_clone_create` and refuses to delete that
unregistered database. An operator must verify its ownership and state before
any cleanup. The fixture's own database creation has the same general limitation
if its client loses the creation acknowledgement.

The optional POSIX lifecycle regression can be run with
`python3 scripts/verify_perf_outer_interrupt.py --signal SIGTERM` and again
with `--signal SIGINT`. It creates a tiny owned fixture, validates the exact
fixture parent/session before signalling it, checks that the server and clone
were cleaned, and checks the outer fixture database is gone. Build provenance is
mocked for this cleanup test; it is not benchmark evidence.

The opt-in rate-limit contention workload exercises the production
`AuthStore::record_rate_limit_attempt` against an invocation-owned fixture:

```text
python3 scripts/perf_representative.py --seed-timeout 1200 -- cargo test --release -p melodee-persistence --test perf_rate_limit -- --ignored --nocapture
```

It checks exact counter values for shared and separate actor keys, reporting
three sequential rounds at one, eight and 32 concurrent workers. The 5 ms
PostgreSQL lock samples are approximate. HTTP, password hashing, mixed
interactive activity and production authentication rates are excluded; the
result alone cannot justify changing security or capacity semantics. Record the
binary/source identity, environment, settings and cache conditions separately.

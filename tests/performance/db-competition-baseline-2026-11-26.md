# Database competition baseline, 2026-11-26

This is a single-host baseline, not a final comparison or an acceptance result.
The isolated release server was built from `a0979cde1fd2aeff5b8027d9d59336b678f1aa12`;
the [build record](db-competition-baseline-build-2026-11-26.json) records binary
SHA-256 `98e48029c573ce369adace7cba474bfe37200abb50fa7023ea16b1cfb614f530`
and source fingerprints taken immediately before and after the build. These
fingerprints cannot establish reproducible compilation or exclude a transient
edit. The independently built migration CLI is hashed in the matrix record, but
its source correspondence was not independently established.

An invocation-owned parent fixture contained 35,000 artists, 50,000 releases,
and 600,000 tracks. Each case ran sequentially on a fresh clone of that fixture,
with startup search rebuilding, 200 imports, and concurrent authenticated admin
statistics and native artist browsing over loopback HTTP. Media were generated
five-second MP3 and FLAC files, one track per imported release. The fixture took
222.0 seconds to seed. Worker stage concurrency was two; the PostgreSQL 18.3
server had `max_connections=100`, `shared_buffers=128MB`, and `work_mem=4MB`.
The host was a 24-logical-CPU AMD Ryzen 9 3900X with approximately 62.7 GiB of
memory and btrfs storage. Operating-system and PostgreSQL cache state was
uncontrolled. Full settings, timestamps, samples, hashes, and exact results are
in the [matrix record](db-competition-baseline-matrix-2-2026-11-26/matrix.json)
and its two linked case JSON files.

| Pool maximum | Workload elapsed | Admin overlap samples; p50 / p95 / p99 | Browse overlap samples; p50 / p95 / p99 | Server CPU / peak RSS |
| --- | ---: | --- | --- | --- |
| 8 | 127.26 s | 102; 837 / 1,097 / 1,298 ms | 374; 39 / 165 / 333 ms | 121.42 s / 433 MiB |
| 32 | 127.25 s | 103; 828 / 1,045 / 1,087 ms | 380; 36 / 125 / 264 ms | 119.97 s / 476 MiB |

Each case observed approximately 115.2 seconds of overlap and 200 heartbeat
events. Both completed without HTTP errors or a workload timeout. Sampling at
0.5 seconds observed one lock waiter in one of 245 samples in each case; this
does not bound shorter waits. CPU and RSS cover the server only, excluding
PostgreSQL and media subprocesses. The historical baseline has no exported
per-class acquisition/hold histograms, so the matrix correctly marks metrics
incomplete; missing values are not zero. The baseline migration CLI and server
were separately built, and the case JSON's end-of-run repository revision refers
to the harness checkout, not the baseline server's revision. The matrix/build
records provide the server provenance.

This run omits idle, ingestion-only, and rebuild-only controls, and consists of
one trial per pool size. The fixture omits physical media for the existing
catalog, realistic ACLs, and representative identity evidence; policy,
discovery, enrichment, and podcast workers were disabled. Discrete database
samples can miss waits, and the measurements do not isolate database CPU, I/O,
locks, or scheduler time. Consequently these numbers alone do not justify a
pool-size decision or claim a before/after improvement.

After the integrated source is stable and the shared build/database slot is
available, build a fresh release server and CLI, then run matching mixed cases
on new invocation-owned clones. Choose new output paths: neither command
overwrites existing evidence. The build record fingerprints the server source;
the CLI hash is recorded separately and its source identity remains a stated
limitation.

```console
python3 scripts/build_perf_server.py --output tests/performance/db-competition-final-build.json
cargo build --release -p melodee-cli
python3 scripts/perf_representative.py --seed-timeout 1200 --migration-cli target/release/melodee-cli -- python3 scripts/run_db_competition_matrix.py --mode mixed --pool-size 8 --pool-size 32 --albums 200 --seconds 900 --server target/release/melodee-server --migration-cli target/release/melodee-cli --provenance tests/performance/db-competition-final-build.json --output-dir tests/performance/db-competition-final-matrix
```

Any changed workload source, configuration, catalog fixture, or missing metric
must be reported when comparing runs. Additional control cases and repeated
trials are needed to assess workload admission independently.

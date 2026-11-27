# Scrobble contention stage isolation, 2026-11-26

This targeted release-build experiment investigates the earlier
[tail-latency repeat](recommendation-tail-repeat-2026-11-26.md). It is not a
full production acceptance or a causal proof. The release test binary SHA-256
was `94824c1ea3f62567f327a96aca7fa4cf6a380c9f8099bc66316dd4c64086537f`,
unchanged after execution. It was built on dirty HEAD
`08de08d50cb1be6e76c66000232dae257c1a2d2f`. The
[hash manifest](recommendation-stage-2026-11-26-source-hashes.json) records
591 source/configuration hashes after compilation and before the workload;
none changed during the workload. This is not a clean committed source tree.

The disposable catalog had 35,000 Artists, 50,000 Releases and 600,000 Tracks
with preexisting primary credits. Unlike the previous repeat fixture, it
already contained Track credits; the harness adds one separately identified
credit per selected Track and verifies all preexisting and added primary
Artists against authoritative plays. This makes the two runs non-identical and
precludes a controlled before/after comparison. Each of five sequential rounds
contains 160 plays across eight workers for each of three scenarios; the third
also performs 20 raw primary-credit SQL updates. All tests passed with zero
errors and exact checked Track/Release/Artist popularity. The test used a
16-connection pool, no concurrent ingestion, no HTTP transport and no physical
media. PostgreSQL backend CPU, memory and I/O were not measured. Cache state
was not controlled, and later rounds reused prior data and warmed caches.

Trace-only instrumentation measured individual binding attempts, Track/Artist/
Release/table retry waits, play INSERT including its database triggers, counter
upserts, and COMMIT. A benchmark-only subscriber collected bounded samples;
all 160 expected INSERT/counter/COMMIT samples per scenario and round were
present. Trace collection and SQLx event counting add measurement overhead,
so these durations are not uninstrumented production latency. Stage samples
are not correlated to individual end-to-end requests. The full sanitized
observations are in [the raw log](recommendation-stage-2026-11-26.log).

| Scenario | End-to-end p99 range across rounds | Relevant stage p99 range | Stage samples per round |
| --- | ---: | ---: | ---: |
| Same Track | 511.423–759.326 ms | Track retry wait 507.005–847.036 ms | 7–12 waits |
| Separate Tracks, shared Artist | 348.858–629.044 ms | Play INSERT 342.232–624.276 ms | 160 INSERTs |
| Same Track plus credit updates | 597.938–868.411 ms | Track retry wait 641.607–861.927 ms | 10–16 waits |

In same-Track rounds, INSERT p99 ranged 0.530–5.500 ms, counter-upsert p99
0.363–1.721 ms, and COMMIT p99 4.755–12.243 ms. The longer retry-wait samples
are consistent with the observed end-to-end tail, but are too few and are not
correlated request by request. In separate-Track rounds, there were zero NOWAIT
probes, while the play INSERT stage itself had the large tail. That INSERT
includes recommendation-trigger updates, but this measurement cannot distinguish
candidate-row locking from the rest of the INSERT and trigger work. The
previously measured admission semaphore wait was small in this workload; that
does not prove admission is harmless at higher concurrency or in another pool.

The corresponding per-round end-to-end p50/p95 ranges were 3.966–4.965 /
7.061–146.967 ms for same Track; 11.996–13.940 / 47.038–115.977 ms for separate
Tracks; and 3.977–4.976 / 8.024–139.321 ms for concurrent credit updates.
There were 361 same-Track and 457 concurrent-update NOWAIT probes across the
five rounds. Percentiles are per 160-sample round, not pooled estimates or
confidence intervals. Further investigation is required before changing the
lock protocol or declaring the regression resolved; synchronous freshness,
small-pool heartbeat progress, cancellation and all-writer lock order remain
constraints on a safe fix.

Reproduce with matching source and the exact release binary:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-persistence --test perf_recommendation_contention --no-run
MELODEE_PERF_RECOMMENDATION_ROUNDS=5 CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_representative.py -- cargo test --release -p melodee-persistence --test perf_recommendation_contention -- --ignored --nocapture
```

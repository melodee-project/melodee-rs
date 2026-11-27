# Concurrent search rebuild and outbox diagnostic, 2026-11-26

One release-build run used an invocation-owned PostgreSQL fixture and the real
search rebuild, outbox, and query service paths. The benchmark process's HEAD
at execution was `08de08d50cb1be6e76c66000232dae257c1a2d2f`; its working tree was dirty and the compiled
binary's exact source identity is unverified. Binary SHA-256
`c2c9cc5c8adf11c4eb89239bca0e58f3c18661c9dfbd374c6450c8c3fc85203b`; benchmark source SHA-256 at run
`c698910a2971b6c94fab43b2457a730195aba1bbedea4ea91b3b6feed38d3499`; fixture script SHA-256
`3e6b5305a7d420aea2881141862c3d4aa4acffe5282c89e4df106a2da7183b7d`. Tracked diff SHA-256 at run was
`f35f8791b0c9dd0413ae37598b68143c9ebbcc009c18a755da8817fc263d6a1c`; it excludes untracked files. Sanitized
raw sample data and rebuild events are in
[search-concurrent-2026-11-26.json](search-concurrent-2026-11-26.json).

The fixture contained 35,000 Artists, 50,000 Releases, 600,000 Tracks, synthetic
relationships, ratings, genres and acoustic evidence. It took 182.0 s to
construct. An initial PostgreSQL-backed generation was rebuilt and activated
before measurement. PostgreSQL 18.3 used 128 MB shared buffers, 4 MB work_mem
and 100 max connections; the test pool limit was 16 connections. Host: AMD
Ryzen 9 3900X, 24 logical CPUs, 62 GiB RAM, Linux 7.3.4 and NVMe-backed btrfs
workspace. Cache was uncontrolled and warm from fixture creation and the first
rebuild.

A feature-gated test barrier paused the second rebuild immediately after its
repeatable-read snapshot began. During that pause, setup SQL atomically changed
1,024 Artist, 512 Release and 512 Track revisions and inserted the matching
2,048 mixed outbox events. The pause added approximately
254.5 ms to snapshot
age. After release, four in-process clients repeatedly called the production
active-generation query service with a 50 ms delay per client, while one
production outbox worker drained the events and the second generation rebuilt.
The test continued query sampling for 150 ms after rebuild completion. SQLx
statement tracing was enabled for counting and may affect timing; per-query
DEBUG output was disabled.

- The old generation was still active when the worker had applied and
  acknowledged all 2,048 events in eight batches over
  548.810 ms (3732
  events/s). The new generation then replayed all 2,048 obligations in eight
  catch-up batches and activated. Updated Artist, Release and Track documents
  were independently found after activation.
- The second snapshot was held for 32.491 s, including the deliberate pause;
  its 69 commits completed before 0.463 s catch-up. The activation database call
  took 6.640 ms. That call time is **not** the duration of its exclusive lock
  hold, which was not separately measured.
- Segment files for the old generation were 18 before and 42 at drain finish;
  the newly active generation had 42 after activation. These snapshots are
  not merge counts or a long-duration segment-churn measurement.
- Peak benchmark-process RSS rose from 248,720
  to 421,164 KiB. It is a process high-water
  mark including the initial rebuild, both generations and query clients; it
  excludes PostgreSQL and cannot isolate writer memory.
- SQLx emitted 8,243 logged
  statement events for **all** concurrent work combined. These are not a
  count of wire round trips or outbox-only statements.

Query latency uses nearest-rank percentiles, one request per client at a time:

| Sample window | Requests | p50 ms | p95 ms | p99 ms | Maximum ms |
|---|---:|---:|---:|---:|---:|
| All measured queries | 2,472 | 2.175 | 3.348 | 4.502 | 14.359 |
| Started during outbox drain | 40 | 1.946 | 3.148 | 3.450 | 3.450 |
| Started after activation | 12 | 2.670 | 6.778 | 6.778 | 6.778 |

All 2,472 queries returned the expected Artist identity; zero errors
were recorded. The 40 drain-window and 12 post-activation samples are too few
for stable tail estimates. No measured request happened to span the activation
completion timestamp, so this run does not prove the behavior of an in-flight
request across the exact switch. Samples repeatedly queried the same synthetic
Artist; they are not independent. The production service was called in-process
without HTTP, authentication, streaming, competing processes, physical media,
or multi-process writer contention. No controlled baseline or long soak was
measured.

This run supplements the [mixed outbox lifecycle measurements](search-outbox-mixed-2026-11-26.md).
It supports retaining the current batch-scoped writer for this specific tested
workload, but does not establish bounded per-writer memory, safe long-lived
writer reuse, cross-process reset fencing, or a general latency guarantee.

Reproduce with the `test-support` feature and an invocation-owned fixture:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-search --features test-support --test perf_outbox_concurrent --no-run
python3 scripts/perf_representative.py -- python3 scripts/run_perf_search_case.py --test-binary PATH_TO_PERF_OUTBOX_CONCURRENT_TEST --timeout-seconds 600
```

The wrapper verifies the fixture marker and bounds execution; the test uses a
private temporary search root. The timeout includes both rebuilds and measured
work but excludes fixture creation.

## Subsequent in-flight reader correctness smoke

The benchmark was extended after the 600,000-track measurement with a
feature-gated query barrier. On a 20-Artist, 30-Release, 60-Track disposable
fixture, a query selected the old-generation reader at 1.182 ms, then was
deliberately paused while 110 mixed events were consumed, replayed into the
new generation, and activated. After activation completed at 170.080 ms and
retirement returned, the held query resumed and returned its expected Artist.
All three updated entity types were visible in the new generation. This is one
small deterministic correctness smoke, not a production-scale activation latency
sample or a naturally occurring in-flight query. The subsequent release test
binary SHA-256 was `b8d248763c58986a5b8ca50ddfdc16f88eeb96f6a87c914706104be608f516a6`; its tree was dirty. The full serialized PostgreSQL
`rebuild_integration` suite also passed 13/13 after the hook change, including
both late-lower-ID and old-generation-acknowledgement regressions.

The current reproduction command above includes this additional held-query
check. Its intentional delay is excluded from the continuous-client latency
samples and must not be interpreted as user-visible latency.

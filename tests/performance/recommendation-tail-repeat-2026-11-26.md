# Recommendation tail-latency repeat, 2026-11-26

A second, targeted release-build experiment investigated the high p99 values in
the preceding [NOWAIT run](recommendation-merge-nowait-2026-11-26.md). It is
not a production-load certification and does not establish a cause. The
prebuilt test executable SHA-256 was
`7511680acd579fa4466733982610102dc3e6e396694372da6bd0c17418550795`; its hash
was unchanged after execution. The build used dirty HEAD `08de08d50cb1be6e76c66000232dae257c1a2d2f`.
The [hash manifest](recommendation-tail-repeat-2026-11-26-source-hashes.json)
records 590 source/configuration hashes at compilation. Two unrelated files,
`apps/melodee-server/tests/perf_interactive_routes.rs` and
`scripts/perf_ingress_workload.py`, changed during the run; the benchmark
binary and fixture script did not. The source-hash manifest identifies but does
not itself preserve the dirty source contents.

An invocation-owned fixture had 35,000 synthetic Artists, 50,000 Releases and
600,000 Tracks and took 150.7 seconds to seed. Five sequential rounds ran each
of three production `record_scrobble` scenarios: eight workers and 160 samples
per round. Each concurrent-credit scenario also ran 20 raw SQL Artist changes.
The binary was executed directly without Cargo rebuilding it. Every scenario
reported zero errors and exact recommendation counts. The eight-worker
benchmark used a 16-connection pool; SQLx event counts exclude BEGIN and
PostgreSQL trigger internals. The test itself measured no PostgreSQL CPU, RAM,
or disk I/O. No media ingestion, HTTP transport or streaming ran concurrently;
cache state was uncontrolled and later rounds reused earlier fixture state.

| Scenario | Per-round p50 range | Per-round p95 range | Per-round p99 range | Total NOWAIT probes |
| --- | ---: | ---: | ---: | ---: |
| Same Track | 4.058–10.259 ms | 44.030–456.047 ms | 574.878–1,280.512 ms | 590 |
| Separate Tracks, shared Artist | 11.045–48.134 ms | 81.192–166.951 ms | 232.747–808.652 ms | 0 |
| Same Track plus credit updates | 4.082–5.048 ms | 7.034–255.105 ms | 679.420–852.263 ms | 513 |

These are ranges of five separately computed 160-sample percentiles, not pooled
percentiles or confidence intervals. Full sanitized observations and histogram
deltas are in [the raw results](recommendation-tail-repeat-2026-11-26.log).
Admission-wait observations sum to 35.637 ms across 590 probes for the same
Track, and 16.502 ms across 513 probes for concurrent credit updates; all but
13 individual admission observations were at most 1 ms, and none exceeded 5 ms.
The separate-Track scenario had no NOWAIT probes while sampled PostgreSQL lock
waits reached seven concurrent waiters. These observations do not support
permit-admission time as the dominant measured component, but do not separate
Track backoff, trigger candidate locks, user-state writes, scheduling, and
other database work. Additional stage isolation is required before choosing or
claiming a fix. In particular, this measurement does not justify declaring the
prior tail-latency concern resolved.

Reproduction with matching source and a prebuilt release test target:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-persistence --test perf_recommendation_contention --no-run
MELODEE_PERF_RECOMMENDATION_ROUNDS=5 CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_representative.py -- cargo test --release -p melodee-persistence --test perf_recommendation_contention -- --ignored --nocapture
```

The fixed histogram upper bounds are 1, 5, 25, 100, 500, 2,500, 10,000 and
60,000 ms plus overflow. Admission instrumentation records successful permit
acquisitions; it does not measure cancelled requests waiting for admission. The
underlying sample size and a single machine limit generalization. Raw direct SQL
credit updates exercise a bypass path and do not establish arbitrary-writer
correctness.

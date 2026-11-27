# Track handoff hybrid comparison, 2026-11-26

The previous [two-arm comparison](recommendation-handoff-comparison-2026-11-26.md)
showed that bounded handoff reduced hot-Track p99 while raising median latency.
An isolated experimental hybrid allowed one nonblocking probe phase for at most
25 ms before escalating to handoff. The hybrid did not improve the observed
median and increased query events, so it is **rejected**; its
[experimental patch](recommendation-hybrid-experiment-2026-11-26.patch) was
never applied to the shared implementation. The handoff candidate was also rejected and removed: its 6–10× hot-Track median
increase and added SQL work were not justified by the tail gain without a product
latency budget or fairness proof.

The disposable fixture contained 35,000 Artists, 50,000 Releases, and 600,000
Tracks with primary credits. Six separately cloned copies ran sequentially in
control, handoff, hybrid, hybrid, handoff, control order. Each clone ran three
rounds of 160 plays per scenario using eight workers and a 16-connection pool;
the concurrent-credit scenario also performed 20 credit changes per round.
All six arms passed, with zero errors and exact checked popularity. Fixture
creation took 177.6 seconds. [Raw measurements](recommendation-hybrid-comparison-2026-11-26.log)
include clone timings, query-event counts, approximate sampled lock waiters,
trace stage durations, and wait-admission counts and histogram buckets. Each
clone and the source fixture were removed on completion.

Ranges below describe six per-round percentiles and rates for each candidate,
not pooled request percentiles or confidence intervals.

| Scenario | Candidate | p50 ms | p95 ms | p99 ms | Plays/s |
| --- | --- | ---: | ---: | ---: | ---: |
| Same Track | Control | 3.97–5.99 | 6.92–196.36 | 564.25–928.89 | 128.86–180.30 |
| Same Track | Handoff | 32.03–46.20 | 37.01–210.01 | 42.12–230.07 | 137.03–239.26 |
| Same Track | Hybrid | 32.02–49.96 | 52.60–128.11 | 112.22–164.05 | 127.24–208.16 |
| Shared Artist, separate Tracks | Control | 12.01–22.94 | 46.03–131.68 | 380.68–507.09 | 141.74–244.70 |
| Shared Artist, separate Tracks | Handoff | 10.02–18.99 | 41.96–166.32 | 330.01–499.03 | 136.93–303.24 |
| Shared Artist, separate Tracks | Hybrid | 11.94–18.95 | 47.97–129.22 | 325.52–611.98 | 141.28–257.84 |
| Same Track and credit changes | Control | 3.98–4.98 | 75.73–179.65 | 738.75–901.00 | 134.71–171.84 |
| Same Track and credit changes | Handoff | 32.02–61.21 | 44.98–160.03 | 91.21–334.27 | 114.00–202.11 |
| Same Track and credit changes | Hybrid | 32.07–48.03 | 49.98–131.04 | 66.00–375.84 | 137.39–220.40 |

For same-Track rounds, SQLx query events were 2,287–2,413 control,
3,027–3,038 handoff and 3,400–3,468 hybrid. Admission counts were 62–100,
158–160 and 294–304 respectively. These counts and the higher hybrid event
count show additional database work, not independently measured backend CPU or
I/O. The handoff requeues after 100 ms and does not guarantee FIFO. Neither
this sample nor the recorded per-round p99 proves starvation freedom or gives
per-request maximum latency. The shared-Artist path still exhibits a distinct
INSERT-stage tail; the measurement cannot attribute it to a particular trigger
or row lock.

All three release binaries were frozen before this run and their SHA-256 hashes
were verified unchanged afterward. The hybrid binary hash was
`ea75a699d18fea2c9f7310cd898d6e8b7ac43aadf72133a56bfd078e3d3d4f1f`;
control and handoff hashes and 592 isolated source/configuration hashes are in
the [manifest](recommendation-hybrid-comparison-2026-11-26-source-hashes.json).
The isolated sources were copied from dirty HEAD
`08de08d50cb1be6e76c66000232dae257c1a2d2f`, then modified by the experimental
patch, and were unchanged during this run. These hashes identify the work but
do not establish clean-commit reproducibility. The hybrid's focused
`native_user_state_phase7` suite passed 22/22 and a small six-arm smoke passed.
The migration CLI hash is in the manifest; applied database migration checksums
were not separately captured.

The test excluded HTTP, ingestion, media, real client arrivals and concurrent
interactive work. Cache state was uncontrolled; clone ordering reduces, but
does not eliminate, order and host effects. Trace collection and SQLx event
counting add overhead; lock polling is approximate. PostgreSQL backend CPU,
memory and I/O were not measured. This is neither target-scale acceptance nor
a recommendation to adopt either candidate. The comparison runner
creates only invocation-owned clones, verifies identities and cleans them up;
its lifecycle and ordering tests pass.

With matching frozen binaries, reproduce with:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_representative.py -- python3 scripts/run_perf_scrobble_comparison.py --control /path/to/control-test-binary --treatment /path/to/handoff-test-binary --hybrid /path/to/hybrid-test-binary --rounds 3
```

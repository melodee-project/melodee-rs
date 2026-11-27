# Track handoff comparison, 2026-11-26

A controlled, instrumented release-build comparison tested a proposed bounded
Track-lock handoff against the preceding NOWAIT retry implementation. The prototype was rejected and removed after the later three-arm comparison; this is
not a general production performance claim. The disposable
fixture contained 35,000 Artists, 50,000 Releases, and 600,000 Tracks with
primary credits. Four fresh clones were tested sequentially in control,
treatment, treatment, control order. Each clone ran three rounds of 160 plays
per scenario, with eight workers and a 16-connection pool. The concurrent-credit
scenario also made 20 credit changes per round. All arms passed with zero
errors and exact checked popularity. Fixture creation took 177.7 seconds;
cloning took 8.78–10.93 seconds per arm. The source fixture and each explicitly
owned clone were removed on completion. See the [raw results](recommendation-handoff-comparison-2026-11-26.log).

The table gives ranges of per-round values across the six control and six
treatment rounds, not pooled request percentiles or confidence intervals.

| Scenario | Implementation | p50 ms | p95 ms | p99 ms | Plays/s |
| --- | --- | ---: | ---: | ---: | ---: |
| Same Track | Control | 3.98–6.00 | 7.82–188.98 | 578.17–1,028.77 | 123.75–181.77 |
| Same Track | Handoff | 32.07–47.94 | 41.00–149.96 | 49.13–375.00 | 115.56–229.95 |
| Separate Tracks, shared Artist | Control | 12.02–17.08 | 44.96–164.36 | 367.63–579.22 | 163.97–242.79 |
| Separate Tracks, shared Artist | Handoff | 12.01–18.11 | 42.20–108.85 | 272.28–621.91 | 155.79–251.36 |
| Same Track and credit changes | Control | 3.98–5.93 | 8.04–261.43 | 703.54–1,387.85 | 80.13–170.26 |
| Same Track and credit changes | Handoff | 32.09–47.12 | 49.15–75.02 | 52.18–116.72 | 163.68–217.95 |

The handoff retained an initial nonblocking Track lock attempt, but in the
same-Track case 159 of 160 plays per treatment round entered Track wait,
compared with 8–14 wait samples per control round. The treatment moved much of
the wait from a few long-running requests into the median while reducing the
observed p99. It also increased SQLx query events from 2,284–2,446 to
3,034–3,044 per same-Track round. Treatment admission-wait totals ranged from
3.34 to 6.56 seconds summed across plays per same-Track round, versus
0.0002–0.004 seconds for control; sums are overlapping waits, not elapsed time.
The shared-Artist scenario still showed an INSERT-stage tail, and this test
cannot isolate recommendation-row locks from the rest of INSERT/trigger work.
The proposed handoff uses a bounded 100 ms wait and requeues after timeout; it
is not FIFO. Per-request maxima, long-duration starvation and contention beyond
this sample were not measured. Pool sizes 1 and 2 use nonblocking probes;
separate focused tests covered cancellation and progress there, but this
performance run did not.

The result does not establish that the prototype is preferable in normal user workloads.
The handoff and its [hybrid](recommendation-hybrid-comparison-2026-11-26.md) were
rejected because the observed 6–10× hot-Track median increase and added SQL work
were not justified by the tail gain without a product latency budget or fairness proof. The fixture excludes HTTP, ingestion, media, real client arrival
patterns and concurrent interactive requests; cache state was not controlled.
Trace collection and SQLx event counting add overhead. PostgreSQL process CPU,
I/O and memory were not captured; benchmark-process peak RSS was 5,952–6,448
KiB. Lock waiter polling is approximate. Identical clones and ABBA ordering
reduce state/order bias but do not remove host or storage variability.

Both binaries were frozen before the run and verified unchanged afterward.
Control SHA-256 `94824c1ea3f62567f327a96aca7fa4cf6a380c9f8099bc66316dd4c64086537f`;
treatment `1d7338084d309897719c1317cd5512ac0a5cc81b44b648ca9d886798570f6e58`.
The treatment's source was dirty at HEAD
`08de08d50cb1be6e76c66000232dae257c1a2d2f`. The
[manifest](recommendation-handoff-comparison-2026-11-26-source-hashes.json)
records 592 source/config hashes captured before seeding and verified unchanged
afterward. The control was the earlier frozen stage-isolation binary; its
source manifest is linked in [that report](recommendation-stage-2026-11-26.md).
This dirty source and non-committed binaries mean the hashes establish what was
run but do not by themselves make those binaries rebuildable from a clean commit.
The migration CLI hash is included in the manifest. Fixture migrations ran
normally; applied database migration checksums were not separately captured.

With matching binaries, reproduce using the owned fixture runner:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 python3 scripts/perf_representative.py -- python3 scripts/run_perf_scrobble_comparison.py --control /path/to/control-test-binary --treatment /path/to/treatment-test-binary --rounds 3
```

The runner validates the fixture URL and topology, creates one UUID-named clone
at a time, verifies the clone's database identity, and removes only clones
created by its invocation. Failure and target checks have Python unit tests.

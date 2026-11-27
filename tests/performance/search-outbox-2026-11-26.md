# Incremental search outbox diagnostic, 2026-11-26

This is one release-build production-path measurement on an invocation-owned
PostgreSQL fixture, based on HEAD `08de08d50cb1be6e76c66000232dae257c1a2d2f` with a dirty working tree.
The exact source identity of the compiled binary is unverified. Binary SHA-256:
`9d10623d6aa8e221ab665527df2f9be6135100cbb9370adbd30a35f67f9e358c`; benchmark source at run SHA-256:
`be0864a837bda4a4bb70399d2c11048f8c919172e24216501bf40b34bb4f50ad`; fixture script SHA-256:
`3e6b5305a7d420aea2881141862c3d4aa4acffe5282c89e4df106a2da7183b7d`. Other run metadata is captured below.

The fixture had 35,000 artists, 50,000 releases, 600,000 tracks, synthetic
acoustic evidence, genre terms, artist relationships and ratings. It did not
include physical media or concurrent client/worker activity. PostgreSQL 18.3
used 128 MB shared buffers and 4 MB work_mem. Host: AMD Ryzen 9 3900X, 24
logical CPUs, 62 GiB RAM, NVMe-backed btrfs workspace, Linux 7.3.4. Fixture
creation took 177.4 s, excluded from the measurement. The index was first
rebuilt and activated; the outbox started empty. Cache conditions were
uncontrolled and warm from seeding and rebuilding.

A single set-based setup updated 2,048 Artist names and revisions and inserted
the corresponding outbox obligations atomically. The measured phase used
`apply_outbox_once` to claim, hydrate, index, commit and acknowledge eight
256-event batches. Every event was applied and acknowledged, no errors occurred,
and an updated Artist was then found through the real active-generation search
service. Direct setup SQL and initial rebuild are excluded from drain timing.

- Drain: 0.433460 s, 4725
  events/s, eight batch samples. The maximum event creation-to-acknowledgement
  lag was 576.569 ms, including queue time.
- SQLx emitted 152 executed-statement events
  during the drain, or 19.0 per batch. This
  is a statement-event count, **not** a count of network round trips.
- One cold active search query took 7.891
  ms; ten subsequent queries had median 1.704
  ms and range 1.468–2.222 ms.
  These are in-process calls, including active-generation lookup and reader
  refresh, without HTTP or concurrent traffic.
- Process peak RSS was 265,816 KiB before
  and 320,236 KiB after the drain and query
  samples. These are process high-water marks that include the initial rebuild;
  they do not isolate per-writer memory or include PostgreSQL.

| Measured operation | Samples | Median ms | Max ms | Total ms |
|---|---:|---:|---:|---:|
| Writable index open | 8 | 0.262 | 0.309 | 2.051 |
| Writer initialization | 8 | 0.807 | 0.842 | 6.203 |
| Writer reader initialization | 8 | 2.819 | 4.030 | 22.967 |
| Commit | 8 | 10.726 | 18.991 | 93.139 |
| Commit reader reload | 8 | 1.190 | 1.699 | 9.901 |
| Cached reader reload | 11 | 0.891 | 1.326 | 10.491 |

The writable index was opened and its writer initialized once per batch. The
eight writer-initialization calls totalled 6.203 ms out of the
433.460 ms drain; index open and reader
initialization are measured separately above. Segment counts observed after
these commits rose from 20 to 41; this is not a measured merge count or a
long-duration segment-churn result. No failure, lease loss, generation change,
multi-process writer contention, mixed entity types, concurrent traffic or
writer-reuse alternative was measured. One small, warm, Artist-only sample
cannot by itself justify the final writer lifecycle decision.

The benchmark source was subsequently extended to run mixed-event trials; the
current test no longer reproduces this exact Artist-only phase. See
[the mixed-event report](search-outbox-mixed-2026-11-26.md). The original run
used the following commands from a built migration CLI and release test executable:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-search --test perf_outbox --no-run
python3 scripts/perf_representative.py -- python3 scripts/run_perf_search_case.py --test-binary PATH_TO_PERF_OUTBOX_TEST --timeout-seconds 600
```

The wrapper times out and reaps only its benchmark child and removes its private
temporary search tree. The outer fixture verifies and removes only its owned
database. The timeout excludes fixture creation and includes initial rebuild.
Raw outbox-phase events follow; the test printed no real user data or paths.

```text

DEBUG melodee_search::store: search index opened elapsed_seconds=0.000183445 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000679346
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.00226841
DEBUG melodee_search::store: search writer committed commit_seconds=0.011957372 reload_seconds=0.001084937 pending_revisions=256 segments=20
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000254739 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000811414
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.002144646
DEBUG melodee_search::store: search writer committed commit_seconds=0.00861135 reload_seconds=0.000926329 pending_revisions=256 segments=23
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000250911 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000801766
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.002395578
DEBUG melodee_search::store: search writer committed commit_seconds=0.009062798 reload_seconds=0.001265366 pending_revisions=256 segments=26
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000210976 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000682541
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.002286494
DEBUG melodee_search::store: search writer committed commit_seconds=0.013539664 reload_seconds=0.001323475 pending_revisions=256 segments=29
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000278473 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000835459
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003322649
DEBUG melodee_search::store: search writer committed commit_seconds=0.009263324 reload_seconds=0.00144818 pending_revisions=256 segments=32
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000294112 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000819679
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.00324325
DEBUG melodee_search::store: search writer committed commit_seconds=0.009495029 reload_seconds=0.00103815 pending_revisions=256 segments=35
DEBUG melodee_search::store: search index opened elapsed_seconds=0.00030895 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000842021
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.004029516
DEBUG melodee_search::store: search writer committed commit_seconds=0.018990569 reload_seconds=0.00169882 pending_revisions=256 segments=38
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000269276 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000730542
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003276362
DEBUG melodee_search::store: search writer committed commit_seconds=0.012218804 reload_seconds=0.001115254 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.00025082 writable=false
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.004504168
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.00132626 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001122788 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001111097 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001038781 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000890833 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.00074047 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000764225 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001042397 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000880433 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000799361 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000774334 success=true
{"workload":"postgres-search-incremental","build_debug_assertions":false,"fixture":{"artists":35000,"releases":50000,"tracks":600000},"events":2048,"batches":8,"batch_size":256,"elapsed_seconds":0.433459689,"events_per_second":4724.776148676654,"sqlx_statement_events":152,"max_end_to_end_outbox_lag_ms":576.569,"batch_elapsed_ms":[55.93042,52.078639,55.823059,52.011933000000006,52.708992,50.240497000000005,63.84693600000001,50.817009000000006],"query_elapsed_ms_first_then_warm":[7.890996000000001,2.222263,2.021817,1.875933,1.646371,1.467616,1.514784,1.8667449999999999,1.760667,1.519713,1.472715],"peak_process_rss_before_kib":265816,"peak_process_rss_after_kib":320236,"limitations":"single process and event type; no concurrent activity or physical media; SQL setup excluded; query timings in-process; SQLx events count executed statements, not network round trips; RSS is process high-water including the full rebuild"}
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 33.48s

exit_code=0
```

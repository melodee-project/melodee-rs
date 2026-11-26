# Performance Remediation — Status Ledger

**Operation:** session 3 (resumed from `ca8ef83`)
**Base revision at session start:** `ca8ef83`
**Toolchain:** Rust 1.99.0 (pinned); PostgreSQL 18.3 (dev compose, port 5433)
**Host budget:** 24 vCPU, 62 GiB RAM. `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=1`.
**Authority:** `PERF-REMEDIATION-SPEC.md`. **Evidence:** `PERF-REMEDIATION-RESULTS.md`.

Terminal states: **IMPLEMENTED AND VERIFIED**, **ALREADY SATISFIED AND VERIFIED**,
**NO CHANGE JUSTIFIED**, **EXTERNALLY BLOCKED**. `PARTIAL`/`NOT IMPLEMENTED` are checkpoints.

## Requirement-level state (current)

| Package | State | What exists | Exact remaining action |
|---|---|---|---|
| RM-00 | PARTIAL | bounded fixed-bucket metrics; pool-acquire metric on catalog/work/search claim paths; three correctness regressions; 50K plan evidence | Build representative 560K–600K catalog + end-to-end release-build benchmarks (import, policy, media, rebuild, interactive, streaming) |
| RM-01 | IMPLEMENTED AND VERIFIED | narrow index 0060; 50K plan 0.029 ms/3 buffers vs 7.778 ms/1303 | — |
| RM-02 | IMPLEMENTED AND VERIFIED | scoped normal + dry-run acoustic queries; plan measured; novelty suites pass | — |
| RM-03 | IMPLEMENTED AND VERIFIED | genre release/surface index; status guard; Track-move term maintenance | — |
| RM-04 | IMPLEMENTED AND VERIFIED | advisory-lock recount protocol; idempotent repair CLI; concurrency regression | — |
| RM-05 | IMPLEMENTED AND VERIFIED | snapshot-visible obligations + exclusive activation fence; two regressions incl. real old-generation consumer | — |
| RM-06 | IMPLEMENTED AND VERIFIED | durable per-class discovery cursors; 100K fairness regression | — |
| RM-07 | IMPLEMENTED AND VERIFIED | bounded 64-entry scheduler result ring; 10k-tick test | — |
| RM-08 | IMPLEMENTED AND VERIFIED | full-batch drain; monotonic external maintenance gate; durable last_prune_at | — |
| RM-09 | PARTIAL | durable seeds persisted atomically; **explicit captured-id deletion (fixes the commit-order/watermark bug)**; bounded drain batch; fixed-point iterations retain seeds on exhaustion; 30-min Artist+Release backstop; seed/idempotency/late-commit tests; global-oracle equivalence for scoped reconciliation not yet established | Scope candidate discovery to the seed set and its transitive closure instead of the catalog-wide algorithms per coalesced pass |
| RM-10 | PARTIAL | prepared manifest revalidation and pool-acquire instrumentation; phase-one filesystem load still happens after opening and locking the write transaction (newly identified gap) | Move filesystem preparation before write locks, prove complete revalidation and connection release; measure background/interactive competition and decide admission |
| RM-11 | PARTIAL | tool-version cache (3 subprocess probes/album avoided) | Eliminate provably redundant content reads/hashes/copies with integrity/failure-injection tests |
| RM-12 | PARTIAL | `inspection_concurrency` now governs actual concurrent execution (bounded, per-item lease heartbeats); in-flight bounded by claim batch | Shared process-wide CPU/FS/subprocess admission beyond per-stage bounds; measured concurrency tuning |
| RM-13 | PARTIAL | bounded verified reader registry (reload-on-query); ordered batch hydration; batch-read fallback | Measure reader/writer overhead; long-lived writer evidence-based decision |
| RM-14 | PARTIAL | fresh-generation bulk insert; admitted blocking pages; commit bounds; single-sample 600K baseline/current PostgreSQL rebuild evidence | Resolve cross-process-safe reset, run concurrent catch-up/activation and final reproducible performance verification |
| RM-15 | PARTIAL | partial leading artist-rating index | Measure and optimize admin stats/OpenSubsonic/native/ACL; preserve wire contracts |
| RM-16 | IMPLEMENTED AND VERIFIED | ADR-0215 opt-in bounded lease-event retention; duplicate indexes dropped | — |
| RM-17 | PARTIAL | blacklist singleflight; byte-bounded artwork cache **keyed by source content hash** with replacement test | One-reader-per-stream, combined auth lookup, rate-limit contention measurement |
| RM-18 | PARTIAL | all evidence batching; sorted-key advisory lock helper | Recommendation/play-trigger contention measurement |
| RM-19 | PARTIAL | set-based merge/split correctness | 1K (and resource-safe 10K) merge/split timing |
| RM-20 | PARTIAL | runbook/docs updated; fresh + populated upgrade verified; `make check`/`make check-nightly` pass | Final closeout only after the remaining mandatory items |

## Migrations

`0060` indexes/duplicate drops · `0061` genre correctness/repair · `0062` discovery progress ·
`0063` search activation replay · `0064` identity seeds + sorted lock helper.

## Verification (session 3)

- `catalog_importer`: 9 PASS, including new late-lower-id seed retention test.
- `melodee-search rebuild_integration` (test-support): 13 PASS with commit bounds.
- `melodee-ingestion inspection` (5) + `environment_faults` (7): PASS.
- `melodee-server` media unit tests (3) incl. artwork replacement invalidation: PASS.
- Workspace fmt/clippy: PASS.
- `make check-nightly` with `RUST_TEST_THREADS=1`: PASS. (One pre-existing podcast lease test is
  timing-sensitive to parallel in-binary tests; it passes isolated/serialized and is unrelated.)

## Remaining mandatory work (not complete)

RM-00 representative benchmark; RM-09 scoped closure; RM-10 competition measurement;
RM-11 content-read reduction; RM-12 shared admission/measured tuning; RM-13 writer evidence;
RM-15 route optimization; RM-17 streaming/auth/rate-limit; RM-18/RM-19 measurements.

## Current execution checkpoint (based on `8a4cf6f`)

The current working tree contains uncommitted work. Previous session results above
are historical and have not been rerun against this working tree. All remaining
package-level requirements and the hard definition of done continue to apply;
the following entries do not replace them.

| Requirement | Implementation | Evidence | Status | Next action |
|---|---|---|---|---|
| RM-00: reproducible disposable Rust catalog fixture | `scripts/perf_representative.py`; UUID-owned database via `scripts/test_database.py` | 20/30/60 smoke and 35,000/50,000/600,000 fixture construction plus CLI count smoke; second full run verified 600,000 acoustic evidence and 1,200,000 genre terms; `tests/python/test_perf_representative.py` | PARTIAL | Validate synthetic ratings at scale; add identity relationships, ACLs, artwork, alternate assets, valid media, workload runners, environment recording and baseline/final production-path measurements. |
| RM-09: acknowledge only successful reconciliation | `crates/melodee-catalog/src/importer.rs` and `scheduler.rs` | `identity_drain_preserves_seeds_when_candidate_discovery_fails` passed on PostgreSQL | PARTIAL | Finish scoped indexed discovery for the remaining Artist algorithms and all evidence mutations; persist and test transitive closure, plans, interruption, and global-oracle equivalence. All five scoped candidate comparisons, 10 importer tests, 3 identity tests, 5 merge/split tests and a targeted seed test pass; exhaustive mutation additional mutation seeds and hard-delete rollback/retry tests pass; exhaustive mutation coverage, transitive closure, plans and concurrency remain incomplete. |
| RM-11: reduce redundant processing | `crates/melodee-media/src/normalize.rs` | 56 unit and 13 real-media tests passed (one ignored); source inventory recorded | PARTIAL | Verify integrity/fault cases, address remaining copy/hash/probe duplication and measure production paths. |
| RM-12: shared resource admission | `crates/melodee-ingestion/src/admission.rs` and ingestion workers | 50 ingestion unit, 53 PostgreSQL integration, and 2 saturation/lease tests passed after the atomic test-helper change; four-permit provisional bound | PARTIAL | Complete process-wide coverage, file/temp/subprocess bounds and measured concurrency matrix. |
| RM-15: reduce administrative aggregate scans and getArtists allocation | `apps/melodee-server/src/admin_stats.rs`, `opensubsonic/browse.rs` | 2 admin statistics and 24 OpenSubsonic browse tests passed after the rating query change | PARTIAL | Capture representative plans and release-build response costs; address remaining routes and ACL/rating queries. |
| RM-17: reduce read scheduling and combine compatible auth lookups | `apps/melodee-server/src/native_stream.rs`, `opensubsonic/auth.rs`, `crates/melodee-persistence/src/auth.rs` | 21 streaming and 15 OpenSubsonic integration tests passed | PARTIAL | Measure throughput, slow-client behavior and rate-limit contention. |

Immediate execution order: finish in-flight focused tests, run the representative
fixture after Cargo is idle, then continue RM-09 closure and all remaining work.
The fixture currently contains synthetic metadata and references only; it is not
representative end-to-end evidence. No final acceptance or integrated gate has run.

Baseline preparation: detached worktree `../melodee-rs-perf-baseline-a0979cd`
points at `a0979cde1fd2aeff5b8027d9d59336b678f1aa12`. It has not been built or
measured. Use a separate target directory and the same disposable fixture and
configuration when constructing a genuinely comparable baseline; do not use
construction smoke timings as application benchmark results.

Additional current execution state:

- RM-10: a source audit found that the prior preparation still began after the
  write transaction/lock and held a connection across evidence I/O. A correction and checkout/hold metrics now pass focused binding, expiry, lease,
  external-reference and cancellation races, importer, evidence and metrics tests.
  Opt-in trace export, separate checkout timing, histogram precision and admin
  classification pass focused metrics, PostgreSQL, admin and Python tests. Next:
  broaden coverage, measure mixed-load contention and run integrated tests.
- RM-12: the new saturation tests passed again after the atomic test-only permit
  acquisition helper change.
- RM-13/14: a source audit found synchronous rebuild generation setup and no
  shared admission for rebuild blocking tasks. Bounded admission and lifecycle metrics passed the admission unit test and all
  13 rebuild integration tests. Reset remains synchronous because process-local
  admission cannot fence a cross-process takeover; exporter, writer lifecycle and
  representative measurements remain open.
- RM-15: a release-build authenticated endpoint benchmark and representative
  rating experiment are being prepared; no measured result yet. The updated
  set-based rating query has a focused test but still needs the full browse suite.
- RM-09: all five scoped queries and several mutation seeds have focused tests;
  the deletion rollback/retry test covers Artist, Release, and Track. A complete
  mutation audit, closure/oracle concurrency tests, and production-shaped plans
  are still required.
- RM-18/19: opt-in production-store benchmark targets have been drafted, and
  the large-release observation scan has been grouped by track. The grouping/outbox changes pass focused tests, and the debug small fixture
  verified a 1K merge/split. Release-build 600K-fixture RM-18 and 1K/10K RM-19 workloads passed before
  migration 0065; rerun after final lock protocol and obtain comparable baseline.
- RM-18 smoke found a reproducible SQLSTATE 40P01 deadlock for concurrent
  scrobbles of the same Track. A Track lock-order correction and concurrent regression pass; the small
  multi-scenario workload passes. Representative contention measurement remains
  outstanding.
- RM-18 reviewer found direct/bulk `native_track_plays` writers bypass the new
  API pre-lock and can still deadlock during the trigger's FK lock upgrade. A
  trigger-level correction, migration, and direct-insert concurrency regression
  are required before treating the scrobble deadlock as fixed for all writers.
- RM-18 lock-protocol review found the discovery popularity repair could write a
  stale recount over a concurrent trigger increment. The draft migration and repair now pass 7 focused concurrency tests plus fresh
  and populated upgrades. The full cross-statement/writer lock-order audit remains
  incomplete, so this is not yet a qualified all-writer correctness claim.
- RM-00 unchanged reimport: a two-release small-fixture redrop reached cataloged
  twice but the harness reported changed Release/Track identifiers. These were distinct work items, for which ADR-0116 permits distinct Release
  identities, so the comparison is not a valid same-item retry oracle. A same-item uncertain-commit retry regression now passes and preserves IDs,
  counts and the exact seed set. A representative reprocessing workload and
  avoided-work measurement are still required.
- RM-18 continued audit: controlled portable/legacy play imports now prelock
  Tracks and use ordered set-based insertion (focused suites pass). Scrobble
  parent/Track snapshot revalidation was added but remains under revision to avoid
  unbounded SCROLL materialization. Match-decision, negative-assertion and other
  multi-statement writers still require lock-order work and concurrency tests.
- Current hard-delete review found a Release→Track membership race with raw
  Track moves. The hard-delete owner is implementing two-phase parent locking,
  transaction-local captured scope and exact revalidation; the previous 6/6
  test result predates this correction and is not final proof.
- The scrobble phased cursor's small-fixture plan contains outer and inner Sort
  nodes despite index-only scans. It does not establish bounded database temp
  work; redesign/admission and a representative plan are required.
- The revised identity benchmark's small augmented smoke produced non-empty
  strong Release results matching the global query. Its full 600K run has not
  been repeated with that case and the current cursor-plan probe.
- Current lock-protocol work is intermediate: an expanded participation closure
  and importer Artist prelocking are being edited by separate owners. Do not run
  integrated gates or treat the current tree as a final candidate until both
  compile and their race regressions pass. Identity benchmark owns DB/load slot.

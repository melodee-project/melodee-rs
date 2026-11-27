# Performance Remediation — Status Ledger

**Operation:** integrated remediation resumed from `8a4cf6f`
**Current intermediate HEAD:** `08de08d`; further implementation is uncommitted
**Toolchain:** Rust 1.99.0 (pinned); PostgreSQL 18.3 (dev compose, port 5433)
**Host budget:** 24 vCPU, 62 GiB RAM. `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=1`.
**Authority:** `PERF-REMEDIATION-SPEC.md`. **Evidence:** `PERF-REMEDIATION-RESULTS.md`.

Terminal states: **IMPLEMENTED AND VERIFIED**, **ALREADY SATISFIED AND VERIFIED**,
**NO CHANGE JUSTIFIED**, **EXTERNALLY BLOCKED**. `PARTIAL`/`NOT IMPLEMENTED` are checkpoints.

## Requirement-level state (current)

| Package | State | What exists | Exact remaining action |
|---|---|---|---|
| RM-00 | PARTIAL | Disposable 35K/50K/600K fixture and real-media workloads; multiple release-build measurements | Complete comparable baseline/current production-path workload matrix, provenance and resource evidence. |
| RM-01 | IMPLEMENTED AND VERIFIED | narrow index 0060; historical 50K plan and regression evidence | Reconfirm in final integrated verification. |
| RM-02 | IMPLEMENTED AND VERIFIED | scoped normal and dry-run acoustic queries; focused tests; 600K production policy measurements | Reconfirm in final integrated verification. |
| RM-03 | IMPLEMENTED AND VERIFIED | genre indexes, status guard, Track-move term maintenance | Reconfirm in final integrated verification. |
| RM-04 | IMPLEMENTED AND VERIFIED | advisory-lock recount protocol; repair CLI and concurrency regression | Reconfirm under final migrations and repair rehearsal. |
| RM-05 | IMPLEMENTED AND VERIFIED | snapshot-visible obligations and activation fence; old-generation consumer regression | Rerun final integrated regression. |
| RM-06 | PARTIAL | durable per-class discovery cursors; 100K fairness regression; bounded traversal and durable staging in progress | Finish bounded traversal and staging, crash fairness, independent review and integrated tests. |
| RM-07 | PARTIAL | bounded 64-entry worker and periodic-source result retention; regression tests | Reverify alongside current discovery and scheduler changes. |
| RM-08 | IMPLEMENTED AND VERIFIED | full-batch drain; monotonic maintenance gate; durable last_prune_at | Reconfirm in final integrated verification. |
| RM-09 | PARTIAL | durable captured seeds and affected-closure candidate discovery for all five algorithms; representative scoped cases | Complete mutation/concurrency audit, closure and interruption equivalence, production-shaped plans and final integrated tests. |
| RM-10 | PARTIAL | filesystem evidence prepared before transaction; fenced revalidation and checkout/hold instrumentation | Complete workload-class instrumentation and representative mixed-load comparison; decide admission based on evidence. |
| RM-11 | PARTIAL | tool-version cache and elimination of some duplicate media processing | Complete operation inventory, integrity/fault verification and representative actual read/hash/probe measurements. |
| RM-12 | PARTIAL | shared ingestion admission and codec concurrency samples; optional connection cap | Complete coverage, durable discovery/storage bounds, cancellation/fairness tests and evidence-backed defaults. |
| RM-13 | PARTIAL | bounded reader registry, batched hydration, outbox writer and lifecycle measurements | Finish reader/writer, memory and concurrency evaluation and substantiate final lifecycle decision. |
| RM-14 | PARTIAL | admitted rebuild pages, commit bounds and single-sample 600K baseline/current rebuild | Finish snapshot, cancellation, catch-up and concurrent activation measurements and final regressions. |
| RM-15 | PARTIAL | admin aggregate and OpenSubsonic rating/allocation improvements; route samples | Finish representative route, ACL and rating plans/measurements and justified optimizations. |
| RM-16 | IMPLEMENTED AND VERIFIED | ADR-0215 opt-in lease-event retention; duplicate indexes dropped | Reconfirm in final integrated verification. |
| RM-17 | PARTIAL | content-aware artwork cache, blacklist singleflight, buffered streaming and combined auth lookup | Measure slow-client streaming and realistic mixed authentication load; assess observed rate-limit contention while preserving exact semantics and verify final compatibility. |
| RM-18 | PARTIAL | batched evidence and recommendation lock-order changes; contention measurements | Complete writer audit and shared-Artist fairness/contended correctness measurement. |
| RM-19 | PARTIAL | set-based merge/split and representative 1K/10K measurements | Reverify final implementation, state/projection correctness and transaction/lock costs. |
| RM-20 | PARTIAL | intermediate upgrade/runbook and historical gate results | Reconcile all requirements, rehearse through final migrations, run final correctness and quality gates, and consolidate evidence. |

## Migrations

`0060` indexes/duplicate drops · `0061` genre correctness/repair · `0062` discovery progress ·
`0063` search activation replay · `0064` identity seeds + sorted lock helper ·
`0065` recommendation projection locks · `0066` non-owning identity seeds ·
`0067` bounded discovery (in progress) · `0068` staged settling snapshots (in progress) ·
`0069` Jellyfin track ordering index · `0070` durable custody intent (in progress).

## Historical verification (session 3; not the current working tree)

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
| RM-09: acknowledge only successful reconciliation | `crates/melodee-catalog/src/importer.rs` and `scheduler.rs` | `identity_drain_preserves_seeds_when_candidate_discovery_fails` passed on PostgreSQL | PARTIAL | Complete mutation coverage, transitive closure, concurrency and interruption comparisons with the global oracle; verify production plans. All five scoped algorithms have representative comparisons and focused seed tests, but these do not prove exhaustive equivalence. |
| RM-11: reduce redundant processing | `crates/melodee-media/src/normalize.rs` | 56 unit and 13 real-media tests passed (one ignored); source inventory recorded | PARTIAL | Verify integrity/fault cases, address remaining copy/hash/probe duplication and measure production paths. |
| RM-12: shared resource admission | `crates/melodee-ingestion/src/admission.rs` and ingestion workers | 50 ingestion unit, 53 PostgreSQL integration, and 2 saturation/lease tests passed after the atomic test-helper change; four-permit provisional bound | PARTIAL | Complete process-wide coverage, file/temp/subprocess bounds and measured concurrency matrix. |
| RM-15: reduce administrative aggregate scans and getArtists allocation | `apps/melodee-server/src/admin_stats.rs`, `opensubsonic/browse.rs` | 2 admin statistics and 24 OpenSubsonic browse tests passed after the rating query change | PARTIAL | Capture representative plans and release-build response costs; address remaining routes and ACL/rating queries. |
| RM-17: reduce read scheduling and combine compatible auth lookups | `apps/melodee-server/src/native_stream.rs`, `opensubsonic/auth.rs`, `crates/melodee-persistence/src/auth.rs` | 21 streaming and 15 OpenSubsonic integration tests passed | PARTIAL | Measure throughput, slow-client behavior and rate-limit contention. |

Current execution: integrate migrations 0067/0068 and bounded discovery, resolve
reviewed ownership/fencing and cleanup risks, then run their focused tests when
the single benchmark/build slot is available. The 600K baseline mixed-load
fixture finished construction, but its first case failed before measurement;
owned databases were cleaned; the port-binding fix passed a 3,600-track smoke
and the full owned baseline mixed-load matrix has restarted (sequential pool
sizes 8 and 32). No final acceptance or integrated gate has run.

The detached baseline worktree `../melodee-rs-perf-baseline-a0979cd` points at
`a0979cde1fd2aeff5b8027d9d59336b678f1aa12` and has been built separately.
Some paired production-path measurements exist; the full mixed-workload matrix
remains incomplete. Fixture construction time is not application performance.

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
  Track moves. Two-phase parent locking and transaction-local scope revalidation now pass
  four raw-move races and an independent review. Broader integrated tests and
  the all-writer name/participation protocol remain unfinished.
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
- Importer Artist reuse now has an unverified parent-first revalidation change;
  a new matching Artist can still appear after the check because not all
  create/rename producers share a name-lock protocol. This remains open.
- Four hard-delete raw Release-move regressions pass after two-phase parent
  fencing. Independent review of the new implementation and full integration
  tests are still required.
- The current name/participation lock protocol is a multi-module in-progress
  change. Importer/name/legacy focused races passed; the refined participation
  closure passed its targeted tests. Hard-delete name-lock and boundedness
  follow-ups are being tested. A complete writer audit, independent review,
  migration rehearsal and integrated gates remain mandatory.
- During this execution, HEAD changed externally from `8a4cf6f` to `08de08d`.
  That commit contains a broad snapshot of the shared in-progress changes despite
  its documentation-only subject. Subsequent uncommitted changes continue on top.
  It is an intermediate checkpoint, not proof of final integration or gates.
- Scrobble uses a new bounded-page NOWAIT / rollback / wait / retry prototype in
  place of the Sort-producing paired cursor. All 13 focused tests passed,
  including pending credit writes and bounded exhaustion. Independent review,
  representative plans and post-migration contention tests are still needed.
- Scrobble contention: a 15-second overall deadline was rejected because
  Jellyfin ignores scrobble errors and could silently lose a play. The current
  work preserves waiting semantics while bounding individual DB lock waits; a
  process-wide wait admission gate, backoff, cancellation and pool-starvation
  tests are still being added. Earlier NOWAIT test results predate this revision.
- Scrobble wait probes now use NOWAIT and release their DB connection between
  jittered retries; 18 focused tests and a timeout-restoration test passed.
  The separate opt-in bounded TCP listener remains under review/testing and has
  no measured safe default, so retained-request bounding is not yet complete.
- Name-scoped importer, create/rename and legacy import/delta focused races
  passed. Participation and hard-delete tests passed at checkpoints, but a
  complete integrated writer audit and final suites are still needed.
- An opt-in HTTP/1 pre-accept connection limit now has focused listener, registry
  and admin-config tests. Its default is disabled and no mixed-load/slow-stream
  evidence establishes a safe default; it does not yet satisfy bounded ingress.
- Binding-phase table-lock contention was found to bypass probe admission; a
  NOWAIT table preflight and admitted retry path are being implemented/tested.
- Latest serialized integration subset: native phase7 20/20, audio identity
  2/2, catalog identity 7/7, Artist merge/split 6/6, migrations 14/14 pass;
  the six historical migration tests were ignored here and remain required.
- Recommendation Track handoff is a 100ms bounded lock attempt/requeue, not a
  FIFO guarantee. Its 22 focused tests pass; a sequential owned-database ABBA
  control/treatment experiment is in progress before any adoption decision.
- Ingress: real HTTP smoke and a 600K fixture experiment passed no-lost-play
  checks, but limits 4/16 delayed unrelated requests; default 0 remains
  unbounded. TTFB and slow-stream samples are synthetic and insufficient to
  select a safe default.
- Original and current disk-backed 200-release ingestion and 50 same-item
  catalog retry samples are recorded, but the current binary's exact dirty
  source is unverified and major subsystems were bypassed.
- The Track handoff and 25 ms hybrid were rejected after controlled comparison:
  they improved hot-Track p99 but increased p50 roughly 6–10× and SQL events.
  The NOWAIT path was restored and its full 22-test phase7 suite passed.
  Fairness and shared-Artist tail contention remain open; do not describe the
  rejected prototypes as completed optimizations.
- Additional scheduler/discovery audit: periodic reconciliation still retains a
  result per configured source, and filesystem discovery plus missing-source
  reconciliation collects source-wide candidates/paths and active rows without
  an explicit bound. This is not covered by the worker result ring; a bounded
  processing and safe missing-source protocol is being designed and tested.
- Periodic reconciliation now processes all configured sources while retaining
  at most 64 results; a stop notification race was corrected. Two unit tests
  and a 70-source PostgreSQL test passed. Public one-shot output remains
  complete. Source-wide filesystem/active-row collection bounds and exact
  missing-path semantics remain unresolved.
- Discovery migration 0067 is a WIP durable per-scope scan/spool protocol; 22
  focused PostgreSQL reconciliation tests, the one-shot busy deadline and a
  populated 0066→0067 migration test passed. Independent review continues. Its
  active spool and per-Release snapshot still lack explicit hard bounds; this is
  not completion of the bounded-discovery requirement.
- Discovery snapshot metadata signatures now use a versioned incremental
  digest with compatibility for stored legacy signatures. Snapshot unit tests,
  legacy settling and staging acquisition tests passed. This does not replace
  cryptographic content/custody checks and does not yet bound file vectors or
  durable source spool storage.
- Migration 0067 and discovery fencing are being rewritten to replace the
  unbounded candidate spool with bounded resumable partitions. Earlier 0067 and
  reconciliation test results do not certify the current WIP. Search concurrent
  workload runs from a previously built binary; no final gates have run.
- The replacement 0067 per-item partition-marker design now compiles and its
  focused alias, 260-candidate, busy fairness, migration and legacy interrupted
  handoff tests pass. Independent review, full suite, 16-pass measurement,
  fixed-interval-crash tail fairness and bounded snapshot vectors remain open.
- 0067 handle-relative discovery is WIP: Linux root/alias regressions passed,
  but the original non-Unix fallback was found to be disabled. A safe Windows
  capability-based path, target-specific dependencies and ADR-0218 are being
  implemented; cross-target and real Windows/Apple runtime verification have not
  run. The source is currently not a final build candidate.
- The Windows-only discovery capability fallback and ADR-0218 are WIP. A prior
  cross-target check passed before the latest fallback edits; it is not Windows
  runtime verification. The narrowly scoped `winx` license exception passes
  the license check. Current focused/cross-target checks and independent review
  are underway; Apple and Windows runtime behavior remain unverified.
- Windows capability fallback passed cross-target compilation and static review,
  but native Windows runtime is unavailable here (`wine`/`wine64` absent; only
  Linux is installed). This is a specific environmental verification gap, not a
  claim of runtime compatibility. Apple runtime verification is also pending.

- Current 0068 review identified worker-claim/settling publication races and
  unbounded per-turn abandonment cleanup. Atomic lease/state-fenced publication,
  bounded resumable cleanup, transactional staging quota, deterministic races and
  production streaming integration are in progress; none is verified yet.
- RM-17 rate-limit contention probe `crates/melodee-persistence/tests/perf_rate_limit.rs`
  is drafted to call the production store against an invocation-owned database;
  it has not been compiled or measured. Measure shared and distinct actor keys
  under concurrency, and retain correctness/capacity guarantees in any decision.
- The full baseline mixed-load fixture loaded 35,000 Artists, 50,000 Releases,
  and 600,000 Tracks; sequential pool-8/pool-32 workloads are running. Fixture
  loading is not a performance result. A tiny baseline reproduction verified
  the harness bind-address correction and 25 Python tests passed.
- 0068 now has an unverified logical staging quota, opaque frozen snapshot
  verification, atomic observation publication, and independent bounded GC.
  Review uncovered possible starvation in discovery's multi-page clear and
  missing-path loops, plus fixed-interval crash prefix replay. Resolve these
  with durable fair progress without unsafe missing-path decisions; test quota
  accounting, cascade/rollback, concurrent claims, GC and shutdown.
- Baseline mixed pool-8 case completed on the owned 600K fixture: 200 generated
  one-track Releases, 127.26 s, 102 overlapping admin samples at 837/1097/1298
  ms p50/p95/p99 and 374 browse samples at 39/165/333 ms, zero HTTP errors.
  One sample, uncontrolled cache, baseline diagnostic metrics unavailable; no
  final-source comparison exists yet. Sequential pool-32 also completed: 127.25 s, 103 admin and 380 browse overlap samples, zero HTTP errors. See the baseline matrix JSON for provenance and limitations; the final comparison remains outstanding.
- 0068 test slot handed to the persistence owner after both baseline cases
  cleaned up. Discovery still has open cancellation supervision, per-turn
  cursor fairness, fixed-interval interruption and Windows path-compatibility
  concerns; proposed limits are not accepted merely because they are bounded.
- Persistence owner reports 0068 focused PostgreSQL suite 9/9 passing, including
  fresh migration and populated 0067→0068, lease claim fencing, quota and
  cascade/rollback, orphan GC and atomic publication. Existing 0067 upgrade and
  ingestion integration tests are next; this is not final integration evidence.
- Ingestion owner reports the current Linux library suite passed 64 tests (one
  ignored benchmark) and PostgreSQL reconciliation passed 39/39 in 226.83 s.
  Windows cross-target compilation is running; native Windows/Apple behavior,
  fixed-interval crash fairness and bounded per-turn cursor work remain open.
  The persistence cleanup-fairness change still needs its own regression/rerun.
- The final post-rotation 0068 persistence suite passed 9/9, including fresh and
  populated upgrades, work-item claim fencing, quota concurrency, disk-full
  rollback, orphan GC and two-capture fairness. This is focused verification;
  the final full migration/repair rehearsal and integrated gates remain open.
- The rate-limit release test compiled and its 60-track smoke passed exact
  shared/separate-actor counter checks. Its measured rate-limit function was text-identical
  to the original baseline, but the smoke is not representative acceptance. The
  owned 600K fixture run is in progress and excludes HTTP/password hashing.
- RM-17: release production-store rate-limit benchmark ran on the owned 600K
  fixture: three rounds, 1/8/32 workers, shared and distinct keys, exact
  counters, zero errors. Singleton-lock contention reaches 31 sampled waiters
  at 32 workers. `tests/performance/rate-limit-600k-2026-11-26.md` records
  hashes, raw samples and exclusions. HTTP, password hashing, traffic mix and
  multi-process load were excluded, so retaining/changing the limiter is not yet
  justified solely by this test. The DB/build slot returned to ingestion tests.
- Latest process-owned four-worker reaper passed its two focused runtime/
  fairness tests, a PostgreSQL scheduler-stop test with a deliberately stalled
  snapshot and Windows cross-target compilation; independent static re-review
  found no new concrete P0/P1. Native Windows and Apple runtime remain unverified.
- Release deep-rescan characterization `tests/performance/discovery-deep-rescan-2026-11-26.md` recorded one warm tmpfs 1,017-entry/17-level traversal at 225 ms.
  It does not establish large/slower-tree scaling or bound fixed-crash replay.
- RM-10/17: rate-limit pool acquisition and hold metrics were added after the
  above measurement. Five metric unit tests and the PostgreSQL atomic/expiry/
  capacity regression passed, including a canceled acquisition recorded without
  a hold. Post-instrumentation measurement remains required; the recorded
  executable predates this instrumentation.
- Discovery recovery-reset cursor passed its focused 513-observation, owner
  handoff, later-source and reappearance test, plus interrupted/alias/root and
  transient-failure cases and a populated upgrade. The added 0067 constraint subsequently passed fresh and populated 0066→0067→0068
  upgrade regressions. Missing
  page bounds, fixed-crash traversal fairness and Windows handle budget remain.
- RM-12 media subprocess audit: ffmpeg/fpcalc error paths return after killing
  and reaping the direct child but before joining pipe-reader threads. Simply
  joining them can hang past the timeout if a descendant inherits a pipe. A
  bounded, supervised pipe/process-tree termination design and regression are
  still required; neither detached readers nor unconditional joins satisfy it.
- Discovery missing-page WIP now records per-item generation markers and yields
  after four pages with full root reobservation on handoff; an interrupted-turn
  two-source regression has been written. The latest 0067 migration/source
  edits are untested. Repeated whole-source scans, fixed-crash enumeration
  fairness and Windows handle limits remain open.
- Media subprocess audit/design is in progress for child-tree termination and
  bounded pipe draining. Jellyfin Audio candidate index plans and Artist page
  optimization are in progress; neither is a completed measured improvement.
- Jellyfin Audio SQL plan evidence on the owned 600K fixture shows the proposed
  `(track_number, public_api_id)` index reducing the tested page from roughly
  110–118 ms to 1.3–1.7 ms; exact count remains 68–77 ms. `0069` and Artist
  page optimization are written but untested. The first focused test build
  failed in unrelated in-progress media code (`Signal::Kill`), so it provides
  no Jellyfin or migration verification. Media owner is fixing the build.
- Media subprocess supervisor: Linux library suite passed 61 tests (one ignored),
  Windows GNU cross-target check passed with an existing warning, license gate
  passed, and real normalization passed 13/13. Native Windows process-tree
  termination is unverified. All-target media Clippy passed after correcting an existing
  release-profile benchmark assertion lint without weakening the assertion.
- Jellyfin Audio plan probe `scripts/perf_jellyfin_audio_plan.py` and six Python
  safety tests were added; the new reusable harness has not yet run against a
  full owned fixture. Previous SQL plan samples used a one-off probe.
- Media subprocess supervisor also passed two additional inherited-pipe and
  admission tests; process-wrap Windows JobObject support and ADR-0219 were
  added. Independent review, wrapper-specific fault tests and a post-change
  release codec/worker matrix remain required; native Windows runtime is not
  available here. Jellyfin focused route/migration tests own the current slot.
- Guarded missing-cursor focused tests passed for 513-item bounded resume,
  reappearance, deterministic root replacement, distinct invalid-byte names
  and non-starving retries. Three existing interrupted/alias/root regressions,
  fresh and populated 0066→0069 upgrades, and Windows cross-target library
  compilation passed. Full integration, representative 35K-missing work and
  fixed-crash enumeration fairness remain open.
- Media owner identified uncached tool-version probes that do not propagate
  lease/shutdown cancellation. A fix and wrapper-level inherited-pipe tests are
  prepared but not yet applied or run; the current media test results predate them.
- Independent media review found additional open P1s: executable-busy retries
  can spawn after a deadline/cancellation, and non-Linux/macOS Unix fallback
  does not terminate a successful leader's remaining process group despite
  the ADR's broader wording. Fix and test before treating RM-12 as complete.
- Jellyfin media API suite passed 20/20 including new Artist pagination and
  Audio null/tie ordering; fresh and populated 0059→0069 upgrade/repair passed
  with 0069's exact definition and validity verified. The reusable SQL probe's
  six safety tests passed, but its owned-clone smoke and route-level before/after
  measurements remain outstanding.
- Independent discovery review found unresolved root-swap-between-check-and-
  commit and lossy non-UTF8 source identity issues; focused tests so far do not
  prove them fixed. It also found volatile 0067 birth-time default could rewrite
  populated work items; migration now uses a stable fast default for old rows,
  then restores the live clock default. The changed migration requires renewed
  fresh/populated upgrade, cutoff and representative lock/duration checks.
- Latest media review fixes passed 66 library tests (one ignored), covering
  executable-busy deadline/cancel, wrapper inherited pipes and cancellation of
  uncached version probes; all-target media Clippy and Windows cross-target
  checks passed. Non-waitid Unix descendant cleanup and native Windows runtime
  remain open; final release codec matrix and independent re-review are pending.
- Revised 0067 fast-default migration passed fresh and populated 0066→0069
  tests, including `atthasmissing`, retained item state and insertion-time birth
  during a long transaction. Representative lock duration/disk and broader
  upgrade/repair rehearsal remain outstanding.
- The instrumented rate-limit release probe passed a small owned-fixture smoke
  (`tests/performance/rate-limit-instrumented-smoke-2026-11-26.log`); it is not
  a 600K before/after comparison. The guarded Jellyfin plan probe also passed a
  tiny real clone smoke and six safety tests. Representative reruns remain.
- Independent media re-review found Android omitted from safe process-group
  cleanup; the cfg is corrected and cross-compiles with `blake3/pure`; ordinary cross-
  compilation failed because the Android NDK compiler is unavailable. Native
  BLAKE3 build and Android runtime remain unverified.
- Post-supervisor release codec matrix completed on 8 generated single-track
  60-second Releases, 1/2/4 workers and two samples per case. Mean MP3 rates
  were 1.731/3.505/6.853 releases/s and FLAC rates 1.419/2.825/5.418; raw
  source-fingerprinted JSON is `tests/performance/codec-matrix-supervised-2026-11-26.json`. This media-stage warm-cache run excludes DB, scheduler and
  interactive traffic and cannot justify the whole-server default by itself.
- Non-Unicode occurrence-path encoding and nested in-root acquisition/cleanup
  are in progress for the newly found lossy-path and alias gaps. Independent
  security/compatibility review, ADR, upgrade docs and focused tests are pending.
- Independent review found P0 acquisition/cleanup races: replacing an ordinary
  directory after verification but before pathname-based staging or removal can
  acquire an unobserved occurrence or delete a replacement. The nested-path
  patch is not safe to ship. Implement an atomic occurrence-bound handoff/
  quarantine protocol that retains unexpected replacements intact, and add
  deterministic tests at both exact race windows for same/cross-filesystem.
- The unsafe provisional nested-acquisition relaxation was removed. Its new
  acceptance test remains intentionally failing until the occurrence-bound
  custody protocol is implemented; the same pathname race also affects the
  earlier direct-child implementation. Non-Unicode path changes remain WIP.
- Media owner is implementing a safe kqueue process-exit observer for Unix
  targets lacking waitid, with target-scoped dependency and ADR update; native
  OpenBSD/runtime verification and an appropriate toolchain remain unverified.
- Further path security review found an index-size risk for long encoded paths
  and possible misassociation of legacy lossy paths with distinct literal
  Unicode names. Full-equality-safe fixed-size lookup and durable ambiguity
  quarantine with end-to-end regressions are required before ADR-0220 can be
  accepted or nested acquisition enabled.
- Path encoding review additionally requires versioned quarantine for ambiguous
  pre-upgrade U+FFFD spellings and equality-safe fixed-size indexed lookup for
  long paths. Scheduler is editing 0067 and claim/handoff code; all earlier
  migration results predate these edits and must be rerun on the final chain.
- OpenBSD has no prebuilt std in this pinned toolchain; attempted FreeBSD
  cross-check is blocked by an upstream nix/libc NOTE_PCTRLMASK compile error.
  A macOS cross-check of the shared kqueue API is being prepared; it cannot
  establish native OpenBSD runtime behavior.
- Review of the WIP legacy ambiguity fence found no operator resolution/repair
  workflow and an unconditional guard that also blocks receipt-verified staged
  recovery. Add audited explicit resolution, visible reason, upgrade/runbook and
  recovery regressions; blocking indefinitely is not a completed repair.
- OpenBSD kqueue child supervision now cross-checks library and test sources
  with Rust 1.99 bootstrap/build-std and pure BLAKE3; macOS shared kqueue API
  cross-check, Linux descendant regression and target dependency license check
  also passed. The OpenBSD check is metadata-only with no sysroot/link/runtime;
  native OpenBSD/Windows/Android testing remains unavailable. FreeBSD's nix
  event cross-check encountered an upstream libc NOTE_PCTRLMASK compile error.
  Independent re-review and final-source codec evidence remain outstanding.
- Focused path-format tests passed for reversible byte keys, two byte-distinct
  occurrences through staging, v0 versus literal-Unicode v1 claim/acquisition,
  and receipt-verified legacy staging without inbound access; fresh and
  populated 0066→0069 migration tests passed for that candidate. Long-key/hash
  collision tests, audited operator resolution, root-swap atomicity, nested
  acquisition and the P0 custody protocol remain open.
- Custody review proposes durable same-filesystem NOREPLACE quarantine under a
  pinned parent, pre/post identity and cryptographic checks, fsync, safe
  no-replace restoration or intact retained quarantine, and idempotent crash
  recovery. POSIX lacks inode-conditional rename; handle both replacement and
  cross-filesystem cases without deleting unverified sources. This is a design
  under implementation, not a resolved P0.
- A retained-MP3 copy-first probe-reuse idea remains an unmeasured prototype
  investigation; no redundant source verification has been removed on that
  basis, and extra destination reads may negate any benefit.
- Audited legacy-path operator workflow and long-key/collision regressions
  passed focused Cargo/DB validation and the CLI parser test. P0
  occurrence-bound custody and nested acquisition acceptance remain incomplete.
- Media retained-MP3 single-probe candidate is an uncompiled prototype with no
  new tests or measurements yet; it must not be counted as an optimization
  until mutation/cancellation/no-clobber regressions and release comparison pass.
- Latest focused path/operator checks passed forced hash-collision concurrent
  uniqueness, v0/v1 coexistence, >2,704-byte encoded discovery, operator
  resolution/receipt recovery, populated fast-default upgrade and CLI parsing.
  Security review and the P0 custody/nested acceptance are still required.
- Custody migration 0070 and a lease-fenced intent/evidence/state persistence
  module are drafted and untested. No quarantine move, pre/post proof, safe
  replacement preservation, cross-device staging or crash recovery is wired;
  the P0 remains open and the nested acceptance test remains unmet.
- Retained-MP3 prototype has new no-clobber/cancellation/mutation tests drafted
  but has not compiled or been benchmarked; subprocess observation during an
  adversarial transient write remains an integrity limitation to assess.
- Independent review found P0: admin pre-catalog purge can use a resolved legacy
  ambiguous v0 spelling to delete a different current literal-name source; the
  immutable resolution FK can fail only after that filesystem deletion. Add a
  durable no-source-delete fence across all purge/cleanup paths, audit-retention
  semantics and deterministic regression before shipping the resolution flow.
- Retained-MP3 probe-reuse prototype weakened independent source/output
  observation and was removed without being benchmarked. RM-11 remains open;
  safe read/hash alternatives and actual operation counts are being investigated.
- Independent review found a second resolution-FK P0: catalog hard-delete
  removes canonical/staging files before DB row deletion; an immutable audit FK
  can then abort deletion, leaving catalog references to removed media. Fence
  before any filesystem action or preserve/archivalize the audit atomically,
  and add reviewed-item hard-delete and related FK deletion regressions.
- Follow-up independent review finds the earlier resolution/purge and
  post-filesystem audit-FK P0s addressed in current source by pre-filesystem
  purge fences and validated non-owning historical UUIDs. Purge-refusal and
  reviewed-target hard-delete/audit-survival regressions are written but unrun.
  The broad v1/U+FFFD purge restriction remains a nonterminal behavior gap; an
  audited safe purge protocol is still required. The unrelated acquisition/
  cleanup occurrence-swap P0 remains open.
- Proposed removal of an adjacent normalization hash was not applied: it would
  change the observable error classification if mutation occurs between the two
  checks. Publication verification/milestone alternatives remain under study.
- Purge/audit and draft custody persistence regressions are queued for the
  exclusive test slot; the filesystem custody protocol remains incomplete.
- Focused admin regressions now pass: ambiguous/encoded purge refuses before
  inbound/staging/work deletion, and catalog hard-delete retains the non-owning
  audit. Draft 0070 persistence fencing and populated 0069→0070 upgrade also
  passed. The broad purge restriction, filesystem custody P0, cross-device
  recovery and intentionally unmet nested acquisition remain open.
- Removed an unused normalization ffprobe version lookup; 14 real-media tests
  passed, including a wrapper count of two actual media probes and zero version
  probes. The saved subprocess occurs only on a cold executable-version cache when
  the configured path resolves directly. At that point bare defaults failed metadata lookup before spawning; the
  subsequent PATH-resolution fix and focused tests restored Unix provenance. This does not complete RM-11.
- A pinned-directory custody hash primitive and replacement test are drafted
  but unintegrated and currently buffer directory entries; the P0, boundedness,
  quarantine/crash recovery and nested acquisition remain unresolved.
- Admin purge for ordinary inbound paths also has the pre-existing same-kind
  replacement race, separate from the legacy ambiguity guard. Include it in
  the occurrence-bound protocol and deterministic source-replacement tests;
  descriptor-relative NOREPLACE/hash primitives are drafted but unintegrated.
- A Python/strace profiler for owned media workloads is being prepared to count
  actual reads/writes/execs; no host security changes or raw traces will be
  retained, and syscall attribution/physical I/O limitations will be stated.
- Custody review also found source-root 0700 is not an isolation boundary from
  same-UID/producers that can write its parent; the current prototype's
  custody_root_handle points at inbound itself and must not be wired. Use a
  separately configured same-device service-owned protected custody root with
  verified ownership/parent permissions, or conservatively retain/fail closed.
  Post-rename intent recovery across every crash boundary is not implemented.
- Custody prototype review also found lossy child-name tree hashing can equate
  distinct native filenames without a BLAKE3 collision, and a source-root swap
  after descriptor acquisition can stage a detached old root. Versioned native
  path hashing with legacy receipt verification, root identity fencing and exact
  swap/collision regressions are in progress; none is verified.
- Linux owned MP3/FLAC strace smokes completed and cleaned their raw traces.
  The corrected MP3 sample includes kernel-assisted copy bytes; each sample is
  one five-second generated track with ptrace overhead, unparsed file calls
  and incomplete attribution. These are diagnostic syscall samples, not
  physical I/O or complete end-to-end read/hash evidence.
- Focused custody tests passed native-name hash collision separation, pinned
  replacement/legacy digest parity, intent/fencing and populated 0070 upgrade.
  Protocol is still unintegrated: root fencing, staged mutation checks, crash
  recovery and quarantine isolation remain open, with dead-code warnings that
  final Clippy will reject. The P0 and nested acceptance are not complete.
- Bare-name tool-version PATH/cache identity correction is written but lacks
  focused tests and final build evidence; Windows bare names use uncached native
  search to avoid inventing CreateProcess semantics.
- An additional custody source-root observation fence and mismatch regression
  were added after the last focused tests; these latest edits are untested and
  do not by themselves solve crash/inode reuse or filesystem/DB atomicity.
- Bare Unix executable PATH resolution/cache tests passed for ordered lookup,
  reuse, same-size/same-mtime replacement, in-flight replacement, failed-probe
  retry and cancellation. A real retained-MP3 test confirms ffprobe provenance
  and two independent data probes. Windows cross-check passed with an existing
  warning; macOS build could not compile native BLAKE3 because host cc lacks
  Apple target flags. A final platform/integration gate remains outstanding.
- Latest disconnected custody tests passed 7/7 for native-name collisions,
  same-size mutation, replacement preservation and post-move root swap, plus
  the DB root/fence test. The later-swap gap, isolated same-filesystem custody,
  crash replay, unintegrated handles and nested acquisition remain unresolved.
- PATH version-cache code uses an Android native-search fallback because
  rustix EACCESS is unavailable there. Android and macOS cross-checks passed
  with pure BLAKE3; native linked builds and runtime behavior remain unverified.
- Latest disconnected custody library tests passed 9/9, including invalid
  namespace and a retained-open-writer characterization. Positive protected
  namespace coverage, writer-proof handoff, integration and crash replay remain
  open. Shared counters compile but have no emitted-count assertion yet and
  currently cause a deprecated-API warning; both need correction.
- The protected custody namespace positive assertion actually ran and passed in
  a disposable workspace child; the direct acquisition operation-counter test
  passed exact one-hash/one-copy and three-byte logical read/write assertions.
  The validator is still unintegrated and does not prove producer descriptors
  were closed. Cross-stage counters and production measurements remain WIP.
- Independent v2 hash review found no encoding ambiguity but flagged custody
  ACL handling and trust of the configured producer UID: ownership/mode checks
  alone cannot be treated as sufficient across supported platforms. Validate
  effective access and trusted deployment identities before production wiring.
- ACL-aware custody validator prototype passed both a real protected-namespace
  positive case and named-user ACL rejection on an owned temporary runtime child;
  deterministic post-rename name inspection also passed. It is Linux-only, not
  integrated, and does not provide writer-descriptor proof or crash recovery.
- Cross-stage fixed-label operation metrics and production harness aggregation
  are written; focused Rust tests and a tiny owned production-path smoke are
  starting. Events suppress parent spans to avoid inheriting sensitive fields.

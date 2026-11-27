# Performance Remediation — Status Ledger

**Operation:** integrated remediation resumed from `8a4cf6f`
**Current intermediate HEAD:** `37f2988`; further implementation remains in progress
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
| RM-11 | PARTIAL | bounded tool admission and some duplicate media processing eliminated; unbound version-result caching disabled after ABA finding | Complete operation inventory, integrity/fault verification and representative actual read/hash/probe measurements. |
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
`0069` Jellyfin track ordering index · `0070` durable custody intent (in progress) ·
`0071` retained-source hash and directory spool (uncommitted, in progress).

## Historical verification (session 3; not the current working tree)

- `catalog_importer`: 9 PASS, including new late-lower-id seed retention test.
- `melodee-search rebuild_integration` (test-support): 13 PASS with commit bounds.
- `melodee-ingestion inspection` (5) + `environment_faults` (7): PASS.
- `melodee-server` media unit tests (3) incl. artwork replacement invalidation: PASS.
- Workspace fmt/clippy: PASS.
- `make check-nightly` with `RUST_TEST_THREADS=1`: PASS. (One pre-existing podcast lease test is
  timing-sensitive to parallel in-binary tests; it passes isolated/serialized and is unrelated.)

## Remaining mandatory work (not complete)

The partial packages in the current table remain mandatory: RM-00, RM-06,
RM-07, RM-09 through RM-15, and RM-17 through RM-20. In particular, safe
source custody and the guarded no-handoff purge regression remain unresolved;
the 600K identity query samples do not establish complete bounded convergence;
and the representative end-to-end matrix, upgrade rehearsals and final gates
have not been completed on the integrated source.

## Current execution checkpoint (based on `8a4cf6f`)

The current working tree contains uncommitted work. Previous session results above
are historical and have not been rerun against this working tree. All remaining
package-level requirements and the hard definition of done continue to apply;
the following entries do not replace them.

| Requirement | Implementation | Evidence | Status | Next action |
|---|---|---|---|---|
| RM-00: reproducible disposable Rust catalog fixture | `scripts/perf_representative.py`; UUID-owned database via `scripts/test_database.py` | Full-size cardinality, ratings, ACL, artwork and alternate-reference checks; `tests/performance/fixture-verification-2026-11-27.md` | PARTIAL | Validate updated no-follow fixture at scale; improve identity history and ACL provenance; add real-media production workloads and comparable baseline/final measurements with complete environment/resource recording. |
| RM-09: acknowledge only successful reconciliation | `crates/melodee-catalog/src/importer.rs` and `scheduler.rs` | `identity_drain_preserves_seeds_when_candidate_discovery_fails` passed on PostgreSQL | PARTIAL | Complete mutation coverage, transitive closure, concurrency and interruption comparisons with the global oracle; verify production plans. All five scoped algorithms have representative comparisons and focused seed tests, but these do not prove exhaustive equivalence. |
| RM-11: reduce redundant processing | `crates/melodee-media/src/normalize.rs` | 56 unit and 13 real-media tests passed (one ignored); source inventory recorded | PARTIAL | Verify integrity/fault cases, address remaining copy/hash/probe duplication and measure production paths. |
| RM-12: shared resource admission | `crates/melodee-ingestion/src/admission.rs` and ingestion workers | 50 ingestion unit, 53 PostgreSQL integration, and 2 saturation/lease tests passed after the atomic test-helper change; four-permit provisional bound | PARTIAL | Complete process-wide coverage, file/temp/subprocess bounds and measured concurrency matrix. |
| RM-15: reduce administrative aggregate scans and getArtists allocation | `apps/melodee-server/src/admin_stats.rs`, `opensubsonic/browse.rs` | 2 admin statistics and 24 OpenSubsonic browse tests passed after the rating query change | PARTIAL | Capture representative plans and release-build response costs; address remaining routes and ACL/rating queries. |
| RM-17: reduce read scheduling and combine compatible auth lookups | `apps/melodee-server/src/native_stream.rs`, `opensubsonic/auth.rs`, `crates/melodee-persistence/src/auth.rs` | 21 streaming and 15 OpenSubsonic integration tests passed | PARTIAL | Measure throughput, slow-client behavior and rate-limit contention. |

Current execution: finish source custody and retained-source discovery, including
independent directory and file scheduling and the guarded no-handoff purge path.
The existing no-handoff purge compatibility test currently fails; safe source
handoff and deletion remain unfinished. An isolated, fingerprinted identity
benchmark completed its sequential 60K- and 600K-track cases; focused retained
source persistence tests are next in the shared build/database/load slot.
The earlier baseline mixed-load cases have completed, but final comparable
integrated measurements, acceptance and quality gates have not run.

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
- Custody namespace review found a Linux bind-mount alias can bypass lexical and
  ancestor inode checks; validate mount topology or fail closed before wiring.
  Retaining sources also requires a durable v2-content backstop so same-inode
  modifications are not suppressed forever. Neither requirement is implemented.
- Fixed-label operation counters passed focused unit and ten publication
  fault/recovery tests; an interim source-fingerprinted release server is building
  for a tiny production-path smoke. It cannot satisfy final gates while custody
  handles are disconnected and produce dead-code warnings.
- During the interim release-server build, HEAD advanced externally from
  `08de08d` to `37f2988`, an intermediate checkpoint of the shared WIP. The
  provenance recorder correctly refused to label that build source-matched; no
  final acceptance or integrated gate is implied. Preserve the commit and rerun
  the incremental build on a frozen source/HEAD before collecting evidence.
- External intermediate commit `37f2988` contains the shared unintegrated WIP;
  an incremental source-fingerprinted server build subsequently succeeded, but
  it is not final. The first tiny metrics smoke stopped before ingestion with
  SQLx VersionMismatch(67) because its debug migration CLI was stale. No metrics
  were collected; owned resources were cleaned. Matching migration CLI build and
  source-hash verification are in progress before retry.
- After rebuilding a migration CLI with hashes matching the interim server, a
  tiny two-release production-path counter smoke cataloged 2/2 in 6.158 s and
  recorded 49 complete events: 12 staging-tree hashes (751,974 logical bytes),
  8 media hashes (438,532 bytes), 2 validation hashes, 2 publication copies,
  8 publication hashes, and 6 ffprobe/2 ffmpeg/2 fpcalc/6 version processes.
  External subprocess reads and logging overhead are unmeasured. This smoke
  validates telemetry wiring, not representative or final acceptance.
- A real isolated user+mount namespace test passed for both source-subtree bind
  alias and nested mount rejection; an initial busy unmount was fixed and only
  its exact owned temporary fixture was removed. No host mount changed. These
  tests do not prove action-boundary fencing, open-writer safety or recovery.
- 0070 is now in the external intermediate checkpoint; 0071 is reserved for
  forward-only durable bounded custody hashing/quota, not yet implemented.
- Tiny production counter smoke observed duplicate cold tool-version probes
  under concurrent imports; bounded cancellation-aware singleflight is being
  implemented and must be tested, with final media performance still pending.
- At an earlier checkpoint, version-probe singleflight passed nine focused tests;
  its result reuse was later removed due to the ABA finding below. Representative
  before/after operation counts and final-source timing remain unmeasured.
- 0071 metadata-only, quota-bounded custody proof staging and lease-fenced page
  operations are WIP; versioned sorted entry-digest proof, complete traversal,
  recovery and production use remain unverified.
- 0071 spool storage tests passed 2/2, covering idempotent bounded pages,
  concurrent quota contention, rollback, fencing and cleanup; populated
  0070→0071 and fresh migration checks passed. Traversal, receipt binding,
  cleanup scheduling and retained-source reobservation remain unimplemented.
- Independent review found a tool-version A→B→A executable pathname race that
  can poison the version cache despite metadata rechecks. The singleflight is
  not accepted until executable identity binding or a safe non-caching fallback
  and deterministic regression are implemented.
- Scheduler disabled/no-source integration passed with 257 settling-spool and
  257 custody-spool rows both draining, and independent review found no fairness
  defect in that cleanup wiring. The spool still lacks complete traversal/receipt
  binding, and the integration build reports 20 disconnected custody warnings
  which must be resolved before Clippy/final acceptance.
- Unbound executable probes now bypass persistent result reuse to prevent an
  A→B→A version-cache poisoning race; bounded in-flight gating remains. The
  deterministic regression is pending. Safe image-bound caching and actual
  cold-probe reduction remain open; prior singleflight benefit is retracted.
- All 10 version-probe unit tests passed, including deterministic A→B→A
  no-cache-poisoning and cancellation. Unbound results are not reused, so the
  earlier cached-probe optimization is not currently preserved for these paths;
  a single observation can still race replacement. Safe image-bound caching and
  representative operation/timing evidence remain open.
- 0071's ordinal-only spool key cannot safely resume an unstable filesystem
  enumeration; reversible per-entry identities and a byte quota are being added
  before traversal is wired. Earlier 0071 tests predate this change.
- Test-only FFmpeg chromaprint shared-decode feasibility on generated 15-second
  MP3 and FLAC matched fpcalc with muxer algorithm 1 (algorithm 2 did not); one
  sample per codec and no production change. Long tracks, other codec/channel
  settings, exact duration, corrupt media, cancellation/reaping and real read/
  time reduction must be proven before this can address RM-11.
- Revised 0071 passed four spool tests including forced lookup-hash collisions,
  8K path, byte quota, reordered replay and owner-epoch revalidation; populated
  0070→0071 and disabled/no-source cleanup regressions also passed. Full
  traversal, retained-source reobservation, receipt binding, crash recovery and
  production custody remain open. The build still emits 20 prototype warnings.
- Extended two-repeat shared-decode feasibility is not equivalent: 125-second
  MP3 fingerprints differed at the 120-second cutoff and Opus duration differed
  (15,010 vs 15,007 ms). Short samples matched and corrupt FLAC was rejected by
  both. Selected logical source reads were halved, but strace attribution had
  unparsed calls and supervisor/fault semantics remain unproven. No production
  change was made; cutoff/duration investigation continues.
- The unintegrated bounded v3 native-path/content visitor passed its focused
  test for enumeration-order independence, invalid UTF-8 separation, same-size
  content changes and cancellation. It still requires supervised traversal,
  complete observation/receipt binding, replay and production integration.
- A second test-only shared-decode experiment matched all ten generated
  fingerprints (five codec cases, two trials, including 125-second MP3) after
  resampling before the exact cutoff. Opus direct duration remained different;
  an experimental tick-rounding approximation matched only this small corpus.
  fpcalc uses nondefault resampler settings, so semantic equivalence and fault
  handling remain unproven; no production code changed.
- Test-only compatible-resampler shared-decode experiment matched fpcalc
  fingerprints in 20/20 generated MP3/FLAC/AAC/Opus trials at 32–96 kHz and
  around the 120-second cutoff. Direct duration differed in three cases; a
  tick/printf approximation matched these samples but format fallback and
  multistream semantics remain unproven. Corrupt/timeout tests passed; no
  production equivalence or performance completion is claimed.
- Shared-decode sample-count experiment matched ten single-stream generated
  fingerprints, but a two-audio-stream/default-second input mismatched fpcalc.
  The test-only candidate explicitly falls back in that case. Missing/format
  duration fallback, cross-version and lease-loss semantics remain unproven;
  no production change or RM-11 completion is claimed.
- Revised 0071 DB-clock lease fencing/spool suite passed 4/4; shared reaper
  tests passed 3/3, including cross-class saturation and shutdown; an owned
  PostgreSQL 260-file provisional capture/replay/mutation test passed. It still
  has no receipt or production handoff; 19 disconnected warnings remain.
- A candidate Native ACL-aware artist-count query scopes grouped inputs to
  requested artist IDs. An owned 600K SQL plan/equivalence probe is being
  prepared; synthetic provenance does not replace real ACL/revocation tests.
- Provisional v3 capture heartbeat now uses a DB-clock check after locking and
  continuously polls the capture while renewal waits; short-operation and
  lock-contention regressions are written but unrun. 0071 quota accounting was
  revised to statement-level transition-table updates after its previous tests;
  the latest migration and immutable-entry checks need a full focused rerun.
- Owned 600K Native artist-count SQL comparison returned identical ordered
  50-artist rows; three EXPLAIN ANALYZE samples were 599–638 ms before and
  13–17 ms with scoped grouped inputs. Cache/order was uncontrolled and the
  synthetic Releases lacked ACL provenance; this is not HTTP or realistic
  authorization-selectivity evidence. Narrow route change and real ACL tests
  are in progress.
- Current 0071 statement-level quota tests passed 4/4, including raw-entry
  mutation rejection and exact multi-row byte conservation; populated upgrade
  and disabled cleanup passed. Provisional heartbeat, row-lock, runtime-stop,
  shared-reaper and 260-file replay tests passed. One initial heartbeat fixture
  violated an existing state constraint and was corrected; its owned leftover
  was removed. Production handoff remains disconnected with 21 warnings.
- Native ACL-aware artist-count predicate pushdown passed independent source
  review; actual ACL, revocation and zero-count route regressions are written
  and awaiting the assigned Cargo/DB test slot.
- Native ACL-aware artist-count pushdown passed its focused ACL/revocation/
  visible-zero-count regression and complete native_catalog_api target (7/7);
  six plan-harness safety tests passed. Representative HTTP/mixed-load results
  on the final integrated release remain outstanding.
- Independent review found no material defect in the provisional v3 capture,
  database-clock heartbeat or shared reaper. Cancellation during a saturated DB pool passed with unchanged lease expiry
  and renewal history; production receipt binding remains absent. Current disconnected custody code emits 33 build warnings.
- Custody heartbeat cancellation under a saturated one-connection pool passed
  with no lease or renewal-history change. A 260-file provisional capture
  passed two quota-failure cancellations then retry, unchanged replay and
  same-size mutation rejection, and verified no staging receipt was issued.
  Production handoff and recovery remain disconnected.
- Current role-aware 0071 suite passed 6/6, including distinct source/staging
  captures, concurrent quota and a DB-clock lease-expiry-before-commit rollback;
  populated upgrade, 260-file provisional retry and disabled cleanup passed.
  A misplaced test-refactor purpose argument was corrected before rerun. Staging
  observation, three-proof receipt binding and production recovery are absent.
- The unintegrated source-before/staging/source-after observer test passed on
  an owned protected fixture: distinct capture IDs and matching v3 values, with
  sequential same-size changes rejected on each side. The 260-file quota/replay
  test also passed after refactoring. Concurrent boundary races, safe copy,
  atomic receipt binding, retention/reobservation and production wiring remain.
- Opaque store-computed three-proof tokens, atomic retained receipt binding and
  cleanup behavior are under implementation; current observers are provisional
  and do not copy, issue receipts or change production handoff. RM-13 lifecycle
  benchmark/alternative analysis is being extended independently.
- The provisional target-binding source and retained marker are formatted but
  uncompiled/unverified; no atomic binder or production retained-copy switch
  exists. Shared sources are frozen for a production-outbox release-test build
  and tiny smoke. Its planned mirrored batch sizes respect the real 512 cap.
- Search lifecycle batch-matrix release test built with source/binary fingerprints;
  tiny owned 3/4/12 fixture passed eight trials with exact acks and three entity
  visibility checks. Collector reader attribution and full representative run
  remain pending; the tiny smoke is not performance evidence.
- Retained staging must use bounded v3 verification at every later boundary,
  since legacy path hashing can collide on invalid UTF-8 names; binder remains
  disconnected until those checks and cleanup are implemented.
- A preliminary DB-only retained binder compares provisional source-before,
  staging and source-after seals and atomically writes a legacy-format receipt
  plus reobservation obligation; cleanup/store methods honor the retained marker.
  This code is uncompiled/unreviewed and must not trust a caller-supplied legacy
  digest. Protected copy, downstream bounded v3 revalidation, scheduled
  reobservation, crash recovery and production wiring are not implemented.
- Representative 600K search batch matrix is running against a precisely
  fingerprinted earlier test binary; it will record workspace differences and is
  not final integrated evidence.
- Owned 600K search batch matrix completed on the earlier fingerprinted test
  binary: eight trials of 2,048 mixed events with exact acks and visibility;
  batch 16 yielded 181–204 events/s vs batch 512 3,171–6,273/s. Writer init
  was 1.2–1.3 ms/batch; batch 16 committed 128 times and segments grew to 401.
  RSS includes rebuild/previous trials, cache/order/segment growth are uncontrolled,
  five custody/persistence files differed from build snapshot, and the migration
  CLI is separately hashed. This is not final-source or long-lived-writer A/B
  evidence; see `tests/performance/search-batch-matrix-2026-11-26.json`.
- Provisional custody spool now records exact sibling-order keys, parent
  identities, type, legacy path/metadata length and byte accounting for bounded
  legacy traversal. This revision is uncompiled. The legacy streaming verifier
  and opaque seal are unfinished; the current binder's caller-supplied legacy
  string remains unsafe and must stay disconnected. Safe copy, reobservation,
  receipt/retention integration and recovery are open.
- The raw legacy-string persistence binder was removed. An uncompiled ingestion-
  private binder now accepts opaque provisional source/staging proofs and writes
  the legacy receipt plus retention obligation atomically. Independent review,
  the bounded verifier, protected copy, downstream v3 checks, retained-source
  backstop and crash recovery remain outstanding; it is not production-wired.
- Independent review found a P0 in the provisional three-proof binder: opaque
  source-before and source-after seals lack persisted chronology, so both can
  predate staging and a same-inode A→B source mutation can be bound as A. Add
  binder-owned ordered session transitions, require source-after only after
  verified staging, and test replay/order/mutation before production wiring.
- The provisional receipt binder now has a durable ordered proof session and
  an unrun regression for the reviewer counterexample (two early A seals,
  staging A, then same-inode/same-size source B), plus correct ordered binding,
  idempotence and cleanup fences. The separate bounded legacy verifier is drafted
  with parity/mutation/cancellation tests; module integration, staging-root
  generation proof, compilation and independent full review are pending.
- Independent review found a further P0 post-check window: source contents can
  change at the same inode/size after the last source-after read but before
  receipt commit. Ordered proof sessions do not prove immutability. The binder
  remains test-only/disconnected; investigate an enforceable snapshot/handoff
  or durable retained-source reobservation with an exact adversarial test. No
  strict-equivalence or custody completion claim is supported.
- Independent review also found staging can mutate in place after its verifier
  and before binding; private paths alone do not prove exclusive ownership.
  Both source and staging post-check races remain open. A separately leased,
  bounded retained-source reobservation backstop is being designed; no receipt
  or destructive custody integration is enabled.
- Legacy verifier 6/6, spool 6/6, populated 0070→0071 and disabled cleanup
  passed. A real-DB ordered-session adversarial test with 41 sibling files and
  nested trees rejected early-after + same-size mutation without a receipt, then
  verified correctly ordered legacy receipt and retained retry. Post-check
  mutation, enforceable stage immutability, safe copy, downstream v3 checks and
  retained-source backstop remain open; no production handoff is enabled.
- An owned disposable Linux fs-verity feasibility probe is being prepared/run;
  it cannot be assumed available on configured filesystems and must not alter
  producer-owned media or host security settings.
- Owned 4096-byte Btrfs fs-verity feasibility passed: open writable FD or mmap
  rejected enable with ETXTBSY; after successful enable, writes failed EPERM and
  bytes matched. Unsupported pipe returned ENOTTY; fixture was removed. This is
  one local kernel/filesystem, not proof of portable support, name isolation,
  source immutability, crash recovery or performance. A safe released binding,
  FOSS/ADR decision and fallback are under investigation.
- Retained-source lease/backstop persistence and 0071 additions have an explicit
  independent owner; ingestion scheduling remains separate. The production
  receipt/copy protocol is still disabled pending source/staging guarantees.
- Owned tmpfs fs-verity test returned ENOTTY for the file and removed its unique
  fixture. A stage-only exclusive-create safe wrapper and dependency review are
  in progress; unsupported filesystems cannot silently downgrade guarantees.
- Separate-token retained backstop persistence/0071 is under an independent
  owner; a bounded copy visitor and mutation/no-overwrite test are drafted but
  uncompiled. Receipt binding, source preservation and production handoff remain
  disconnected.
- The disconnected Linux stage-only fs-verity wrapper resolved exact
  `composefs-ioctls` 0.10.0 and passed four focused tests: exclusive creation,
  retained writer refusal, replacement/unsupported refusal and sealed-file
  write denial on workspace Btrfs. Cargo-deny, independent review, crash/fault
  tests, platform fallback and production receipt integration remain pending;
  this helper never seals producer-owned source files.
- `cargo deny check` passed for the added fs-verity wrapper dependency
  (advisories, bans, licenses, sources). ADR-0221 remains Proposed; protected
  names, fault cleanup, unsupported-filesystem behavior, production integration
  and representative performance remain unverified.
- fs-verity helper and composefs-ioctls are now Linux test-only/dev-only;
  `cargo tree --edges normal` confirmed no ordinary production dependency.
  Revised focused tests passed 4/4, including symlink/no-overwrite and failure
  byte preservation. Promotion still requires a validated namespace capability,
  accepted ADR/unsafe-boundary audit, fallback, integration and final gates.
- The first opaque-stage-adapter test used a workspace ancestor with an ACL and
  failed safely; retry on an ACL-free owned parent exposed a real EBADF because
  ACL inspection received an O_PATH-like child handle. No adapter test passed
  in that run; a readable no-follow capability fix and rerun are required.
- Backstop review also found a processing heartbeat that could revive a lease
  after waiting past its original expiry; a locked-original-expiry fix and
  deterministic stalled-renewal regression remain to be implemented/tested.
- The stage-parent ACL descriptor EBADF was corrected using a readable
  descriptor-relative no-follow handle. Protected-parent positive/replacement
  and named-user ACL rejection checks passed. A stalled processing-heartbeat
  regression also passed after checking the original locked expiry; the stage
  adapter rerun is pending and disconnected custody still emits 22 warnings.
- Opaque protected-stage adapter test functions passed 7/7 on an ACL-free
  owned tmpfs parent after the descriptor fix. Three actual fs-verity-dependent
  branches (open-writer rejection, successful seal/write denial and post-seal
  parent replacement) could not execute there because tmpfs is unsupported.
  Prior standalone Btrfs positive does not prove the protected-capability path.
  A suitable owned protected verity-capable environment or equivalent safe
  fixture remains required; no production stage-verity path is enabled.
- Retained-token takeover/cleanup and five disconnected copy-filter tests now
  pass. The first copy run found EBADF from fsync on an O_PATH-like directory;
  reopening the pinned directory readably fixed it before the successful rerun.
  Independent adapter review found no new P0/P1, but source/staging atomicity,
  protected-parent plus real fs-verity, crash recovery and production wiring
  remain open.
- A later heartbeat change also requires the proposed new expiry to be live at
  the same database-clock instant as the original expiry; it has not yet been
  rerun. Retained-source orchestration and production handoff remain disconnected.
- Separate retained-source persistence suite passed 4/4 (DB-clock/token lease,
  retry/quota, supersession/replay, ambiguous exclusion, wrong-root and blocked
  expiry/cancellation). Scheduler/visitor integration and representative due
  index plans remain unverified; superseded history is outside active quota.
- Isolated Btrfs stage namespace harness had inherited-FD and propagation issues
  fixed, then passed independent static review and seven safety tests. The
  owned test binary is being rebuilt; no actual isolated stage result yet.
- Independent harness review cleared inherited-FD and propagation checks. Two
  owned private-namespace stage attempts failed before tests; retry identified
  host `/proc` bind as the failing step (mount exit 32). Parent user/mount
  namespace identities were unchanged and both owned directories removed. A
  fresh proc in a private PID namespace is under investigation and needs safety
  review; protected Btrfs + actual fs-verity remains unverified.
- Retained-source persistence suite passed 4/4 again after adding the durable
  `Ambiguous` retry reason/constraint. The private namespace harness now uses a
  private PID namespace and fresh read-only proc in draft, with eight safety
  tests; independent review is required before execution.
- Disconnected retained traversal/turn tests passed 2/2 on PostgreSQL: changed
  bytes create a new discovered occurrence without deleting source; duplicate
  roots and missing occurrences defer with distinct reasons. Updated stalled
  renewal regression passed. Post-check root/content races, crash-prefix
  fairness and actual scheduler integration remain open.
- Independently reviewed private-user/mount/PID namespace test succeeded on an
  owned Btrfs fixture: all seven stage-adapter test functions ran with zero
  unsupported branches, including real open-writer denial, sealing/write denial
  and parent replacement. The binary matched its recorded build snapshot; six workspace files differ
  now, including custody.rs. Parent namespace identities were unchanged and both
  owned roots were removed. This is test-only, not final-source, production
  custody, fallback or performance acceptance.
- Deterministic post-final-observation/pre-commit same-size source mutation
  regression passed: the source remains intact, the obligation remains active,
  and later retained turns discover the changed bytes as a new occurrence. This
  proves compensation for one interleaving only; the retained turn is disconnected,
  staging immutability, fixed-crash fairness and production custody remain open.
- Exact post-root-check/pre-commit replacement test passed with both owned trees
  intact and replacement rediscovered by manually forced retained turns. Review
  confirms this is not production compensation: scheduler is disconnected and
  tests force due times. Independently budgeted natural-due/restart/live-work
  scheduling and tests remain required before enabling retained receipts.
- A separately supervised one-item retained scheduler is now connected. Two
  PostgreSQL integration tests passed natural-due restart with a live processing
  lease, and deleted-original recovery coexisting with ordinary discovery; no
  post-seed SQL forced the due time. Binder/production retained-copy activation,
  fixed-crash traversal fairness, full integration and final gates remain open.
- Connected retained scheduler currently rehashes unchanged retained sources
  twice every five minutes; one-at-a-time admission alone does not bound total
  catalog I/O. Durable pacing/fairness and representative mixed-load measurement
  are required before accepting defaults or enabling retained receipts.
- Retained scheduler currently has untested configurable durable recheck spacing
  and a process-wide 8 MiB/s provisional application-read pacer. Verify bounded
  cancellation, DB/heartbeat independence, fairness, docs, mixed-load behavior
  and a measured default before acceptance. Safe copy, stage sealing and binder
  remain disconnected.
- Retained-read pacer focused tests passed for a shared 1 MiB/s timeline,
  cancellation, a real paced 256 KiB read, disabled ingestion with due work,
  and shutdown while a 4 MiB source was open (slots restored, source and
  obligation intact). The 8 MiB/s default remains provisional and lacks
  representative mixed-load/total-I/O evidence.
- Pacer review found one large retained source can monopolize its sole worker
  and shared heavy permit for hours despite a byte rate cap. Durable per-turn
  byte/time budgets, safe resumable progress and a slow-first/fair-second
  regression are required; rate tests alone do not establish fairness.
- Independent owned retained mixed-load harness is being prepared; its rate
  defaults will remain provisional until measured.
- A test-only versioned 64 KiB chunk-Merkle proof/checkpoint is drafted for
  resumable retained files. No media payload is stored in the checkpoint; the
  primitive is uncompiled and unintegrated. Directory cursor safety, DB fencing,
  source revalidation, legacy receipt compatibility and crash replay remain open.
- Independent v4 review found stale checkpoint-prefix sealing, absent format/
  chunk-size identifiers and unbounded pre-validation allocation. A test-only
  fix now treats resumed state as change-detection only, validates explicit
  format/algorithm/chunk size, and bounds raw decode/peaks; it is uncompiled.
  Directory progress, fresh proof, quotas and production wiring remain open.
- Test-only v4 change-detection primitive passed 3/3 for resumed-prefix taint,
  fresh same-size mutation detection, malformed/oversized/wrong-version input,
  and distinct native paths. It cannot seal a resumed mixed-time proof. The
  new persistent checkpoint schema, traversal and production scheduling remain
  untested/incomplete.
- Retained-source PostgreSQL suite passed 6/6 with provisional checkpoint CAS,
  final-chunk length, carry parity, stale-lease takeover, quota, corruption and
  expired-GC cases. A peak stack alone cannot compare revisited chunks against
  a verified baseline; exact-path/index baseline storage, directory frontier,
  complete traversal and production proof remain unresolved.
- The retained-rate mixed-load Python harness has nine safety tests and a
  reviewed seed design; independent security review and tiny smoke are pending.
  It measures seeded reobservation, not ingestion/copy/receipt.
- Test-only v4 independent vectors passed 4/4. The blocked-checkpoint expiry/
  cancellation test initially timed out because its lock-wait detector looked
  only for relation locks; after switching to `pg_stat_activity` lock waits it
  passed without weakening the assertion. Its owned DB was confirmed cleaned.
  Verified baseline mapping, directory frontier and production use remain open.
- Provisional retained-baseline PostgreSQL test passed 1/1 for stage coverage,
  zero-length files, invalid length/digest, replay conflict/freeze idempotence,
  immutable frozen children and abandoned-GC accounting. Same-stream proof,
  private publication, retained cursor/change signal and end-to-end behavior
  remain incomplete. The retained-load ownership-manifest fix has 14 Python
  safety tests and awaits independent re-review before any DB/load execution.
- Provisional same-stream staging tests passed: 129 chunk hashes over an 8 MiB
  staged file plus an empty file, independent final-chunk preimage and no receipt;
  same-size mid-read mutation rejected before file verification. Immutable
  staging, private atomic publication, retained cursor and full behavior remain
  open. The load harness ownership-manifest revision has 16 Python safety tests
  and is awaiting independent review; it has not been executed.
- Independent review found no P0/P1 in the retained-load harness's durable
  ownership-marker/manifest repair (17 Python safety tests). Its real tiny
  scheduler smoke still awaits matching frozen 0071/server/CLI builds; the
  documented privileged replacement race requires an exclusive test server.
- Provisional baseline/cursor PostgreSQL suite passed 2/2 after set-based
  append: at most 16 chunks per page, pass wrap, stale/replayed acknowledgements,
  lease takeover, sticky mismatch, zero-byte files, freeze and GC. Protected
  same-stream immutability binding, additions/removals and integration remain.
- P0 newly found in connected retained scheduler: it can create a normal
  Discovered successor whose existing acquisition path renames/deletes the
  retained producer source. A fail-closed gate with intact obligation/source and
  regression is being added immediately; no successor may be enabled until a
  durable copy-only intake binding is integrated and proven. Prior natural-due
  tests do not establish this safety property.
- Further retained-origin P0: even after gating explicit successors, deletion of
  an old work item followed by ordinary discovery can create a normal item and
  its acquisition/purge may drain the retained path. Atomic exact source-ID/
  path active-obligation fencing across begin-handoff, purge and concurrency is
  being implemented/reviewed. Retained-origin activation remains unsafe until
  every source-deleting path is fenced and regressions pass.
- Focused persistence gate suites passed: retained_baseline 2/2 including
  set-based quota rollback, and retained_sources 7/7 including changed/inactive
  fail-closed hold, sticky signal, awaiting_intake and expiry/cancellation. The
  separate ordinary-discovery/acquisition/purge bypass is still open.
- Review retracted a suspected same-item cleanup/binder race because committed
  handoff states exclude the interleaving. Confirmed open: no production admin
  purge guard yet, plus occurrence-advisory/item-row lock-order inversion.
  Durable pre-filesystem guard, common lock order, deleted-original→fresh-item
  purge and crash recovery tests are required. No-handoff purge is being
  temporarily failed closed; legitimate replacement intake remains nonterminal.
- Purge occurrence-guard use and occurrence-first binder/handoff locking are
  now drafted, with temporary no-handoff fail-closed behavior and conservative
  same-path rejection. They are not tested/integrated. Concurrent claim/lease
  fencing, fresh transaction snapshots, same-path rows, crash recovery and
  copy-only replacement intake remain open; earlier P0 guard absence is not yet
  accepted as resolved.
- Purge/claim review: nonblocking occurrence lock followed by fresh guard
  revalidation avoids the previously identified lock inversion. Still WIP:
  exclude inactive historical same-path rows appropriately; preflight storage
  before guard reservation and implement safe token-fenced abandon/recovery
  because a missing storage root currently strands the guard. Integrated
  claim/purge and crash-window regressions are required.
- Production purge guard/claim interlock and audited operator recovery CLI are
  drafted and frozen for integrated race tests. Predictable storage checks occur
  before reservation; uncertain guard recovery requires an explicit audited
  operator action. No-handoff inbound purge currently fails closed and the
  original inbound-delete compatibility test remains intentionally unmet until
  safe occurrence-bound deletion exists. Results are pending.
- Latest integrated guard run: retained_occurrence_fence 3/3,
  retained_purge_interlock 2/2 and retained-recheck ingestion 2/2 passed. Admin
  purge passed 8/9; unchanged `purge_removes_inbound_source_when_no_staging_handoff_exists` failed (Null vs `Never Acquired`) under temporary fail-closed
  behavior. This compatibility requirement remains open; do not suppress the
  assertion. The build emitted 56 disconnected-prototype warnings.
- Purge-guard recovery CLI parser/attestation regression passed 1/1. The
  application build still emits 56 warnings from disconnected custody/retained
  prototypes; no warning suppression or final gate claim is appropriate.
- Provisional baseline consumer-drop and DB quota-failure tests each passed,
  confirming producer/reaper release and no publication in those cases. This
  does not establish staging immutability, safe intake or production binding.
- Purge recovery predicate now excludes irrelevant inactive historic-path leases;
  its focused regression passed, as did the unsafe staging-path preflight test
  (no guard created, work bytes preserved). Independent review found no further
  P0/P1 in that CLI/store/runbook boundary. The no-handoff purge compatibility
  failure and general occurrence-bound acquisition/deletion remain open; 55
  disconnected prototype warnings still prevent the final lint gate.
- Real admin purge containment regression passed for a retained obligation after
  original-row deletion and a fresh conflicting item/handoff: source, stage,
  work bytes and active obligation remained. This does not implement safe
  no-handoff deletion or close its existing compatibility failure.
- Occurrence-lock vs lease-renewal race passed: renewal returned without
  deadlock and left the original expiry unchanged. Safe no-handoff source
  deletion remains open because directory rename cannot revoke an already-open
  producer writer; the unchanged admin compatibility test still fails.
- Disconnected custody-intent DB suite passed 2/2 after adding database-clock
  lease checks at entry and before commit, including expiry inside INSERT with
  full rollback and stale-caller-time rejection. It remains provisional and does
  not implement source-safe handoff/deletion or recovery.
- Narrow retained persistence suites passed after adding baseline-ID access and
  sticky pass completion: retained_baseline 3/3 including empty generation
  mismatch, and retained_sources 7/7 including no-baseline ID. Ingestion page
  branch tests are next; directory completeness, protected baseline publication
  and fairness of older no-baseline obligations remain open.
- Bounded page-reader unit tests passed 2/2 for selected-chunk change, no false
  whole-tree proof from unvisited chunks, and symlink replacement rejection.
  Fair-turn/restart scheduler integration, safe baseline publisher and full
  directory addition/removal detection remain pending.
- A real-DB bounded page-turn fairness test passed: a 17-chunk first
  occurrence yielded after 16 and a second due source advanced next, with both
  sources intact. Its authoritative baseline was synthetic SQL, not a safe
  production publication. Restart/crash, additions/removals and no-baseline
  fairness remain open.
- A new isolated production-import/drain RM-09 scoped/global candidate oracle
  test passed 1/1: all five candidate outputs matched without truncation;
  same-release, strong Artist and strong Release sets were nonempty, and the
  fixture covered manual/negative decisions, conflicting same-name Artists,
  three-Artist redirect closure, interrupted discovery and late lower-ID commit.
  Repeated-credit/name-proposal sets were not asserted nonempty, and this does
  not prove full global-drainer outcome equivalence or replace 600K evidence.
- Missing/unreadable retained baseline pages can still starve later files within
  one occurrence. A separate fenced sticky-verification-required advance/yield
  with explicit reason is being implemented; it must not claim reread success.
- Bounded retained-page worker-reclaim test passed: after natural due delay a
  new worker resumed the first source at chunk 16 and another due source
  progressed. This is reclaim evidence, not a process-crash or directory
  completeness proof. Unreadable-page signaling remains under implementation.
- Oracle review found P1 coverage gaps: the current fixture seeds all connected
  identities and lacks an unrelated mergeable group, so it cannot rule out a
  global fallback; its three-way identity is not a true single-seed multi-hop
  graph; strong-release final merge/redirect is not asserted. Extend the oracle
  with off-closure exclusion, A↔B↔C seeded only at A, final Release outcomes,
  and nonempty repeated-credit/name-proposal sets before treating RM-09 complete.
- Unreadable/missing baseline page signal/advance/yield persistence and
  ingestion tests are drafted and frozen, with no claim of successful reread;
  focused runs are starting. The expanded RM-09 oracle additionally includes
  off-closure candidates, nonempty repeated/name cases, one-seed multi-hop and
  a separate real global-startup outcome fixture; it is unrun and queued next.
- Unreadable/missing page tests passed: persistence 4/4; ingestion missing
  first file with natural yield and later-file progress 1/1; page-reader units
  3/3; two-occurrence 17-chunk fairness/reclaim 1/1. Baselines were synthetic;
  safe publisher, directory additions/removals and no-baseline fairness remain
  unproven. Expanded RM-09 oracle is next in the exclusive slot.
- Expanded RM-09 oracle passed 2/2 with all five candidate sets nonempty,
  off-closure exclusion, manual/negative and same-name cases, true one-seed
  cross-predicate multi-hop closure, strong-release outcome, interrupted drain
  and late seed; an independent production global-startup outcome partition
  matched. Reviewer found the startup fixture did not exercise the separate
  strong-Release backstop, so an unseeded historical Release pair is being added.
- Worker-reclaim page fairness passed, but known-file pages alone cannot detect
  additions/removals or ensure crash fairness. Verified directory baseline,
  bounded cursor/quota and native enumeration are being implemented separately;
  no absence/deletion may be inferred from partial or changed scans.
- Final expanded isolated RM-09 correctness oracle passed 2/2 (debug, owned
  DB, dirty tree on 37f2988): includes unseeded startup strong-Release pair,
  exactly one active target, redirect and reconciliation audit, plus prior
  five nonempty candidate cases, scope exclusion, closure and concurrency.
  This is not representative performance or a clean-commit claim; independent
  re-review and final integrated tests remain.
- Linux stable-directory getdents page test passed on 132 native names with
  cookie resume and symlink classification. Cookies remain hints, not evidence
  of mutation-safe/cross-platform traversal or absence. DirectoryVerified
  verifier test was not started and is next in the exclusive slot.
- DirectoryVerified unit test passed for matching v3 directory evidence and
  forged-digest rejection. End-to-end capture, mutation safety and non-Linux
  traversal remain unverified. Directory-baseline PostgreSQL tests and a durable
  file/directory fairness turn bit are still being written.
- Directory-baseline PG suites passed (directory 1/1, baseline 4/4); the
  separate unreadable-directory signal/yield test then passed without inventing
  generation or asserting absence. Ingestion directory tests are running.
- Identity multi-cardinality harness cleared independent safety review after
  fixes for parser/oracle consistency, post-setup relation counts and helper
  fingerprints. Seven Python tests pass; release build and tiny smoke are queued.
- Ingestion directory reader passed 2/2; synthetic real-DB file/directory
  fairness and missing-file tests passed. Same-stream baseline integration
  initially failed due to a misplaced empty-directory fixture; after correcting
  the fixture it passed for root, nonempty and empty directory records. These
  are provisional, not protected publication, mutation-safe complete absence,
  non-Linux or crash-fairness acceptance.
- Directory/file turn bit and bounded native directory-page design are WIP.
  The current file-pass completion may delay every subsequent directory turn by
  the full recheck interval; independent per-class due/complete pacing, empty/
  mutable directory correctness, crash fairness and non-Linux handling remain.
- Identity two-cardinality release build and matching migration CLI/tiny smoke
  are starting from a frozen intermediate tree. Final integrated measurements
  still require the later final source/commit.
- Fingerprinted release identity test and matching migration CLI built; tiny
  50/50/150 owned smoke passed all five nonempty scoped/global comparisons and
  production drain. Full 60K/600K plan/work comparison remains pending; an
  isolated source snapshot is being prepared so implementation can continue.
- The isolated identity snapshot verified 569 source fingerprints and both
  binary hashes. Its 3,500-artist/5,000-release/60,000-track case completed:
  five nonempty scoped/global candidate comparisons agreed, ten plans were
  collected, and eight seeds produced three Artist merges and one Release merge.
  The 35,000-artist/50,000-release/600,000-track case also completed, with
  five nonempty scoped/global comparisons equal, ten plans captured, and eight
  initial seeds producing three Artist merges and one Release merge; 68 durable
  seeds remained at the end of the bounded run, without classification as
  captured or newly enqueued. This is an intermediate source
  snapshot; one drain leaving seeds does not demonstrate full convergence.
  Its CLI build linkage was independently verified rather than automatically
  recorded by that run's collector. The shared harness now checks this linkage
  for future runs, with focused Python tests passed. Sanitized results and plans
  are in `tests/performance/identity-scaling-2026-11-27.md` and associated JSON
  files. PostgreSQL settings, storage and augmentation time were not collected.
- Independent file and directory due scheduling and a typed directory advance
  API have been drafted together. Focused persistence suites passed
  (retained-baseline 4/4 and retained-directory 2/2), including independent
  due selection and directory invalidation. Stable-source ingestion reruns
  passed: two-occurrence fair turn 1/1, missing-file progression 1/1, and Linux
  directory reader 2/2. Earlier tests overlapping source edits are disregarded.
  These changes do not establish a safe protected baseline publisher or
  authorize deletion.
- Independent review found a non-Linux/Android starvation case: an unsupported
  directory turn can be repeatedly deferred without allowing due file pages to
  run. A fenced unsupported-directory yield and independent file progression
  passed the persistence directory suite 2/2. A scheduler integration test
  forcing the unsupported branch also passed 1/1, including no directory
  acknowledgement and later file progression; native macOS runtime is untested.
  Protected per-file verity evidence and a separate freeze are provisional;
  their focused database suites passed (protected baseline 1/1, baseline 4/4,
  directory 2/2), including synthetic protected-coverage and receipt guards.
  Receipt publication is not connected, and metadata fixtures do not prove
  successful fs-verity sealing or end-to-end publication.
- The identity benchmark now records each subsequent bounded drain turn, the
  total queue and remaining original seed IDs, and stops on quiescence, an
  explicit turn/deadline bound or an unavailable drain. Nine Python tests pass;
  source-fingerprinted release rebuild, smoke and representative measurement
  remain outstanding. The existing 68-seed observation remains uncategorized.
- A new intermediate identity closure-run snapshot was captured and checked
  against 573 source inputs; its isolated release CLI and test builds are
  being prepared in the exclusive slot. The release CLI build succeeded; its
  SHA-256 is `29825cb1c31ea19325ff34dfe1fd1c495f68ab864fe6df76b899fa1770405093`.
  The identity test build and workload have not run. The snapshot includes
  unverified custody work and is not the final integration. Its older fixture
  script must be replaced with the reviewed no-follow version and re-attested
  before running. Review also confirmed a P1 early-reap/session-ID reuse race
  in `run_perf_identity_case.py` and shared `perf_media_io_trace.py` cleanup;
  a central unreaped-leader fix and updates to six callers have passed focused
  Python suites, including exited-leader/live-descendant cases. Independent
  static security re-review cleared the fix; the affected harnesses have not
  yet been rerun with the new cleanup. The private identity snapshot has been
  re-attested against all 573 build inputs and 11 reviewed workload scripts;
  the nested runner now selects only the intended exact test. The release test
  and CLI builds succeeded with matching source proofs. The initial tiny smoke
  failed closed with a `ValueError` after fixture creation; its owned database
  was cleaned. The exact libtest runner had prefixed the fixture-cardinality
  marker. The collector now accepts only that fixed prefix, with a parser
  regression passing. The tiny smoke then passed with automatic CLI proof
  validation, followed by a 35K/50K/600K run: all five nonempty scoped/global
  vectors matched; eight captured seeds were acknowledged; 68 newly generated
  seeds were consumed in one additional 32.029 ms bounded turn, reaching zero.
  This is intermediate-snapshot evidence, not final integrated or concurrent
  import acceptance. Sanitized capture and proofs are recorded in
  `tests/performance/identity-closure-600k-2026-11-27.md` and linked JSON files.
- Independent review confirmed the bounded baseline scheduler is currently
  exercised only by fixtures: production retained-source creation binds a NULL
  baseline ID, while the provisional 0071 immutable trigger prohibits changing
  it. Implement and test a fenced atomic protected-baseline, receipt and retained
  obligation publication, including interruption and restart; do not classify
  the bounded production path as active until that contract exists. The sticky
  changed/unreadable signals also have no production consumer; add a durable,
  safe intake or actionable operator disposition without deleting the source.
- The administrative statistics single-scan candidate passed static review and
  the PostgreSQL API suite 3/3, including its independent SQL oracle. A
  representative A/B measurement remains pending. The candidate harness now
  checks the entire parsed response, excluding only a validated time-varying
  `generatedAt`; it compares the order-preserving normalized serialization,
  not original byte representation. Order-preserving timestamp removal and its
  Rust-level positional regressions passed independent static re-review and
  both isolated release binaries. The baseline and candidate executables were
  built with distinct hashes after an initially reused candidate was discarded.
  The matching migration CLI also built. The first small owned smoke failed
  closed with a `ValueError` and cleaned its database; its exact cause was not
  established. A re-attested 35/50/150 smoke passed four ABBA runs with matching
  normalized response strings. The representative 35K/50K/600K fixture seeded
  in 189.1 seconds, but the A/B child returned `RuntimeError` before writing a
  result and the owned database was cleaned. The failed run is recorded in
  `tests/performance/admin-stats-ab-representative-failed-2026-11-27.json`.
  No route performance result is claimed; an allowlisted diagnostic has passed
  Python tests and review. The diagnostic retry seeded the 600K fixture in
  191.6 seconds but failed because normalized responses changed within one
  process; no timing result was recorded and the owned database was cleaned.
  The second failure is retained as
  `tests/performance/admin-stats-ab-representative-diagnostic-failed-2026-11-27.json`.
  Identify the changing field without exposing values, then determine whether
  a narrowly justified stable comparison is possible before rerunning. A third
  diagnostic identified `database.totalBytes` and `database.tables`, physical
  PostgreSQL sizes that changed within one process. No samples are claimed;
  an opt-in owned-database vacuum and bounded autovacuum wait are being added
  without excluding response fields or changing host settings. The reviewed
  quiescence option passed a small 35/50/150 smoke with three stable size
  observations and all four strict-response A/B runs; representative rerun
  failed because `database.poolInUse` changed within one process, despite
  stable physical sizes. No timing result was retained. The route's count is
  based on PostgreSQL sessions; investigate prewarming and stabilizing the
  benchmark pool without changing response semantics or excluding fields.
  A later small prewarm/ninth-session smoke reached all within-process checks
  but failed strict cross-process response equality; no result was recorded.
  The failure is preserved in
  `tests/performance/admin-stats-ab-prewarm-smoke-failed-2026-11-27.json`.
  A fixed-label cross-process diagnostic and reviewed correction are required
  before another representative run. Diagnostic small smoke identified only
  `database.totalBytes` changing across processes, a physical-size observation;
  within-process equality and ninth-session guard passed. A narrowly explicit
  cross-process comparison preserving that field's position/type while masking
  its volatile value passed focused Python tests and independent static review.
  An owned small smoke passed the ninth-session guard, four A/B runs, and strict
  ordered comparison with only this explicitly validated cross-process value
  masked. A 35K/50K/600K quiesced in-process ABBA comparison then completed:
  16 warm samples per variant had baseline p50/p95 615.140/640.197 ms and
  candidate 528.069/567.805 ms. The source is an intermediate dirty snapshot,
  p99 is the observed maximum, PostgreSQL/cache and shared-host limitations
  apply, and `database.totalBytes` alone is masked across processes. See
  `tests/performance/admin-stats-ab-representative-2026-11-27.json`; final-source,
  concurrent workload and TCP measurements remain pending.
  The bounded operator listing
  query passed its persistence suite (retained_sources 8/8), parser test 1/1
  and real-CLI PostgreSQL test 1/1, including reversible path encodings, keyset
  pages and no mutation. Listing alone is not a safe source-recovery protocol.
- The test-only protected-file copy helper's stage-verity suite passed 9/9,
  including cancellation and chunk observation. That fixture filesystem did
  not support fs-verity, so successful sealing/reopen and positive write-denial
  branches were not exercised. The helper is not connected to production;
  supported-filesystem tests and a resumable bounded copy remain outstanding.
- A separate Btrfs-backed source snapshot has now been verified against 576
  inputs for the supported-filesystem sealed-tree test. Its debug libtest build
  succeeded, and static review cleared its build proof and exact selector; the
  owned private-namespace run reached its sealed-tree success marker but exited
  101 afterward. This is a failure, not a pass; diagnostics are in progress.
  The harness reported unchanged parent namespace identities. Its sanitized
  result and build proof are `tests/performance/stage-verity-sealed-tree-2026-11-27.json`
  and `tests/performance/stage-verity-sealed-tree-build-2026-11-27.json`.
- A separately fingerprinted diagnostic rerun also failed: setting a named-user
  ACL returned EINVAL because the private namespace mapped only UID 0 while the
  test selected unmapped UID 1. Prior sealed-copy and promotion assertions ran,
  but the overall test did not pass. Add a valid mapped-UID ACL case retaining
  the nontrivial-ACL rejection assertion, and separately preserve distinct-user
  coverage; a corrected 577-input isolated snapshot has been verified and a
  fresh build/rerun failed earlier with ENOENT when its cross-process lock probe
  could not execute inside the private chroot. The lock assertion remains
  required. The draft re-exec override was found to accept an arbitrary inherited
  executable; it now accepts only the exact verified mount when set, with a
  native fallback when absent. The regression and fix passed static re-review;
  a new 577-input isolated run still failed with ENOENT when spawning the lock
  probe inside the chroot. Missing `/dev/null` during `Command::output()` is a
  hypothesis under investigation, not yet a verified cause. The test now
  inherits the already-open stdin. After fixing a moved-value build error and
  bounding/reviewing build-output capture, a later fingerprinted intermediate
  private-Btrfs run passed the exact sealed-tree test 1/1 with the positive
  fs-verity marker, no unsupported marker or panic, and unchanged parent
  namespace identities. It used the one-UID ACL case and excludes newer source
  changes; this does not establish production or final integrated safety.
  Evidence: `tests/performance/stage-verity-sealed-tree-inherit-2026-11-27.json`
  and `tests/performance/stage-verity-sealed-tree-inherit-build-2026-11-27.json`.
  One earlier failure and its build proof are retained as
  `tests/performance/stage-verity-sealed-tree-corrected-2026-11-27.json`
  and `tests/performance/stage-verity-sealed-tree-corrected-build-2026-11-27.json`.
  No host UID/security changes were made.
- The test-only protected namespace/attempt test passed 1/1 after a missing
  cfg-gated import was fixed. It checked exclusive creation, marker replacement
  rejection and ambiguous namespace preservation. The fixture lacks fs-verity
  support, so successful sealed-tree copying was not exercised. Production
  activation, promotion, receipt binding and resumable turn bounds remain open.
- A separate test-support real-DB attempt wrapper passed 1/1 for atomic claim
  epoch, reservation, marker creation/fsync, acknowledgement, source retention
  and stale same-worker reclaim rejection. It does not publish a receipt, promote
  a staged tree or establish a production binder. Independent static review
  found no new P0/P1 in this disconnected attempt/marker/copy scope; it does not
  prove the missing source-to-stage protocol.
- Corrected the fixture's Release and Track rating distributions: the prior
  filter and rating expressions combined to produce only rating 1 for both.
  On a new invocation-owned 50-Artist/100-Release/1,000-Track database, the
  actual seeded Artist, Release and Track ratings each covered 1 through 5;
  the Python harness suite passed 13 tests with this database-only test skipped
  outside an owned fixture. Earlier captures used the old rating distribution
  and must not be described as representative rating distributions. A final
  600K fixture still needs to verify counts and distributions at scale.
- The representative fixture now creates alternate media-asset references for
  a subset of Tracks in addition to their playable assets. A new owned
  50/100/1,000 smoke verified both reference types and all rating values; the
  large fixture, ACLs and final comparable workloads remain
  to be validated. Existing captures predate this fixture change.
- The same fixture now installs one deterministic PNG in its owned asset root
  and creates content-addressed Artist and Release artwork references. A small
  owned-database smoke verified both kinds of reference, stored length and
  SHA-256; the harness unit suite passed 13 tests with three owned-database
  checks skipped outside the fixture. Scale and real artwork-route workload
  validation remain pending; old captures do not include these references.
- The fixture now creates two membership groups, four bound library identities
  and synthetic publication-provenance rows for a subset of Releases. The owned
  50/100/1,000 smoke verified that two non-admin users each see allowed and
  denied Releases through `native_user_can_access_release`; all four fixture
  integration checks passed. These are placeholder receipts and manifests, not
  actual ingestion evidence. The Python suite passed 13 tests and skipped four
  owned-database checks outside the fixture. Representative-scale ACL behavior,
  realistic provenance and before/after workloads remain unverified.
- The updated fixture also passed these four shape checks at 35,000 Artists,
  50,000 Releases and 600,000 Tracks, with 30,000 alternate links. Its one
  seeding run took 183.2 seconds; this is setup, not an application benchmark.
  See `tests/performance/fixture-verification-2026-11-27.md` for exact scope,
  hashes and limitations. Matched production-path workloads remain open.
- Independent review found that the original artwork installer could follow a
  shard symlink after migration. It now uses descriptor-relative no-follow and
  exclusive creation; independent static re-review and symlinked root, shard,
  object and existing-object regressions passed. A post-fix 50/100/1,000
  owned-database smoke passed all four ratings, playable/alternate, artwork and
  ACL checks. Non-POSIX fixture execution fails closed for this
  operation. The completed 600K run
  used the older installer with its known local CLI and owned root; future
  workloads must use the fixed helper.
- A proposed source-probe reuse was rejected after review: inspection hashes
  and probes a mutable path in separate reads, so an A→B→A change can associate
  A's hash with B's observation; executable identity can also change. A pinned
  descriptor alone does not prevent same-inode writes. Probe reuse requires a
  verified immutable byte source or a demonstrably equivalent bound stream,
  with fallback for formats requiring seek and independent output verification.
  This is an open RM-11 implementation requirement, not a completed reduction.
- The proposed admin-stats A/B harness has not run. Independent review found
  that it must select the exact audited test, faithfully attest dirty/shared
  provenance, and close a PID-reuse race in child cleanup. Those collector
  fixes passed static re-review. The separate private-worktree restore helper
  then failed review for symlink and hardlink races; its atomic replacement and
  pinned-ancestor fixes passed subsequent static re-review. The helper and A/B
  workload have not yet executed; correctness and measured performance remain
  open.
- Protected-stage attempt design exposed a same-owner lease-reclaim ABA:
  worker identity plus a caller-supplied acquisition timestamp is not a
  unique claim. A DB-issued epoch per actual claim is being designed across
  the distinct work-item claim sites; it must remain stable on heartbeat and
  ordinary transitions. Audit found seven actual claim/reclaim SQL sites assign
  fresh epochs and ordinary heartbeat/transition statements do not. A focused
  PostgreSQL regression passed all seven claim APIs, including retry inspection,
  normalization, validation, publication, cataloging and both policy paths;
  normal and policy heartbeats preserved epochs. Integrated cross-class testing
  and measured hot-path overhead remain unfinished.
- The epoch is now drafted as an opaque token returned atomically by the claim,
  with protected-attempt reservation, acknowledgement and heartbeat APIs.
  Its focused persistence suite passed 4/4, including same-owner/reused-time
  ABA, cross-stage claim rotation, heartbeat cancellation/expiry, bounded
  ambiguous reservations and an active-lease populated upgrade from ≤0070.
  The same suite passed again after handoff-intent mutation checks were added
  to acknowledgement and adoption. Namespace proof and production activation
  are still missing, and the production-path integration has not passed.
- Existing staging consumers expect `releases/<item>/source`, while a protected
  attempt is reserved under a unique `protected-<uuid>/source`. Durable
  promotion intent/acknowledgement metadata and replay passed a focused 4/4
  persistence suite, including same-worker reclaim and identity mismatch.
  A no-replace filesystem promotion unit passed 1/1 for interruption before
  and after rename/parent fsync and replacement rejection, on a fixture without
  fs-verity. Sealed promotion, integrated crash recovery and the private binder
  remain unverified; attempt metadata alone cannot prove promotion.
  Successfully published attempts also require a durable bounded cleanup-pending
  record until their marker and namespace are identity-verified and removed;
  receipt commit alone does not justify releasing their quota. Lease expiry and
  inode checks alone cannot revoke a stale worker's open filesystem descriptors;
  cleanup additionally needs proof that writers terminated or an explicit
  maintenance fence.
- A provisional synthetic PostgreSQL publication transaction passed 1/1:
  mismatched ordered proof rolled back, matching inputs atomically committed
  handoff, baseline, obligation, session and cleanup-pending attempt, and
  same-token replay succeeded. This is not real filesystem or fs-verity proof,
  a private production binder, or filesystem recovery. A subsequent synthetic
  new-claim lookup regression passed 1/1: stale epoch rejected, fresh epoch read
  the exact receipt/baseline/namespace identities, and mismatched receipt refused.
  This read-only lookup does not grant filesystem authority.
- Synthetic cleanup/publication and protected-attempt PostgreSQL suites passed
  1/1 and 4/4 respectively, covering cleanup lease takeover, cancellation, busy
  retry with quota retained, replay, tombstone and metadata accounting. These
  include a subsequent passing cleanup-heartbeat regression for blocked expiry,
  same-owner token takeover and stale renewal rejection. They do not prove
  kernel-lock behavior or safe filesystem cleanup; production cleanup remains
  disconnected.
- Recovery after attempt cleanup needs a separate durable final-stage lifecycle
  lock: removing the attempt lock removes its old cross-process fence. A cleaned
  receipt lookup and verity pages alone are historical evidence and must not
  authorize reuse before a fresh epoch, namespace and content revalidation.
  Synthetic lifecycle persistence suites passed 1/1, 1/1, 4/4 and 2/2,
  covering stale epoch, parent/lock mismatch, tombstones and verity paging;
  a later 1/1 test also covered cleaned receipt recovery and bounded root
  directory paging. Another 1/1 regression covered bounded exact membership,
  extra/type mismatch, wrong parent and a real truncated-hash collision.
  Filesystem proof and all-user participation remain open.
- Lifecycle audit found ordinary normalization still calls `owned_source_root`
  without that lock. Every final-stage reader, mutation, purge and cleanup must
  participate under one order before protected receipts can be activated.
  Releasing the lock between remeasurement pages cannot prove one coherent tree;
  bounded evidence and binding must retain a coherent protection boundary.
- A further ABA gap remains: provisional source/stage spool captures, ordered
  proof sessions and protected baselines are bound to worker ID but not the
  opaque processing-claim epoch. A same-worker reclaim could mix old observations
  with a newly adopted attempt. Epoch fencing across protected captures,
  baselines, ordered sessions and publication is drafted with a deterministic
  reclaim regression, but has not compiled or run. Protected publication
  remains disconnected.
- `OwnedSource` currently exposes a clonable path. A protected lifecycle guard
  must accompany the actual inspection/normalization blocking work, subprocesses,
  cancellation reaper and deletion routes; checking only the initial receipt
  cannot retain the lock through real work termination. An internal wrapper
  now carries a provisional hold into inspection/normalization blocking work;
  it is uncompiled/unrun, and the directory script hook fails closed when a
  protected hold is present. Subprocess/deletion participation and distinguishing
  shared hold lifetime from exclusive mutation authority remain open. Independent
  review confirmed all production callers still construct an unguarded owner;
  only test-only code can attach a hold. Acquire an opaque verified guard from
  the protected receipt and propagate it through actual pipelines before claiming
  lifecycle retention. Review also found the provisional guard rejects every
  installed scripting gate, including disabled/no-hook configurations; preserve
  those cases and hold protection through actual enabled-hook subprocess work.
  A later focused run passed synthetic receipt orchestration 1/1, guard
  blocking/subprocess cancellation 3/3, configuration 1/1 and guarded/legacy
  scripting cases 6/6. These do not connect the production guarded owner or prove
  full-tree custody; 15 prototype warnings remained in that build.
- Independent epoch review found another activation blocker: generic owner-only
  heartbeat and acquisition mutations could accept stale work after same-worker
  reclaim and bypass the protected epoch. Protected work must use epoch-bearing
  APIs end-to-end or generic APIs must reject protected-reserved items. Add a
  stale E1/same-owner E2 acquisition regression proving no receipt or source
  cleanup; the protected flow remains disconnected. A durable DB marker cannot
  revoke stale work that crossed generic acquisition before the marker, so the
  shared lifecycle/attempt lock must also cover that earlier filesystem boundary.
- Operator review deletion/correction and catalog hard-delete can mutate or
  remove work items outside worker APIs. Audit and integrate the same lifecycle
  and retained-source interlocks there before protected mode activation; existing
  retained-obligation purge checks do not prove every operator path is covered.
  Audit identified release, artist and track hard-delete plus unpublished and
  published review deletion and correction. A durable operator-deletion
  reservation must precede filesystem effects and exclude protected claims;
  already-running ordinary and protected writers also require an enforceable
  shared filesystem/lifecycle lock or explicit maintenance fence; database
  expiry alone is not proof that filesystem work stopped. Focused
  deletion hold-worker tests passed 5/5 including cancellation and oversize
  preservation. Reservation persistence tests passed 3/3 for blocked
  cancellation, expired intent reclaim and live/protected work refusal; routes,
  legacy lock registry and reservation integration remain incomplete. A later
  run passed lifecycle invalidation 2/2, reservation 3/3 and catalog hard-delete
  14/14, including rejecting a Track deletion under a release reservation.
  A synthetic unpublished-review finalization test passed 2/2 for token
  takeover, lost-ack replay and cancellation rollback. Catalog closure
  finalization and filesystem integration remain unfinished. A bounded draft
  for the offline release-media portion records deduplicated assets and
  per-page acknowledgements; its focused test passed 1/1 for 21 assets across
  the page boundary, shared-asset exclusion, stale tokens and replay. A later
  expanded 1/1 regression passed canonical/staging paths, exclusive-image
  deduplication and shared-image exclusion after its initial fixture was fixed
  to include a valid staging handoff. Track/Artist mappings, actual filesystem
  proof and atomic deletion remained open. Subsequent provisional DB closure
  tests passed 3/3 for exact Release/Track/Artist binding, shared-asset
  preservation, ownership-change rejection, bounded pages and acknowledgement
  semantics. A later 6/6 suite passed set-based page insertion, intra-scope
  deduplication, intent replay and blocked expiry/cancellation. Another 6/6 run
  passed exact current catalog binding, same-size hash mutation and outside-scope
  revalidation. A subsequent set-based page-revalidation regression passed
  1/1 for newly shared outside-scope media, same-size hash mutation, exact
  replay and epoch fencing. An explicitly invoked release-build owned-DB
  10K-Track closure test passed: capture took 22,999 ms across 629 turns in one
  sample. This excludes filesystem deletion, ingestion, interactive load,
  statement/lock counts and the 600K catalog; filesystem proof,
  token-aware finalization and representative plans remain open.
- Destructive finalization needs a shared non-fabricable custody capability:
  constructors must actually acquire and validate the ordered locks and perform
  descriptor-relative verified work for the exact reservation/epoch/page. The
  private orchestrator must consume page/scope proofs while retaining the held
  locks; public DB assertions alone cannot authorize deletion. Implementation,
  architecture decision, integration and regressions remain outstanding. The
  new lower `melodee-custody` crate is registered and its lock-only focused
  tests passed 2/2 for contention, replacement and scope bounds; it does not
  yet create page proofs or authorize media/DB deletion. Independent review
  found it drops root/ancestor descriptors and retains reusable dev/inode
  values, leaving a namespace ABA gap. The fix retains ancestry descriptors;
  focused lock tests passed 3/3, including unlinked-root/ancestor replacement,
  and two scope-transfer/lifetime cancellation tests passed 1/1 each. The fix
  passed independent static re-review. A later shared crate suite passed 4/4
  including flock descriptor transfer without unlock, and a real-DB worker
  fixture passed 1/1 for cancellation, retained scope, stale reservation and
  source preservation after fixing its test-runtime scheduling. Page/deletion
  authority remains absent; three ingestion test-build warnings remained. The
  shared suite later passed 6/6 with bounded read-only BLAKE3/SHA-256 checks,
  same-size mutation, path/symlink and cancellation cases. Its 256 MiB cap
  fails closed for larger files and is not a complete production solution. A
  later in-process scanner retains the pinned file and unique lock over ≤8 MiB
  turns. Focused tests passed 2/2, including a 257 MiB owned sparse file in 33
  turns, exact BLAKE3, lock retention, both hash algorithms, same-size mutation
  and stale-fence poisoning. Independent static review found no new P0/P1 in
  the disconnected scanner, but its lock is not type-bound to the source
  root/path/work item. Authoritative integration must bind those identities to
  the durable intent and recheck at the action boundary. The callbacks are not
  DB proof and no deletion authority is produced; process loss restarts at zero.
- The observational scanner was later extended to own the real multi-item
  scope. The shared suite passed 9/9, including two-item lock replacement and
  large-file scanning; a real-DB provisioning fixture passed 1/1 for cancellation
  and lock retention. Its generic revalidation interface remains forgeable and
  observation-only, not an authoritative proof.
- A durable page-intent metadata test passed 1/1 for exact metadata, same-epoch
  replay, mutation rejection, blocked assertion-only acknowledgement/discard,
  quarantine and new-epoch refusal. Filesystem proofs and proof-backed
  acknowledgement/finalization are still missing. A subsequent 1/1 regression
  passed with an explicit immutable snapshot revision, exact replay, epoch
  fencing and no cross-epoch adoption. Blocked page-intent expiry/cancellation
  tests also passed 2/2, asserting no intent, entry or quota changes committed.
  A subsequent exact-intent 1/1 regression also rejects late appended members.
  A separate object-intent regression passed 1/1 for replaced artwork and
  staging-directory bindings invalidating the intent before filesystem work.
- A synthetic DB-only completion cleanup test passed 1/1 for exact intent
  coverage, guard exclusion, bounded cleanup, quota accounting and immutable
  tombstone replay. Its fabricated verification rows do not prove filesystem
  deletion. A durable verified archive/retention protocol is still needed before
  compact tombstone quota can safely be released.
- A test-only archive filesystem prototype passed 2/2 for bounded encoding,
  exclusive no-replace creation/readback, interruption preservation, same-size
  mutation rejection and detached-worker lock retention. It has no DB intent or
  epoch authority, and crash replay remains quarantine-only. Independent
  static review found no P0/P1 in that test-only scope but confirmed that an
  immutable/sealed artifact or action-boundary revalidation plus durable DB
  fencing is required for authority; production integration remains pending.
  A separate synthetic archive-intent regression passed 1/1 for deterministic
  bytes/name, replay, root replacement and expired epoch after fixing an initial
  compile error in digest formatting. Its later 1/1 rerun also covered
  deterministic archive-lock intent, idempotent same-generation acknowledgement
  and changed-generation rejection. A later test-only filesystem suite passed
  3/3 with the per-completion lock in the write/fsync/readback worker, including
  contention, replacement, interruption, mutation and detached retention. It
  still has no DB intent/epoch or immutable-artifact proof and cannot release quota.
  A synthetic DB intent/lock readback regression passed 1/1 for absent/present
  lock identity, exact immutable binding and stale-epoch rejection; this readback
  does not establish approved-root provenance or authorize quota release.
- A test-only exact-intent real-DB/filesystem observational scan passed 1/1
  for derived path/hash/size, held scope and both DB fences, same-size DB hash
  change rejection, fresh retry and source/canonical preservation. A later 1/1
  rerun passed separate expired reservation and maintenance-fence cases; the
  strict lowercase digest parser passed 1/1. It cannot acknowledge or delete.
  Provisional public acknowledgement bits are not proof;
  finalization must require complete exact verified-intent coverage or
  non-fabricable proof records. The forged-ack coverage regression passed 1/1,
  including audit rollback and frozen-closure insertion rejection; a trusted
  finalizer or filesystem deletion is not enabled.
- The shared crate's strictly observational, non-cloneable credential carrier
  passed a real-DB/FS test 1/1 for original/wrong/expired epochs, changed target
  semantics and source preservation. A later 1/1 test passed stronger original-
  epoch/root-set observation and both lease expiries. It deliberately grants no
  filesystem or completion authority; proof construction remains incomplete.
- Review found a check-then-unlink replacement race. A durable bounded
  per-entry quarantine intent has passed a focused 1/1 DB regression for
  exact binding, replay, replacement rejection and nonce conflict. The private executor must move
  without replacement on the same filesystem, reopen and independently verify
  the moved inode and full digest, and preserve uncertain/mismatched bytes.
  Metadata alone cannot authorize the move or unlink. A test-only owned-FS
  experiment passed 1/1 for an injected post-check replacement, existing-target
  conflict and a valid move while preserving bytes. It uses an 8 MiB observation
  cap, never unlinks and is not bound to the DB intent or crash recovery. The
  intent lacks an immutable exact native quarantine-parent path/root-role, so a
  restarted worker could not safely locate it without guessing. The new
  versioned native path/root-role binding and fenced readback passed 1/1 for
  non-Unicode path, changed parent and stale epoch; a separate synthetic cleanup
  test passed 1/1 for quarantine quota accounting. The first owned DB/FS
  no-replace move test passed 1/1 after a fixture compile fix: non-Unicode
  parent, inode/digest verification and source preservation were exercised,
  and the DB row remained `intent` without unlink/ack. DB-bound interruption,
  replacement races, verified unlink/ack and crash recovery remain open.
- Coverage must require an exact verified quarantine record for each media and
  artwork member; a page-level verified bit alone is insufficient. Directory
  rows remain fail-closed until separate namespace/subtree proof exists. A
  strengthened synthetic track-only coverage/cleanup regression passed 1/1;
  a later 1/1 regression explicitly rejected a synthetically verified page
  while its individual quarantine row was unverified. The first DB/FS
  no-replace move test also passed, but none of these constitutes completed
  unlink or recovery. A separate 1/1 regression confirmed that a synthetic
  verified page and acknowledgement cannot certify an unproven directory subtree.
- The DB lifecycle registry currently binds lock/staging identities but not the
  canonical/asset roots. A same-content clone at a caller-supplied root must not
  authorize an intended deletion. Add a maintenance-fenced immutable complete-
  root registry, bind identities to intent/proof, and test root substitution.
  A shared raw native-path codec passed 1/1 for non-Unicode round trip and
  malformed/traversal/ambiguous inputs. A provisional root-set/intent regression
  passed 1/1 for identical replay, changed inode, immutable revision and epoch
  non-adoption after its initial fixture was corrected to include the reserved
  source identity. This is synthetic metadata, not filesystem identity proof.
  Its subsequent bounded root-readback regression passed 1/1 for exact round
  trip, immutable replay, changed generation rejection and epoch retention;
  a later 1/1 rerun also permanently asserts omitted reserved sources fail.
  A non-authorizing supplied-root verifier passed 1/1 for non-Unicode path,
  exact identities, duplicate rejection and canonical-root replacement; the
  first mistyped exact filter ran zero tests and is not counted. Equality with
  the entire trusted live configured set and DB/action binding remain unfinished.
  A caller-supplied configured-root comparison later passed 1/1 for omission,
  mismatch, alias and replacement. A later test-only scan derived roots from
  `IngestionConfig`, bound the DB root revision and passed 1/1 for a different
  canonical root, expired epochs, changed DB hash and byte preservation. It is
  still observational and does not authorize an action.
  Provisional nonempty frozen-snapshot discard currently conflicts with its
  immutability trigger; a fenced resumable `discarding` state and paged quota
  decrements now passed a focused 1/1 regression for ≤16-entry root pages,
  capture/restart refusal, idempotent completion and quota return. An updated
  1/1 rerun also passed stale-epoch rejection, fresh-epoch reclaim and completion.
  Another 1/1 regression passed for refusing out-of-band frozen-row deletion
  while allowing fenced discard and reclaim. The prior
  conflict was discovered before this fix; integrated recovery remains open.
- Repository doc validation currently reports link errors from invocation-owned
  `.stage-verity-proof-*` and `.bind-alias-proof-*` snapshot trees inside the
  workspace. Six exact registered, source/inode-verified owned worktrees were
  relocated without deletion to a private same-filesystem sibling quarantine;
  both bind fixtures and private logs were preserved. Rerun documentation
  validation after remaining in-repo worktrees are safely handled. ADR-0222 is
  Proposed and not an authorization to enable deletion.
- A test-only legacy lifecycle capability passed 1/1 for validated private
  namespace, distinct-user ACL checks, exclusive contention and replacement
  preservation. Its expanded 1/1 regression also passed independent child
  contention/acquisition, wrong root/inode and lifetime-hold checks. It is not
  wired to workers, operator routes or registry
  readiness. A provisional registry now drafts exact immutable identities,
  keyset cursor, lost-ack replay and quarantine; its tests, newly discovered
  item provisioning and old-binary maintenance fencing remain unfinished. A
  focused synthetic registry/provisioning suite passed 2/2 and reservation
  suite passed 3/3, confirming maintenance completion does not activate runtime
  enforcement; actual filesystem/old-binary fencing is not certified. Its
  maintenance-completion switch must remain unavailable until every runtime
  participant and verified operational fence are implemented; a DB flag cannot
  certify that old writers have terminated. Draft exact-identity invalidation
  quarantines changed ready rows and excludes them from claims without cursor
  advance; its focused retest remains pending.
- A test-only provisioning/topology fixture passed 1/1 on one same-mount
  filesystem: interruption before acknowledgement quarantined the exact inode,
  bytes were preserved, asset-root overlap was refused before creation, and
  pass wrap/ready replay succeeded. An updated 1/1 rerun also covered typed
  attestations and all configured fixture roots. It does not establish bind-mount or
  cross-filesystem support, production routing or the operational fence.
- Review of the draft cross-device topology policy found other-root alias gaps:
  lexical `..`/symlink aliases can reach the private root, and other roots
  are not checked pairwise. Reject non-normal paths, compare pinned root
  identities and ancestry across all roots. The draft fix and alias/nesting
  regressions passed independent static re-review. A pure rules test and an
  owned different-device legacy-lock test then passed 1/1 each, including the
  unchanged rename-custody same-mount rejection and no mount commands. This is
  local capability evidence, not completed production cross-filesystem intake
  or deletion; full support remains unproven. A later audit found the ingestion
  helper retains other-root dev/inode numbers but not ancestor descriptors,
  potentially permitting ABA substitution. The descriptor-pinning fix passed
  its 1/1 unlink/recreate regression; independent re-review remains pending. A
  temporary 512-handle cap can reject valid 128-item scopes, so shared topology
  ownership and compatibility verification are still required. Re-review also
  found revalidation may hold the old descriptor set while opening another full
  set, exceeding the cap. Shared topology and streaming revalidation now passed
  a real 128-item scope 1/1, a 128-root observed-FD-peak test 1/1 and shared
  custody suite 10/10. Independent static re-review cleared the overlap defect.
  More than 512 distinct topology ancestors still fail closed, lock descriptors
  are separately bounded, and representative resource measurements remain
  pending; the capability is non-authorizing.
- Typed maintenance attestation persistence tests passed: registry 4/4,
  including refusal of false/missing assertions, idempotent append-only audit,
  and blocked expiry/cancellation rollback; admin review reservation passed
  3/3. The recorded stop/inspection/backup statements are operator assertions,
  not database proof that old writers terminated.
- P0 safety review found the new ignored bind-alias test checks for a distinct
  mount namespace but not private propagation; directly running it could affect
  the host. Do not execute it. Add a dedicated private-propagation launcher and
  independent mountinfo preflight rejecting shared/slave propagation before any
  mount. A pure no-command-on-unsafe-fixture regression passed 1/1 for shared,
  slave, unknown, malformed, empty and oversized inputs. The launcher and
  independent review initially found the draft launcher closed
  its pinned fixture FD before exec and passed a mutable pathname as TMPDIR,
  cwd and parent log directory. Retain the directory capability across all of
  those operations and test replacement before rerunning the review. A further
  P0 review found automatic `TemporaryDirectory` cleanup could recursively
  remove a replacement directory after child execution. The launcher must use
  identity-safe cleanup or quarantine on uncertainty, with an interleaving
  sentinel regression. The revised launcher retains/quarantines its owned
  fixture and pins its directory through execution; independent static review
  cleared these races. Since `/proc/self/fd/...` TMPDIR is incompatible with
  Rust no-follow and mountinfo path checks, the launcher now uses a pinned-root
  private chroot with read-only tools. Both launcher and Rust in-test propagation
  preflight passed independent static review. An isolated 587-input source
  snapshot was verified and built, but the private-namespace launcher exited 1
  before the exact bind test passed. Parent user/mount/PID namespaces remained
  unchanged; the owned fixture was retained. Sanitized result and proof are
  `tests/performance/bind-alias-2026-11-27.json` and
  `tests/performance/bind-alias-build-2026-11-27.json`. A reviewed bounded
  diagnostic rerun failed at private `proc_mount` setup before the Rust test;
  parent namespaces remained unchanged and its fixture is quarantined. Further
  bounded mount-stderr diagnostics passed Python tests and independent static
  review. A syscall-only rerun reported `move_mount:EINVAL` during proc setup;
  the Rust test did not run, parent namespaces remained unchanged and the fixture
  was retained. A subagent's further diagnostic turn was flagged by the tool
  environment. A separate attempt to resume that subagent on maintenance CLI
  work was also flagged; neither turn produced implementation. Continue other
  independent work while assessing safe alternatives. No host
  mount/security changes are authorized.
- Launcher diagnostics review found another P1: a temporary log could grow
  arbitrarily between size polls. Bound capture during writes or enforce a
  verified file-size limit. Incremental bounded pipe capture and a real noisy-
  child termination test passed independent static re-review; a new
  source-fingerprinted namespace run is pending.
- Focused epoch/interlock runs passed append expiry/cancellation 2/2,
  baseline/session/publication expiry/cancellation 6/6 and protected
  publication/ABA/receipt lookup 3/3. The protected-attempt suite subsequently
  passed 4/4 after implementing protected normalization claims; it asserts the
  ordinary claim skips marked work and the protected claim returns a fresh
  opaque epoch. Initial compile errors and the prior 3/4 run were corrected and
  rerun; the final focused run emitted no warnings. A subsequent audit found
  other worker-only mutations, including manifest, catalog and policy paths,
  still need protected-mode rejection or exact-epoch variants before activation.
  Generic `release_lease` also needs protected-mode fencing and a stale-release
  regression; same-owner stale work must not release its successor's lease.
- Follow-up focused suites passed protected stage writes 2/2, publication 3/3
  and attempts 4/4, including bounded interrupted-inspection recovery past a
  locked occurrence, wraparound, fresh reclaim epoch and stale heartbeat/release
  rejection. The hard-cap recovery regression also passed 1/1, including a
  rejected size of 129 without cursor advance. These are persistence/metadata tests; independently held filesystem
  protection and end-to-end production custody remain unfinished.
- The intermediate A/B migration-CLI build emitted 51 ingestion warnings,
  mainly disconnected custody helpers, including an unused import. They are
  not a final quality-gate pass; integrate or correctly scope the helpers and
  resolve warnings without suppressing real issues before final Clippy.
- Test-only writer-lock/capability tests passed: namespace test 1/1 covered
  cross-process lock contention, reacquisition and lock-path replacement; a
  real-DB wrapper 1/1 checked lock-before-namespace, epoch revalidation,
  acknowledged identities and stale same-worker rejection. The test filesystem
  lacked fs-verity; complete coverage and production cleanup remain open. A
  later real-DB remeasurement test passed 1/1 with a cancelled waiter, verifying
  that the paused blocking worker retained the lifecycle lock until termination.
  Further namespace/known-directory and real-DB oversized-page regressions
  passed 2/2, but positive sealed-file remeasurement did not execute on that
  unsupported filesystem. A subsequent 1/1 real-DB directory-page regression
  passed for bounded pages, lock retention, stale stage replacement rejection,
  preservation of both trees, cancellation and oversize inputs. Its updated
  lifecycle-root/wrong-root version also passed 1/1; these later changes were not
  included in the intermediate supported-Btrfs run.
- A test-only bounded operator-scope acquisition fixture passed 1/1 for
  contention, lifetime hold after owner drop and reacquisition. It does not
  authorize deletion or prove complete page and cancellation coverage. Two
  updated focused tests later passed for stale reservation, root replacement,
  lock lifetime and bounded page replay/order/quota; the shared proof chain is
  still absent.
- A test-only real-DB cleaned-receipt membership wrapper passed 1/1 for an
  unknown receipt failing closed before filesystem work, with earlier bounded
  page, wrong-root, stage-swap and cancellation assertions. A positive cleaned
  receipt fixture later passed 1/1 for config/lock/receipt binding, stale epoch,
  cancellation, page mutation and source preservation, using synthetic
  evidence. Complete membership/content verification and production recovery
  remain unverified.
- Review of the proposed supported-filesystem diagnostic runner found that it
  could copy untrusted child JSON verbatim into sanitized evidence. Validate an
  exact allowlisted schema and test injection: the fix passed 14 Python tests
  and independent static re-review. A fresh build proof and owned diagnostic
  rerun remain pending; the earlier failed evidence must be retained.
- Handoff restart (2026-11-27, main `9b6a9e4`): completed the interrupted
  recovery-specific quiescence isolation. `confirm_lifecycle_quiescence_inner`
  now persists the exact recovery reservation in
  `work_item_lifecycle_maintenance.recovery_reservation_id/token` and keeps
  `quiesced=false` (matching migration 0071 CHECK 2), while a generic
  confirmation still sets `quiesced=true` and clears any recovery binding;
  `reclaim` and `finish` clear stale bindings. The read-only recovery locator
  (`inspect_admin_quarantine_recovery`, `read_admin_quarantine_binding`) gained
  a dedicated `fences_readonly_recovery`/`lock_fence_recovery` path that
  accepts either generic quiescence or the exact live recovery reservation;
  every mutating page/acknowledgement/leaf fence is unchanged and still
  requires generic quiescence. A database lease expiry remains explicitly
  non-proofs of worker termination. Focused evidence on a fresh owned
  database: `catalog_hard_delete::maintenance_page_intent_is_exact_immutable_
  and_never_adopted_by_new_epoch` now additionally asserts the persisted
  binding state `(quiesced=false, recovery_reservation_id=current)`, that a
  recovery attestation cannot authorize `next_lifecycle_provision_item`, new
  page creation, `acknowledge_maintenance_media_page`, quarantine disposition
  changes, or a second reservation, plus the preexisting wrong/stale/incomplete
  and double-attestation cases; pass 1/1. The full binary passed 23/23
  (1 ignored). With `--features test-support`, all 135 ingestion lib tests
  passed again after the fence change.
- Handoff restart (cont.): implemented the guarded no-handoff admin purge path
  so `purge_removes_inbound_source_when_no_staging_handoff_exists` passes
  without restoring the unsafe baseline unlink. New production primitive
  `melodee_ingestion::delete_purged_inbound_occurrence` (secure_fs
  `remove_empty_occurrence_tree`): pins the configured inbound root and every
  intermediate directory no-follow, requires a directory leaf, refuses a
  non-empty occurrence, moves the leaf to a unique no-replace dot-quarantine
  sibling within the pinned parent, fsyncs the parent, re-opens the moved
  name and compares `dev/ino` against the pinned occurrence, re-verifies
  emptiness, unlinks only the verified empty directory, and fsyncs again.
  Missing leaves are idempotent; any observed change leaves moved bytes under
  the quarantine name and fails closed. The purge route resolves the inbound
  path and pre-checks directory/emptiness before creating the durable guard
  (avoiding guard leaks on the common refusal), then performs the bounded,
  admitted removal under `begin_pre_catalog_purge` before other storage
  deletions and the fenced DB completion. Evidence: ingestion lib
  `deletion::tests` 9/9 (new: empty-occurrence removal+idempotency,
  non-empty refusal with retained bytes and later success, leaf symlink
  refusal); `melodee-server --test admin_purge_api` 11/11 including both
  no-handoff regressions on a fresh owned database. The production-wide
  occurrence protocol for non-empty inbound removals (bytes verification,
  retained-source reobservation) remains PARTIAL and is unchanged by this.
- Handoff restart (cont.): converged the working tree on the repository strict
  quality gates and reran the claimed intermediate suites on fresh owned
  databases with the final integrated sources. Strict workspace
  `cargo clippy --workspace --all-targets --all-features -D warnings` is clean
  (previously numerous disconnected custody warnings; three in an ingestion
  test build), `cargo fmt --check` is clean, and `python3 scripts/check.py
  fast` passes (repo validation, requirements index, fmt, clippy, script-worker
  build). Fresh-database reruns: `melodee-catalog identity_scoped_oracle` 2/2
  (scoped/global oracle equivalence, review decisions), `melodee-search
  rebuild_integration` 13/13 after correcting the 0045-deferral fixture base to
  skip only migration 0045 (the new 0071 depends on the 0046-0059 additive
  schema; the old <=44 base predates 0071), ingestion `inspection` 7/7,
  `environment_faults` 7/7, `normalization` 15/15,
  `publication_worker_resilience` 5/5, `reconciliation` 46/46, media
  `normalization_real` 14/14, persistence `custody_hash_spool` 8/8,
  `inbound_custody` 2/2, `retained_baseline` 4/4, `retained_sources` 8/8, and
  the full `catalog_hard_delete` binary 23/23 including the extended recovery
  authority test.
- Handoff restart (cont.): repaired the historical populated-upgrade fixtures
  that the 0071-era engine broke, and brought all six ignored historical
  migration compatibility tests plus 17 non-ignored schema tests green on the
  final tree. The 0041 pre-repair base now covers every migration except the
  0041 subject and the recommendation fixture covers everything except the
  0065/0066 subjects (both rely on no later-migration dependency on the
  subject), because the current claim/policy engine unconditionally references
  the 0071 guard columns/tables. The 0067/0070/0071 DDL subjects seed data via
  a new old-engine-faithful raw work-item insert helper (`seed_raw_work_item`)
  instead of driving the current guarded `WorkItemStore.create` against a
  schema that predates its guard tables; read paths remain plain selects.
  Evidence on fresh owned databases: migrations.rs 17/17 non-ignored (was
  13/4) and `make test-migrations` 6/6 ignored (was 5/1). Additional
  fresh-database reruns on final sources: `catalog_merge_split` 6/6,
  `claiming` 9/9, `connection_metrics` 1/1, `catalog_import` 31/31,
  `claim_epoch_paths` 1/1, `inbound_occurrence_identity` 3/3,
  `catalog_concurrency` 6/6, `catalog_identity` 7/7. Honest limitation: the
  current engine cannot execute claim/write paths against a pre-0071 schema;
  fixtures therefore never do, and any real deployment still migrates before
  the binary starts per the runbook.
- Handoff restart (cont.): comprehensive fresh-database regression sweep on the
  final integrated sources. The full persistence integration set passes:
  85 test binaries, 368 tests, 0 failures (plus `perf_claims` 1/1 on a
  10,000-item queue with 32 workers, zero duplicate claims, after aligning its
  EXPLAIN with the six-parameter claim lane; the invocation-owned
  release-build merge fixture stays ignored by design). Also green:
  `settling_capture` 9/9 (after raw-seeding fix), application lib/bins 41/41,
  domain/MQL/policy/media 229/229, and server suites
  admin_catalog 4, admin_policies 3, admin_stats 12, admin_system 10,
  admin_review 11, admin_catalog_delete 4, admin_purge 11 (reconfirmed),
  native_catalog 24, native_auth 34, native_stream 19, native_stream_acl 21,
  opensubsonic_browse 18, opensubsonic_media 7, opensubsonic_conditional 1,
  jellyfin_core 32, jellyfin_media 20, worker_pools 4, discovery_worker 7,
  policy_worker 11, scripting_service 3, podcast_retention 4 (serialized
  harness), enrichment_capacity 12, lastfm_scrobble 15, notification_smtp 2,
  admin_identity 10, admin_duplicates 5, retained_obligations CLI 1/1.
- Handoff restart (cont.): the full pre-merge gate `make check` passes on the
  final integrated tree (exit 0): repository/doc validation (123 ADRs,
  8 mockups, 239 requirements), requirements index, Python gate, Admin UI
  pnpm frozen-lockfile/typecheck/lint/i18n/unit tests (318 passed),
  `cargo fmt --check`, strict workspace clippy, and `cargo deny` (advisories,
  bans, licenses, sources). The release-readiness `make check-nightly` tier
  remains the only unexecuted gate this session.

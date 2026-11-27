# Melodee-rs: Finish the Performance Remediation Program (updated handoff, 2026-11-27)

## Assignment to the next coding agent

Operate as the autonomous implementation engineer responsible for completing
the entire performance remediation program. **Implement, integrate, measure and
verify the remaining requirements. Do not return another roadmap or stop after
one work package, one benchmark, or a passing gate.** The authoritative
acceptance contract is [PERF-REMEDIATION-SPEC.md](PERF-REMEDIATION-SPEC.md),
including all 21 work packages and all 17 original findings. Status documents
are evidence and a ledger, not permission to reduce the specification.

Read these documents **in full** before making changes:

- [AGENTS.md](AGENTS.md), [PERFORMANCE.md](PERFORMANCE.md).
- [PERF-CONCERNS-REVIEW.md](PERF-CONCERNS-REVIEW.md),
  [PERF-CONCERNS-VALIDATION.md](PERF-CONCERNS-VALIDATION.md), and
  [PERF-REMEDIATION-SPEC.md](PERF-REMEDIATION-SPEC.md).
- [PERF-REMEDIATION-STATUS.md](PERF-REMEDIATION-STATUS.md) **including its
  long, chronological tail**, and [PERF-REMEDIATION-RESULTS.md](PERF-REMEDIATION-RESULTS.md).
- Relevant accepted ADRs and migrations, particularly
  [the upgrade/repair runbook](design/runbooks/PERF-REMEDIATION-UPGRADE.md),
  the benchmark [README](tests/performance/README.md), and ADRs 0215–0222.
  ADR-0221 and ADR-0222 are **Proposed**, not permission to claim their
  production designs are implemented or approved.

Keep `PERF-REMEDIATION-STATUS.md` as the single requirement-level execution
ledger. For each mandatory subrequirement record exact requirement,
implementation, evidence, status, and next executable action. An unexecuted
test, hard design problem, rejected prototype, or convenient architectural
preference is not evidence for `NO CHANGE JUSTIFIED` or `EXTERNALLY BLOCKED`.
If work is interrupted, append a precise, truthful recovery checkpoint to the
ledger's chronological tail.

Do not perform release/deployment orchestration, push, publish a release,
deploy to production, access production data, alter host security settings, or
modify unrelated work. Routine safe engineering decisions, owned disposable
test databases/directories, and in-repository implementation are already
authorized.

## Exact handoff state (verify yourself before acting)

This handoff was created 2026-11-27 by the previous agent when the token
budget was exhausted. **The working tree was left clean and compiling**, with
no background builds running.

- **Main branch HEAD at handoff: the `fix(migrations)` restructure commit on
  top of `e7de341`; operation history is `9b6a9e4` + the twelve commits listed
  in `git log --oneline 9b6a9e4..HEAD`.** All are descendants of the earlier
  intermediate snapshots `37f2988`, `08de08d`, `8a4cf6f`, `ca8ef83`, `63394a1`
  and of the original pre-remediation baseline `a0979cd`, all of which verify
  as ancestors. `git status --short` was empty at handoff (clean).
- The immediately previous agent completed all P0 items from the first handoff
  and much more (see "Completed and verified" below).
- **The nightly tier (`make check-nightly`) has never completed green on this
  final tree and is the FIRST next action.** The previous run failed only on
  `catalog_reuse::migration_repairs_projection_and_appends_overlay_without_
  rewriting_history`, which was fixed and verified 4/4 on a fresh owned
  database immediately before this handoff; all other suites up to that point
  in the nightly order passed or had already passed in the same run. Nothing
  is believed broken, but a full nightly rerun is required evidence and has
  not yet been executed to completion.

### Already completed and verified during this operation (from the ledger tail)

The ledger tail records each entry with full detail; summary:

1. **P0 recovery-fence isolation**: recovery quiescence binds the maintenance
   fence to the exact live reservation and keeps `quiesced=false`; read-only
   recovery locator accepts either generic quiescence or the exact binding;
   mutation fences unchanged. Extended authority-negation tests pass
   (catalog_hard_delete 23/23; ingestion lib 135/135).
2. **Guarded no-handoff admin purge** production protocol for the empty
   occurrence case (`delete_purged_inbound_occurrence` /
   `secure_fs::remove_empty_occurrence_tree`): descriptor-relative
   pin/refuse-non-empty/no-replace quarantine move/identity
   re-verify/re-verify empty/unlink/fsync, quarantine retention closed on any
   race. Both no-handoff purge regressions pass; admin_purge_api 11/11.
   Non-empty inbound removal remains `source_custody_required` (PARTIAL).
3. **Strict quality convergence**: workspace `cargo clippy --all-targets
   --all-features -D warnings` clean; `cargo fmt --check` clean; `make
   check-fast` and **`make check` (full pre-merge gate incl. Admin UI 318
   tests and cargo-deny) pass**.
4. **Migration/upgraded fixtures repaired**: the 0071-era engine cannot drive
   pre-0071 schemas; affected fixtures now use "full chain minus the subject
   migration" bases (0041, 0045×2, recommendation 0065/0066) or
   old-engine-faithful raw inserts (`common::seed_raw_work_item`; 0067/0070/
   0071, settling snapshot). All 17 non-ignored migrations tests 17/17 and
   the six ignored historical tests 6/6 (`make test-migrations`).
5. **Comprehensive fresh-database regression sweep** on final sources:
   persistence 85 binaries / 368 tests 0 failures, domain/MQL/policy/media
   229, application 41, and ~28 melodee-server suites (all green; details in
   ledger tail), ingestion inspection/environment_faults/normalization/
   reconciliation/publication green, media real-media 14/14.
6. **Nested candidate acquisition** implemented (resolve_candidate and
   cleanup_inbound now accept nested all-Normal candidates through the
   same no-follow component checks; traversal/absolute still rejected before
   I/O); retained fixtures provide the exact `stage-<baseline>.lock` identity
   the 0071 CHECK requires; `RuntimeDefinitions` gained the two documented
   `RETAINED_RECHECK_*` settings; the sticky changed-source request and its
   `awaiting_intake` reason are one atomic transition (5× stable).
   Public docs (admin-api, troubleshooting) and the runbook updated for the
   guarded empty-occurrence purge.
7. `perf_claims` EXPLAIN aligned to the six-parameter claim lane; the
   10k/32-worker claim benchmark passes with zero duplicate claims.

## Resource, safety and implementation rules

Begin every relevant shell session with:

```bash
export CARGO_BUILD_JOBS=2
export RUST_TEST_THREADS=1
```

Use **one coordinator for expensive work**. Do not concurrently run multiple
large Cargo builds, fixture generators, media benchmarks, or load tests. Rust
concurrency regressions must still exercise concurrent actors internally.
Parallelize independent source analysis and bounded implementation when safe.
Before scheduling expensive work, inspect `ps aux | grep -E 'cargo|rustc'` —
unrelated VS Code `rust-analyzer` jobs may run without your control; do not
kill or claim results from unrelated processes, and label or defer any
contaminated performance samples.

Use only explicitly owned disposable PostgreSQL databases, worktrees, and
media roots. Repository scripts and new orchestration must follow the
Python-only rules in `AGENTS.md`. All entry points must have safe `--help`;
destructive helpers must verify exact ownership, fail closed, and have scope
tests. Fresh-database test runs use `python3 scripts/test_database.py -- cargo
test …` (per-test UUID-owned databases); dev Postgres is on host port **5433**
(`make postgres-up` if it is down).

Never replace cryptographic content evidence with timestamps, inode, length or
assumed immutability. A persisted inode can be reused after a crash; a pinned
FD does not prevent concurrent writes to the same inode; a lease or database
flag cannot prove a worker with open filesystem descriptors has terminated.
Admission must occur before expensive work is spawned or queued; permits and
guards must remain held until the actual blocking work, subprocess, pipe
draining, termination and reaping finish, including cancellation.

Preserve public API fields, ordering, IDs, auth, ACLs, ratings, pagination,
JSON/XML, range/streaming semantics, manual/negative identity decisions,
same-name distinct artists, redirects, and all existing compatibility tests.
Never suppress or weaken tests to obtain a green gate. The historical podcast
lease test is timing-sensitive; use the serialized harness and flake honestly.

## Current package ledger and remaining work

Confirmed at earlier/intermediate checkpoints and revalidated in the sweep
(RM-01..RM-05, RM-08, RM-16 also need "reconfirm" evidence recorded in the
ledger): their final integrated regressions have run green in the sweep above.
All remaining packages below are partial and mandatory.

### Immediate next actions (in order)

1. **Run `make check-nightly` to completion** with the env limits above. The
   last run failed only at `catalog_reuse`, which is fixed. If new failures
   appear, diagnose and fix without suppressing tests, rerunning the focused
   suite first (pattern that worked: fixture bases "full chain minus subject"
   or old-engine-faithful raw seeds; engine-side ordering fixes as one durable
   transition). Do not stop until the whole nightly is green, then record it
   in the ledger with the full log location.
2. **Custody P0 integration**, continuing from the ledger's itemized P0 list
   (items 2–8 remain): integrated claim-bound guarded owner with production
   stage readers/writers; protected stage protocol publication (ADR-0221 is
   Proposed); retained-source obligations genuinely live/production-reachable
   (NULL baseline-ID obligations and the immutable-trigger interplay are known
   open); per-entry exact intent/private quarantine fault cases + safe
   two-name recovery; bounded deletion/repair orchestration with durable
   reservations before filesystem effects; the shared non-fabricable
   capability and private executor under proposed ADR-0222. The empty-occurrence
   purge (item 1) is done as described above; non-empty inbound removal still
   refuses by design.
3. **RM-00 representative benchmark matrix**: reusable
   `scripts/perf_representative.py` exists; a 35K/50K/600K/30K data-shape
   smoke evidence exists at
   `tests/performance/fixture-verification-2026-11-27.md`. Still needed:
   a valid bounded media corpus, actual release-build production path
   execution, and the comparable baseline/final matrix of the 13 workloads in
   the original handoff table, with complete provenance (commit, fixture
   seed, hardware, PostgreSQL/app settings, sample counts, bypassed
   subsystems). Baseline worktree `../melodee-rs-perf-baseline-a0979cd`
   (detached at `a0979cd`) — verify before reuse. Fixture seed time is not
   import throughput; treat timeouts as censored.
4. **RM-06/07**: finish integrated bounded traversal and durable staging
   across platforms (native/non-Unicode names, directory add/remove, large
   directories, independent file/directory due scheduling, no-baseline and
   crash/restart fairness, ownership/lease fencing, quotas, scheduler bounds).
5. **RM-09**: concurrent imports, interrupted/resumed convergence, unchanged
   reimport, historical/transitive oracle equivalence, production-shaped plans
   showing proportional indexed discovery, final integrated regressions.
6. **RM-10/13/14**: complete pooled acquisition/hold instrumentation for all
   workload classes; mixed-load pool decision; long-lived Tantivy writer A/B;
   a real PostgreSQL-backed 600K rebuild measurement (elapsed, commits,
   memory, disk, snapshot age, catch-up, activation pause); rerun the
   late-lower-ID and old-generation consumer regressions on final sources.
7. **RM-11/12**: operation inventory and safe elimination of redundant
   reads/hashes/probes with integrity preserved (do not reuse the inspection
   probe on hash match; do not adopt the rejected shared-decode path as
   completion); process-wide admission coverage; measured codec matrix.
8. **RM-15/17/18/19**: representative interactive route measurements and
   evidenced optimizations; slow-client streaming; rate-limit contention
   decision; recommendation/play-trigger contention and shared-artist
   fairness; merge/split reverify at 1 000 and 10 000 Track releases.
9. **RM-20 closeout**: reconcile all 21 packages + 17 findings in
   `PERF-REMEDIATION-STATUS.md`; update `PERF-REMEDIATION-RESULTS.md`,
   `PERFORMANCE.md`, ADRs, public docs, requirement traceability; rehearse
   fresh/populated upgrades through the final migration and the runbook;
   final gates (`cargo fmt --all`, `make check-fast`, `make check`,
   `make check-nightly`, `python3 scripts/do-prechecks.py`); coherent final
   checkpoint without pushing or publishing.

## Evidence and housekeeping notes

- **Clean-tree check at start**: the previous agent found `git status --short`
  empty; if yours is not, someone/something changed the tree — inspect before
  acting, never reset or clean blindly.
- Original baseline worktree: `../melodee-rs-perf-baseline-a0979cd`, detached
  at `a0979cde…aa12`. Verify before reuse.
- Two invocation-owned A/B snapshot worktrees remain **inside** this repo:
  `.admin-ab-baseline-dd7f597b8c114e2d855a4a92cbb80899/tree` and
  `.admin-ab-candidate-a77c68b9bb8f4fb5a37670d74b54b65a/tree`. Both are
  registered git worktrees with **prunable gitdirs**; preserve needed
  evidence, then perform only exact, identity-verified cleanup (e.g.
  `git worktree remove` with the exact recorded paths, or `git worktree
  prune` after confirming the directories are generated artifacts). Do not
  blindly `git clean` or recursively delete a mutable path.
- Six exact, previously verified stage/bind worktrees were relocated without
  deletion to sibling `.melodee-perf-quarantine-7qs2xgmn`. Retained bind
  fixtures and private logs are intentionally quarantined because pathname
  replacement made automatic cleanup unsafe. Do not blindly remove them or
  expose raw logs; use recorded ownership/identity evidence and fail closed
  on uncertainty.
- The latest supported-filesystem fs-verity test passed 1/1 on an
  intermediate test-only snapshot; it used a one-UID private namespace,
  excludes newer changes, and is not production custody or final integrated
  safety.
- The ignored bind-alias test must **not** be run directly. Host-propagation,
  pathname replacement, cleanup and unbounded-output hazards were reviewed;
  isolated attempts failed before the Rust test at private proc-mount setup
  (last bounded diagnostic: `move_mount:EINVAL`). Parent namespaces were
  unchanged and fixtures retained. A diagnostic subagent turn and a separate
  attempt to resume it were flagged by the tool environment as a possible
  cybersecurity risk; neither produced implementation. Respect
  platform/security restrictions, do not attempt to evade a tool restriction
  or change host security configuration, and do not represent this test as
  passed. Record any genuine remaining external limitation precisely.
- Representative identity and admin evidence in `tests/performance/` are
  intermediate single-host samples, not the complete before/after workload.
  The admin A/B (`admin-stats-ab-representative-2026-11-27.md`) used warmed
  in-process calls masking only the validated volatile
  `database.totalBytes`.
- Existing output and claims can be stale; the ledger's chronological tail is
  the newest truth. Avoid citing old focused results for newly changed code.

## Hard definition of done

Do not report successful completion until all of the following are true:

- RM-00 through RM-20 and all 17 findings have individual evidence-backed
  dispositions, with no mandatory implementation left partial/unimplemented.
- Routine identity reconciliation follows affected relationships; historical,
  concurrent, interrupted and late-commit cases are correct.
- Substantive media read/hash/probe amplification is addressed with integrity
  preserved, and shared resource admission is implemented and tested.
- Database, search, interactive APIs, streaming, authentication,
  recommendations and merge/split have the required measurements and justified
  changes or evidence-backed conditional decisions.
- All mandatory correctness regressions pass; comparable representative
  560K–600K **production-path** before/after measurements exist.
- Fresh/populated upgrades, existing-data repair, interruption/recovery and
  the complete upgrade/repair runbook are verified; required final quality
  gates pass on the final integrated source (`cargo fmt --all`, `make
  check-fast`, `make check`, `make check-nightly`,
  `python3 scripts/do-prechecks.py`, docs-site checks where affected).
- `PERF-REMEDIATION-STATUS.md`, `PERF-REMEDIATION-RESULTS.md`,
  `PERFORMANCE.md`, applicable ADRs, public docs, benchmark harnesses and
  sanitized evidence all agree with actual code and results. Record final
  commit/source identities, operator steps, limitations, documentation impact
  and any genuine external blocker precisely. Produce a coherent final
  integrated checkpoint without pushing, deploying or publishing.

## Recovery checkpoint (2026-11-27, second restart — supersedes the state above)

**Git state at this checkpoint:** branch `main`, HEAD `1b7ee81` (three
documentation/benchmark commits on top of `554e9d5`: `9889896` matched
rebuild/policy measurements and policy-benchmark scoping, `68e371e` matched
media-normalization matrix, `1b7ee81` recommendation-contention evidence and
merge-benchmark drift record). Working tree clean; no background builds. The
nightly/policy/media/identity measurement logs are in the gitignored
`.local-dev/` directory; the sanitized JSON evidence and build records are
committed under `tests/performance/`.

**Completed and verified in this restart (exact evidence):**

1. **`make check-nightly` completed green on `554e9d5` (exit 0)** — the first
   full green nightly of the operation. Log `.local-dev/check-nightly-20261127-113614.log`:
   619 Python tests, Admin UI gate, fmt, strict workspace clippy, workspace
   lib/bin tests, database-free integration crates, 208 PostgreSQL-backed
   integration binaries (2,475 tests passed, 0 failed), six ignored historical
   migration tests 6/6, `cargo deny` all-ok.
2. **RM-00 progressed with three matched baseline/final production-path pairs
   plus two final-only measurements** at the representative 560K–600K tier.
   Full provenance, commands and limitations:
   `tests/performance/final-matrix-2026-11-27.md`.
   - Full search rebuild: baseline 42.32 s/16,188 docs/s/217.5 MiB vs final
     35.21 s/19,454 docs/s (69 commits median 64.68 ms, activation 6.60 ms), 279.1 MiB.
   - Policy single evaluate: 1,203–1,279 ms, 600,014 rows, 600,008 retained
     digests (38.4 MB) vs 8.2–9.7 ms, 7 rows, 1 digest. Bounded 8-Release
     dry-run: 15.8–16.4 s, 4,800,057 digests (307 MB) vs 374.7–396.4 ms, 1 digest.
   - Media normalization (4 codecs × 1/2/4 workers × 2 samples): final is
     9–12 % faster at 1–2 workers and 9–12 % slower at 4 workers than baseline,
     with four-worker peak RSS 60–93 MiB lower; an earlier unsupervised
     intermediate reached 6.65 MP3 releases/s at 4 workers.
   - Identity reconciliation final-only (no baseline harness): scoped/global
     6.53/59.72, 15.78/2,963.83, 6.32/1,365.02, 2.24/17.90, 5.68/718.75 ms;
     drain 3,188 ms over 8 seeds; closure quiescent in 32.1 ms.
   - `perf_recommendation_contention` final-source pass:
     `recommendation_consistency=exact`, 3 scenarios, 1 round, 8 workers.
3. **Two real benchmark defects fixed with regression coverage** (no production
   code changed): the policy benchmark no longer assumes its own Releases are
   the earliest `catalog_imports` rows (it discovers the bounded scope through
   the production loader), and `scripts/perf_representative.py` gained
   `--without-acl-placeholders` (Python regression in
   `tests/python/test_perf_representative.py`). Verified with
   `cargo fmt --all`, strict workspace clippy, the perf Python suite (25 tests),
   the policy benchmark at 20-Release smoke and 600K, and **`make check` (full
   pre-merge gate) exit 0 on `1b7ee81`**.
4. **Newly discovered, unfixed:** `perf_large_release_merge` aborts before
   timing because its pre-timing selection needs Tracks without
   `observed_artists`/`track_artists` rows and the fixture now seeds credits for
   essentially all Tracks (601,002 credits / 600,000 Tracks). Fix the benchmark
   fixture (dedicated source/target Releases or move credits with the Tracks)
   and rerun 1K/10K; RM-19 has no final-source measurement.

**Ordered next actions from here:**

1. Fix and rerun `perf_large_release_merge` (1K and 10K) — see item 4. Then run
   the remaining nine RM-00 workload rows on the final source and collect
   matching baseline pairs where a baseline harness is possible: single-album
   import, 200-album import, unchanged reimport, concurrent genre imports,
   incremental indexing, concurrent interactive load, streaming during
   ingestion, worker scheduling, merge/split.
2. Custody P0 items 2–8 from "Immediate next actions" above remain entirely
   open (guarded owner with production stage readers/writers, protected stage
   protocol publication, production-reachable retained-source obligations,
   per-entry intent/quarantine fault cases and two-name recovery, deletion/
   repair orchestration with durable pre-filesystem reservations, shared
   capability under proposed ADR-0222). ADR-0221/0222 remain Proposed.
3. The partial packages in the ledger table (RM-06, RM-07, RM-09–RM-15,
   RM-17–RM-20) remain mandatory. Rebuild the CLI proof record after every
   source change (`scripts/build_perf_cli.py`): the fixture validates
   `commit == HEAD` and the workspace source fingerprint, so a commit or an
   `apps/crates` edit invalidates the previous record.
4. Finish RM-20 reconciliation, public doc updates, upgrade/repair rehearsal and
   the final `make check-nightly` / `do-prechecks.py` run on the then-final
   source.

**Measurement hygiene notes for the next agent:** fixture seed time is 178–186 s
on the current schema and 693–704 s on the pre-remediation schema — never
import throughput. Every measured case used a fresh UUID-owned database, one
workload at a time, release builds, `CARGO_BUILD_JOBS=2`,
`RUST_TEST_THREADS=1`, uncontrolled post-seed cache, and no competing Cargo
process. Existing intermediate artifacts for the nine remaining rows must not be
relabelled as final-source before/after evidence.

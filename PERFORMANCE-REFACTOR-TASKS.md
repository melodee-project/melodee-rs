# Performance Refactor — Pending Work and Status

**Purpose.** This is the single, task-oriented inventory of what remains before the
performance remediation program (`PERF-REMEDIATION-SPEC.md`) can be declared
complete under the hard definition of done in `OUTSTANDING-GPT.md`. It is a
*work list*, not the evidence ledger: `PERF-REMEDIATION-STATUS.md` remains the
authoritative chronological ledger, `PERF-REMEDIATION-RESULTS.md` the results
record, and `tests/performance/final-matrix-2026-11-27.md` the workload matrix.

**Baseline for this document:** `main` at `1228d09` (code state `5e4fae1`),
clean worktree. `make check-fast`, `make check` and `make check-nightly` pass on
this line of work; the last nightly log is
`.local-dev/check-nightly-green-20261127-174450.log` (2,475 tests, 0 failures,
`cargo deny` all-ok).

**Status legend:** `DONE` = implemented and verified with committed evidence;
`MEASURED` = all required measurements exist but acceptance/closeout is open;
`PARTIAL` = mandatory implementation or verification still missing; `BLOCKED` =
waiting on a documented decision or external approval (none currently).

**Do not claim, in any status document or report:** production custody
activation (ADR-0221/0222 are Proposed), that the bind-alias test passed, that
a final-only workload has before/after evidence, or that a draft/test-only
prototype is production behavior.

---

## 1. Status at a glance

| Package | State | Remaining work in one line |
|---|---|---|
| RM-00 | MEASURED (not accepted) | Disposition four final-only rows; declare the noise-aware regression budget. |
| RM-01 | DONE | None (reconfirmed in final verification). |
| RM-02 | DONE | None; matched 600K baseline/final evidence recorded. |
| RM-03 | DONE | None. |
| RM-04 | DONE | None; repair CLI and concurrency regression recorded. |
| RM-05 | DONE | None; activation/old-generation regressions recorded. |
| RM-06 | PARTIAL | Finish bounded traversal + durable staging integration and its cross-platform/fault tests. |
| RM-07 | PARTIAL | Reverify bounded retention and rolling refill against the current scheduler. |
| RM-08 | DONE | None. |
| RM-09 | PARTIAL | Complete mutation/concurrency audit, oracle/interruption equivalence, production plans. |
| RM-10 | PARTIAL | Finish acquire+hold instrumentation for every workload class; mixed-load admission decision. |
| RM-11 | PARTIAL | Complete operation inventory; remove remaining read/hash/probe amplification with integrity tests. |
| RM-12 | PARTIAL | Process-wide admission coverage, cancellation/fairness, evidence-backed defaults. |
| RM-13 | PARTIAL | Writer lifecycle A/B, reader/writer memory/concurrency evaluation, final decision. |
| RM-14 | PARTIAL | Snapshot-age, catch-up and concurrent-activation measurements; final regressions. |
| RM-15 | PARTIAL | Representative route/ACL/rating plans and measurements; justified optimizations. |
| RM-16 | DONE | None. |
| RM-17 | PARTIAL | Slow-client backpressure, mixed authentication load, rate-limit contention decision. |
| RM-18 | PARTIAL | Writer audit + shared-Artist fairness/contended correctness measurement. |
| RM-19 | PARTIAL | Reverify set-based merge/split state, projection and lock costs in the integrated build. |
| RM-20 | PARTIAL | Reconcile all packages/findings, rehearsal, docs, final gates, final report. |
| Custody P0 | PARTIAL | Items 2–8 of the ledger P0 list (section 3) — the largest remaining workstream. |

Terminal count: **7 of 21** packages (`RM-01..05`, `RM-08`, `RM-16`), plus
`RM-02`/`RM-19` measurement work already recorded.

---

## 2. Package work list

Each item lists what already exists, the mandatory remaining work, and the
acceptance evidence the package cannot close without. Spec sections are in
`PERF-REMEDIATION-SPEC.md`.

### RM-00 — Baseline, bounded metrics, permanent regression harness — MEASURED
- **Exists:** thirteen workload rows measured on the final source (four matched
  baseline/final pairs, five measured single/repeat runs, four final-only rows);
  disposable 35K/50K/600K fixture; provenance records; bounded metrics.
- **Pending:**
  - [ ] Record an explicit, evidence-backed disposition for the four final-only
    rows (identity reconciliation, incremental indexing, streaming during
    ingestion, merge/split) — either a documented limitation/waiver with the
    reason no baseline harness exists, or build the baseline harness. They must
    not be labelled before/after.
  - [ ] Declare the noise-aware regression budget **before** final testing
    (`PERFORMANCE.md` §8.10) and keep it unchanged afterwards.
  - [ ] Re-run the matrix rows whose committed evidence is single-sample at the
    final commit if any further code changes touch them.
- **Acceptance:** every row carries a stated evidence class and limitations; the
  regression budget is written down; no number is presented as a target or an
  estimate.
- **Evidence:** `tests/performance/final-matrix-2026-11-27.md`.

### RM-06 — Durable discovery progress and fairness — PARTIAL
- **Exists:** durable per-class cursors with pass completion/wraparound; 100K
  fairness regression; bounded scan/staging primitives; retained recheck.
- **Pending:**
  - [ ] Finish integrated bounded traversal and durable staging across supported
    platforms: native and non-Unicode names, directory add/remove, large
    directories, independent file/directory due scheduling.
  - [ ] Prove no-baseline and crash/restart fairness (checkpoint advances only
    after durable effects; replay idempotent; cursors cannot regress).
  - [ ] Ownership/lease fencing and quota/scheduler bounds under the integrated
    scheduler.
  - [ ] Independent design review + integrated PostgreSQL tests.
- **Acceptance:** every stable eligible item is visited within the configured
  sweep budget; bounded work per invocation; no prefix-only repetition or chart
  starvation; fault/restart tests.

### RM-07 — Bounded scheduler results and continuous worker refill — PARTIAL
- **Exists:** `MAX_RETAINED_SCHEDULER_RESULTS = 64` ring in
  `crates/melodee-ingestion/src/scheduler.rs`; periodic-source retention;
  regression tests for the bound.
- **Pending:**
  - [ ] Reverify bounded retention and rolling refill against the current
    discovery/scheduler code (inspection/normalization/validation/publication
    stages); confirm no batch barrier and no pre-claimed backlog behind permits.
  - [ ] Confirm empty/error backoff, per-lane weighting, heartbeat scheduling,
    cancellation/shutdown, and nested album/track concurrency sharing global
    budgets.
- **Acceptance:** evidence that available slots refill without waiting for the
  slowest item, retained result storage is bounded, and queue depth/lease
  behavior is bounded under saturation.
- **Tests:** scheduler unit tests, `worker_pools`, ingestion saturation tests.

### RM-09 — Durable affected-identity reconciliation — PARTIAL
- **Exists:** durable seeds committed atomically with the catalog write; scoped
  candidate discovery for all five algorithms; 600K scoped-vs-global evidence;
  identity-scoped oracle and hard-delete/merge regressions.
- **Pending:**
  - [ ] Complete the mutation/concurrency audit (every mutation that can change
    identity facts must seed the affected closure; no missing writer).
  - [ ] Historical/transitive closure and interrupted-session equivalence against
    the global oracle; late-commit and concurrent-import cases.
  - [ ] Production-shaped `EXPLAIN (ANALYZE, BUFFERS)` plans for every scoped
    query (present/absent/common/rare values) showing proportional discovery.
  - [ ] Final integrated regressions on the final tree.
- **Acceptance:** convergence equals the global oracle for historical,
  concurrent, interrupted and late-commit cases; ordinary import cost scales
  with the affected closure, not catalog size; no new global work on idempotent
  re-import.
- **Evidence/tools:** `perf_identity_reconciliation`,
  `python3 scripts/run_perf_identity_case.py`, `identity_scoped_oracle`,
  `catalog_hard_delete`.

### RM-10 — Shorter manifest transactions and measured DB admission — PARTIAL
- **Exists:** filesystem evidence prepared before the write transaction; fenced
  revalidation; checkout/hold instrumentation (opt-in trace); representative
  mixed-load pair.
- **Pending:**
  - [ ] Complete pooled acquisition **and hold** instrumentation for every
    workload class (ingestion, rebuild, search, HTTP, worker stages), with
    fixed-cardinality, bounded labels and no parameters/IDs/paths.
  - [ ] Run the mixed-load comparison and make the admission decision from that
    evidence; document the chosen configuration and rejected alternatives.
  - [ ] Verify no write transaction spans filesystem/subprocess work anywhere
    else, and that moved-out reads revalidate work-item state, lease and
    bindings.
- **Acceptance:** measured acquisition wait separated from SQL time for all
  classes; an evidence-backed admission/pool decision; no transaction holds a
  connection across blocking work.
- **Tools:** `perf_db_competition.py`, `scripts/summarize_perf_metrics.py`,
  `scripts/perf_claim…` targets.

### RM-11 — Eliminate redundant media work, preserve integrity — PARTIAL
- **Exists:** bounded tool admission; some duplicate work removed; unbound
  version-result caching disabled after the ABA finding; codec matrix (matched
  baseline/final).
- **Pending:**
  - [ ] Complete the media operation inventory (read/hash/probe/subprocess per
    stage, including cancellation cleanup).
  - [ ] Remove remaining redundant reads/hashes/probes **without** reusing the
    inspection probe on a hash match or adopting the rejected shared-decode path.
  - [ ] Integrity/fault tests: same-size content mutation, crypto evidence
    preserved, interrupted operations, no timestamp/inode/length substitution.
- **Acceptance:** measured reduction in reads/hashes/probes on real media with
  content integrity preserved and fault cases covered.

### RM-12 — Bounded blocking work, subprocess lifecycle, useful concurrency — PARTIAL
- **Exists:** shared ingestion admission (four-permit provisional bound); codec
  concurrency samples; subprocess timeout/termination/drain/reap; optional
  connection cap.
- **Pending:**
  - [ ] Process-wide admission coverage for all expensive blocking work
    (filesystem/hash, CPU media, subprocess, Tantivy open/commit), with permits
    held through cancellation cleanup.
  - [ ] Durable discovery/storage bounds; cancellation and fairness tests under
    saturation (not only at idle).
  - [ ] Evidence-backed defaults from the measured matrix; document acquisition
    order and absence of circular waits.
- **Acceptance:** no unbounded blocking queue; permits held until work actually
  stops; saturation/cancellation tests; measured concurrency defaults.

### RM-13 — Ordered search hydration and reusable readers/writers — PARTIAL
- **Exists:** ordered batched outbox hydration; bounded reader registry;
  lifecycle metrics.
- **Pending:**
  - [ ] Long-lived writer A/B with generation/lease reopen; reader/writer memory
    and concurrency evaluation; substantiate the final lifecycle decision.
  - [ ] Verify retired generations are not deleted while a live reader may use
    them, and multi-process ownership respects the projection lease.
- **Acceptance:** measured memory/concurrency behavior; a documented lifecycle
  decision backed by the A/B.

### RM-14 — Production search rebuild efficiency — PARTIAL
- **Exists:** admitted rebuild pages; commit bounds (10k docs / 2 s); matched
  baseline/final 600K rebuild; commit/activation/catch-up instrumentation.
- **Pending:**
  - [ ] Snapshot-age, cancellation, catch-up-under-load and
    concurrent-activation measurements (rebuild while writers/clients run).
  - [ ] Final regressions for late lower-id commits and old-generation
    acknowledgement replay on the final source.
- **Acceptance:** measured elapsed, commits, memory, disk, snapshot age,
  catch-up and activation pause with concurrent activity; correctness
  regressions green.

### RM-15 — Interactive query, aggregation, ACL, serialization — PARTIAL
- **Exists:** admin aggregate and OpenSubsonic rating/allocation improvements;
  `EXPLAIN` samples; admin-stats A/B; bounded mixed-load admin/browse
  percentiles.
- **Pending:**
  - [ ] Representative plans and release-build measurements for the remaining
    routes (ACL and rating queries, pagination/ordering, serialization).
  - [ ] Implement and justify any further optimization; otherwise record a
    NO-CHANGE-JUSTIFIED decision with the measurement.
- **Acceptance:** before/after or justified no-change evidence per route class;
  wire contracts, ordering, pagination and status codes unchanged; golden
  fixtures where a route changed.

### RM-17 — Streaming, auth lookups, blacklist, artwork — PARTIAL
- **Exists:** content-aware artwork cache with invalidation; blacklist
  singleflight; buffered streaming; combined auth lookup; streaming TTFB under
  ingestion.
- **Pending:**
  - [ ] Slow-client backpressure/resource behavior (no blocking thread held per
    slow client; per-user/global limits, disconnect, shutdown).
  - [ ] Realistic mixed authentication load; assess the observed rate-limit
    contention while preserving exact multi-process admission and the retained
    key ceiling.
  - [ ] Final compatibility verification (native/OpenSubsonic/Jellyfin streaming).
- **Acceptance:** measured slow-client behavior and auth load; exact rate-limit
  semantics retained; compatibility suites green.

### RM-18 — Batched evidence/identity writes and recommendation decision — PARTIAL
- **Exists:** batched evidence writes; recommendation lock-order changes;
  `perf_recommendation_contention` passes (`recommendation_consistency=exact`,
  3 scenarios, 8 workers).
- **Pending:**
  - [ ] Complete the writer audit (all evidence/identity writers batched and
    lock-ordered; no per-item loops).
  - [ ] Shared-Artist fairness and contended correctness measurement (multiple
    artists/releases contending on the same artist rows).
- **Acceptance:** audit closure + measured fairness with exact counters.

### RM-19 — Set-based release merge/split — PARTIAL
- **Exists:** set-based merge/split; `perf_large_release_merge` green at 1K
  (merge 3.56 s / split 6.86 s) and 10K (12.22 s / 9.57 s) with constant 36/24
  statements and full evidence round-trip; `catalog_merge_split` functional
  suite.
- **Pending:**
  - [ ] Reverify final implementation, projection/state correctness, and
    transaction/lock costs in the integrated build; document measured lock
    spans as sampled approximations.
- **Acceptance:** recorded reverification with correctness and cost evidence.

### RM-20 — Integration, standards, final handoff — PARTIAL
See section 5.

---

## 3. Custody P0 workstream (items 2–8)

This is safety-critical and cross-cutting; the ledger's P0 list is the source of
truth. **No production activation may be claimed while ADR-0221 and ADR-0222 are
Proposed.**

- [ ] **Guarded owner integrated with production stage readers/writers.** One
  claim-bound protected owner used end-to-end by readers/writers; generic
  acquisition/lease APIs reject protected-reserved work (stale same-worker
  reacquisition must produce no receipt and no source cleanup).
- [ ] **Protected stage protocol publication.** Implement the protected
  stage/attempt/marker/receipt path for production, including epoch-bearing APIs
  end-to-end and a shared lifecycle/attempt lock covering the filesystem
  boundary (a DB marker alone cannot revoke stale work that crossed generic
  acquisition). ADR-0221 stays Proposed until accepted.
- [ ] **Retained-source obligations production-reachable.** Make NULL
  baseline-ID obligations and the immutable-trigger interplay live and
  production-reachable, including the changed-source request + durable reason as
  one atomic transition and a resumable fenced discard state.
- [ ] **Per-entry intent/quarantine fault cases + two-name recovery.** Exact
  per-entry verified quarantine records for each media/artwork member; DB-bound
  interruption, replacement races, verified unlink/ack and crash recovery; safe
  recovery when two names exist; directory rows remain fail-closed until subtree
  proof exists.
- [ ] **Deletion/repair orchestration with durable pre-filesystem
  reservations.** Cover operator deletion/correction and catalog hard-delete
  paths (release/artist/track hard-delete, unpublished/published review
  deletion and correction) with the same lifecycle and retained-source
  interlocks; a durable reservation must precede filesystem effects and exclude
  protected claims; already-running writers need an enforceable shared lock or
  maintenance fence.
- [ ] **Shared non-fabricable capability / private executor (ADR-0222).**
  Constructors must acquire and validate the ordered locks and perform
  descriptor-relative verified work for the exact reservation/epoch/page; the
  private orchestrator consumes page/scope proofs while retaining held locks;
  public DB assertions alone cannot authorize deletion. Bind source
  root/path/work-item identity to the durable intent and recheck at the action
  boundary.
- [ ] **Operator-facing correctness:** non-empty inbound removal still refuses
  with `source_custody_required` by design; document that limitation until the
  protocol is activated.
- [ ] **Independent adversarial review** of each integrated slice; the many
  disconnected/test-only prototypes in the ledger are evidence of exploration,
  not authorization.

Refs: `design/adr/ADR-0221-…`, `design/adr/ADR-0222-…`, ledger tail,
`design/runbooks/PERF-REMEDIATION-UPGRADE.md`.

---

## 4. Cross-cutting obligations (block RM-20 acceptance)

- [ ] **Noise-aware regression budget** declared from the measured matrix before
  final testing (`PERFORMANCE.md` §8.10), then held constant.
- [ ] **Mandatory correctness regressions** re-run on the final integrated
  source: concurrent genre counts, late-lower-id search activation/old
  generation consumption, discovery coverage, identity closure/oracle, merge/
  split, podcast publication race, and the no-handoff purge.
- [ ] **Comparable 560K–600K production-path before/after** evidence re-checked
  against the final commit for any row touched by remaining code changes
  (the four final-only rows keep their documented class).
- [ ] **Upgrade/repair rehearsal** through the final migration (including the
  custody migrations 0070/0071): fresh install, populated upgrade with
  representative skew, interrupted upgrade recovery, and the derived-data
  repair path in read-only check and idempotent apply modes.
- [ ] **Runbook verification:** every operator step in
  `design/runbooks/PERF-REMEDIATION-UPGRADE.md` exercised; rollback/forward
  plan stated; maintenance-fence requirements documented.
- [ ] **Documentation and traceability:** reconcile `PERF-REMEDIATION-RESULTS.md`,
  `PERFORMANCE.md`, this file, the ledger table, affected ADR statuses, public
  `/docs`, the requirements index (`design/requirements/`), and the benchmark
  READMEs with the actual code/results; update requirement traceability for any
  package that becomes implemented/tested.
- [ ] **Final gates on the final integrated source:**
  `cargo fmt --all --check`, `make check-fast`, `make check`,
  `make check-nightly`, `python3 scripts/do-prechecks.py`, docs-site checks when
  `/docs` is affected, Admin UI gates/e2e when UI is affected.
- [ ] **Final report** per `PERF-REMEDIATION-SPEC.md` §11.5: files changed,
  requirements satisfied, ADRs applied/proposed, tests and counts, before/after
  evidence with provenance, compatibility/data/migration impact, documentation
  impact, remaining risks/limitations, final commit identities. No push,
  deployment or publication.

---

## 5. RM-20 closeout checklist

- [ ] Reconcile all **21 packages** and **17 findings** with individual,
  evidence-backed dispositions (no `PARTIAL` without an explicit
  justification/waiver).
- [ ] Reconcile the ledger table with the chronological tail; no contradictory
  READY/DEFER labels anywhere.
- [ ] Confirm the four final-only rows and the sampled metrics are described as
  such in every document.
- [ ] Record the declaration of the regression budget and the workload matrix's
  evidence classes in `PERFORMANCE.md`/`PERF-REMEDIATION-RESULTS.md`.
- [ ] Complete the upgrade/repair rehearsal and runbook verification (section 4).
- [ ] Run the final gates and attach exact log locations and counts.
- [ ] Produce the coherent final checkpoint without pushing/publishing.

---

## 6. Suggested execution order

1. **Custody P0** (section 3) — safety-critical, gates trustworthy claims about
   media/source deletion and protected publication. Independent review each
   slice; no activation until the ADRs are accepted.
2. **Code-completion packages:** RM-06, RM-11, RM-12, RM-19 reverification;
   then RM-09 audit/plans.
3. **Measurement/decision packages:** RM-07, RM-10, RM-13, RM-14, RM-15, RM-17,
   RM-18. Each ends in a recorded decision (change or justified no-change).
4. **RM-00 disposition + regression budget** (needed before final testing).
5. **RM-20**: docs/traceability, upgrade/repair rehearsal, final gates, final
   report.

Run `make check-fast` during iteration and `make check` before each merge;
reserve `make check-nightly` for integrated checkpoints. Use one resource
coordinator: `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1`, one Cargo/database-heavy
workload at a time.

---

## 7. Operational rules and known limitations

**Rules that must not be relaxed**
- Never weaken/suppress tests or replace cryptographic content evidence with
  timestamps, inode or length.
- A draft, test-only or Proposed-ADR primitive is not production activation.
- Regenerate the migration CLI proof (`scripts/build_perf_cli.py`) after every
  commit or `apps/crates` edit: it binds `commit == HEAD` plus the workspace
  source fingerprint.
- Ingestion-driven benchmark workloads require `--without-acl-placeholders`
  (ACL publication placeholders create work items the workloads refuse).
- Baseline comparisons must be launched from the verified worktree
  `../melodee-rs-perf-baseline-a0979cd`; only benchmark harness/test files are
  added there.
- Do not remove the quarantined worktrees under
  `.melodee-perf-quarantine-7qs2xgmn` or the two A/B trees except by exact
  identity-verified cleanup.
- Do not run the ignored bind-alias test; it is not passed evidence.

**Known limitations to carry into the final report**
- Four RM-00 rows are final-only by harness limitation; worker-scheduling and
  lock/RSS figures are sampled; several cells are single-sample.
- Non-empty inbound removal refuses by design (`source_custody_required`) until
  the custody protocol is activated.
- The bind-alias/private-namespace diagnostic is blocked by the test
  environment; no host mount/security changes are authorized.
- No external credential/provider blocker applies to the pending performance
  work; ADR-0221/0222 acceptance is a design decision, not a measurement.

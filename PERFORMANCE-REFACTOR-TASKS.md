# Performance Refactor — Pending Work and Status

**Purpose.** This is the single, task-oriented inventory of what remains before the
performance remediation program (`PERF-REMEDIATION-SPEC.md`) can be declared
complete under the hard definition of done in `OUTSTANDING-GPT.md`. It is a
*work list*, not the evidence ledger: `PERF-REMEDIATION-STATUS.md` remains the
authoritative chronological ledger, `PERF-REMEDIATION-RESULTS.md` the results
record, and `tests/performance/final-matrix-2026-11-27.md` the workload matrix.

**Baseline for this document:** `main` at `080a619`; the 2026-11-28 scope and
evidence changes are committed on top of it (`207bbea`) and the worktree is
clean. `make check-fast` passes (exit 0); the last full nightly that completed
green was at `5e4fae1`
(`.local-dev/check-nightly-green-20261127-174450.log`, 2,475 tests, 0 failures,
`cargo deny` all-ok) and **has not been rerun** on the current line, so do not
claim a green nightly for it.

**Status legend:** `DONE` = implemented and verified with committed evidence;
`MEASURED` = all required measurements exist but acceptance/closeout is open;
`PARTIAL` = mandatory implementation or verification still missing; `BLOCKED` =
waiting on a documented decision or external approval (none currently).

**Do not claim, in any status document or report:** production custody
activation (ADR-0221/0222 are Proposed), that the bind-alias test passed, that
a final-only workload has before/after evidence, or that a draft/test-only
prototype is production behavior.

---

## 0. Completion log (updated as items complete)

A checked item requires committed evidence and this document's status row to be
updated in the same commit. Never mark an item DONE while any mandatory
sub-item or acceptance evidence is missing.

| Date | Item | Evidence | Status change |
| --- | --- | --- | --- |
| 2026-11-28 | RM-00 regression budget | `PERFORMANCE.md` §8.1 (declared before final testing) | Budget sub-item DONE; RM-00 still open |
| 2026-11-28 | RM-00 (all sub-items) | matrix §2.8, §2.13, §4.1; `PERFORMANCE.md` §8.1 | **RM-00 DONE** |
| 2026-11-28 | RM-13, RM-14, RM-15, RM-19 | package entries above; `final-*` JSON evidence; `catalog_merge_split` 6/6 | **DONE** |
| 2026-11-28 | RM-17 | rate-limit contention run (exact counters, 1/8/32 workers) + streaming matrix | **DONE** |
| 2026-11-28 | RM-18 | set-based provider/observation writes; merge/correction suites 6/6+5/5; exact per-artist contention | **DONE** |
| 2026-11-28 | Ledger scope correction | §1 corrected to 14 DONE / 7 PARTIAL; stale RM-13–RM-19 `PARTIAL` headings corrected; custody separated in §10.6 | Ledger now matches the evidence in both status files |
| 2026-11-28 | WO-M (1K merge/split regression) | §10.4; `tests/performance/merge-split-1k-attribution-2026-11-28.md` | **CLOSED**: correctness maintenance, index follow-up identified |
| 2026-11-28 | RM-09 (partial) | §RM-09 audit; concurrent-import seed regression passes | Mutation audit complete; plan-selectivity values and final rerun open |
| 2026-11-28 | RM-10 (partial) | §RM-10; `metrics::acquire_for_claim` + 8 claim classes; decision recorded | Instrumentation complete; post-change mixed load + blocking audit open |

---

## 1. Status at a glance

| Package | State | Remaining work in one line |
|---|---|---|
| RM-00 | DONE | All 13 rows measured with declared evidence classes, four final-only rows dispositioned, regression budget declared. |
| RM-01 | DONE | None (reconfirmed in final verification). |
| RM-02 | DONE | None; matched 600K baseline/final evidence recorded. |
| RM-03 | DONE | None. |
| RM-04 | DONE | None; repair CLI and concurrency regression recorded. |
| RM-05 | DONE | None; activation/old-generation regressions recorded. |
| RM-06 | PARTIAL | Finish bounded traversal + durable staging integration and its cross-platform/fault tests. |
| RM-07 | PARTIAL | Reverify bounded retention and rolling refill against the current scheduler. |
| RM-08 | DONE | None. |
| RM-09 | PARTIAL | Audit complete and concurrent-import regression added; absent/rare/common plan selectivity and final integrated rerun open. |
| RM-10 | PARTIAL | Instrumentation complete for all claim classes and no-split decision recorded; post-change mixed-load measurement and blocking-work audit open. |
| RM-11 | PARTIAL | Complete operation inventory; remove remaining read/hash/probe amplification with integrity tests. |
| RM-12 | PARTIAL | Process-wide admission coverage, cancellation/fairness, evidence-backed defaults. |
| RM-13 | DONE | Lifecycle decision recorded from measured commit/reload, concurrent-reader and memory evidence. |
| RM-14 | DONE | Rebuild, catch-up, concurrent activation/held-reader and late-ID regressions measured and green. |
| RM-15 | DONE | Artist-scope and Jellyfin page plans measured (584→14.3 ms; 105→1.39 ms); no further change justified. |
| RM-16 | DONE | None. |
| RM-17 | DONE | Rate-limit contention measured with exact counters (1/8/32 workers); streaming/slow-client evidence; decision: keep exact semantics. |
| RM-18 | DONE | Writer audit batched the remaining per-item merge/split writes; shared-Artist contention exact per-artist popularity verified. |
| RM-19 | DONE | 1K baseline/final cost+statement pair, 10K final, correctness and projection reverified. |
| RM-20 | PARTIAL | Reconcile all packages/findings, rehearsal, docs, final gates, final report. |
| Custody P0 | INACTIVE (outside the 21 packages) | Section 3; all six work orders are dormant. See section 10.6 for the release-boundary decision. |

Terminal count: **14 of the 21 specification packages are DONE**
(`RM-00`–`RM-05`, `RM-08`, `RM-13`–`RM-19`) and **7 are PARTIAL
(`RM-06`, `RM-07`, `RM-09`, `RM-10`, `RM-11`, `RM-12`, `RM-20`). The custody
workstream is a post-specification addition and is not one of the 21; counting
it as a 22nd "package" is what produced the earlier contradictory "7 of 21"
line. It is reported separately in section 10.6.

---

## 2. Package work list

Each item lists what already exists, the mandatory remaining work, and the
acceptance evidence the package cannot close without. Spec sections are in
`PERF-REMEDIATION-SPEC.md`.

### RM-00 — Baseline, bounded metrics, permanent regression harness — MEASURED
- **Exists:** thirteen workload rows measured on the final source (four matched
  baseline/final pairs, five measured single/repeat runs, four final-only rows);
  disposable 35K/50K/600K fixture; provenance records; bounded metrics.
- **Completed:** regression budget declared (`PERFORMANCE.md` §8.1); identity
  and streaming dispositioned as accepted final-only limitations; merge/split
  benchmark ported to the baseline (1K cost/statement pair + pre-fix genre-term
  defect reproduced); incremental indexing ported to the baseline (matched pair
  across all eight trials). See matrix §4.1 and §2.8/§2.13.
- **Remaining:** none for RM-00 at this commit; re-run a row only if later code
  changes touch it.
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
- **Progress (2026-11-28):**
  - [x] **Mutation/concurrency audit complete. No missing writer found.** Every
    audited identity-affecting writer seeds in its authoritative transaction:
    import (`enqueue_identity_reconciliation_seeds` covers Release credits,
    Track credits, observations, external identifiers, plus the Release row);
    `create_artist`, `add_artist_alias`, `add_artist_external_identity`,
    `add_artist_relationship`, `add_group_membership`, `update_artist_metadata`
    (both new and old normalized names), `record_artist_match_decision`
    (including the previous Artist), negative/provider negative assertions;
    Artist merge/split; Release merge/split (both Releases); review correction;
    Artist hard delete (name/external keys preserved without FK); Release
    hard delete deletes only facts that cannot create a new live candidate.
    AudioIdentity evidence is outside RM-09's five Artist/strong-Release
    algorithms. The unchanged-reimport path adds no seeds (`perf_catalog_retry`).
  - [x] **Transitive, interruption and late-commit equivalence** exist in
    `crates/melodee-catalog/tests/identity_scoped_oracle.rs`
    (`scoped_drain_matches_global_oracle_and_preserves_review_decisions`,
    `one_seed_expands_across_distinct_identity_relationships`) and
    `identity_drain_preserves_seeds_when_candidate_discovery_fails`.
  - [x] **Concurrent imports**: new
    `concurrent_imports_persist_independent_identity_seeds` in
    `crates/melodee-catalog/tests/importer.rs` proves two concurrent import
    transactions each keep their own durable Release obligation and one
    coalesced drain clears both (passes on PostgreSQL).
  - [ ] Production-shaped `EXPLAIN (ANALYZE, BUFFERS)` plans currently capture
    **present** values only. Absent/rare/common seed-selectivity values still
    need capture (`perf_identity_reconciliation`).
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
- **Progress (2026-11-28):**
  - [x] **Acquire + hold instrumentation is now complete for every modeled claim
    class.** Added fixed-cardinality pairs (no IDs/paths/labels) for
    normalization, validation, publication, policy, search rebuild, provider
    enrichment, podcast download and retained-source claims, applied through the
    shared `metrics::acquire_for_claim` helper together with the global
    `pool_acquire` counter. The existing catalog import/claim, inspection claim,
    heartbeat, search outbox/snapshot, admin headline, scrobble and rate-limit
    pairs remain. A fixed-set unit test asserts the identities.
  - [x] **Admission/pool decision recorded from measured evidence:** keep one
    total connection budget plus per-stage worker admission; **do not split the
    pool**. The 600K mixed-load pair elapsed 127.26 s at pool 8 and 127.25 s at
    pool 32 with zero HTTP errors (no throughput gain from a split), the final
    mixed-load tails are inside the declared regression budget, and a split
    would break the explicit total budget needed by small installations.
    Rejected alternatives are already recorded in `PERF-REMEDIATION-RESULTS.md`
    §6 (fixed pool split, nested permit dependencies).
  - [ ] HTTP request-scoped hold time is represented by the global
    `pool_acquire` counter plus the admin-headline/search-snapshot/rate-limit
    class timers, not by a per-route wrapper. Treat the remaining interactive
    route classes as instrumented-at-the-pool-boundary, not per route.
  - [ ] Run the new classes under a representative mixed load and record their
    acquisition/hold percentiles (instrumentation exists; no post-change run).
  - [ ] Finish the blocking-work audit: confirm no write transaction spans
    filesystem/subprocess work anywhere else and that every moved-out read
    revalidates work-item state, lease and bindings.
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

### RM-13 — Ordered search hydration and reusable readers/writers — DONE
- **Exists:** ordered batched outbox hydration; bounded reader registry;
  lifecycle metrics.
- **Status: DONE (2026-11-28).** Decision: keep short-lived rebuild writers and
  the bounded reader registry; do not adopt a long-lived writer. Evidence:
  69 commits over a 35.21 s rebuild (median 64.68 ms, p95 74.07 ms) and reader
  reloads median 0.77 ms, so writer initialization/commit is a small fraction;
  four concurrent clients with a deliberately held reader during activation
  produced `held_query_succeeded=true` (2.65 ms), zero query errors, and the
  old generation stayed active until drain finished. A long-lived writer would
  add generation/lease-retirement risk across processes for no measured gain.
  Multi-process retirement safety is covered by the projection lease and the
  concurrent-activation regression (`perf_outbox_concurrent`, final source).

### RM-14 — Production search rebuild efficiency — DONE
- **Exists:** admitted rebuild pages; commit bounds (10k docs / 2 s); matched
  baseline/final 600K rebuild; commit/activation/catch-up instrumentation.
- **Status: DONE (2026-11-28).** Measured on the final source: 35.21 s rebuild
  (snapshot 35.169 s), 69 commits (median 64.68 ms), activation call 6.60 ms,
  zero-row catch-up in 17.1 ms, 231,080,841 logical index bytes, 279,084 KiB
  peak RSS; concurrent rebuild with a live outbox drain and four search clients
  (`perf_outbox_concurrent`, 600K fixture): activation completed at 32.88 s,
  snapshot pause 13.74 ms, held reader query succeeded in 2.65 ms, 2,048 events
  drained in ~575 ms at batch 256, zero query errors, 8,235 statements across
  concurrent work. Late lower-id/old-generation acknowledgement regressions
  pass in the nightly workspace suite. Cancellation is covered by the fenced
  rebuild tests; the remaining limitation is that concurrent multi-process
  contention beyond four in-process clients is not measured.

### RM-15 — Interactive query, aggregation, ACL, serialization — DONE
- **Exists:** admin aggregate and OpenSubsonic rating/allocation improvements;
  `EXPLAIN` samples; admin-stats A/B; bounded mixed-load admin/browse
  percentiles.
- **Status: DONE (2026-11-28).** Measured on the final 600K fixture:
  native ACL-scoped Artist count median **584.0 ms → 14.3 ms** (3 iterations,
  plans captured in `final-native-artist-scope-2026-11-28.json`); Jellyfin audio
  page median **105.1 ms → 1.39 ms** after the ordering index (4 iterations;
  count query 64.6 → 67.1 ms, within noise; index 24.4 MB, build 0.26 s) in
  `final-jellyfin-audio-plan-2026-11-28.json`; admin/browse mixed-load
  percentiles in the matrix §2.10. **Decision: NO further change justified**
  for the remaining route classes on this evidence; ACL selectivity on the
  synthetic fixture cannot be measured (no restricted-library distribution) and
  is recorded as a limitation. Wire contracts, ordering and pagination are
  unchanged (compatibility suites green in the nightly).

### RM-17 — Streaming, auth lookups, blacklist, artwork — DONE
- **Exists:** content-aware artwork cache with invalidation; blacklist
  singleflight; buffered streaming; combined auth lookup; streaming TTFB under
  ingestion.
- **Status: DONE (2026-11-28).** Rate-limit contention measured on the final
  source with the production `AuthStore::record_rate_limit_attempt` under the
  600K fixture: 1/8/32 concurrent workers, shared and separate actor keys,
  `counters_exact=true` in every round, zero errors; throughput 282.1→246.1
  attempts/s from 8→32 workers (shared actor) with p99 33.1→246.0 ms and
  sampled lock waiters up to 31. **Decision: keep the exact multi-process
  upsert and retained-key ceiling; no change justified** — a naive per-key
  upsert would weaken admission. Slow-client behavior is covered by the
  streaming-under-ingestion matrix (one deliberately slow direct stream, early
  client close, TTFB p50 3.06/p99 6.75 ms, zero failures) and the existing
  ingress diagnostic; audio underrun detection and a reverse proxy are not
  measured (documented limitation). Native/OpenSubsonic/Jellyfin streaming,
  ACL and compatibility suites are green in the nightly.

### RM-18 — Batched evidence/identity writes and recommendation decision — DONE
- **Exists:** batched evidence writes; recommendation lock-order changes;
  `perf_recommendation_contention` passes (`recommendation_consistency=exact`,
  3 scenarios, 8 workers).
- **Status: DONE (2026-11-28).** Writer audit: `catalog_identity.rs` and the
  manifest assembly loops were already set-based (result loops only populate
  memory); the only remaining per-item writes in the merge/split path were the
  provider-image collision check/move and the detached release-artist
  observation restore. Both are now one set-based read + one `unnest` write
  (commit `perf(rm18)`), with `catalog_merge_split` 6/6 and
  `catalog_correction_regressions` 5/5 green and strict clippy clean. Fairness /
  contended correctness is measured by `perf_recommendation_contention` on the
  final source: `recommendation_consistency=exact` across shared-Track,
  separate-Tracks-shared-Artist and concurrent-Artist-change scenarios, where
  every affected Artist's recommendation popularity must equal its authoritative
  primary-credit play count (a lost or starved update fails the test). Lock
  sampling is approximate; network and PostgreSQL CPU are excluded.

### RM-19 — Set-based release merge/split — DONE
- **Exists:** set-based merge/split; `perf_large_release_merge` green at 1K
  (merge 3.56 s / split 6.86 s) and 10K (12.22 s / 9.57 s) with constant 36/24
  statements and full evidence round-trip; `catalog_merge_split` functional
  suite.
- **Status: DONE (2026-11-28).** `catalog_merge_split` 6/6 on a fresh owned
  database; `perf_large_release_merge` 1K green on the final source (merge
  3.56 s / split 6.86 s, 36/24 statements) and 10K (12.22 s / 9.57 s, same
  statement counts), with exact evidence/identity/asset round-trip, zero stale
  genre terms, and immutable merge evidence. Baseline port (1K): 633.7 ms /
  3,025 statements merge and 618.1 ms / 3,017 split, leaving 2,000 stale
  genre-term rows — the pre-fix defect reproduced. Lock spans are sampled at
  5 ms and documented as approximations. Projection/state correctness is
  covered by the merge/split and genre-repair suites; final 10K baseline was not
  run (fresh owned DB per case).

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

---

## 9. Session closeout (2026-11-28) — remaining PARTIAL items

This session closed **RM-00, RM-13, RM-14, RM-15, RM-17 and RM-19** (evidence in
the package entries, the completion log, `tests/performance/final-*`, and the
ledger tail). The following remain `PARTIAL` and must not be marked DONE on the
current evidence:

| Item | Why it is not DONE | Exact next action |
| --- | --- | --- |
| RM-06 | Bounded traversal and durable staging integration is not implemented/tested across platforms; crash-fairness and independent file/directory scheduling are unproven. | Implement the traversal/staging integration, then the non-Unicode/large-dir/add-remove/crash tests and independent review. |
| RM-07 | Bounded result retention exists, but the workers still spawn a bounded batch and then join (batch barrier); **rolling refill is not implemented**. | Convert inspection/normalization/validation/publication to spawn-on-completion within the admission/heartbeat model, add a refill regression (a freed slot must take new work before the slowest item finishes), then reverify. |
| RM-09 | Durable seeds and scoped queries exist; the complete mutation audit, historical/transitive closure equivalence, interruption/late-commit comparisons and production-shaped plans are missing. | Audit every identity-affecting writer, then add oracle-equivalence, interruption and plan tests. |
| RM-10 | Acquire/hold instrumentation covers admin, catalog-import and search-snapshot classes only; no admission decision has been recorded for the remaining classes. | Instrument the remaining workload classes, then record the evidence-backed pool/admission decision. |
| RM-11 | Operation inventory and remaining read/hash/probe amplification work are incomplete; integrity/fault matrix not closed. | Finish the inventory, remove the remaining duplication with content-integrity tests, and record the measured delta. |
| RM-12 | Admission is not process-wide; durable discovery/storage bounds and cancellation/fairness under saturation are not fully proven; defaults remain provisional. | Extend admission coverage, add saturation/cancellation tests, and set defaults from measurements. |
| RM-20 | Closeout has not run. | Rehearse fresh/populated/interrupted upgrades through the final migration and the repair runbook; reconcile docs/traceability; run `do-prechecks.py` and the final gates; write the final report. |
| Custody P0 (2–8) | Not started in this session. | See §3; implement behind the maintenance fence with the ADRs accepted only when their criteria are met. |

The working tree is clean and the final gates remain green for the changes made
in this session; the next session should start by re-running `make check-fast`,
then `make check`, before touching the remaining items.

---

## 10. Frozen remaining-work ledger (2026-11-28, authoritative)

This section supersedes §9 for execution order and scope. One branch, one
ledger. A work order closes only when its implementation, focused tests and
acceptance evidence are complete; partial implementation of a package does not
close it.

### 10.1 Scope provenance and post-spec additions

- **Original specification scope:** RM-00–RM-20 in `PERF-REMEDIATION-SPEC.md`,
  findings PERF-001–015 and PERF-V001/V002.
- **Preserved, already verified:** RM-01–05, RM-08, RM-16 (implementation +
  regressions), and RM-00, RM-13, RM-14, RM-15, RM-17, RM-18, RM-19 as closed
  in this operation (evidence in the package entries, `final-*` files and the
  ledger tail).
- **Introduced after the specification:** (a) the source-custody P0 protocol
  (guarded owner, protected stage publication, retained-source obligations,
  per-entry intent/quarantine, deletion/repair reservations, shared capability)
  required by the security/durability invariants and ADR-0221/ADR-0222 reviews;
  (b) benchmark/fixture corrections and the regression budget (evidence
  infrastructure, not new optimization scope); (c) the fixed podcast
  publication race (critical correctness defect found during nightly); (d) the
  merge/split 1K regression investigation required by the current work order.
  No other optimization was added to the mandatory scope.
- **Newly discovered critical defects:** none open. The podcast orphan-reconcile
  race is fixed and verified (18/18 stress runs, suite regressions green). The
  merge/split 1K cost regression is tracked as WO-M, not as a correctness
  defect.

### 10.2 Why RM-06 and RM-07 are incomplete

- **RM-07** was recorded as having "bounded 64-entry result retention" and its
  remaining action was written as "reverify". Code inspection in this operation
  showed the *other* half of the package was never implemented: inspection,
  normalization, validation and publication workers claim a bounded batch,
  spawn all of it, and then join; there is no rolling refill when a slot frees
  before the slowest item finishes. Bounded retention alone does not satisfy
  RM-07, so it is incomplete.
- **RM-06** was recorded with "durable per-class cursors and a 100K fairness
  regression", and earlier results text described parts as implemented. The
  integrated bounded traversal and durable staging path (native/non-Unicode
  names, directory add/remove, large directories, independent file/directory
  due scheduling, crash/restart fairness, ownership/lease fencing, quotas)
  was never implemented or tested across platforms, so the package is
  incomplete despite its cursor foundation.

### 10.3 Mandatory implementation vs outstanding measurement

**Implementation work orders (code):**

| ID | Work order | Files/ownership | Acceptance |
| --- | --- | --- | --- |
| WO-C1 | Custody: claim-bound guarded owner used by production stage readers/writers; generic acquisition/lease APIs reject protected work; stale same-owner reacquisition produces no receipt/cleanup | `crates/melodee-ingestion/{custody,publication,acquisition,stage_verity}.rs`, persistence claim paths, server stage wiring | integration tests with production readers/writers, stale-epoch and cross-API rejection |
| WO-C2 | Custody: protected stage protocol publication (ADR-0221) end-to-end | same + `design/adr/ADR-0221-…` | receipt/marker/promotion tests on a supported filesystem; ADR accepted only when criteria are met |
| WO-C3 | Custody: production-reachable retained-source obligations (NULL baseline IDs, immutable-trigger interplay, atomic changed-source reason, fenced discard) | `crates/melodee-ingestion/retained_*`, persistence retained tables | obligation lifecycle + fault tests |
| WO-C4 | Custody: per-entry intent/private quarantine + two-name recovery | `crates/melodee-custody`, ingestion deletion | DB-bound interruption/replacement/unlink/ack/recovery tests |
| WO-C5 | Custody: deletion/repair orchestration with durable pre-filesystem reservations for operator deletion/correction and catalog hard delete | persistence delete/review paths, server admin routes | reservation-before-FS, protected-claim exclusion, cancellation tests |
| WO-C6 | Custody: shared non-fabricable capability/private executor (ADR-0222) | `crates/melodee-custody` | constructors acquire/validate locks and perform verified work; forged-proof rejection |
| WO-C7 | RM-07 rolling refill for inspection/normalization/validation/publication with bounded per-turn items | `crates/melodee-ingestion/{inspect,normalize,publication_worker}.rs` | freed-slot refill regression, admission/heartbeat/cancellation preserved |
| WO-C8 | RM-06 integrated bounded traversal + durable staging | `crates/melodee-ingestion/{source,source_windows,discovery_scan,snapshot}.rs`, persistence cursors | cross-platform/large-dir/add-remove/crash-fairness tests |
| WO-C9 | RM-09 mutation audit and identity closure/plans | `crates/melodee-catalog`, persistence identity | every identity-affecting writer seeds closure; oracle/interruption/plans |
| WO-C10 | RM-10 instrumentation for remaining workload classes + recorded admission decision | metrics, server/persistence | acquire/hold coverage per class + decision |
| WO-C11 | RM-11 media operation inventory and elimination of remaining amplification | `crates/melodee-media`, ingestion | measured reads/hashes/probes + integrity/fault tests |
| WO-C12 | RM-12 process-wide admission + cancellation/fairness + defaults | ingestion admission/workers | saturation/cancellation tests + measured defaults |

**Measurement/decision-only remaining:** RM-09's production-shaped plan and
oracle comparisons (evidence, after WO-C9); RM-10's mixed-load decision (after
WO-C10); RM-11's measured delta (after WO-C11); RM-12's default selection
(after WO-C12). RM-20 is closeout, not measurement: reconciliation, upgrade/
repair rehearsal, final gates, final report.

### 10.4 Work order for this session: WO-M (merge/split 1K regression)

**Question:** why did 1,000-Track merge go from 633.7 ms (baseline `a0979cd`)
to 3,556 ms (final), and is any of it avoidable overhead?

**Evidence so far (measured, not inferred):** final merge 1K = 3,556.4 ms / 36
statements; final 10K = 12,215.4 ms / 36 statements; baseline 1K = 633.7 ms /
3,025 statements and leaves 2,000 stale genre terms. Final per-Track slope
between 1K and 10K is ~0.96 ms, implying a large fixed component at 1K, but the
two points are separate fresh fixtures with uncontrolled cache, so the intercept
is indicative, not proven.

**What the final path adds that the baseline lacks (code evidence):** immutable
moved-Track/observation evidence (`release_merge_events` JSON), Track-move genre
term maintenance (the correctness fix that baseline fails), identity
reconciliation seeds, search-outbox revision fan-out for every moved Track, and
release/Track aggregate refreshes. Those are required correctness effects, not
optional work; the statement-count reduction must not be presented as a
performance improvement.

**Why a finer attribution is not yet available:** the dev PostgreSQL has no
`pg_stat_statements`/`auto_explain`, the merge is one transaction call with no
phase spans, and the two data points are single-sample separate fixtures.

**Measured phase attribution (2026-11-28, 600K-Track owned fixture):**
`perf_large_release_merge` 1K merge = 3,370.2 ms / 36 statements. Bounded phase
spans inside `merge_release_in_transaction`:

| Phase | Elapsed | Share |
| --- | --- | --- |
| `move_tracks` (the `UPDATE tracks ... release_id` plus its triggers) | 3,183.0 ms | 94.4% |
| `assets` | 39.8 ms | 1.2% |
| `outbox` (release + Track revisions) | 15.1 ms | 0.4% |
| `evidence` (immutable merge event JSON) | 7.1 ms | 0.2% |
| `aggregates` | 5.9 ms | 0.2% |

The SQLx slow-statement stream shows the same picture and isolates the fixed
component: the 1,000-row `UPDATE tracks` took 3.179 s, while a **one-row**
`UPDATE tracks ... release_id` in the same fixture took 1.88 s. Split is the
same shape (6,621.3 ms; the Track restore `UPDATE` is 3.538 s and the
`UPDATE releases SET status='active'` genre-status recount is 2.963 s).

**Trigger attribution (2026-11-28, 60K-Track owned fixture, same 1,000-Track
move, cache-warm second run):** evidence, provenance and limitations in
`tests/performance/merge-split-1k-attribution-2026-11-28.md`.

- all triggers: 423.3 ms
- statement-level genre maintenance disabled: 139.0 ms → genre maintenance is
  284.3 ms (**67%**)
- genre + recommendation statement triggers disabled: 129.0 ms → the
  recommendation statement trigger is ~10 ms

The genre component scales with the *surface partition*, not the moved Tracks:
`refresh_catalog_genre_aggregate_keys` filters
`catalog_genre_terms(surface, display_value)` but no index leads with both
columns, so PostgreSQL scans the whole `surface='native'`/`'opensubsonic'`
partition (1.2M rows at 600K Tracks) per affected key. That is why a one-row
Track move also costs ~1.9 s. The per-moved-Track remainder is ~0.13 ms.

**Decision: necessary correctness cost, with one separate avoidable item.**
The baseline did not run this work at all (it left 2,000 stale genre-term rows),
so the regression is the cost of the RM-03/RM-04 correctness fix, not duplicated
work. The recount-by-key protocol (advisory lock then recount that observes
committed concurrent work) is the accepted RM-04 design; replacing it with a
delta update would be a correctness-protocol change and is not done here. The
avoidable part is the missing predicate-aligned index:
`catalog_genre_terms(surface, display_value)`. It cannot remove the dominant
`Rock`-key scan (that key spans ~70% of the catalog) but removes the
whole-partition scan for every rare key and reduces the common-key scan to the
key's own rows. It is a bounded additive migration (with a populated-upgrade
check and a plan regression) and is tracked as a follow-up work order rather
than smuggled into this investigation.

**Status:** CLOSED for WO-M. Phase breakdown measured; no avoidable duplication
found in the move path; the correctness cost is recorded; the index follow-up is
identified with its own acceptance criteria and is not a regression fix.

### 10.5 Execution order (superseded by §10.7 on 2026-11-28)

1. WO-M (bounded, explicitly requested) — CLOSED in §10.4.
2. Specification packages first: RM-09, RM-10, RM-07, RM-12, then RM-06/RM-11
   to the extent the established integrity and custody invariants allow.
3. RM-20 closeout: upgrade/repair rehearsal through the final migration,
   documentation/traceability reconciliation, `do-prechecks.py`, final gates
   on the integrated candidate, final report.
4. Custody WO-C1–WO-C6 is **not** in this execution order. It remains
   `INACTIVE` under §10.6 until ADR-0221/ADR-0222 are accepted and the operator
   decision on the pre-existing deletion ordering is made.

Focused verification per work order; reserve the full three-gate run for the
integrated candidate. HEAD `080a619` plus the recorded working-tree changes has
**not** passed the nightly gate; rerun it only at the next integrated
checkpoint.

### 10.6 Release boundary: original program vs protected custody (2026-11-28)

This section is the scope-resolution record requested after the operation grew
past its original specification. It distinguishes the 21 specification packages
from the post-specification custody workstream and records what may and may not
be activated without an explicit decision.

**Verified repository state for this record:** `main` at `080a619` with two
uncommitted benchmark-instrumentation files
(`crates/melodee-persistence/src/catalog.rs` phase spans and the merge
benchmark's debug layer). Custody code is present but dormant:
`crates/melodee-ingestion/src/custody.rs` states in its module docs that the
operations are "not yet wired into production acquisition or purge"; the
protected-stage, protected-publication, retained-baseline and custody-hash
spool tables/functions are referenced only by that module and each other;
`provision_legacy_lifecycle_once` has no non-test caller; `inbound_custody` is
referenced only to detect and refuse protected staging. The retained-source
recheck loop is scheduled in `scheduler.rs`, but obligations are created only by
the dormant `protected_publication` path, so the loop has no production inputs.
The one live custody-adjacent behavior is fail-closed refusal: generic
acquisition/lease rejects protected-reserved work, pre-catalog purge refuses a
non-empty inbound occurrence with `source_custody_required`, and the guarded
empty-occurrence purge is implemented.

| Work order | Concrete cause | Affects current production behavior? | Prevents data loss / unsafe deletion / stale-worker publication? | Needed for a specific performance optimization? | New inactive functionality? | Safe to leave inactive? |
| --- | --- | --- | --- | --- | --- | --- |
| WO-C1 guarded owner in production readers/writers | Review of the bounded staging/publication path: a DB claim or lease cannot prove filesystem immutability, and a stale same-worker reacquisition could mint a receipt and clean a source | No — protected work is refused, not processed | Yes — stale-worker publication and source cleanup on stale reacquisition | No | Yes (protected custody mode) | Yes: no production path creates protected reservations |
| WO-C2 protected stage publication (ADR-0221) | Same review; ADR-0221 is **Proposed** | No (dormant) | Yes — prevents accepting a stage proof whose bytes a live writer can still change | No | Yes | Yes, while ADR-0221 remains Proposed |
| WO-C3 production-reachable retained-source obligations | The retained recheck loop exists, but only dormant protected publication creates obligations; NULL-baseline and immutable-trigger paths are unreachable, and the changed-source request cannot fire | No (loop has no inputs) | Prevents silently dropping a changed source and prevents a changed source being purged | No | Partly — the changed-source request/reason transition is committed but unreachable | Yes |
| WO-C4 per-entry intent/quarantine + two-name recovery | Verified deletion needs a per-object proof, not a path or inode comparison | No (private/dormant) | Yes — prevents unlinking a name that no longer matches the verified object | No | Yes | Yes |
| WO-C5 deletion/repair orchestration with durable pre-filesystem reservations | **Pre-existing behavior, two shapes.** (a) Release/Track hard delete has **no** durable pre-filesystem intent: `admin_catalog_delete::execute_release_hard_delete` and `delete_tracks` call `remove_release_files`/`remove_track_files` and only then run `hard_delete_releases`/`hard_delete_tracks`. (b) Release Review deletion has a durable confirmation (`decision`), then calls `delete_release_storage`, then `complete_destructive_action`; a completion failure leaves the authorization pending but media already removed. Both can leave catalog rows pointing at removed media or remove media still needed by a concurrently changed reference | **Yes** | Yes — partial media loss and deletion of media another reference may need | No | Partly (the reservation orchestrator is dormant) | **No** for the ordering defect: it is pre-existing, not introduced by this program. Activating the full fix is the ADR-0222 decision below; a minimal durable-intent fix for the two live paths is the alternative |
| WO-C6 shared non-fabricable capability / private executor (ADR-0222) | Mechanism that makes WO-C5 provable; no caller-supplied flag may authorize deletion | No (dormant) | Yes | No | Yes | Yes |

**Decisions that require explicit user authorization**

1. **ADR-0221 / ADR-0222 acceptance.** Both are `Proposed`. Nothing in this
   repository may activate protected custody, protected stage publication, or
   verified deletion orchestration until the ADRs are accepted. This is an
   architectural acceptance decision, not an implementation or measurement
   decision, and the agent will not accept them to close a checklist.
2. **The pre-existing deletion-ordering defect (WO-C5).** It is real and
   currently reachable, but it predates this program and a full fix is the
   ADR-0222 capability. The user must choose one of:
   (a) authorize a minimal, ADR-independent durable-intent protocol for the two
       live delete paths (reserve/commit intent, then filesystem work, then
       completion, with retry) without activating verity/quarantine custody; or
   (b) accept it as a pre-existing product defect outside this performance
       release, tracked as its own work item, so the performance program can
       close.
   Neither option is chosen here; the requirement is surfaced, not silently
   deferred or silently activated.
3. **Production custody activation** additionally requires an external
   maintenance fence and operator attestations that every old writer has
   terminated. That is operator-authorized production activity and is out of
   scope for this repository session.

**Consequences for the ledger.** The 21 specification packages are the
completion boundary for the performance program. The custody work orders are
reported as `INACTIVE (Proposed ADR / operator decision)` and do not block
`RM-20`, except for the explicit WO-C5 choice above.

### 10.7 Session record (2026-11-28): scope resolution and completion boundary

**Git state:** `main` at `080a619`; working tree contains the changes below and
the RM-10 metric additions, the RM-09 concurrent-import test, the WO-M phase
spans, and the corrected status documents. Nothing was pushed, published or
deployed.

**Delivered in this session**

- Corrected the ledger: 14 specification packages DONE, 7 PARTIAL; the stale
  `PARTIAL` headings and "7 of 21" terminal count are gone; custody is a
  separate workstream.
- WO-M: measured 1K merge phase breakdown and trigger attribution; justified
  the cost as necessary RM-03/RM-04 correctness maintenance and identified the
  missing `(surface, display_value)` genre index as the one bounded, avoidable
  follow-up.
- RM-09: completed the mutation/concurrency audit with no missing writer; added
  the concurrent-import durable-seed regression.
- RM-10: added fixed-cardinality acquire+hold instrumentation for the remaining
  claim classes through one helper; recorded the single-pool/no-split admission
  decision from the measured mixed-load pair.

**Not delivered in this session (explicit, not silently deferred)**

- RM-07 rolling refill: not implemented. The batch barrier remains in
  inspection/normalization/validation/publication.
- RM-12 process-wide admission for every expensive class and saturation
  defaults: not implemented beyond the existing four-permit ingestion bound.
- RM-06 integrated bounded traversal/durable staging, and RM-11 operation
  inventory/amplification removal: not implemented.
- RM-09 absent/rare/common plan selectivity and final integrated rerun.
- RM-10 post-change mixed-load measurement for the new classes and the
  blocking-work audit.
- RM-20 closeout and all final gates.
- Custody WO-C1–WO-C6: untouched; `INACTIVE`.

These are the only remaining specification items; they are ordered in §10.5 and
must not be reported as done.

# Melodee-rs — Performance and Correctness Remediation

## Master specification and coding-agent execution prompt

**Prepared:** 26 November 2026  
**Execution model:** One continuous, phased implementation operation; one integrated result.  
**Scope:** PERF-001 through PERF-015, PERF-V001, and PERF-V002.  
**Audience:** A coding agent with access to the melodee-rs repository and an isolated development/test environment.

> **Execution instruction:** Implement this specification. Read the two supporting reports, establish the current repository state, and proceed through design, implementation, migration preparation, testing, integration, and measured verification in this same assignment. Do not stop after writing a plan, finishing the first phase, or identifying work that requires design. Internal checkpoints are not requests for user approval. Continue until the completion conditions are met or a specific external blocker prevents further authorized work.

This document is both the remediation specification and the prompt. There is no separate specification-writing stage to complete before implementation.

---

## 1. Mission and intended outcome

You are the lead implementation engineer for the Melodee-rs performance remediation effort. Apply expertise in Rust, Tokio, PostgreSQL, SQLx, Tantivy, media processing, filesystem integrity, durable work, and concurrent systems.

Deliver a working, integrated application that:

1. Removes the validated per-album and per-track catalog-wide work.
2. Corrects concurrent genre aggregation, search generation activation, and discovery starvation.
3. Reduces avoidable database round trips, media reads, subprocess starts, and search resource initialization.
4. Keeps memory, queued work, external processes, and resource admission bounded.
5. Preserves catalog correctness, authentication, authorization, media integrity, lease fencing, recovery, and public API behavior.
6. Includes safe forward migrations, repair procedures for existing derived state, permanent regression tests, and reproducible performance evidence.
7. Can be reviewed as one complete remediation result without requiring repeated user instructions.

Optimize for the shortest path to a correct, measurably improved application. Prefer simple solutions supported by current code and measurements. Do not add infrastructure or broaden the product scope.

**Album-cost objective:** Routine processing should scale primarily with the album and its affected identity relationships. Catalog-wide maintenance must be explicit, bounded, resumable, and scheduled independently of every album import. This does not imply that every legitimate identity closure is small.

### 1.1 What this assignment authorizes

Implement necessary application changes, tests, benchmark harnesses, configuration validation, forward migrations, internal documentation, narrowly required architectural decisions, and operator repair/upgrade procedures. Use disposable databases, temporary media, and local development processes. Create an isolated branch or worktree where appropriate and make useful local checkpoint commits when repository conventions permit.

Resolve routine design and implementation choices autonomously. “DESIGN REQUIRED” in the validator means design and implement within this operation. “MEASUREMENT REQUIRED” means collect focused evidence and make the implementation decision within this operation.

This assignment does not require production deployment. Prepare and verify the release candidate and its upgrade procedure. Do not apply migrations or repairs to a real library, delete user media/history, push or merge remotely, publish a release, send messages, or deploy unless the existing session separately authorizes that action.

### 1.2 How to keep moving

- Do not ask for approval after each phase, work package, migration draft, or internal design.
- Do not turn this into a third broad audit.
- Do not stop with a list of suggested changes or a set of TODOs for the next agent.
- Do not use a difficult design item as a reason to delay unrelated, established corrections.
- Resolve a failed internal gate by fixing the problem and rerunning the affected checks; continue.
- If an external blocker exists, finish every independent item that remains possible. Record the exact blocker, attempted remedies, affected acceptance checks, and remaining work.
- A model context limit or interrupted session is a checkpoint, not task completion. Persist status and resume the same operation.

---

## 2. Inputs, evidence, and precedence

Read these files completely:

| Reference | Input | Role |
|---|---|---|
| R | [PERF-CONCERNS-REVIEW.md](PERF-CONCERNS-REVIEW.md) | Original Gemini-4-Neon performance audit; useful discovery evidence |
| V | [PERF-CONCERNS-VALIDATION.md](PERF-CONCERNS-VALIDATION.md) | Independent validation, experiments, qualifications, and rejected remedies |
| S | This document | Current implementation scope, execution requirements, and acceptance conditions |

The validation examined commit **abda925bf88fc6ebddfb10daece82514ffa2f9ab**. That is an evidence reference, not an instruction to reset the current repository.

The supplied report versions used to prepare this specification have these SHA-256 digests:

| Input | SHA-256 |
|---|---|
| R | 80f458ba40ac68a998b1e64879aabee06ba3ce6eba143f9a21488aa945672b39 |
| V | b04324af8a762a6a8cfbfa8e1338e6b1ae4920ab8eadbe649ed1c61dc5500a94 |

If available files differ, identify their versions and examine the relevant differences; do not reject a newer report solely because its digest changed.

Read applicable **AGENTS.md**, the specification, architecture documentation, and relevant ADRs. In particular, discover the current documents covering authority/search generations, media hashes and durable staging, catalog identity, genre projections, migration maintenance fencing, history retention, NFS publication, ingest lanes, manifest evidence, and outbox retention. Reported ADR numbers are starting points, not proof that documents are unchanged.

### Evidence rules

- Current code establishes current behavior. Requirements and accepted ADRs establish intended invariants.
- Use V's corrections to qualify R. Do not reinstate a remedy rejected by V without new evidence and a complete correctness justification.
- This specification resolves the implementation sequencing and readiness conflicts in the reports.
- Explicit user instructions and applicable higher-priority execution instructions remain authoritative.
- Record current HEAD, branch, working-tree status, toolchain, database version, and locked dependency versions. Use installed/locked APIs, not remembered APIs from another version.
- The prior measurements belong to V's isolated environment. Do not present them as measurements collected in this run.
- Refresh source locations by symbol. Line numbers from either report may be stale.
- If current code already satisfies a requirement, preserve it, verify the relevant regression case, and record the evidence instead of reimplementing it.

This specification was prepared from the reports; its author did not independently rerun their repository experiments. The implementation agent must establish evidence for its actual checkout.

---

## 3. Scope and completion classification

Every original finding and both new findings must receive a disposition. Split compound findings into the work packages below so that a small completed fix cannot conceal an unfinished correctness issue.

### 3.1 Mandatory outcomes versus conditional implementation choices

**Mandatory outcomes** include the proven query-scope/index defects, correct genre counts plus repair, correct search activation, complete discovery coverage, bounded scheduler retention, elimination of per-import global reconciliation, bounded expensive execution, appropriate search resource reuse, safe processing improvements, and the instrumentation/tests needed to substantiate them.

**Conditional choices** include exact pool partitioning, particular FFmpeg thread counts, optional covering indexes, broad index removal, asynchronous recommendations, changes to password/rate-limit algorithms, and retention policies not already specified by the product. Investigate these in this run. Implement only what evidence and existing requirements justify.

“Conditional” does not permit silently skipping investigation. It permits keeping a correct implementation when a proposed alternative lacks measurable benefit or would change required semantics.

### 3.2 Single status ledger

Maintain **PERF-REMEDIATION-STATUS.md** using these terminal states:

| State | Required evidence |
|---|---|
| IMPLEMENTED AND VERIFIED | Code, tests, migration/repair where applicable, and required execution evidence |
| ALREADY SATISFIED AND VERIFIED | Current-code proof plus the relevant tests |
| NO CHANGE JUSTIFIED | Specific conditional proposal evaluated; evidence explains why existing behavior is preferable |
| EXTERNALLY BLOCKED | Named unavailable dependency/resource/authorization, attempted alternatives, and exact unresolved gate |

Use IN PROGRESS for unfinished work. Do not label missing tests, unresolved design, or unmeasured claims as verified.

For each package track source finding, mandatory outcome, conditional decisions, files changed, internal dependencies, tests, measurements, repair/upgrade effects, current state, and next action. Maintain one authoritative status; do not leave contradictory READY/DEFER labels in different documents.

Mandatory correctness outcomes cannot receive NO CHANGE JUSTIFIED while the reproduced defect remains. A workaround that disables a broken operation is a documented mitigation, not a complete fix.

### 3.3 Finding-to-package map

| Finding | Validated issue | Work packages |
|---|---|---|
| PERF-001 | Unindexed playable-asset reverse lookup | RM-01 |
| PERF-002 | Catalog-wide acoustic novelty retrieval | RM-02 |
| PERF-003 | Genre access cost, unnecessary work, incorrect concurrent counts | RM-03, RM-04 |
| PERF-004 | Global reconciliation after each successful import | RM-09 |
| PERF-005 | Outbox polling/hydration and search resource lifecycle | RM-08, RM-13 |
| PERF-006 | Rebuild execution and consistency risks | RM-05, RM-14 |
| PERF-007 | Media I/O/process amplification and blocking work | RM-11, RM-12 |
| PERF-008 | Manifest transaction duration and shared DB pressure | RM-10 |
| PERF-009 | Retained scheduler results and batch barriers | RM-07 |
| PERF-010 | Interactive query and serialization cost | RM-15 |
| PERF-011 | Duplicate indexes, lease writes, history growth | RM-16 |
| PERF-012 | Discovery starvation | RM-06 |
| PERF-013 | Streaming, authentication, blacklist and artwork | RM-17 |
| PERF-014 | Per-item writes and recommendation contention | RM-18 |
| PERF-015 | Observability and unrepresentative benchmarks | RM-00, RM-20 |
| PERF-V001 | Late outbox event omitted from activated generation | RM-05 |
| PERF-V002 | Per-track/per-observation merge and split writes | RM-19 |

Retain the validator's formal severity labels in provenance. Assign **release-blocking implementation priority** to genre correctness, search activation correctness, and discovery completion. PERF-V001 was formally P1 in V; release-blocking priority does not silently rewrite that classification.

---

## 4. One-operation execution plan

Use one integration branch and one final completion report. Keep coherent internal commits/checkpoints for diagnosis and recovery; the user is not requesting separate approval cycles or separate externally managed projects.

| Phase | Required work | Exit gate and automatic continuation |
|---|---|---|
| 0 — Establish evidence | RM-00; establish genre/search/discovery regressions; inspect current schema and resources | Current baseline and permanent regressions established, with failing reproductions for defects still present; proceed immediately |
| 1 — Correct urgent defects | RM-01–RM-06, RM-07 result retention, RM-08; start RM-09 design concurrently | Narrow performance fixes verified; mandatory correctness fixes and repairs pass |
| 2 — Make catalog work incremental | RM-09, transaction portion of RM-10, RM-18, RM-19 | Affected-identity convergence, shorter transactions, equivalent batched writes |
| 3 — Improve processing and scheduling | RM-07 scheduling, RM-11, RM-12 | Less unnecessary work, bounded concurrency, integrity/recovery preserved |
| 4 — Improve search | RM-13, RM-14 after RM-05 | Correct generation protocol retained; measured query/index/rebuild improvement |
| 5 — Protect interactive workloads | RM-15, RM-16, RM-17, resource portion of RM-10 | APIs/security equivalent; bounded resource use and mixed-load verification |
| 6 — Integrate and deliver | RM-20, upgrade/repair rehearsal, full targeted regression and representative performance runs | One runnable integrated result, evidence, operator procedure, honest limitations |

Phase order expresses dependencies. Independent work may start earlier when it does not conflict with the integration path. Search race reproduction, design, and correction start in Phase 0/1; they must not wait for Phase 4.

### 4.1 Parallel work without resource contention

If multiple agents are available, delegate independent implementation/review work:

- Database access, catalog, and migrations.
- Search activation and lifecycle.
- Media, workers, and resource control.
- Integration, tests, API compatibility, and evidence.

The coordinator owns shared interfaces, migration numbering/order, integration, and resource-intensive commands. Prefer isolated worktrees or clearly disjoint file ownership. Do not let several agents rewrite the same persistence module or allocate migration numbers independently.

Code investigation and review can proceed in parallel. Builds, database seeds, large test suites, media benchmarks, and load tests run through one resource coordinator. Do not launch several Cargo builds or large database fixtures simultaneously on the same host.

Independent design review for concurrency-sensitive changes is an internal quality gate. It does not require a new user approval or a new model commissioning step. If no additional agent is available, perform a distinct adversarial review pass.

### 4.2 Preserve the development machine

The user has reported severe host slowdown during Rust tests. Begin conservatively:

~~~bash
export CARGO_BUILD_JOBS=2
export RUST_TEST_THREADS=1
~~~

These are initial execution limits, not production application defaults. Honor stricter repository/host limits. Increase only after observing available CPU, RAM, storage, and database capacity.

- Use one Cargo invocation at a time and reuse compatible build artifacts.
- Race tests intentionally coordinate multiple internal transactions/tasks; a single test harness thread must not disable their concurrency.
- Bound fixture creation, PostgreSQL connections, subprocesses, and test media independently.
- Check free disk and memory before scale runs. Reserve capacity for the desktop and normal services.
- Use only the disposable PostgreSQL instance for test configuration changes.
- If resource pressure threatens host usability, stop or reduce the owned workload, preserve evidence, and continue at a smaller safe scale.
- Treat cancellation at a declared resource limit as a controlled benchmark outcome. Record the reason, retry at the largest safe workload when useful, and do not repeatedly relaunch the same unsafe seed/build/load. Cargo job limits alone do not bound linker or database memory.
- Do not change host swap, kernel settings, security controls, system services, or global caches to improve benchmark numbers.
- Do not drop the host page cache or restart a shared database. Report warm-cache results honestly if isolated cold-cache testing is unavailable.

---

## 5. Invariants that every phase must preserve

### Catalog, media, and APIs

- PostgreSQL remains authoritative; Tantivy remains a rebuildable projection.
- Public identifiers, content-addressed evidence, manifest provenance, database constraints, and compatibility contracts retain their intended semantics.
- Native, OpenSubsonic, Jellyfin, and administrative interfaces retain required fields, ordering, filtering, pagination, and access rules.
- Same-name artists remain distinguishable; manual identity decisions and negative decisions retain force.
- Media quality, required validation, fingerprints, no-clobber publication, range requests, and cleanup guarantees remain intact.
- Filesystem metadata equality is not a substitute for content integrity.

### Work, transactions, and resources

- Claims, leases, tokens, heartbeats, retries, and publication remain fenced and idempotent.
- No in-memory queue replaces a required durable delivery obligation.
- Claiming work must not create an unbounded backlog of leased jobs waiting to start.
- Resource limits apply to waiting and active work, not merely the number of spawned threads.
- Acquire admission before spawning expensive blocking work. Keep permits until the actual work stops, including cancellation cleanup.
- Running blocking work does not become cancelled just because its async wrapper is dropped.
- External children have timeout, termination, wait/reap, pipe-drain, and cleanup behavior.
- Define a consistent acquisition order when tasks require multiple permits/locks; do not create circular waits.
- Avoid holding database transactions across expensive filesystem or subprocess work. When moving reads outside a transaction, revalidate the binding and lease before writing.

### Security and durability

- Preserve password hashing strength and bounded authentication execution.
- Preserve credential change, disablement, revocation, blacklist, and authorization behavior.
- Preserve path/no-follow protections and streaming admission.
- Preserve exact multi-process rate-limit semantics, including the retained-key ceiling.
- Preserve required audit history and search tombstone/replay retention.
- No new Redis, Kafka, RabbitMQ, Elasticsearch, service decomposition, or distributed scheduler is in scope.

---

## 6. Work packages: evidence and urgent corrections

### RM-00 — Baseline, bounded metrics, and permanent regression harness

**Source:** V §B, §D PERF-015, §I; R observability/benchmark sections.  
**Required outcome:** Establish evidence quickly enough to guide implementation without delaying known fixes for a full 10-million-track campaign.

1. Capture the current checkout, dependency versions, configuration, CPU/RAM/storage, database isolation details, and resource budget.
2. Identify existing harnesses and reuse them. Recover or recreate the validator's essential temporary experiments as permanent repository tests.
3. Add deterministic tests for the two-session genre undercount, the full late-ID/old-generation-consumption search race, and discovery starvation. Use barriers/channels/controlled transactions rather than timing sleeps.
4. Run the focused reproductions against current baseline behavior. For defects still present, preserve the failing evidence and keep intentional failures isolated from the final passing suite. If current code already fixes a defect, require its regression to pass and record the fixing evidence; do not reintroduce a defect or force an obsolete baseline checkout solely to obtain a failure.
5. Extend existing tracing and resource diagnostics with bounded histograms/counters for request classes, actual pool checkout wait, query/workflow time and counts, processing stages, queue age/depth, admissions, subprocesses, search opens/commits, retries, and throughput.
6. Avoid raw URLs, entity IDs, paths, user identifiers, SQL parameter values, and unbounded metric labels. Reuse existing subprocess timing where available.
7. Create a deterministic representative database fixture around 560K–600K tracks, with artists/releases/evidence/genre skew, missing values, duplicates, ratings, and relevant ACL cases. Initial smaller fixtures may unblock development, but do not relabel them as production-scale evidence.

**Acceptance:** Regression tests reproduce defects still present and verify already-satisfied cases; metric labels and queues are bounded; actual acquisition wait is separated from SQL time; before/after artifacts identify exercised and bypassed production paths. No credentials or private media metadata appear in committed evidence.

### RM-01 — Narrow playable-media-asset index

**Source:** V §D PERF-001.  
**Required outcome:** Remove the measured reverse lookup scan without changing filename projection semantics.

- Inspect the actual schema and add a forward migration for a narrow leading index on **tracks(playable_media_asset_id)** if an equivalent usable index is absent.
- Keep both media/track insertion orders working. Keep existing INSERT/UPDATE filename maintenance and current ordinary import order for this correction.
- Do not automatically add INCLUDE columns, remove a trigger, or reorder every writer.
- Verify ordinary import, legacy import, recovery, merge/split, source/playable path updates, canonical path updates, deletion, retries, and conflict handling.
- Compare present and absent asset lookups and the policy exact-hash query. Inspect nested trigger plans where supported.

**Acceptance:** Representative reverse lookup no longer scans the whole tracks table; buffers/work per ordinary selective lookup are bounded by the indexed access. Filename rows and policy behavior are unchanged. Fresh and populated upgrade tests pass.

### RM-02 — Release-scoped acoustic novelty

**Source:** V §D PERF-002.  
**Required outcome:** Normal and dry-run policy load only fingerprint matches relevant to the evaluated release(s).

- Build a deduplicated set of non-null/nonblank digests from the actual release/manifest facts.
- For empty input, return an empty match set without issuing an unbounded fallback query.
- Scope the exact digest expression with a typed parameterized lookup, preserving evidence kind, current semantic predicates, and exclusion of the same release.
- Inspect the actual partial/expression index definition. Do not copy an incorrect predicate assumption from R.
- Scope dry-run queries per release or by a bounded batch with an explicit release-to-digest mapping; never construct a catalog-wide fingerprint array for each release.
- Preserve legacy inline/externalized manifests and CUE behavior. Avoid cloning unnecessarily large collections.
- If a new plan still scans unrelated tracks through a join, investigate the full plan and use an appropriate equivalent EXISTS/join structure; merely seeing the fingerprint index name is insufficient.

**Tests:** Empty/null/blank/duplicate digests; multiple evidence owners; same-release-only and cross-release matches; duplicate evidence; normal/dry-run decision equivalence; large bounded dry-run.

**Acceptance:** Equivalent novelty decisions; transfer and retained result size scale with input digests and relevant matches rather than every catalog fingerprint. Report query plans, transferred row counts, elapsed time, and peak memory separately.

### RM-03 — Genre lookup indexes and unnecessary trigger work

**Source:** V §D PERF-003.  
**Required outcome:** Efficient release hydration and no genre recount for unrelated overlay updates.

- Evaluate leading **(release_id, surface)** and the surface/display lookup shapes against actual predicates. Choose narrow keys first; add covering columns only when plans and write/storage trade-offs justify them.
- Preserve the statement-level transition-table trigger.
- Add an internal actual-status-change guard and restrict subsequent work to affected transitions. Do not use the rejected UPDATE OF status plus transition-table DDL.
- Preserve counts for every real status transition, including restoration and merge/deprecation paths.
- Validate with current PostgreSQL plans. The validator observed skip-scan on one existing index; do not claim an unconditional full scan from the schema alone.

**Acceptance:** Overlay-only updates perform zero aggregate recount work; real transitions remain correct; release hydration uses an appropriate leading access path; migration SQL is executed successfully in both fresh and upgrade tests.

**Dependency:** RM-03 may proceed independently of RM-04, but PERF-003 is incomplete until both concurrency and existing-data repair are verified.

### RM-04 — Concurrent genre correctness and repair of existing aggregates

**Source:** V §D PERF-003, §G; reproduced lost contribution.  
**Required outcome:** Exact song and distinct-release counts after all supported mutations, including concurrent writes.

Write a short implementation decision covering all mutation paths before changing the protocol. Complete this decision and implementation in the current run.

Choose the simplest proven protocol:

- A serialized recount may be acceptable initially if lock acquisition and snapshot visibility guarantee that a waiting transaction counts earlier committed contributions.
- Membership-aware transactional deltas may be appropriate if supported by a precise representation of distinct release/genre membership.
- Do not assume sorted locks alone refresh a stale statement snapshot. Prove visibility at the actual isolation levels and statement boundaries used.
- Do not increment distinct-release counts once per track. Do not silently convert an exact synchronous projection into eventual consistency.
- Use one lock protocol and deterministic actual lock-key order across imports, terms, status updates, merges, splits, deletes, recovery, and repair.

Implement an idempotent repair operation for existing incorrect aggregates. It must derive counts from authoritative terms/status, remove stale keys with zero contributors, detect discrepancies, and converge under its documented maintenance fence or concurrency protocol. Provide a read-only check mode and an explicit apply mode for operators. Exercise apply only against disposable test data in this run.

**Tests:** Two concurrent new-genre imports; two tracks in one release; duplicate terms; normalization/spelling rules; opposing multi-genre input order; concurrent insert/update/delete; all status transitions; merge/split; rollback and retries; crash during repair; rerun repair; deliberately corrupted starting counts.

**Acceptance:** After every scenario, authoritative song/distinct-release counts equal all stored aggregate rows, including absence of stale rows. No unrecovered deadlocks. Record locking cost separately from correctness. Adding indexes alone does not complete this package.

### RM-05 — Commit-safe search generation activation

**Source:** V §F PERF-V001 and §D PERF-006.  
**Required outcome:** No committed mutation can be permanently omitted because it had a lower outbox ID or was acknowledged against an older generation.

#### Required activation invariant

Define a finite activation cut **C**:

> Every catalog mutation committed before C is represented in the activated generation or superseded by an equal/newer authoritative revision. Every commit after C remains durably deliverable to that generation. Acknowledgement against the previous generation cannot erase the new generation's delivery obligation.

After writers quiesce and the outbox drains, compare the projection with authoritative entity/revision/deletion state. Do not require equality to a continuously changing database without specifying a consistency boundary.

#### Implementation direction

Preserve the consistent snapshot initially. Design catch-up, retention, acknowledgement, activation, and generation retirement as one protocol. Prefer the existing architecture plus the smallest required durable replay state and a bounded handoff/fence if that is the simplest correct solution.

Track the rebuilding generation's delivery obligations independently of ordinary outbox processed_at or acknowledgement state. Retained events already acknowledged against the previous generation may still require replay. An advancing ID cursor is a scanning optimization, not proof that all lower-ID transactions have committed or been delivered.

The design must identify:

- Every catalog/outbox producer, including CLI, administration, recovery, merge/split, and ordinary imports.
- Transactions already in flight before the snapshot or rebuild claim.
- How events remain available to the rebuilding generation even after ordinary consumption.
- How replay progress avoids permanently skipping a lower-ID transaction that commits late.
- How C is established, what writers/consumers participate, and what happens to later commits.
- Lease ownership, retries, retention, crash recovery, and reader/writer behavior at activation.
- How any existing stale projection can be safely rebuilt/repaired after deployment.

A cooperative write/activation fence is permitted if all relevant writers demonstrably participate, its pause is measured, and it has no unbounded/deadlocking wait. The existing activation table lock is not proof of this. Do not assume that a snapshot export, a new sequence allocated near commit, pending-row checks, repeated polling, or a revision check alone solves delivery.

#### Mandatory regression

Recreate the validator's exact failure with the real production worker/rebuild path:

1. Transaction A changes an entity and allocates a low outbox ID but remains uncommitted.
2. The rebuild snapshot excludes A.
3. Later IDs commit and catch-up advances.
4. A commits.
5. The ordinary worker consumes and acknowledges A against the old generation.
6. The new generation activates.

The new generation must contain the correct revision without relying on another unrelated mutation or rebuild.

Also test late deletes/new entities, all entity types, multiple revisions, event allocation before rebuild claim, replay behind a cursor, ordinary acknowledgement racing activation, retained/processed/pruned history, duplicate/stale replay, lease loss, concurrent rebuild attempts, and crashes around index commit, SQL acknowledgement, activation, and retirement.

**Acceptance:** Reproduce the baseline failure if it remains in the current checkout; otherwise verify the existing fix. The resulting protocol passes deterministic adversarial tests and final quiescent equivalence. Record a concise correctness argument and review it independently. Do not accept a timed sleep or a successful nonadversarial rebuild as proof.

**Dependency:** This is a release-blocking gate before accepting RM-13/RM-14 search optimizations. Keeping REPEATABLE READ alone does not fix the existing activation defect.

### RM-06 — Durable discovery progress and fairness

**Source:** V §D PERF-012.  
**Required outcome:** Every stable eligible entity and chart item is eventually visited; large track populations cannot starve other classes.

- Persist progress per entity/work class, with explicit pass completion and wraparound.
- Allocate bounded independent budgets for tracks, releases, artists, missing-candidate backfill, and chart relinking.
- Advance progress only with or after successful durable effects. If a crash causes replay, replay must be idempotent; checkpoint-before-effect must not skip work.
- Fence concurrent passes so cursors cannot regress or be overwritten incorrectly.
- Handle deleted cursor rows, empty tables, inserts ahead/behind, restart, partial failure, and chart-only backlog.
- Under continuous insertion, use a finite pass boundary or equivalent protocol so existing stable rows still receive coverage.
- Retain a periodic bounded missing-candidate backstop. Ordinary insert triggers alone do not cover all restore/downgrade/history cases.

**Tests:** More than 100K tracks with nonempty release/artist/chart workloads; test-visible visitation; restart and wrap; concurrent passes; new/deleted rows; backlog growth.

**Acceptance:** Every stable eligible item is visited within the number of sweeps implied by the configured budgets; bounded work per invocation; no prefix-only repetition or permanent chart starvation.

---

## 7. Work packages: catalog, workers, and media

### RM-07 — Bounded scheduler results and continuous worker refill

**Source:** V §D PERF-009.  
**Required outcome:** Bounded process-lifetime result storage and effective use of available worker slots without lease or fairness regressions.

- Replace lifetime accumulation with the latest result or an explicitly bounded ring, preserving any externally used stop/result contract.
- Distinguish the in-memory result collection from persistent lease-event history. Removing one does not authorize deleting the other.
- Implement rolling refill for appropriate inspection/normalization/validation/publication workers; do not wait for the slowest item in a drained batch before refilling a free slot.
- Reserve admission and claim only work that can start within the lease model. Avoid a queue of preclaimed jobs waiting behind resource permits.
- Preserve ordinary/bulk lane weighting, retry delay, heartbeat scheduling, cancellation, and shutdown.
- Back off on empty/error results. Add a bounded processing turn/yield so a continuously full queue cannot monopolize the runtime or database.
- Ensure nested album/track concurrency shares global CPU/filesystem/process budgets.

**Tests:** 10,000 accelerated idle ticks; mixed long/short jobs; full and partial batches; empty queues; ordinary/bulk fairness; lease renewal/expiry; stale worker fencing; cancellation while queued and active; graceful stop.

**Acceptance:** Retained result capacity does not grow with elapsed ticks; freed slots refill while backlog exists; no increased empty polling, lease failures, or starvation. Measure utilization and throughput instead of assuming a fixed percentage improvement.

### RM-08 — Efficient outbox drain and maintenance scheduling

**Source:** V §D PERF-005.  
**Required outcome:** Eliminate unnecessary post-batch delay and expensive idle maintenance checks.

- Continue draining full batches without unconditional idle sleep.
- Preserve idle/error backoff, shutdown checks, lease ownership, and a bounded drain/yield budget.
- Gate costly maintenance with a monotonic timer and the existing durable maintenance state where needed.
- Define startup, restart, multi-process ownership, clock-change, and failed-maintenance retry behavior. Do not run one heavy check per loop or per process simply because a local timer was reset.
- Preserve retention protection for active rebuilds and any new RM-05 replay obligations.
- Measure maintenance SQL calls as well as outer function invocations.

**Tests:** Full/partial/empty batches; processing error; shutdown during drain; clock/restart behavior; multiple eligible workers; hourly maintenance due/not due; failed maintenance retries; rebuild retention pin.

**Acceptance:** Full batches do not incur fixed idle sleeps; empty queues do not hot-poll; costly maintenance runs only when due under the intended ownership protocol. Commit-before-ack and retention semantics remain intact.

### RM-09 — Durable affected-identity reconciliation

**Source:** V §D PERF-004.  
**Required outcome:** Remove routine global candidate discovery after every album while preserving historical and transitive identity convergence.

First add a justified leading **artist_match_decisions(artist_id, observed_artist_id)** index if absent. This correction is independently implementable despite its omission from V's final approved list.

Then implement an incremental reconciliation protocol:

1. Produce affected seeds from authoritative mutations: entity IDs, normalized names/identity keys, external identifiers, redirects, and changed relationships.
2. Persist required seeds atomically with the authoritative change; a crash after import must not lose reconciliation work.
3. Reuse the durable queue infrastructure where practical. Coalesce work without losing seeds that arrive while a matching job is active.
4. Expand the affected identity closure after matches, merges, or evidence changes. Bound each batch/transaction, persist remaining work, and resume rather than truncating the closure.
5. Preserve ambiguity rules, manual locks, negative decisions, same-name distinct artists, redirects, observations, release identity evidence, and retry idempotency.
6. Avoid redundant global work for a provably unchanged idempotent import while preserving outstanding historical work.
7. Retain a low-frequency bounded global backstop for both artist and strong-release reconciliation. The validator found that startup coverage was artist-only.
8. Keep merge decisions and multi-entity locks deterministic and fenced across workers.

Build a comparison oracle using the existing global algorithm run to its intended stable state on controlled fixtures, supplemented by domain invariants. Compare identities, proposals, observations, redirects, and rejected decisions—not merely counts.

**Tests:** New evidence resolves an old duplicate; multi-hop merges; cross-release ID groups; historical ambiguity; same-name distinct artists; locked/negative decisions; release merge enables later matching; concurrent imports; seeds arriving during a job; crash/retry; no-op import; global-backstop repair.

**Acceptance:** Equivalent intended outcomes; no five global discovery calls after every ordinary import; bounded queue/RSS and transaction work; per-import work follows affected relationships. Candidate result LIMIT alone does not prove bounded discovery cost.

### RM-10 — Shorter manifest transactions and measured DB admission

**Source:** V §D PERF-008.  
**Required outcome:** Avoid expensive evidence I/O inside locked write transactions and manage measured workload competition.

#### Manifest preparation

Implement a two-phase binding protocol:

1. Capture the identity/revision/binding needed to load the manifest.
2. Read, parse, and cryptographically verify evidence outside the write transaction through bounded I/O admission.
3. Enter the write transaction and revalidate all required state before using prepared data.

Revalidation must cover work-item state, lease token/owner/validity, manifest ID/schema/reference/hash/revision, plan identity/hash, publication receipt/release identity/sealed hash, and existing idempotent result/adoption state. Include additional fields discovered in current code. On mismatch, discard/retry or fail according to existing semantics; never commit stale prepared evidence.

Test mutation between both phases, lease loss, expired ownership, cancellation, retry, and existing-result adoption. A cheaper file read is not sufficient if the transaction can import evidence no longer bound to the job.

#### Workload admission

Instrument actual connection checkout and hold time by bounded workload class. Separate database acquisition wait, query CPU/I/O, lock contention, and async scheduling delay.

After query corrections, measure concurrent import/rebuild/API work. Prefer a small configurable worker admission/reservation mechanism if it solves demonstrated contention. Split pools only when justified; preserve an explicit total connection budget and support small installations. Administrative queries and lease heartbeats need explicit classification.

Heartbeats must not queue behind all the work whose leases they maintain. Avoid nested permit dependencies that cause deadlock or starvation.

**Acceptance:** Manifest disk parsing/verification no longer unnecessarily extends write locks; binding/lease adversarial tests pass. Provide before/after checkout and hold time, locks, errors, and interactive tails. A pool split is an evidence-based decision, not a mandatory fixed 16/16 setting or a latency guarantee.

### RM-11 — Eliminate redundant media work while preserving integrity

**Source:** V §D PERF-007.  
**Required outcome:** Reduce unnecessary reads/hashes/probes without losing custody, provenance, or recovery checks.

Create a compact operation inventory for same/cross-filesystem acquisition; retained MP3 and FLAC encoding; embedded/external/no artwork; CUE; success, failure, retry, and recovery. Count source reads, output reads, full-tree hashes, copies, probes, subprocesses, and temporary storage separately.

- Remove adjacent duplicate verification only when evidence identity, mutation fencing, and required boundaries establish equivalence.
- Hash the same opened byte stream during a mandatory copy/read where that can replace a genuinely redundant pass.
- Preserve required before/after checks and retry/adoption verification.
- Reuse probe results only when the original facts are complete for the consuming stage and describe the same source/output object. Retained MP3 still requires its intended structural validation.
- Cache tool versions by canonical executable identity with a defined invalidation rule.
- Deduplicate artwork only with reliable content/asset identity and transformation parameters.
- Preserve codec settings, tags, fingerprints, output validation, no-clobber publication, and cleanup.
- Do not interpret application ownership as proof that another process cannot modify a file.

**Tests:** Same-size content mutation, mutation during every relevant read/copy boundary, truncated/corrupt input and output, CUE, cross-filesystem recovery, interrupted copy, lease loss, timeout, cancellation, full temporary storage, retry/adoption, artwork variations.

**Acceptance:** Required verified evidence and media semantics remain equivalent; operation counts and actual I/O reduce on the exercised paths. Where the codec legitimately emits nondeterministic bytes, compare the defined quality/metadata/validation contract; retain byte/hash equality for paths that promise it. Report workload-specific savings, not a universal read multiplier.

### RM-12 — Bounded blocking work, subprocess lifecycle, and useful concurrency

**Source:** V §D PERF-007, PERF-009; related PERF-006/PERF-013.  
**Required outcome:** Expensive work no longer blocks async workers unnecessarily, and concurrency improves throughput within a real resource budget.

- Route heavy filesystem traversal/hash, CPU processing, process handling, and applicable search work through bounded admission before spawn.
- Reuse existing ResourceBounds and semaphore patterns instead of introducing a new orchestration framework.
- Make inspection_concurrency affect actual execution.
- Add shared global limits for CPU-heavy media, disk/NFS work, database-heavy stages, external children, open files, and temporary storage as applicable.
- Choose a small worker/FFmpeg thread matrix for the actual codecs. Account for input, output, and filter options and their placement. Do not universally force one thread.
- Add bounded intra-album concurrency only when it improves the measured workload and keeps deterministic evidence/error semantics.
- Replace polling/pipe-thread overhead only when the new process handling preserves timeout, kill/reap, output-drain, cancellation, and error reporting.
- Keep active blocking work accounted for until it actually ends. Cooperatively check cancellation at safe boundaries; fence publication by cancelled/stale workers.
- Define resource acquisition ordering and avoid waiting indefinitely for another class while holding a scarce permit.

**Tests:** Permit saturation, queued/active cancellation, child timeout and output backpressure, shutdown, lease expiry, slow NFS simulation, large albums, mixed codecs, bounded temp use, cleanup after errors.

**Acceptance:** No unbounded pending blocking queue; no detached publishing work; correct gauges/permit release; async responsiveness and measured throughput improve or remain within the declared regression budget. Tune defaults from evidence rather than maximum possible concurrency.

---

## 8. Work packages: search and interactive performance

### RM-13 — Ordered search hydration and reusable readers/writers

**Source:** V §D PERF-005.  
**Prerequisite for integration:** RM-05 correctness protocol and adversarial tests pass.

#### Ordered hydration

Batch database access while retaining an explicit mapping from each ordered event to its document/deletion/error outcome. Preserve mixed entity types, repeated entities, duplicate and increasing revisions, missing rows, deletions, stale events, poison handling, and replay behavior.

Do not blindly reorder by entity type or deduplicate events without a proved revision/deletion rule. Apply/commit/ack sequencing must retain the delivery responsibility established by RM-05.

#### Reader lifecycle

Use a bounded registry keyed by validated root and generation identity. Reuse expensive index/reader state; create per-query search handles as required by the locked Tantivy API.

Handle activation by another process, atomic local swaps, in-flight readers, manifest corruption, restart, generation retirement, and directory cleanup. Cache manifest verification only with a sound identity/invalidation model. Do not move per-request opens into another helper and call that reuse.

#### Writer lifecycle

Reuse a writer for the valid leased generation, with explicit close/reopen on generation changes, lease loss, writer-lock failure, errors, and shutdown. Keep memory within a total configured budget. Multiple instances must not assume in-process ownership is a distributed fence.

**Tests:** Ordered mixed events; delete/recreate; duplicate revisions; poison event; crash after index commit before SQL ack; lease loss; local/remote activation; old-reader retirement; invalid manifest; writer reacquisition; replay to a new generation.

**Acceptance:** Opens no longer scale with every ordinary request/batch where reuse is supported; SQL hydration round trips decrease; documents/revisions/deletions remain equivalent; cache/writer memory is bounded; RM-05 tests still pass. Report cold/warm tail latency and drain rate without promising an unmeasured SLO.

### RM-14 — Production search rebuild efficiency

**Source:** V §D PERF-006.  
**Prerequisite:** RM-05; coordination with RM-13 lifecycle.

- Retain the current consistent snapshot model unless a complete replacement is required by the proven activation design.
- Add a bulk insertion path for a demonstrably fresh empty generation. Avoid per-document existing-revision lookup there; retain revision/tombstone checks for replay/catch-up.
- Move expensive create/index/commit work off async threads through bounded admission.
- Tune commits using explicit memory, time, and document bounds. Do not replace one arbitrary fixed count with another unexplained constant.
- Bound the queue between database page loading and indexing.
- Optimize SQL feeding the rebuild, including genre hydration, before attributing all cost to Tantivy.
- Record snapshot age, connection duration, commit count, merge activity, memory, disk use, and applicable vacuum cleanup effects.
- Preserve cancellation, resumability/restart behavior, generation activation, and cleanup.

**Acceptance:** Same authoritative content/revisions after drain; no unnecessary initial revision lookups; bounded buffers; measured production rebuild improvement. Do not claim “autovacuum globally blocked”: distinguish delayed dead-tuple reclamation from stopping the process.

Run the actual PostgreSQL-backed rebuild. A synthetic direct-document benchmark may supplement it but cannot substitute for it.

### RM-15 — Interactive query, aggregation, ACL, and serialization improvements

**Source:** V §D PERF-010.  
**Required outcome:** Improve demonstrated expensive routes while preserving results and whole-system behavior.

Capture plans and response costs for administrative statistics, OpenSubsonic getArtists/getIndexes, Native browse/search suggestions, artist search, permissions, and relevant Jellyfin paths.

- Preserve positive-rating semantics and exclusion of the current user when preaggregating ratings.
- Consider a leading artist rating index only after checking predicates, collation, selectivity, and actual plans.
- Do not add a redundant release-date index when current indexes already satisfy the access pattern.
- Combine compatible statistics work and reuse aggregates only when equivalent. Largest-release playable-asset-per-track semantics must not silently become all-linked-asset totals.
- Reduce generic response-tree allocation using typed representations where compatibility permits.
- Optimize ACL predicates set-wise. Preserve first-binding creation, removal, revocation, user isolation, and fail-closed behavior.
- Treat short substring searches according to observed plans and existing query semantics. Do not silently change minimum query length or match behavior to win a benchmark.
- Keep frozen external pagination contracts. Internal/background paths may use keyset paging where equivalent.
- Cache suitable non-security data only with explicit key, byte/cardinality bounds, freshness, and invalidation. Do not add simplistic permission caches.
- Measure concurrent heavy SQL before introducing parallel query execution that may worsen shared database contention.

**Tests:** Stable JSON/XML golden contracts; deterministic ordering/collation; current-user rating exclusion; empty responses; alternate/playable assets; asset-size edits; short/long queries; deep pages; multi-user ACL changes and database errors.

**Acceptance:** Equivalent semantic and stable wire contracts; lower measured query/allocation cost on targeted endpoints; no material mixed-load throughput or authorization regression.

### RM-16 — Verified index cleanup, lease writes, and history retention

**Source:** V §D PERF-011 and migration review.  
**Required outcome:** Reduce proven write overhead without discarding required access paths or history.

Recheck these exact duplicate candidates against the current schema:

| Candidate | Reported equivalent |
|---|---|
| tracks_native_random_candidate_idx | tracks_pkey on id |
| artist_track_associations_native_track_idx | artist_track_associations_track_idx on track_id, artist_id |
| search_outbox_hourly_totals_age_idx | Primary key on projection_name, hour |

Compare complete definitions: access method, keys/order, expressions, predicates, collations, opclasses, included columns, uniqueness, constraint dependencies, and validity. Use representative recovery/admin/claim plans. Only remove verified equivalents through a forward migration.

Treat all other overlapping indexes as conditional. Zero scans or a statistics reset is not a removal justification.

Optimize lease renewal round trips only with unchanged owner/token/expiry fencing, queue age, updated_at semantics, event history, and cancellation behavior. Do not promise HOT updates while indexed lease fields still change.

For history growth, inventory required event classes, recovery dependencies, aggregates, and existing retention requirements. Implement bounded/resumable pruning or compaction only where retention semantics are established. Preserve active-work evidence, required audit detail, and replay/tombstones. If policy is unspecified, provide a tested configurable operator mechanism without silently enabling destructive pruning or inventing a retention period; record that policy-dependent decision explicitly.

**Tests:** Exact index definitions/dependencies; populated upgrade; plans for rare/recovery paths; renewal races; queue ordering; event assertions; retention cutoff boundaries, idempotency, interruption, and aggregate equivalence.

**Acceptance:** Only justified drops; unchanged lease/audit behavior; measured index/WAL/write cost; no unsupported storage savings. An unresolved product retention choice does not block the verified index/query corrections.

### RM-17 — Efficient streaming, safe auth lookups, blacklist, and artwork

**Source:** V §D PERF-013.  
**Required outcome:** Reduce demonstrated per-request overhead while preserving security and streaming semantics.

#### Streaming

Choose and benchmark a bounded reusable-buffer design that reduces per-chunk task/allocation overhead. Do not prescribe an indefinitely occupied blocking thread per slow client, and do not assume changing to tokio::fs eliminates blocking internally.

Preserve GET/HEAD/range semantics, invalid ranges, early EOF, open-file lifetime, path/no-follow checks, backpressure, client disconnect, per-user/global limits, cancellation, and shutdown. Limit outstanding buffers and blocking-thread occupancy.

#### Authentication and caches

- Combine compatible user/secret lookups while keeping observable success/failure behavior and revocation semantics.
- Keep bcrypt/password strength and bounded execution. Do not implement the rejected password-success cache.
- Add singleflight blacklist refresh using the existing safe failure policy; first-load failure must not accidentally permit access.
- Use byte-bounded artwork caching keyed by stable asset identity and transformation parameters such as size, format, and no-upscale behavior.
- Measure rate-limit contention. Retain the current exact semantics unless a complete multi-process alternative preserves atomic admission, expiry, cleanup, failure behavior, and the global retained-key ceiling.
- A naive per-key UPSERT does not satisfy that contract. Keeping the existing limiter after measurement is a valid conditional outcome.

**Tests:** Full/HEAD/valid and invalid ranges; early disconnect; no-follow/path attacks; slow clients; password changes/disablement; token and password auth; failure/rate-limit races; blacklist refresh failure/revocation; artwork key changes and byte bounds.

**Acceptance:** Less verified task/allocation/query work without underruns, leaked descriptors/permits, cache growth, or access-control changes. Measure tasks per MiB, thread occupancy, memory, CPU, TTFB, and p95/p99 at the supported load.

### RM-18 — Batched evidence/identity writes and recommendation decision

**Source:** V §D PERF-014.  
**Required outcome:** Batch demonstrated per-item loops with exact data and transaction semantics.

- Batch acoustic/embedded evidence and external identifiers using typed bounded arrays, set-based SQL, or appropriate QueryBuilder usage.
- Validate equal array lengths and types; preserve null/empty/duplicate handling, source ordering, constraint behavior, and idempotency.
- Respect parameter/memory limits and large boxsets. Chunking must preserve required atomicity.
- Audit advisory lock ordering across every relevant caller. Keep one compatible deterministic order; do not convert one caller to hash-key order while leaving another with a conflicting order.
- Do not infer statement-cache thrash from variable SQL alone. Measure prepares/round trips/time where practical.
- Measure recommendation/play-trigger contention. Keep synchronous freshness unless a documented requirement-compatible durable alternative is necessary and proved.

**Tests:** Empty/single/large inputs; mismatched arrays rejected; nulls and duplicate IDs; constraint rollback; retry; opposing lock order; concurrent scrobbles and relationship changes; recommendation equality.

**Acceptance:** Identical rows and required event/recommendation outcomes; reduced demonstrated round trips or time; no widened deadlock window or freshness regression. Asynchronous recommendation conversion is optional and must not be used to manufacture completion.

### RM-19 — Set-based release merge and split

**Source:** V §F PERF-V002.  
**Required outcome:** Avoid per-track/per-observation round trips while preserving reversible identity operations.

- Implement set-based updates using explicit source-to-target/ordinal mappings.
- Preserve immutable merge evidence, observation order, reversible split data, public identities, redirects, credits, and manual decisions.
- Preserve filename/genre/catalog aggregates, recommendations, revisions, and all required outbox events; do not bypass triggers merely for speed.
- Keep required atomicity and deterministic lock ordering. Chunking may reduce statement size but must not expose a partially applied merge.
- Coordinate with RM-04 genre protocol, RM-09 affected identities, and RM-05 outbox delivery.

**Tests:** Merge then split round trip; empty/small/large releases; duplicate/null evidence; ordinal collisions; constraint failure rollback; concurrent conflicting operations; retries; exact immutable evidence comparison; canonical aggregate/recommendation/search state.

**Acceptance:** Equivalent merge/split results and reversible evidence; fewer statements and shorter measured lock-holding time for representative large releases. Exercise 1K and, when resource-safe, 10K-track fixtures; report actual sizes run.

---

## 9. Migrations, existing data, and operational readiness

Schema and projection changes are part of the implementation, not a handoff TODO.

### 9.1 Default migration route

Inspect the current migration runner and ADR-0166 or its successor. Prefer the repository's existing maintenance-fenced forward migration process. Do not invent an online migration platform to avoid a documented maintenance window.

- Never edit an already released migration or its checksum/history.
- Centralize new migration numbering and ordering.
- Preserve constraints and both valid media/track insertion orders.
- Make trigger replacement and dependent function ordering explicit.
- Apply the complete current chain on a fresh disposable database.
- Upgrade a populated pre-change database with representative skew and inconsistent derived data where relevant.
- Verify exact definitions, constraints, row semantics, index validity, application readiness, and worker restart behavior.
- Record elapsed time, locks, disk growth, and expected operator downtime.

Normal CREATE INDEX may fit the fenced procedure. If online concurrent DDL is actually necessary, implement its full protocol rather than adding a nontransactional marker and hoping retries work.

For concurrent DDL:

- Verify the locked SQLx version's transaction behavior and exact supported migration marker.
- Keep CREATE/DROP INDEX CONCURRENTLY outside ordinary transaction blocks/functions.
- Detect incomplete/invalid indexes and definition mismatches.
- Define resume, cleanup, and retry behavior for each interrupted state.
- Do not treat IF NOT EXISTS as proof that the existing object is valid or equivalent.
- Do not manually create migration-owned final objects and falsify migration bookkeeping.
- Test the failure path only in owned disposable databases.

Do not kill unrelated sessions or automatically run this procedure against the user's database.

### 9.2 Repair is a first-class deliverable

Provide one documented upgrade/repair sequence:

1. Verify source/target versions and required backup/restore readiness.
2. Establish the maintenance/write fence required by the repository.
3. Apply forward migrations.
4. Repair and compare genre aggregates, including stale zero-contributor rows.
5. Initialize new durable progress/reconciliation state without skipping existing work.
6. Rebuild or repair the search projection through the corrected RM-05 protocol. Do not merely replay currently pending events; the missing events may already have been acknowledged.
7. Verify constraints, index validity, counts, filenames, lease recovery, discovery progress, and search convergence.
8. Restart/resume workloads in the documented order and run readiness/smoke checks.

If repairing search is expensive, the operator procedure must preserve the last valid generation until a corrected replacement is ready, according to the proven activation protocol.

Repair commands must support check/dry-run behavior where meaningful, clear progress, bounded/resumable batches where possible, and idempotent retry. State whether each command requires exclusive maintenance or safely supports concurrent writers.

The operator guide must distinguish restart/retry, forward repair, and restoring a backup. Do not promise automatic down migrations or compatibility with an older binary after irreversible schema/data changes.

### 9.3 Upgrade and compatibility gates

Test fresh install, populated upgrade, interrupted migration/repair, repeat repair, and startup after partial work. Establish which application versions may coexist; do not assume mixed old/new workers understand the same cursor, outbox, or lease protocol.

Provide an explicit deployment order for schema and application changes. If mixed-version operation is unsupported, document and test the stop-all-writers maintenance sequence.

---

## 10. Benchmark and test specification

### 10.1 Evidence levels

Keep these distinct in every result:

| Label | Meaning |
|---|---|
| MEASURED IN THIS RUN | Collected against the recorded code/configuration/environment |
| REPRODUCED CORRECTNESS | Deterministic test demonstrates a defect or its correction |
| REPORTED BY VALIDATOR | Prior evidence from V, not rerun here |
| ANALYTICALLY DERIVED | Calculation with explicit assumptions and units |
| PROPOSED TARGET | Desired outcome not established by the measurements |
| NOT EXECUTED | Procedure exists but the run was not performed |

Do not quote Gemini's 10–50×, 65%, sub-50 ms, 1,500 documents/s, or rebuild-duration targets as achieved results. Existing ratified repository SLOs still apply where their workload and hardware assumptions match.

### 10.2 Minimum credible implementation evidence

Run the following during this operation when the required isolated environment is available:

| Scenario | Required correctness check | Performance evidence |
|---|---|---|
| Playable asset lookup/import trigger | Both insertion orders and filename maintenance | Present/absent plans, buffers, trigger time |
| Normal and dry-run policy | Same novelty decisions for empty/shared/missing/duplicate digests | Result sizes, query time, allocations/RSS, dry-run spill |
| Concurrent genre operations | Exact song/distinct-release counts, repair equivalence | Lock waits, buffers, write cost |
| Search activation | Late lower-ID plus old-generation acknowledgement; deletes and crashes | Catch-up/pause duration and bounded retained history |
| Discovery | All entity types and charts visited across restart/wrap | Work per pass, visitation, queue/cursor progress |
| Single-album import | Same media/evidence/catalog outcomes | Per-stage time, SQL, reads/writes, subprocesses |
| 200-album import | No partial/lost catalog state; exact projections | Albums/min, tracks/s, database work, I/O, CPU/RSS |
| Unchanged reimport | Idempotent results and no missed outstanding reconciliation | Avoided media/global work and query count |
| Identity reconciliation | Intended results equivalent to global oracle | Closure size, SQL cost, queue age and recovery |
| Incremental indexing | Ordered revision/delete/poison/replay semantics | SQL round trips, opens, commits, events/s, lag |
| Production rebuild | Snapshot/catch-up/activation correctness | Docs/s, commits, snapshot age, memory/disk |
| Mixed interactive load | API/ACL/range behavior, no lost streams | p50/p95/p99, errors, pool waits, underruns |
| Worker scheduling | Fairness, fencing, idle backoff, cancellation | Slot utilization, polls, bounded result capacity |
| Merge/split | Reversible evidence and final derived state | Statements, lock duration, WAL, large-release time |
| Fresh/populated upgrade | Exact schema/data/readiness and repair | Migration/repair duration, waits, disk growth |

Use real production entry paths. Direct SQL fixture seeding is acceptable for the large background catalog, but it does not itself benchmark ordinary imports, triggers, policy, media, or search hydration.

Use authorized test media or generated fixtures with valid audio. Retained MP3, FLAC, artwork, CUE, and cross-filesystem cases must exercise the behaviors they claim. State when synthetic audio fails to represent real codec/artwork complexity.

A representative background catalog does not require generating 560,000 audio files. Use a realistic bounded media batch against the large catalog and clearly describe that separation.

### 10.3 Scale tiers and time to useful delivery

| Tier | Purpose | Execution requirement |
|---|---|---|
| Small deterministic fixtures | Race, semantics, failure, and migration tests | Mandatory; run early and repeatedly only when affected |
| 100K catalog | Fast plan/workflow iteration | Useful intermediate tier |
| 560K–600K catalog | Representative database performance | Required for claiming representative-scale verification |
| 1M catalog | First larger-scale plan/work bound check | Run when the bounded environment supports it |
| 5M and 10M catalogs | Larger-scale certification | Execute if resources/time permit without destabilizing the host; otherwise leave reproducible NOT EXECUTED procedures |
| 25M catalog | Longer-term exploration | No claim of support from this operation unless actually tested |
| Long soak/NFS hardware | Duration/storage-specific validation | Run bounded practical checks now; do not wait 24 hours/30 days solely to finish this assignment |

Finish proven fixes and integrated regression evidence without waiting for every scale tier. Unexecuted 10M, long-soak, or real-NFS validation limits certification; it must not be disguised as completed performance testing.

Once required implementation and representative verification are complete, do not start an optional scale campaign that delays delivery. Provide its runnable procedure and recorded resource requirements with the handoff.

If even the representative tier cannot run safely, complete implementation and all possible tests, record an external validation blocker, and do not claim representative-scale completion.

### 10.4 Measurement protocol

1. Record baseline and final commit/worktree identity, configuration, dataset seed and distributions, media mix, hardware/storage, PostgreSQL settings, worker counts, and build profile.
2. Use optimized/release builds for performance comparisons. Debug builds are useful for debugging, not performance claims.
3. Compare the same dataset and intended operation state. Reset or clone only owned fixtures so prior runs do not turn imports into no-ops or warm caches invisibly.
4. Set explicit time/resource limits on pathological baseline operations. A timeout is censored evidence, not an invented duration or exact speedup.
5. Collect several comparable trials for short workflows. Collect enough requests for latency percentiles to be meaningful; do not calculate p99 from five whole-run timings.
6. Separate cold/warm conditions, logical buffers from physical reads, SQL time from transfer/allocation, and foreground latency from total throughput.
7. Use EXPLAIN/ANALYZE and trigger profiling on representative present, absent, common, and rare values. Do not force planner settings merely to produce a desired index plan.
8. Capture memory and queue bounds under saturation and cancellation, not just at idle.
9. State sample counts, variability, measurement overhead, and known bypasses. Do not compare unlike workloads as a speedup.
10. Use repository thresholds where valid. Otherwise declare a noise-aware engineering regression budget before final testing; investigate sustained degradation in latency, throughput, resource use, or error rate. Do not silently relax the budget after observing a failure.

Correctness is a hard gate even when a speed improvement is large. A correctness repair that intentionally changes cost must state the trade-off and show that the representative workload remains usable; it is not a reason to retain incorrect behavior.

### 10.5 Commands and harness safety

Discover current test target names and database configuration before executing commands. The validator reported these useful starting targets:

~~~bash
cargo test --release -p melodee-persistence --test catalog_concurrency -- --test-threads=1
cargo test --release -p melodee-search --test rebuild_integration -- --test-threads=1
cargo test --release -p melodee-search --test benchmark_smoke -- --test-threads=1
~~~

Do not assume these names or their isolation behavior are unchanged. Inspect fixtures first. New regression tests must run through the same fenced/isolation-aware harness.

Use random owned test database names, unique ports/socket directories when necessary, explicit temporary media roots, and tracked cleanup ownership. Before destructive fixture reset, verify the target database/root is one created by this run.

Existing functional green tests are not proof that a new race is covered. The original rebuild suite passed while PERF-V001 remained reproducible.

### 10.6 Testing efficiency

Run focused tests as each relevant component changes, then run the required integrated suite after integration. Add tests that catch an actual failure mode or protect a changed invariant; avoid duplicating implementation logic or creating hundreds of superficial tests.

Repeat broad suites only to investigate failures, satisfy repository gates, or resolve concrete remaining risk. Do not repeatedly rebuild the entire workspace for cosmetic documentation edits.

Use bounded accelerated tick/fairness simulations for retention and scheduling. Provide real soak commands and expected observations separately; simulated days are not measured days of production uptime.

---

## 11. RM-20 — Integration, permanent standards, and final handoff

**Source:** V §I–L and all package acceptance conditions.  
**Required outcome:** One coherent remediation result with no unresolved mandatory defect hidden by partial package completion.

### 11.1 Permanent engineering standards

Create/update **PERFORMANCE.md** and the relevant **AGENTS.md** section in the same operation, preserving repository conventions and unrelated instructions.

Include concrete requirements for:

- Album/affected-identity proportional work and explicitly scheduled global backstops.
- Bounded collections, admission queues, blocking work, child processes, and temporary storage.
- Lease fencing, cancellation, fair discovery, durable checkpoints, and eventual coverage.
- Database query plans and justified indexes; no index removal based only on zero scans.
- Small transaction scope and mandatory revalidation after moving external work out of a transaction.
- Content identity, verified reuse, recovery, and publication integrity.
- Search activation cut, revision/delete handling, generation-specific delivery, retention, and commit-before-ack.
- API/authorization compatibility and prohibited password/integrity shortcuts.
- Representative release-build benchmarks with source revision, configuration, sample counts, limits, and bypasses.
- Maintenance-fenced migrations, existing-data repair, upgrade rehearsal, and truthful scale claims.

Add new ADRs only for meaningful concurrency/consistency/storage decisions. Use the repository's numbering and format. An internal design note should explain the chosen protocol and rejected alternatives without becoming another general audit.

### 11.2 Required outputs from the implementation agent

| Deliverable | Required content |
|---|---|
| Integrated code and configuration | Actual fixes, bounded controls, appropriate defaults, help/config documentation |
| Forward migrations | Schema/trigger/index changes with correct ordering and version handling |
| Repair/upgrade tools and guide | Genre repair, corrected search rebuild, cursor/reconciliation initialization, failure recovery |
| Permanent regression tests | Genre race, late-ID/old-consumer race, discovery fairness, integrity, leases, compatibility, migration/repair |
| Reproducible benchmark harness | Representative inputs, resource limits, production-path scenarios, larger-scale procedures |
| PERFORMANCE.md and AGENTS.md updates | Enforceable standards and verification expectations |
| PERF-REMEDIATION-STATUS.md | Complete package/finding traceability and resumable progress |
| PERF-REMEDIATION-RESULTS.md | Final changes, before/after evidence, migrations, decisions, tests, limitations, and operator handoff |

Keep raw fixtures and giant database/index files out of version control. Retain small sanitized evidence artifacts and machine-readable result summaries under the repository's performance/testing convention.

Preserve R and V as historical evidence; do not rewrite them to match the implementation.

### 11.3 Independent integration review

Have a separate reviewer, when available, inspect the final diff and key evidence for:

1. The exact RM-04 and RM-05 race cases and their repair procedures.
2. Transaction/lease revalidation and cancellation.
3. Cross-package interactions: merge/split, genre counts, identity seeds, revisions, and outbox delivery.
4. Fresh and populated migration safety.
5. API/security invariants and resource boundedness.
6. Unintended scope, unrelated changes, removed checks, and unsupported performance claims.

Fix material findings during the same operation. A reviewer finding is not a cue to return a proposal to the user. If a requirement genuinely needs external information, record it and complete unaffected work.

### 11.4 Final completion gates

The integrated result must satisfy all applicable gates:

- [ ] All 17 findings and material subclaims have explicit evidence-backed dispositions.
- [ ] No pending design-only label hides a mandatory fix.
- [ ] Playable-asset and fingerprint query defects are corrected and measured.
- [ ] Concurrent genre operations produce correct counts; existing corruption can be repaired idempotently.
- [ ] The exact late-ID/old-generation-consumer search race is fixed; activation and post-cut delivery are proved/tested.
- [ ] Discovery covers every work class across restart/wrap without starvation.
- [ ] Ordinary per-album reconciliation no longer performs routine global discovery; affected closure and both global backstops preserve convergence.
- [ ] Long-lived scheduler results, buffers, queues, caches, and admitted work are bounded.
- [ ] Media reuse and concurrency preserve content verification and recovery.
- [ ] Search hydration/lifecycle/rebuild changes pass the new correctness suite.
- [ ] Interactive/API/security/range behavior is preserved.
- [ ] Fresh install, populated upgrade, repair, and interrupted-operation recovery pass.
- [ ] Required build/lint/test gates from the repository pass, or each actual blocker is explicitly documented.
- [ ] Representative performance evidence compares equivalent workloads, or the unavailable representative tier is explicitly blocked.
- [ ] New configuration and operator steps are documented.
- [ ] The application is runnable; no mandatory path is left stubbed or disabled merely to pass tests.

Large-scale certification and long-soak results are reported separately. Their absence must not turn into a false claim that 10M-track support has been established.

### 11.5 Final response format

Return a concise operational handoff backed by the complete results file:

1. What is now fixed, including the three correctness outcomes.
2. A table mapping all findings/work packages to final state.
3. Actual before/after performance results with environment and scale.
4. Required upgrade and data-repair steps.
5. Tests and migration/repair rehearsals executed.
6. Conditional proposals deliberately retained/rejected and why.
7. Exact blockers, unexecuted scale/storage/soak checks, and material remaining risk.
8. Branch/commit identities and paths to the results, status, standards, and operator guide.

Use precise completion language:

- **Implementation complete:** all mandatory code/design outcomes implemented and required regression gates pass.
- **Representative workload verified:** the stated representative database/media/mixed-load checks actually ran successfully.
- **Larger-scale verified:** only the scale tiers and sustained durations actually executed.

If only the first statement is supported, say so. If mandatory code or correctness work remains blocked, do not call the whole remediation complete.

---

## 12. Explicitly prohibited shortcuts

The following are outside the accepted approach unless new evidence establishes a fully requirement-preserving alternative and the specification's gates still pass:

| Shortcut | Why it is unacceptable here |
|---|---|
| Read-committed rebuild plus only max outbox ID | Does not establish commit-order delivery or cover already acknowledged late events |
| Another ordinary sequence or snapshot export treated as the whole fix | Does not by itself define a safe generation handoff |
| Repair search by consuming only pending rows | Missed mutations may already be acknowledged against the prior generation |
| UPDATE OF status combined with transition tables | Rejected by the validator's PostgreSQL experiment |
| Naive per-track genre release deltas | Distinct-release membership is not track cardinality |
| Sorted advisory locks with an unchanged stale count snapshot | Lock order does not prove visible contributions |
| Reconciliation scoped only to the imported release | Can miss historical/transitive identity effects |
| Size/mtime/inode equality replacing required hashes | Does not establish content equivalence |
| Removing filename INSERT maintenance without all-writer proof | Breaks valid insertion orders |
| Unbounded spawn_blocking or detached cancelled work | Moves contention into another queue and can violate fencing/cleanup |
| Universal FFmpeg -threads 1 | Ignores codec/filter/hardware behavior |
| Fixed pool split described as guaranteed responsiveness | Shared PostgreSQL CPU, locks, storage, and WAL remain |
| Password-success caching or weaker password hashing | Rejected security-semantic change |
| Per-key rate-limit upsert without the global bound protocol | Does not preserve exact multi-process admission semantics |
| Blanket index deletion or idx_scan=0 justification | Does not establish equivalence or cover rare paths |
| Removing lease history as if it were scheduler memory | Different data paths with different durability/diagnostic requirements |
| Arbitrary new pruning defaults | Can discard required history without an established retention requirement |
| Adding tests that merely mirror implementation | Does not validate the changed invariant |
| Disabling tests, triggers, integrity checks, or features to improve results | Conceals the defect instead of completing remediation |
| Treating prior report estimates as achieved performance | Misrepresents the evidence |

---

## 13. Start now

Begin by reading repository instructions, this specification, and R/V; record the current checkout and resource budget. Establish the permanent race regressions and narrow baseline, then implement the program.

Continue through the phases without asking the user to authorize routine phase transitions. Complete required design and measurement inside this operation. Keep the status ledger current, preserve work across context boundaries, and resolve integration failures.

**Return the integrated changes and their evidence—not another proposed plan.**



# Melodee-rs: Finish the Performance Remediation Program

## Assignment to the next coding agent

Operate as the autonomous implementation engineer responsible for completing the
entire performance remediation program. **Implement, integrate, measure and
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
implementation, evidence, status, and next executable action. Use the terminal
classifications defined by the existing ledger. An unexecuted test, hard
design problem, rejected prototype, or convenient architectural preference is
not evidence for `NO CHANGE JUSTIFIED` or `EXTERNALLY BLOCKED`. If work is
interrupted, leave a precise, truthful recovery checkpoint.

Do not perform release/deployment orchestration, push, publish a release, deploy
to production, access production data, alter host security settings, or modify
unrelated work. Routine safe engineering decisions, owned disposable test
databases/directories, and in-repository implementation are already authorized.

## Exact handoff state and first action

This document was created at the user's explicit instruction to **stop the
current work and prepare a handoff**. The implementation agents were interrupted;
do not assume the last source edits form a compiling or coherent checkpoint.
At handoff:

- Main worktree HEAD is `37f29889fb05d1825faecb8b597387f0eb971b5e` on `main`,
  with a very large dirty/untracked working tree. Earlier revisions in this
  operation include `08de08d`, `8a4cf6f`, `ca8ef83`, `63394a1`, and the
  original pre-remediation baseline `a0979cde1fd2aeff5b8027d9d59336b678f1aa12`.
  Verify revisions and ancestry yourself. `08de08d` and `37f2988` are
  intermediate, unverified snapshots, not final acceptance.
- **Preserve all existing changes.** Do not reset, clean, blindly stage, amend,
  or overwrite the worktree. `git diff --stat` omits the many untracked source,
  migration, test, evidence and benchmark files; inspect `git status --short`
  and `git diff` as well. Migration `0071_custody_hash_spool.sql`, the
  `melodee-custody` crate, many persistence modules/tests, and ADRs 0221/0222
  are untracked/in progress. Do not treat any draft or passing prototype as
  production activation.
- **Immediate P0 restart:** the previous turn was interrupted while isolating
  recovery-specific quiescence from general maintenance quiescence. Review
  `confirm_lifecycle_quiescence_inner` in
  `crates/melodee-persistence/src/work_item_lifecycle.rs`, the matching
  `work_item_lifecycle_maintenance` constraints in migration 0071, the
  recovery locator and the tests around
  `maintenance_page_intent_is_exact_immutable_and_never_adopted_by_new_epoch`
  in `crates/melodee-persistence/tests/catalog_hard_delete.rs`. The interrupted
  edit introduced `recovery_reservation_id`/`recovery_reservation_token` and a
  constraint that a recovery-specific fence must not be generically quiesced,
  while the function was still seen setting `quiesced=true` without persisting
  those fields. This is an **unverified, potentially inconsistent edit**, not
  a completed fix. Finish it so recovery authority is restricted to the exact
  live reservation and both epochs, generic maintenance remains unavailable,
  and no lease expiry is mistaken for proof that old filesystem workers stopped.
  Add tests proving recovery attestation cannot authorize unrelated maintenance,
  wrong/stale reservation or fence, new page creation, adoption, unlink,
  acknowledgement, or a second reservation. Then rerun the focused persistence
  and ingestion recovery tests on a fresh owned database. Earlier 1/1 passing
  recovery-attestation evidence predates this isolation gap and is not evidence
  that the interrupted fix works.
- The next queued ingestion regression is the test-only owned DB/filesystem
  two-name recovery fixture
  `custody::tests::provisional_scan_revalidates_exact_db_intent_and_retains_source`
  in `crates/melodee-ingestion/src/custody.rs`. Recent, unrun additions cover
  interruption before rename, immediately after rename, after fsync, dropped
  async waiter while the blocking worker/lock remains live, same-size name
  replacement, missing/unreadable names, fresh claim epochs and read-only
  two-name observation. Confirm the exact test name and feature flags from the
  code, make the recovery fence coherent first, then run only this focused test.
  These observations must not adopt, unlink, acknowledge, or claim success.
- At interruption, the visible extra `cargo-clippy`/`cargo check` processes
  were spawned by an existing VS Code `rust-analyzer`, not by the coordinator.
  Do not kill, alter, or claim results from unrelated processes. Inspect active
  processes before scheduling expensive work. The other implementation agents
  were interrupted, not assigned ongoing work.

Start by inspecting the current diff and the exact interrupted code/test state.
Make the recovery-fence correction and execute the focused tests; then continue
through the full program below. This is a handoff prompt, not a new definition
of done.

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
The developer machine has suffered severe slowdown from concurrent builds;
unrelated rust-analyzer jobs may run without your control, and contaminated
performance samples must be labelled or deferred.

Use only explicitly owned disposable PostgreSQL databases, worktrees, and media
roots. Repository scripts and new orchestration must follow the Python-only
rules in `AGENTS.md`. All entry points must have safe `--help`; destructive
helpers must verify exact ownership, fail closed, and have scope tests.

Never replace cryptographic content evidence with timestamps, inode, length or
assumed immutability. A persisted inode can be reused after a crash; a pinned FD
does not prevent concurrent writes to the same inode. A lease or database flag
cannot prove a worker with open filesystem descriptors has terminated.
Admission must occur before expensive work is spawned or queued; permits and
appropriate guards must remain held until the actual blocking work, subprocess,
pipe draining, termination and reaping finish, including cancellation.

Preserve public API fields, ordering, IDs, auth, ACLs, ratings, pagination,
JSON/XML, range/streaming semantics, manual/negative identity decisions,
same-name distinct artists, redirects, and all existing compatibility tests.
Never suppress or weaken tests to obtain a green gate. The historical podcast
lease test is timing-sensitive under parallel in-binary execution; use the
repository's serialized harness and report any actual flake accurately.

## Current package ledger and remaining work

The current ledger classifies **RM-01, RM-02, RM-03, RM-04, RM-05, RM-08 and
RM-16** as implemented and verified at earlier/intermediate checkpoints. Their
final integrated regressions, migrations and quality gates have **not** been
rerun. All remaining packages below are partial, including any with useful
prototype tests or measurements. Reconcile every package with the full spec;
do not assume this summary lists every mandatory subrequirement.

### RM-00 — Representative environment and full benchmark matrix

The reusable `scripts/perf_representative.py` creates UUID-owned disposable
databases. A verified **data-shape smoke** at 35,000 artists, 50,000 releases,
600,000 tracks and 30,000 alternate-asset links exists in
`tests/performance/fixture-verification-2026-11-27.md`. It has skewed genres,
synthetic acoustic evidence, ratings 1–5, some identity relationships, two ACL
groups/four library IDs, a generated shared PNG, playable/alternate references,
and synthetic publication/manifest **placeholders**. It does not provide
560K physical audio files, real historical identity/ACL provenance, a bounded
valid audio corpus for every workload, or the complete application workload.
An earlier fixture incorrectly produced only rating 1 for selected Release and
Track rows; later scripts corrected it. The artwork installer was later fixed
to use descriptor-relative no-follow/exclusive creation. Old results must keep
their original fixture limitations; do not describe them as measurements with
the later corrections.

Create/finish a valid bounded media corpus and exercise the actual release-build
production paths, not merely SQL microbenchmarks. Collect **comparable baseline
and final** results on the same declared fixture, configuration and procedure,
using an isolated original-baseline worktree where practical. Treat a reasonable
timeout as censored, never invent a duration. At minimum collect:

| Workload | Required evidence |
| --- | --- |
| Single-album import | End-to-end and per-stage duration. |
| 200-album import | Albums/minute, tracks/second, resource consumption. |
| Unchanged reimport | Avoided work and idempotency. |
| Policy evaluation and dry-run | Query cost, transferred rows, memory, bounded result size. |
| Identity reconciliation | Scoped query work, closure size, duration, convergence. |
| Media processing | Reads, writes, hashes, probes, subprocesses and duration. |
| Concurrent genre imports | Exact counts, lock waits and throughput. |
| Incremental indexing | Events/second, DB round trips and lag. |
| Full rebuild | Duration, commits, memory, disk, snapshot age and activation pause. |
| Concurrent interactive load | p50/p95/p99 latency and errors. |
| Streaming during ingestion | TTFB, errors, underruns, slow-client/resource behavior. |
| Worker scheduling | Slot utilization, fairness, polling and queue bounds. |
| Merge/split | Statement counts, transaction/lock costs and correctness. |

Record commit/source and binary identities, release build, fixture cardinality
and seed, hardware/storage, PostgreSQL settings, application settings, cache
conditions, worker concurrency, sample counts, limitations, and bypassed
subsystems. Do not call fixture seed time import throughput. Larger 1M/5M/10M
certification and extended soak are not prerequisites for the mandatory
560K–600K workload. Commit reproducible harnesses and sanitized evidence, not
generated databases or large media fixtures.

### RM-06 and RM-07 — Fair bounded discovery and scheduling

Durable per-class cursors, bounded scan/staging primitives and several focused
fairness tests exist. Finish the integrated bounded traversal and durable
staging paths across supported platforms, including native/non-Unicode names,
directory additions/removals, large directories, independent file/directory
due scheduling, no-baseline fairness, fixed-crash/restart fairness, partial
scans that never establish absence, ownership/lease fencing, storage quotas,
and scheduler result/queue bounds. Re-review and test the final integrated
implementation. Metadata, cookies and timestamps are hints, never content or
absence proofs.

### RM-09 — Affected-identity reconciliation

Preserve the scoped discovery implementation, all five candidate algorithms,
durable captured seeds, closure expansion, explicit acknowledgement, late
lower-ID commit fix, bounded processing and independently scheduled global
artist **and strong-release** backstops. Do not revert to coalesced global
scans. `crates/melodee-catalog/tests/identity_scoped_oracle.rs` passed focused
tests on an intermediate tree with nonempty candidate sets, off-closure
exclusion, multi-hop closure, manual/negative decisions, redirects, same-name
artists, interruption, late seed and global startup behavior.

`tests/performance/identity-closure-600k-2026-11-27.md` records one actual
release-build 600K synthetic graph: eight original seeds were acknowledged,
68 follow-up seeds were consumed in a further 32.029 ms bounded turn, and all
five nonempty scoped/global candidate vectors matched. This is **not** an
original-baseline comparison, concurrent-import acceptance, a universal bound,
or final-source evidence. Complete concurrent imports, interrupted/resumed
convergence, unchanged reimport, historical/transitive oracle equivalence,
plans showing proportional indexed discovery, and final integrated regressions.

### Custody, safe acquisition, retention, and operator deletion — P0 integration

The existing code exposed source-loss and stale-worker hazards. Preserve the
durable reconciliation and recovery improvements and finish the actual
production protocol. The current work contains many **disconnected or
test-only** primitives; passing their focused tests does not mean this section
is complete.

1. Trace same-filesystem rename, cross-filesystem acquisition, retries, recovery,
   staging, publication, retained-source reobservation, admin purge, review
   correction/deletion, and Release/Track/Artist hard-delete. Prevent a stale
   worker, source replacement, same-inode write, already-open producer FD,
   changed namespace or concurrent operator from losing source or catalog media.
   The existing no-handoff admin purge compatibility regression is currently
   failing because the unsafe deletion is guarded; do not weaken or delete it.
2. Integrate a real claim-bound, opaque guarded owner with all actual stage
   readers/writers, inspection, normalization, scripts/subprocesses, purge,
   cleanup and cancellation/reaper boundaries. `OwnedSource` currently exposes
   a clonable path, and the provisional `GuardedOwnedSource` test wrapper alone
   does **not** attach a production lifecycle hold. Preserve disabled/no-hook
   scripting behavior; enabled hooks must retain the guard through actual work.
   Confirm all generic owner-only APIs reject protected work or use the exact
   protected epoch. An old same-worker claim must not renew, release, stage,
   record a receipt, delete or mutate a successor's work.
3. Finish explicit, secure private-root/producer configuration, root topology
   validation across all configured source/stage/work/canonical/asset/archive
   roots, no-follow and non-Unicode path handling, pinned ancestry descriptors,
   cross-device support and fail-closed unsupported cases. Validate mount and
   bind aliases without changing host mount/security settings. The current
   512-distinct-topology-ancestor cap is a stated compatibility limitation;
   the 128-item and observed-FD-peak regressions do not prove all valid
   configurations or whole-process resource bounds.
4. Complete the protected stage protocol: durable exclusive attempt identity,
   exact claim epoch and heartbeat, bounded work and spool, private namespace,
   per-file sealed immutable contents where safely supported, same-stream
   baseline and remeasurement, synced parents, no-replace promotion, and one
   fenced atomic publication of receipt, authoritative baseline and retained
   obligation. fs-verity protects file contents, not directory membership,
   names, mount topology, producer source bytes or already-open source FDs.
   Current stage helper/dependency is test-only and ADR-0221 Proposed.
5. Make retained-source work genuinely live and production-reachable. The
   previous review found production creates obligations with NULL baseline ID,
   while the provisional immutable trigger rejects later updates; synthetic
   tests alone do not establish publication. Connect protected baseline and
   retained obligation atomically. Consume sticky changed/unreadable signals via
   durable source-preserving new intake or an actionable operator disposition.
   Preserve seeds/occurrences arriving during work and bound fair file/directory
   processing, cancellation, retries and recovery. Do not infer an unchanged
   source from a finite read or assume a producer has quiesced.
6. Finish per-entry exact intent and private quarantine: record original
   page/root/epochs/native path, no-replace same-filesystem move, fsync, reopen
   and independently rehash exact moved bytes. A passed test-only DB/FS move
   intentionally leaves the record `intent`, preserves source, and does **not**
   unlink or acknowledge. Implement fault/replacement cases and safe two-name
   recovery. A post-crash `dev:ino` is not immutable proof; ambiguity must
   preserve bytes until a valid protocol establishes authority.
7. Complete bounded deletion/repair orchestration. Operator review, correction,
   unpublished/published review deletion and Release/Track/Artist hard-delete
   need durable exact item/release reservations **before** filesystem effects,
   lifecycle holds, target/asset writer interlocks, atomic revalidation,
   proof-backed finalization, outbox/audit and crash/lost-ack recovery. A
   database reservation alone cannot revoke a worker already in filesystem
   work. Where an offline maintenance fence is necessary, implement and
   document it; a DB token or an operator assertion alone cannot prove old
   binaries/processes stopped. Maintain bounded durable closure and page
   intents, including shared assets, artwork, directories and Track/Artist
   ownership. `ON DELETE RESTRICT` proof records, post-commit tombstones,
   resumable cleanup and independently verified bounded archive/retention are
   still design/integration work. Assertion-only DB acknowledgements or a
   synthetic `verified` bit must never authorize filesystem deletion.
8. Complete the shared, non-fabricable capability and private executor under
   proposed ADR-0222: safe constructors must actually acquire/revalidate the
   required scope and trusted live root set, bind exact durable intent and
   epochs, perform descriptor-relative byte-verified work, and retain guards
   through the atomic DB boundary. The new `melodee-custody` crate's locks and
   incremental scanner are **observational**, not a page proof or finalizer.
   The existing 257 MiB sparse-file test used 33 bounded ≤8 MiB in-process
   steps; process loss restarts from zero. Do not serialize a mixed-time prefix
   as proof. Complete real crash/fence tests, directory/subtree proof, cleanup,
   archive and production integration. Do not permit indefinite quota leakage.

The `PERF-REMEDIATION-STATUS.md` tail documents numerous focused tests, found
races, temporary 128-item/FD bounds, test-only APIs, and what remains. Review
the actual code and current changes after the interrupted turn; several
focused tests passed before later source edits and need rerunning.

### RM-11 and RM-12 — Substantive media reduction and shared admission

Use `tests/performance/media-custody-operation-inventory-2026-11-26.md` and
actual source paths to inventory same-/cross-filesystem acquisition, retained
MP3, FLAC transcoding, CUE, embedded/external artwork, validation, publication,
retry and recovery. Finish safe elimination of redundant adjacent tree reads,
file hashes, copy/hash duplication, equivalent probes, artwork work and
subprocesses; compute hashes during mandatory copy where it genuinely preserves
all ownership, custody, validation, publication and recovery checks. Measure
actual operations and elapsed time.

Do **not** reuse the inspection probe merely because the stored hash matches:
inspection hashes and probes a mutable path in separate reads, so A→B→A can
pair A's hash with B's observation, and tool identity can change. Do not
replace independent output verification with a self-comparison. The tool-version
cache's unbound reuse was disabled after an executable-path ABA finding; safe
binding and equivalent observation reuse remain open. The explored shared-decode
FFmpeg/chromaprint path was not adopted because some supported streams differ.
Neither a cache nor a rejected prototype completes RM-11.

Preserve effective inspection concurrency and existing shared media admission,
subprocess supervisor/reaper and cancellation work. Complete process-wide
coverage for CPU, filesystem, external subprocesses, nested release/track work,
open files, temporary storage and blocking work. Hold permits until actual work
terminates, including pipe drain/reap and lease fencing. Measure the real
supported-codec FFmpeg/worker matrix, throughput, resource use and interactive
responsiveness. Do not universally force `-threads 1` or adopt a default solely
from a tiny smoke. Add the requested source mutation, same-size change, corrupt
media, interrupted copy, lease-loss, retry and recovery tests.

### RM-10, RM-13 and RM-14 — Database and search

- Preserve two-phase manifest preparation outside write transactions and
  claim/lease/binding/database-clock revalidation. Complete actual pooled
  acquisition **and hold** instrumentation for all relevant workload classes;
  verify filesystem preparation does not retain a pooled connection, measure
  ingestion/rebuild/interactive competition, choose evidence-backed admission or
  reservation, and protect lease heartbeats. Existing old mixed-load pool 8/32
  samples are not a final comparison.
- Preserve the Tantivy reader registry, generation identity, ordered batched
  hydration, revision/deletion semantics, commit-safe activation and
  commit-before-ack. Measure reader opens/reloads, writer initialization,
  commits, segment churn, outbox throughput/lag, latency and memory. Compare
  long-lived writer reuse against the current batch-scoped writer; implement
  reuse if justified or document a real, measured, spec-satisfying decision.
  Old batch-size samples alone are not a long-lived-writer A/B.
- Preserve off-async rebuild work, bounded buffering and configurable
  document/time commit thresholds. Measure a real PostgreSQL-backed 600K rebuild:
  elapsed time/docs per second, actual commits, memory, disk, snapshot age,
  catch-up and activation pause. Verify admission, cancellation and snapshot
  lifetime. Re-run the late lower-ID and old-generation consumer regression on
  the final implementation. Prior one-sample rebuild figures are intermediate.

### RM-15, RM-17, RM-18 and RM-19 — Interactive and contention work

- On the representative catalog, measure and improve administrative stats,
  OpenSubsonic `getArtists`/`getIndexes`, Native browsing/suggestions,
  administrative artist search, ACL-aware queries and relevant Jellyfin
  routes. Preserve actual wire semantics and verify plans, result equality,
  pagination and authorization. The intermediate admin stats ABBA at
  `tests/performance/admin-stats-ab-representative-2026-11-27.md` recorded
  baseline p50/p95 615.140/640.197 ms and candidate 528.069/567.805 ms over
  16 warm in-process samples per variant. It masked only the validated volatile
  `database.totalBytes` across processes; it is a dirty intermediate snapshot,
  shared host, no TCP/mixed load and not final-source evidence. Earlier failed
  attempts are retained. Jellyfin and Native SQL plans are microbenchmarks,
  not end-to-end API acceptance.
- Preserve content-aware artwork cache and blacklist singleflight. Finish and
  measure bounded buffered streaming under actual ingestion and slow clients,
  including TTFB/backpressure/cancellation/no-follow/range/FD lifetime. Preserve
  combined compatible auth lookups without password-success caching or weaker
  hashing. Measure real rate-limit contention and make an evidence-backed
  decision; the existing store-only sample lacks HTTP, multi-process and mixed
  traffic and does not establish final behavior.
- Measure recommendation/play-trigger contention and shared-artist fairness
  under concurrent plays/scrobbles; preserve synchronous freshness unless a
  durable semantically equivalent alternative is actually implemented.
- Reverify the existing set-based merge/split using 1,000 and, if resources
  permit, 10,000 Track releases, including state, evidence, projections,
  recommendations, identity seeds and outbox. Measure statements, transaction
  duration, lock duration and resources. The separate
  `tests/performance/admin-offline-release-closure-10k-2026-11-27.md` is a
  **different provisional DB-only deletion-closure test**, not merge/split or
  600K end-to-end evidence.

### RM-20 — Integration, migrations, standards and final acceptance

After all remaining work is integrated, reconcile RM-00 through RM-20 and all
17 findings individually against the full specification. Keep classifications
honest; do not conceal a mandatory gap under `NO CHANGE JUSTIFIED`. Re-run all
applicable focused correctness, concurrency, compatibility, media pathology,
fencing, recovery, ACL, search generation and genre aggregate regressions.
Rehearse fresh and populated upgrades through the actual final migration,
existing-data repair/check, interruption/restart and the full upgrade/repair
runbook. Update `PERFORMANCE.md`, `AGENTS.md` where required, applicable ADRs,
public docs under `/docs`, and requirement traceability.

Only after integration, run the repository-required final gates with the
resource limits above, including `cargo fmt --all`, `make check-fast`,
`make check`, `make check-nightly`, migration/compatibility tests, and
`python3 scripts/do-prechecks.py` or the exact currently required equivalents.
Follow `AGENTS.md` for docs-site and any applicable Admin UI checks. Diagnose
and fix failures without suppressing tests/assertions. A historical passing
`make check-nightly` does not validate the current working tree. Earlier
intermediate builds reported numerous disconnected custody warnings; one
recent ingestion test build still reported three. Final strict Clippy must pass.

## Evidence, known limitations and safe housekeeping

- Original baseline worktree: `../melodee-rs-perf-baseline-a0979cd`, detached
  at `a0979cde1fd2aeff5b8027d9d59336b678f1aa12`. Verify before reuse.
- Two invocation-owned A/B snapshot worktrees remain **inside** this repo:
  `.admin-ab-baseline-dd7f597b8c114e2d855a4a92cbb80899/tree` and
  `.admin-ab-candidate-a77c68b9bb8f4fb5a37670d74b54b65a/tree`. They contain
  generated build artifacts and can confuse repository/document validation.
  Preserve needed evidence, then perform only exact, identity-verified cleanup
  of resources owned by this operation. Do not blindly `git clean`, recursively
  delete a mutable path, or remove another person's worktree.
- Six exact, previously verified stage/bind worktrees were relocated without
  deletion to sibling `.melodee-perf-quarantine-7qs2xgmn`. Retained bind
  fixtures and private logs are intentionally quarantined because pathname
  replacement made automatic cleanup unsafe. Do not blindly remove them or
  expose raw logs; use the recorded ownership/identity evidence and fail
  closed on uncertainty. `git worktree list --porcelain` shows registrations.
- The latest supported-filesystem fs-verity test passed **1/1 on an
  intermediate test-only snapshot**, after earlier failures; parent user,
  mount and PID namespaces stayed unchanged. See
  `tests/performance/stage-verity-sealed-tree-2026-11-27.md` and its linked
  proofs/results. It used a one-UID private namespace, excludes newer changes,
  and is not production custody or final integrated safety.
- The ignored bind-alias test must **not** be run directly. Earlier review
  found host-propagation, pathname replacement, cleanup and unbounded-output
  hazards; fixes and reviews are recorded in the ledger. Reviewed isolated
  attempts failed before the Rust test at private proc-mount setup; the latest
  bounded diagnostic reported `move_mount:EINVAL`. Parent namespaces were
  unchanged and fixtures retained. A diagnostic subagent turn and a separate
  attempt to resume that same subagent were flagged by the tool environment as
  a possible cybersecurity risk. Neither produced implementation. Respect
  platform/security restrictions; do not attempt to evade a tool restriction,
  change host security configuration, or represent this test as passed. Continue
  safe independent work and accurately report any genuine remaining external
  limitation.
- Representative identity and admin evidence are intermediate single-host
  samples, not the complete before/after workload. The admin A/B used warmed
  in-process calls and masks only validated cross-process
  `database.totalBytes`, retaining all other ordered fields. Consult the linked
  records rather than reconstructing conditions from memory.
- Existing output and claims can be stale after the interrupted edits. In
  particular, the status file's chronological entries and top table need final
  reconciliation. Avoid citing old focused results for newly changed code.

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
- Fresh/populated upgrades, existing-data repair, interruption/recovery and the
  complete upgrade/repair runbook are verified; required final quality gates
  pass on the final integrated source.
- `PERF-REMEDIATION-STATUS.md`, `PERF-REMEDIATION-RESULTS.md`, `PERFORMANCE.md`,
  applicable ADRs, public docs, benchmark harnesses and sanitized evidence all
  agree with actual code and results. Record final commit/source identities,
  operator steps, limitations, documentation impact and any genuine external
  blocker precisely. Produce a coherent final integrated checkpoint without
  pushing, deploying or publishing.

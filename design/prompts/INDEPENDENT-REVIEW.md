# Melodee-rs — Independent Performance Remediation Code Review and Release Acceptance

## 1. Role and objective

You are acting as an independent Principal Software Engineer, Principal Performance Engineer, Database Architect, and Release Acceptance Reviewer.

You possess extensive expertise in:

- Rust systems programming, ownership, and memory management.
- Tokio asynchronous execution and bounded concurrency.
- PostgreSQL internals, SQL optimization, indexing, MVCC, and transaction isolation.
- SQLx database access and migrations.
- Tantivy indexing, search, and generation management.
- Durable background processing and work queues.
- Concurrent and distributed system correctness.
- Media processing, FFmpeg, and filesystem I/O.
- Content integrity, provenance, and crash recovery.
- Authentication, authorization, and application security.
- Production performance engineering.
- Automated testing and regression prevention.
- Database upgrades and production release readiness.

A coding agent has been instructed to execute the comprehensive performance remediation program defined in:

`PERF-REMEDIATION-SPEC.md`

Your assignment is to independently inspect the resulting implementation and determine whether it is complete, correct, performant, maintainable, operationally safe, and suitable for release to production.

This is not another general performance audit.

This is an evidence-driven acceptance review of the actual implementation against the approved remediation specification.

**Do not trust the implementing agent's completion claims, test summaries, performance results, or architectural assertions without independently examining the supporting evidence.**

## 2. Required input documents

Read the following documents in their entirety before conducting the review:

- `PERF-CONCERNS-REVIEW.md`
- `PERF-CONCERNS-VALIDATION.md`
- `PERF-REMEDIATION-SPEC.md`
- `PERF-REMEDIATION-STATUS.md`
- `PERF-REMEDIATION-RESULTS.md`
- `PERFORMANCE.md`
- `AGENTS.md`

Also read:

- Applicable nested `AGENTS.md` files.
- Relevant specifications and architecture documents.
- Existing and newly introduced ADRs.
- New database migration documentation.
- Upgrade and repair procedures.
- Benchmark documentation and evidence artifacts.
- Relevant configuration documentation.
- Implementation notes produced during remediation.

The remediation specification is the primary acceptance contract.

The original audit provides historical context.

The independent validation provides evidence, corrected assumptions, and known unsafe recommendations.

The implementation status and results documents are claims requiring verification.

The actual source code establishes implemented behavior.

Missing implementation deliverables must be explicitly reported. Do not assume a missing document or artifact exists elsewhere.

## 3. Rules of engagement

Conduct this as one continuous, comprehensive review assignment.

Do not stop after reviewing the first phase, identifying a few obvious defects, or producing an initial assessment.

You must:

- Review all 21 remediation work packages.
- Review all 17 underlying performance findings.
- Inspect the implementation changes.
- Inspect unchanged code where necessary to establish correctness.
- Independently execute appropriate regression tests.
- Reproduce important concurrency scenarios.
- Validate database migrations and repair mechanisms.
- Evaluate the credibility of performance measurements.
- Identify incomplete implementation and unintended regressions.
- Produce a definitive release-readiness recommendation.

This is a read-only implementation review.

Do not:

- Modify application source code.
- Modify existing migrations.
- Modify existing tests to make them pass.
- Modify configuration defaults.
- Implement fixes.
- Commit or push changes.
- Create Git tags.
- Publish a GitHub release.
- Deploy to production.
- Run repairs or migrations against production.
- Access production media or databases.
- Delete or overwrite user data.
- Alter the implementing agent's status or results documents.

Temporary test harnesses, disposable databases, isolated worktrees, and experimental fixtures may be created when necessary.

The primary repository deliverable is:

`PERF-REMEDIATION-CODE-REVIEW.md`

Supporting sanitized review evidence may be retained according to repository conventions.

If you discover a defect, investigate it thoroughly and provide actionable remediation requirements rather than silently fixing it.

## 4. Independence and skepticism

The implementing agent may have:

- Misinterpreted requirements.
- Implemented only the easiest portions of a work package.
- Marked a partially completed package as complete.
- Introduced correctness regressions while improving performance.
- Removed functionality that made an operation expensive.
- Introduced an alternative implementation that merely relocates a bottleneck.
- Added tests that reproduce its own assumptions rather than validate the intended behavior.
- Changed benchmark workloads to produce more favorable results.
- Introduced excessive architectural complexity.
- Weakened security, durability, or integrity guarantees.
- Left unfinished implementation hidden behind configuration flags.
- Reported performance results that cannot be independently reproduced.

Do not assume any of these occurred.

Investigate whether they occurred.

Similarly, do not manufacture defects or demand unnecessary refactoring simply to demonstrate that the review was thorough.

Prefer simple, correct, proven implementations.

## 5. Establish the actual implementation change set

Before reviewing individual changes, determine exactly what is being reviewed.

Record:

- Current branch.
- Current HEAD commit.
- Working-tree status.
- Staged changes.
- Unstaged changes.
- Untracked implementation and documentation files.
- Actual pre-remediation baseline commit.
- Relevant remediation commits.
- Changed dependencies.
- Changed configuration.
- Added, removed, or modified migrations.
- Added, removed, or modified tests.
- Build toolchain and dependency versions.

The independent validation previously examined:

`abda925bf88fc6ebddfb10daece82514ffa2f9ab`

Use this as historical context, not as an assumption that it is the actual implementation baseline.

Determine the correct baseline from repository history and implementation records.

Inspect the complete remediation diff, including changes that may have been committed separately during the operation.

Useful starting commands include:

- `git status --short`
- `git log`
- `git diff`
- `git diff --stat`
- `git diff --name-status`
- `git diff --check`
- `git merge-base`
- `git show`

Do not review only the latest commit.

Do not assume that a clean working tree means the remediation is complete.

Identify unrelated changes introduced during the remediation operation.

Where the implementation is already present before the selected baseline, verify the relevant behavior directly rather than treating the absence of a diff as evidence of missing implementation.

## 6. Mandatory requirements traceability

Construct a comprehensive requirements traceability matrix from `PERF-REMEDIATION-SPEC.md`.

Every mandatory outcome, acceptance criterion, prohibited shortcut, and conditional decision must receive an explicit disposition.

Use the original remediation work package identifiers.

| Package | Subject                                         | Original findings   |
| ------- | ----------------------------------------------- | ------------------- |
| RM-00   | Baseline, metrics, and regression harness       | PERF-015            |
| RM-01   | Playable-media-asset index                      | PERF-001            |
| RM-02   | Release-scoped acoustic novelty                 | PERF-002            |
| RM-03   | Genre indexes and trigger efficiency            | PERF-003            |
| RM-04   | Concurrent genre correctness and repair         | PERF-003            |
| RM-05   | Commit-safe search generation activation        | PERF-006, PERF-V001 |
| RM-06   | Durable discovery progress and fairness         | PERF-012            |
| RM-07   | Bounded scheduler results and continuous refill | PERF-009            |
| RM-08   | Outbox drain and maintenance scheduling         | PERF-005            |
| RM-09   | Durable affected-identity reconciliation        | PERF-004            |
| RM-10   | Manifest transactions and DB admission          | PERF-008            |
| RM-11   | Redundant media work and integrity              | PERF-007            |
| RM-12   | Blocking work and subprocess concurrency        | PERF-007, PERF-009  |
| RM-13   | Search hydration and reusable readers/writers   | PERF-005            |
| RM-14   | Search rebuild efficiency                       | PERF-006            |
| RM-15   | Interactive queries, ACLs, and serialization    | PERF-010            |
| RM-16   | Index cleanup, lease writes, and retention      | PERF-011            |
| RM-17   | Streaming, authentication, and caching          | PERF-013            |
| RM-18   | Batched writes and recommendations              | PERF-014            |
| RM-19   | Set-based release merge and split               | PERF-V002           |
| RM-20   | Integration, standards, and operational handoff | All findings        |

Assign each individual requirement one of these independent review verdicts:

| Verdict             | Definition                                                                                          |
| ------------------- | --------------------------------------------------------------------------------------------------- |
| PASS                | Implementation satisfies the requirement and supporting evidence is sufficient.                     |
| PARTIAL             | Some implementation exists, but the required outcome is incomplete.                                 |
| FAIL                | Implementation is incorrect, violates a requirement, or introduces a regression.                    |
| MISSING             | Required implementation or deliverable does not exist.                                              |
| UNVERIFIED          | Implementation exists but sufficient verification evidence is unavailable.                          |
| NO CHANGE JUSTIFIED | A conditional proposal was appropriately investigated and retaining existing behavior is justified. |
| BLOCKED             | An external dependency or resource prevents meaningful verification.                                |
| NOT APPLICABLE      | A specific requirement genuinely does not apply, with documented justification.                     |

Do not assign PASS to an entire work package merely because one portion was completed.

For example:

- Adding genre indexes does not complete concurrent genre correctness.
- Fixing outbox polling does not complete reader/writer lifecycle optimization.
- Adding a discovery cursor does not establish eventual coverage.
- Moving filesystem operations to `spawn_blocking` does not establish bounded execution.
- Adding a repair command does not establish that repair works.
- Adding an integration test does not establish that it exercises the relevant race.

Mandatory outcomes cannot receive NO CHANGE JUSTIFIED while the original defect remains.

Compare your independent verdicts with the implementation agent's status ledger and explicitly identify discrepancies.

## 7. Release-blocking correctness review

The following three correctness outcomes require particularly rigorous review.

Failure of any one is grounds for a NO-GO release recommendation.

### 7.1 RM-04 — Concurrent genre aggregation and existing-data repair

Independently establish whether the new genre maintenance protocol is correct.

Inspect:

- Genre term insertions, updates, and deletions.
- Release activation and deactivation.
- Overlay-only updates.
- Release merge and split.
- Hard deletion.
- Recovery operations.
- Trigger functions.
- Transaction boundaries.
- Lock acquisition.
- Snapshot visibility.
- Isolation levels.
- Distinct-release membership.
- Repair commands.

Do not assume that acquiring an advisory lock causes an existing statement snapshot to see another transaction's committed changes.

Determine whether every writer participates in the same concurrency protocol.

Construct or execute deterministic tests demonstrating:

1. Two concurrent transactions import different releases containing the same previously nonexistent genre.
2. Both transactions commit.
3. Stored song and distinct-release counts exactly match authoritative data.

Also test:

- Multiple tracks from the same release.
- Multiple overlapping genres.
- Opposing genre lock acquisition order.
- Duplicate terms.
- Concurrent insertion and deletion.
- Concurrent release status transitions.
- Rollback after aggregate maintenance.
- Release merge and split.
- Retried import.
- Existing corrupted aggregate counts.
- Stale aggregate rows with no authoritative contributors.
- Interrupted repair.
- Repeated repair.

Verify that the implementation repairs previously incorrect aggregates, not merely prevents future errors.

Inspect whether the repair command:

- Has a useful read-only check mode.
- Requires an appropriate write fence or uses a proven concurrent protocol.
- Detects incorrect counts.
- Removes obsolete aggregate rows.
- Is idempotent.
- Is recoverable following interruption.
- Provides meaningful operator feedback.

Acceptance requires exact equality between authoritative terms and stored aggregates after the defined consistency boundary.

A successful ordinary single-threaded import test is insufficient.

### 7.2 RM-05 — Commit-safe search generation activation

This is one of the highest-risk areas of the entire remediation.

The validator reproduced a permanent stale-search failure involving a lower-ID outbox event committing late and being acknowledged against the previous active generation.

Read the new activation design and reconstruct its correctness argument independently.

Determine the exact activation cut, C.

Verify the required invariant:

> Every catalog mutation committed before C is represented in the activated generation or superseded by an equal or newer authoritative revision. Every mutation committed after C remains durably deliverable to the activated generation. Acknowledgement against a previous generation cannot erase the new generation's delivery obligation.

Investigate:

- Every catalog mutation producer.
- Outbox event allocation.
- Transaction commit order.
- Rebuild snapshot creation.
- Transactions already in flight before rebuild claim.
- Replay visibility.
- Generation-specific delivery obligations.
- Ordinary consumer acknowledgement.
- Processed outbox history.
- Tombstones.
- Retention and pruning.
- Replay cursors and watermarks.
- Generation activation.
- Old-generation retirement.
- Worker leases.
- Multi-process behavior.
- Failure and restart recovery.

An increasing sequence or maximum outbox ID is not, by itself, a commit-order guarantee.

Reproduce the exact historical failure using the actual production search worker and rebuild implementation:

1. Transaction A updates an entity and allocates a low outbox ID.
2. Transaction A remains uncommitted.
3. A rebuild begins and excludes the uncommitted change.
4. Later outbox events commit.
5. Catch-up advances beyond the lower ID.
6. Transaction A commits.
7. The ordinary outbox worker processes and acknowledges that event against the old generation.
8. The new generation activates.
9. The activated generation must contain the correct entity revision.

Do not accept eventual correction caused by another unrelated mutation or a later rebuild.

Also test:

- Late deletes.
- Late entity creation.
- Multiple revisions of the same entity.
- Duplicate events.
- Delete followed by recreation.
- Events allocated before rebuild claim.
- Multiple entity types.
- Retention during active rebuild.
- Previously acknowledged events.
- Crash after Tantivy commit but before PostgreSQL acknowledgement.
- Crash during activation.
- Lease loss.
- Multiple server instances.
- Concurrent rebuild attempts.
- Retired generation cleanup while readers remain active.
- Changes immediately before and after the activation boundary.

Use deterministic barriers, controlled transactions, or explicit synchronization. Do not rely exclusively on timing sleeps to establish correctness.

Verify that the operator upgrade procedure can repair a search projection that was already incorrect before the remediation.

Replaying only pending events is insufficient because the missing event may already have been acknowledged.

Require authoritative catalog-to-projection equivalence after writers quiesce and delivery drains.

A passing existing rebuild integration suite is not sufficient unless the reproduced historical race is explicitly covered.

### 7.3 RM-06 — Discovery progress and starvation

Verify that discovery processing is genuinely bounded, durable, resumable, and starvation-free.

Inspect all cursor and progress persistence.

Determine whether progress advances before or after the corresponding effects become durable.

Verify that:

- Tracks cannot consume the entire recurring processing budget indefinitely.
- Releases receive processing time.
- Artists receive processing time.
- Chart relinking receives processing time.
- Missing-candidate correction receives processing time.
- New entities behind the current cursor are eventually visited.
- Deleted cursor entities do not stall progress.
- Continuous insertion cannot prevent an existing stable population from receiving coverage.
- Restart does not reset progress to a permanently repeated prefix.
- Concurrent workers cannot regress or corrupt cursor state.
- Failed batches do not silently skip work.

Execute a test with more than 100,000 tracks and nonempty workloads in every other supported discovery class.

Prove coverage using observed visitation or durable effects, not merely evidence that cursor values changed.

The acceptance criterion is eventual coverage of every stable eligible entity within the bounds implied by the configured budgets and pass protocol.

## 8. Database and catalog review

### 8.1 RM-01 — Playable-media-asset index

Verify the actual final database schema.

Confirm that an appropriate leading index exists on `tracks(playable_media_asset_id)`.

Inspect:

- Index definition and validity.
- Migration safety.
- Trigger execution plans.
- Present and absent reverse lookups.
- Policy exact-hash queries.
- Both valid track/media insertion orders.
- Native filename projection maintenance.
- Legacy import.
- Release merge and split.
- Recovery and administrative mutation paths.

Ensure that the implementing agent did not remove necessary INSERT-trigger behavior or change persistence semantics unnecessarily.

Require representative query-plan evidence demonstrating that selective reverse lookups no longer scan the entire tracks table.

### 8.2 RM-02 — Acoustic novelty queries

Inspect both ordinary and dry-run policy execution paths.

Verify that catalog-wide fingerprint retrieval was eliminated.

Check:

- Deduplication.
- Empty input handling.
- Null and blank digest behavior.
- Same-release exclusion.
- Cross-release matches.
- Multiple evidence rows.
- Duplicate fingerprints.
- Legacy and externalized manifests.
- CUE processing.
- Partial expression index compatibility.
- Complete execution plans, including joined relations.

Do not accept merely observing the acoustic fingerprint index in EXPLAIN output if another part of the plan still performs substantial unrelated catalog-wide work.

Compare policy decisions before and after remediation.

Validate that large dry-run requests cannot construct a catalog-sized fingerprint collection for every release.

### 8.3 RM-03 — Genre query and trigger efficiency

Verify:

- Predicate-aligned genre indexes.
- Search hydration efficiency.
- Valid PostgreSQL transition-table trigger syntax.
- Internal status-change filtering.
- Zero unnecessary recount work for overlay-only updates.
- Correct handling of actual status transitions.

Review execution plans rather than assuming all previous plans were sequential scans.

This package does not pass solely because a migration added an index.

### 8.4 RM-09 — Affected-identity reconciliation

Determine whether ordinary album imports still execute unscoped global candidate discovery.

Inspect all five original reconciliation paths.

Verify that the new implementation:

- Persists affected identity seeds atomically with authoritative changes.
- Includes relevant artist IDs, release IDs, names, external identifiers, redirects, and relationships.
- Handles seeds arriving while a matching job is already active.
- Expands the transitive affected closure.
- Persists unfinished work.
- Does not silently truncate a large closure.
- Preserves manual and negative identity decisions.
- Distinguishes unrelated artists with identical names.
- Preserves historical ambiguity resolution.
- Handles retries and worker crashes.
- Retains bounded global backstops for both artist and release reconciliation.

Compare the new implementation against the intended stable results of the previous global algorithm.

Review whether the comparison oracle actually exercises historical and transitive cases.

Test an import that creates new evidence which resolves an existing historical duplicate and then enables another merge.

Ensure that an apparently idempotent import does not suppress outstanding reconciliation work.

The intended improvement is removal of global reconciliation from the per-album hot path without compromising identity convergence.

### 8.5 RM-10 — Transaction duration and resource allocation

Inspect the new manifest preparation protocol.

Verify expensive evidence loading and parsing occurs outside the write transaction where intended.

Determine whether all required bindings are revalidated after entering the write transaction, including:

- Work-item state.
- Lease owner and token.
- Lease validity.
- Manifest identity and revision.
- Manifest reference and hash.
- Manifest schema.
- Processing plan identity and hash.
- Publication receipt.
- Release identity.
- Sealed publication hash.
- Existing idempotent result.
- Adoption and recovery state.

Challenge time-of-check/time-of-use behavior.

Construct a test in which a relevant binding changes between preparation and the final write transaction.

The old prepared data must not be committed incorrectly.

Review any new pool separation or admission control.

Verify:

- Total connection limits.
- Small-installation behavior.
- Worker fairness.
- Interactive access.
- Administrative access.
- Heartbeat access.
- Permit acquisition ordering.
- Cancellation.
- Shutdown.

Determine whether pool changes solve an observed problem.

A larger pool or separate pools are not proof of improved responsiveness.

### 8.6 RM-16 — Index removal, lease writes, and history retention

Inspect every index added or removed.

Verify:

- Complete index definitions.
- Access methods.
- Key ordering.
- Predicates.
- Included columns.
- Operator classes.
- Collations.
- Uniqueness.
- Constraint dependencies.
- Representative and rare query paths.

An index with zero recorded scans is not automatically unnecessary.

Review lease renewal for:

- Owner and token fencing.
- Expiry semantics.
- Queue age.
- `updated_at` semantics.
- History events.
- Concurrent renewal.
- Retry and cancellation.

Do not accept claims of HOT updates merely because one indexed column was removed from an UPDATE while another indexed column still changes.

Review retention for:

- Required history.
- Active-work evidence.
- Search replay dependencies.
- Tombstones.
- Operational diagnostics.
- Configured retention policy.
- Bounded deletion.
- Interruption and recovery.
- Idempotency.

Reject arbitrary destructive retention defaults that were not established by product requirements.

### 8.7 RM-18 — Batched persistence

Inspect all new set-based and batched database writes.

Verify:

- Array length validation.
- Null handling.
- Duplicate values.
- Empty input.
- Type correctness.
- Parameter limits.
- Large batch behavior.
- Source ordering.
- Constraint enforcement.
- Transaction atomicity.
- Retry idempotency.
- Advisory lock consistency.

Determine whether all callers acquire locks in a compatible deterministic order.

Do not assume that a batch operation is equivalent merely because it creates the same number of rows under a normal fixture.

Review recommendation and popularity processing for any freshness or durability changes.

### 8.8 RM-19 — Release merge and split

Inspect the set-based merge/split implementation.

Verify preservation of:

- Immutable merge evidence.
- Reversible split evidence.
- Original observations.
- Ordinal mappings.
- Public identifiers.
- Artist and track relationships.
- Credits.
- Redirects.
- Manual decisions.
- Genre aggregates.
- Filename projections.
- Recommendations.
- Revisions.
- Outbox events.
- Reconciliation seeds.

Execute a merge followed by a split and compare the resulting authoritative and derived state.

Test conflicting concurrent operations, constraint failures, rollback, and retries.

Ensure large merges do not expose partially applied identity mutations.

## 9. Media processing and worker review

### 9.1 RM-11 — Redundant media work

Verify that the implementation eliminates unnecessary work without weakening required content verification.

Trace:

- Same-filesystem acquisition.
- Cross-filesystem acquisition.
- Retained MP3 processing.
- FLAC encoding.
- Embedded artwork.
- External artwork.
- CUE processing.
- Validation.
- Publication.
- Retry.
- Recovery.
- Cancellation.
- Source mutation.

Identify each removed hash, probe, copy, or validation operation.

For every removed operation, determine:

1. Why it previously existed.
2. Which invariant it protected.
3. Why it is now redundant.
4. What evidence replaces it.
5. Whether concurrent source mutation changes the conclusion.
6. Whether retries and recovery retain equivalent guarantees.

File size, inode, modification time, and device identity do not establish cryptographic content equivalence.

Reject metadata-only substitutions for required content verification.

Verify that cached tool versions have a reliable identity and invalidation strategy.

Verify that reused probe results describe the correct byte stream and contain all required facts.

Inspect actual media output and validation behavior.

Require tests covering:

- Same-size content modifications.
- File replacement.
- Truncation.
- Corrupt retained MP3.
- Source mutation during copy.
- Source mutation during hashing.
- Lease loss.
- Interrupted processing.
- Insufficient temporary storage.
- Retry and adoption.
- Artwork identity.

Do not accept lower I/O counts achieved by skipping required validation.

### 9.2 RM-12 — Blocking work and subprocess lifecycle

Inspect every new blocking execution and concurrency mechanism.

Pay particular attention to:

- `spawn_blocking`.
- Dedicated threads.
- Semaphores.
- Channels.
- Queued jobs.
- JoinSets.
- Task handles.
- External processes.
- FFmpeg concurrency.
- Nested album and track concurrency.
- Temporary storage.
- Open files.

Verify that admission happens before spawning expensive work.

A semaphore acquired inside an already-created unbounded task queue is not necessarily an effective resource bound.

Determine whether permits remain held until actual blocking work terminates.

Dropping an async wrapper does not automatically cancel an already-running blocking operation.

Investigate:

- Cancellation while waiting for admission.
- Cancellation during active blocking work.
- Timeout.
- Subprocess termination.
- Child process reaping.
- Pipe draining.
- Slow storage.
- Lease expiry.
- Shutdown.
- Resource acquisition order.
- Potential deadlocks.
- Detached work.
- Publication by stale workers.

Verify FFmpeg thread configuration against actual codec and filter behavior.

Do not accept universal thread-count assumptions without workload evidence.

Ensure increased parallelism does not simply increase memory usage, storage contention, or temporary disk consumption.

### 9.3 RM-07 — Worker scheduling

Verify that lifetime result accumulation was eliminated or explicitly bounded.

Inspect the rolling worker refill implementation.

Determine whether:

- Free slots are refilled promptly.
- Slow jobs no longer impose unnecessary batch barriers.
- Jobs are claimed only when they can begin appropriately.
- Lease time is not consumed waiting unnecessarily for resource admission.
- Weighted ordinary/bulk processing lanes remain fair.
- Retry backoff remains effective.
- Empty queues do not create a busy polling loop.
- Continuous backlog cannot monopolize the runtime or database.
- Shutdown behavior remains correct.

Execute accelerated idle-tick tests and mixed-duration processing scenarios.

Do not describe simulated time as measured production uptime.

### 9.4 RM-08 — Outbox scheduling

Verify that full batches no longer incur an unconditional idle delay.

Check:

- Full batches.
- Partial batches.
- Empty queues.
- Errors.
- Retry.
- Shutdown.
- Maintenance due/not due.
- Restart.
- Multiple eligible workers.
- Rebuild retention protection.

Ensure maintenance frequency is controlled across the intended ownership model, rather than simply controlled independently by every process.

Verify that aggressive draining cannot starve other important work.

## 10. Tantivy performance review

### 10.1 RM-13 — Search hydration and resource reuse

Inspect the lifetime of:

- `Index`.
- `IndexReader`.
- `IndexWriter`.
- Per-query search handles.
- Active generation metadata.
- Manifest validation.
- Reader registries.
- Writer ownership.
- Retired generations.

Verify that expensive resources are actually reused.

Moving a per-request index open into a helper function does not constitute resource reuse.

Investigate:

- Registry size bounds.
- Generation changes.
- Local and remote activation.
- Reader reload.
- Manifest corruption.
- Concurrent queries.
- In-flight old-generation readers.
- Generation cleanup.
- Writer lease loss.
- Writer lock failure.
- Shutdown.
- Restart.
- Multi-process behavior.

For batched hydration, verify explicit mapping between ordered outbox events and their outcomes.

Preserve:

- Mixed entity types.
- Multiple events for one entity.
- Revisions.
- Deletions.
- Missing rows.
- Poison events.
- Failure handling.
- Replay.
- Commit-before-acknowledgement.

Review how RM-13 interacts with the corrected RM-05 activation protocol.

### 10.2 RM-14 — Rebuild efficiency

Verify:

- The empty-generation bulk insertion path is only used for a demonstrably fresh generation.
- Required revision checks remain active for catch-up and replay.
- Expensive indexing and commit work executes through bounded blocking admission.
- Database page loading cannot create an unbounded indexing backlog.
- Commit frequency has meaningful bounds.
- Writer memory is bounded.
- Snapshot and transaction lifetime are measured.
- Cancellation and recovery remain correct.
- Generation activation remains correct.

Require evidence from the actual PostgreSQL-backed production rebuild path.

A standalone Tantivy benchmark that constructs synthetic documents directly does not establish end-to-end rebuild performance.

Execute the RM-05 adversarial regression suite against the final integrated search implementation.

## 11. Interactive APIs, security, and streaming

### 11.1 RM-15 — Interactive query performance

Inspect the modified administrative, Native, OpenSubsonic, and applicable Jellyfin paths.

Verify preservation of:

- Required response fields.
- Response shapes.
- JSON/XML compatibility.
- Ordering.
- Collation.
- Pagination.
- Rating semantics.
- Current-user rating exclusion.
- Filtering.
- Search matching behavior.
- Access control.
- Library visibility.

Pay particular attention to administrative statistics.

Ensure largest-release statistics still represent the intended playable media semantics rather than a different aggregation over all linked assets.

Verify that authorization-related query optimizations preserve first-binding creation, removal, revocation, isolation, and failure behavior.

Reject simplistic TTL-based authorization caching that weakens access restrictions.

Determine whether concurrent expensive queries improve one request while degrading overall system throughput.

### 11.2 RM-17 — Streaming

Inspect the new streaming implementation.

Verify:

- GET and HEAD.
- Valid ranges.
- Invalid ranges.
- Partial content.
- Early EOF.
- Slow clients.
- Client disconnect.
- Backpressure.
- Open-file lifetime.
- Symlink protection.
- Path traversal protection.
- Cancellation.
- Per-user limits.
- Global limits.
- Descriptor cleanup.
- Buffer bounds.
- Blocking thread occupancy.

A streaming design that reduces task creation by permanently occupying a blocking thread for each slow client may introduce another scalability problem.

Measure the actual resource behavior.

### 11.3 RM-17 — Authentication and caching

Verify that:

- Password hashing strength has not been reduced.
- Password-success caching has not been introduced.
- Credential revocation remains effective.
- Password changes take effect correctly.
- Disabled users cannot authenticate.
- User and secret query consolidation preserves behavior.
- Blacklist refresh is appropriately coordinated.
- Refresh failures do not permit access unexpectedly.
- Artwork caches have explicit byte bounds.
- Cache keys include required transformation parameters.
- Cache invalidation follows authoritative asset identity.

If rate limiting was changed, independently verify the entire multi-process contract, including atomic admission, expiry, cleanup, failure behavior, and the global retained-key ceiling.

Do not approve a simplified per-key UPSERT that loses the existing global resource bound.

## 12. Cross-package integration review

Do not review remediation packages as isolated components.

The integrated implementation may introduce defects even if each individual package appears correct.

Specifically investigate interactions between:

| Components    | Primary integration risk                             |
| ------------- | ---------------------------------------------------- |
| RM-04 + RM-19 | Incorrect genre counts during merge/split            |
| RM-09 + RM-19 | Missing affected-identity seeds                      |
| RM-05 + RM-19 | Lost merge/split search mutations                    |
| RM-05 + RM-08 | Replay retention or acknowledgement races            |
| RM-05 + RM-13 | Stale readers or incorrect writer ownership          |
| RM-05 + RM-14 | Rebuild optimization invalidating activation proof   |
| RM-07 + RM-12 | Claims expiring while waiting for resource admission |
| RM-10 + RM-07 | Lease changes during manifest preparation            |
| RM-10 + RM-12 | Circular resource acquisition or starvation          |
| RM-11 + RM-12 | Parallel processing weakening integrity guarantees   |
| RM-15 + RM-17 | Authorization and cache consistency                  |
| RM-16 + RM-05 | Retention removing required replay history           |
| RM-18 + RM-04 | Batched changes bypassing aggregate semantics        |
| RM-18 + RM-09 | Missing identity-related effects                     |

Construct representative end-to-end scenarios involving multiple remediation packages.

Examples:

- Import an album, reconcile identities, merge a release, evaluate policy, update genre projections, and search for the final result.
- Rebuild search while importing, modifying, and deleting releases.
- Cancel processing during media publication while another worker attempts recovery.
- Perform concurrent same-genre imports while interactive browsing and streaming continue.
- Interrupt an upgrade or repair, restart, and verify all durable work converges.

Look for inconsistent lock ordering, duplicate work, stale state, lost delivery obligations, incorrect resource ownership, and incomplete cleanup.

## 13. PostgreSQL migration and existing-data upgrade review

Inspect every remediation migration.

Verify that previously released migrations and checksums were not modified.

Evaluate:

- Forward migration ordering.
- SQL syntax.
- SQLx transaction behavior.
- Trigger replacement.
- Function dependencies.
- Constraints.
- Index validity.
- Data backfills.
- Repair requirements.
- Lock acquisition.
- Application version compatibility.
- Startup behavior.
- Recovery following partial failure.

Require actual execution against:

1. A fresh disposable database.
2. A populated pre-remediation database.
3. A database with deliberately incorrect derived state.
4. Interrupted migration or repair scenarios where applicable.
5. Repeated repair execution.

For concurrent DDL, verify:

- Correct nontransactional execution.
- Failure recovery.
- Invalid-index detection.
- Definition verification.
- Idempotent retry procedures.
- No falsification of migration history.

Do not assume `IF NOT EXISTS` establishes index correctness.

Check the repository's maintenance-fenced migration requirements, including ADR-0166 or its current successor.

Review the complete operator upgrade procedure.

Verify it explicitly addresses:

- Backup and restore readiness.
- Stopping or fencing incompatible writers.
- Applying migrations.
- Existing genre aggregate repair.
- Discovery and reconciliation initialization.
- Corrected search rebuild or repair.
- Worker restart.
- Application readiness.
- Version and integrity verification.
- Recovery when a step fails.
- Whether older and newer application versions can coexist.

The user intends to deploy a new release to an existing production installation.

A successful fresh installation does not establish that the production upgrade is safe.

## 14. Performance evidence and benchmark integrity

Independently assess the implementing agent's before-and-after performance claims.

Verify:

- Baseline and final commit identities.
- Equivalent datasets.
- Dataset cardinality and distribution.
- Hardware.
- Storage.
- PostgreSQL settings.
- Media codec mix.
- Concurrency.
- Configuration.
- Release-build profile.
- Cache conditions.
- Sample counts.
- Measurement variability.
- Actual execution paths.
- Resource limits.
- Benchmark bypasses.

Investigate whether performance improvements were obtained by:

- Disabling functionality.
- Removing required verification.
- Reducing the dataset.
- Changing the workload.
- Comparing debug and release builds.
- Comparing cold and warm cache results.
- Importing already-processed media.
- Using no-op imports.
- Bypassing database triggers.
- Bypassing policy evaluation.
- Bypassing real search hydration.
- Reducing concurrent workloads.
- Excluding expensive processing stages.
- Ignoring failed operations.
- Counting queued work instead of completed work.

Require correctness assertions in benchmark workloads.

A benchmark is not successful merely because it completed quickly.

Independently reproduce the most important feasible measurements, prioritizing:

| Scenario                    | Required evidence                              |
| --------------------------- | ---------------------------------------------- |
| Playable-asset lookup       | Query plans, buffers, elapsed time             |
| Acoustic policy evaluation  | Returned rows, SQL time, retained memory       |
| Genre operations            | Correct counts, lock waits, throughput         |
| Album import                | Actual stage durations and completed albums    |
| Identity reconciliation     | Query cost and affected closure                |
| Media processing            | Reads, hashes, subprocesses, throughput        |
| Incremental indexing        | Opens, SQL round trips, commits, events/sec    |
| Full rebuild                | Duration, documents/sec, snapshot age, memory  |
| Worker scheduling           | Slot utilization, polling, fairness            |
| Concurrent interactive load | Latency percentiles, errors, pool waits        |
| Streaming                   | TTFB, task overhead, resource occupancy        |
| Merge/split                 | Statements, lock duration, correct final state |

Use the representative 560K–600K catalog where the available isolated environment supports it.

The desired workload includes approximately:

- 35,000 artists.
- 50,000 releases.
- 560,000 tracks.
- A realistic 200-album inbound batch.
- Concurrent interactive activity.

Distinguish:

- Measured during this review.
- Measured by the implementing agent and independently reproduced.
- Measured by the implementing agent but not independently reproduced.
- Historical validation evidence.
- Analytical estimates.
- Proposed targets.
- Not executed.

Do not present unsupported speedup multipliers as measured results.

Do not require a complete 10-million-track or 30-day soak campaign merely to finish this review if the mandatory implementation and representative verification are otherwise complete.

However, do not certify those larger scales or durations without evidence.

If the representative workload cannot be tested, report that limitation explicitly.

## 15. Test quality and independent verification

Review the tests themselves, not merely the test results.

Look for:

- Missing assertions.
- Tests that only verify successful return values.
- Tests that duplicate implementation logic.
- Tests that never execute the relevant production path.
- Incorrect fixtures.
- Mocked components that conceal real defects.
- Concurrency tests that execute sequentially.
- Fragile timing assumptions.
- Disabled or ignored regressions.
- Removed assertions.
- Weakened acceptance conditions.
- Tests that pass despite the original defect.
- Excessive reliance on snapshots updated to match incorrect behavior.
- Environment-sensitive tests.
- Resource leaks in test harnesses.

Where practical, demonstrate that important regression tests would detect the original defect.

Use an isolated baseline worktree or targeted experimental harness when safe.

Do not reset the user's current repository or intentionally reintroduce defects into the implementation under review.

For concurrency-sensitive tests, require controlled transaction interleavings rather than assuming repeated nondeterministic execution is proof of correctness.

Run relevant repository-required:

- Build checks.
- Formatting checks.
- Lint checks.
- Unit tests.
- Integration tests.
- Migration tests.
- API compatibility tests.
- Security tests.
- Concurrency tests.
- Failure and recovery tests.
- Representative performance tests.

Discover the actual repository commands and environment requirements rather than assuming target names have remained unchanged.

A passing test suite is necessary but not sufficient.

## 16. Development host protection

This review must not render the development machine unusable.

Start conservatively with:

- `CARGO_BUILD_JOBS=2`
- `RUST_TEST_THREADS=1`

Honor stricter repository or host limits.

Use one coordinator for resource-intensive operations.

Do not concurrently launch multiple expensive:

- Cargo builds.
- PostgreSQL fixture-generation tasks.
- Integration test suites.
- Media processing benchmarks.
- Tantivy rebuilds.
- Load tests.

Independent source inspection and reasoning can be parallelized.

Resource-intensive verification must be coordinated.

The test harness thread limit must not disable internal concurrency in tests specifically designed to reproduce races.

Check available memory and disk before larger runs.

Use explicit resource and execution limits.

If an owned workload threatens system usability:

- Stop or reduce that workload safely.
- Preserve available evidence.
- Record the limitation.
- Continue unaffected review work.
- Do not repeatedly execute the same unsafe workload.

Do not change host security settings, kernel configuration, swap configuration, system services, or shared database settings.

Use only disposable, explicitly owned databases and temporary media roots.

## 17. Maintainability and architectural review

Evaluate whether the remediation implementation remains consistent with Melodee-rs engineering philosophy.

Look for:

- Overengineering.
- Excessive abstractions.
- Unnecessary traits or generic layers.
- Duplicate resource-management mechanisms.
- Excessive configuration knobs.
- Unnecessary dependency additions.
- Multiple competing implementations of the same behavior.
- Hidden global mutable state.
- Unclear ownership.
- Complex synchronization without documented invariants.
- Unbounded background queues.
- Dead code.
- Temporary compatibility shims.
- TODO-driven implementation.
- Feature flags hiding unfinished behavior.
- Error suppression.
- Swallowed failures.
- Excessive logging.
- Sensitive data exposure.
- Panic paths in production processing.
- Unjustified use of unsafe Rust.

Preserve:

- Modular monolith architecture.
- PostgreSQL authority.
- PostgreSQL-backed durable processing.
- Tantivy as a rebuildable search projection.
- Content-addressed filesystem evidence.
- Existing public API contracts.
- Existing operational simplicity.

Do not recommend Redis, Kafka, RabbitMQ, Elasticsearch, microservices, or distributed orchestration merely because an implementation issue is difficult.

Do not penalize a straightforward implementation simply because a more sophisticated alternative exists.

The desired result is efficient, semantically clear, maintainable code.

## 18. Permanent performance standards

Verify that RM-20 updated the permanent repository engineering standards.

Inspect `PERFORMANCE.md` and relevant `AGENTS.md` requirements.

Confirm that they address:

- Album and affected-identity-proportional work.
- Bounded global maintenance.
- Appropriate database indexes and execution plans.
- Explicit resource admission.
- Bounded queues and retained collections.
- Blocking execution.
- Cancellation.
- Lease fencing.
- Durable checkpoints.
- Discovery fairness.
- Transaction scope.
- Manifest revalidation.
- Media integrity.
- Search generation correctness.
- Authentication and authorization.
- Migration safety.
- Existing-data repair.
- Reproducible representative benchmarks.
- Truthful performance and scalability claims.

Determine whether the new standards are actionable and enforceable rather than generic aspirations.

Verify that unrelated existing repository instructions were preserved.

Review newly added ADRs for consistency with implemented behavior.

A design document that describes behavior the application does not actually implement is a finding.

## 19. Explicitly prohibited shortcuts

Search for evidence that the implementation introduced any approach rejected by the remediation specification.

At minimum, investigate:

- Read-committed rebuild using only a maximum outbox ID.
- Sequence ordering treated as commit ordering.
- Search repair limited to pending events.
- Invalid column-specific transition-table trigger definitions.
- Naive per-track distinct-release counter increments.
- Advisory locking without appropriate snapshot visibility.
- Reconciliation limited to the directly imported release.
- Metadata-only replacement of required cryptographic verification.
- Removal of filename INSERT maintenance without all-writer proof.
- Unbounded `spawn_blocking` submissions.
- Detached blocking work after cancellation.
- Universal FFmpeg thread-count assumptions.
- Pool splitting presented as guaranteed interactive responsiveness.
- Password-success caching.
- Weaker password hashing.
- Rate-limit changes that remove global bounds.
- Blanket index deletion.
- Index deletion justified solely by `idx_scan = 0`.
- Removal of required lease history.
- Arbitrary destructive retention defaults.
- Disabled tests or validation.
- Artificially favorable benchmark comparisons.
- Hard-coded shortcuts used only by benchmarks.
- Disabled mandatory application functionality.
- Stubs or TODOs presented as completed implementation.

Report any such occurrence with exact source evidence and affected requirements.

Do not reject a genuinely correct alternative solely because it differs from an illustrative implementation suggestion. Evaluate it against the required outcome and preserved invariants.

## 20. Finding severity and evidence

Assign each review finding a unique identifier beginning with:

`PERF-CR-001`

Use the following severity classifications.

| Severity      | Definition                                                                                                                        |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| P0 — Critical | Potential authoritative data loss, security violation, serious corruption, unsafe upgrade, or application-wide failure.           |
| P1 — High     | Material correctness defect, significant regression, unresolved mandatory remediation, or serious production performance concern. |
| P2 — Medium   | Meaningful implementation deficiency, incomplete verification, or maintainability concern that warrants correction.               |
| P3 — Low      | Limited-impact improvement or minor technical concern.                                                                            |

Separately classify each finding's release impact:

- RELEASE BLOCKER
- REQUIRED BEFORE PRODUCTION
- FOLLOW-UP RECOMMENDED
- INFORMATIONAL

Severity and release impact are related but not identical.

The previously identified genre concurrency, search activation, and discovery completion requirements are release-blocking gates regardless of their historical severity labels.

Classify evidence as:

- Reproduced defect.
- Measured performance regression.
- Confirmed source or schema defect.
- Confirmed requirement omission.
- Confirmed migration or operational defect.
- Strongly supported concern requiring measurement.
- Insufficient verification evidence.

Do not report speculation as a confirmed defect.

For each finding provide:

- Unique identifier.
- Title.
- Severity.
- Release impact.
- Confidence.
- Related remediation work package.
- Related original performance finding.
- Exact requirement violated.
- Affected files and line ranges.
- Relevant functions and SQL.
- Execution path.
- Independent evidence.
- Reproduction procedure.
- Expected behavior.
- Actual behavior.
- Production consequences.
- Recommended correction.
- Mandatory regression tests.
- Acceptance criteria.
- Dependencies on other corrections.

Group symptoms under their underlying root cause.

Do not inflate finding counts by reporting the same defect repeatedly.

## 21. Required review deliverable

Create:

`PERF-REMEDIATION-CODE-REVIEW.md`

Use the following structure.

# Melodee-rs — Performance Remediation Implementation Review

## A. Release recommendation

Provide the overall release decision immediately:

- GO
- NO-GO

A GO recommendation requires the mandatory correctness, implementation, migration, and release-readiness gates to be satisfied.

Optional larger-scale certification limitations may be documented separately.

Do not issue GO when mandatory verification is unavailable or a release-blocking defect remains.

Explain the decisive reasons.

## B. Prioritized findings

Present actionable findings before lengthy architectural commentary.

| ID  | Severity | Release Impact | Work Package | Finding | Evidence | Required Action |
| --- | -------- | -------------- | ------------ | ------- | -------- | --------------- |

Order by release impact and severity.

If no material findings exist, explicitly state that. Do not manufacture findings.

## C. Executive summary

Summarize:

- Overall implementation quality.
- Requirements compliance.
- Confirmed correctness.
- Remaining defects.
- Incomplete remediation.
- Migration readiness.
- Operational readiness.
- Performance improvement credibility.
- Architectural impact.
- Security and durability posture.

## D. Review scope and revision

Record:

- Baseline commit.
- Reviewed commit.
- Branch.
- Working-tree status.
- Commit range.
- Dependency versions.
- Changed files.
- Reviewed modules.
- Reviewed migrations.
- Tests executed.
- Benchmarks executed.
- Environmental limitations.

## E. Work package acceptance matrix

Include all RM-00 through RM-20.

| Package | Implementer's Status | Independent Verdict | Evidence | Outstanding Requirements | Release Impact |
| ------- | -------------------- | ------------------- | -------- | ------------------------ | -------------- |

Provide subordinate requirement-level details where a package contains multiple mandatory outcomes or conditional decisions.

Do not omit packages that appear complete.

Explicitly identify cases where the implementation agent overstated completion.

## F. Detailed review findings

Provide a comprehensive section for every material finding using the evidence format defined above.

## G. Mandatory correctness gates

Report each gate independently:

| Gate                             | Result | Independent Evidence |
| -------------------------------- | ------ | -------------------- |
| Concurrent genre correctness     |        |                      |
| Existing genre repair            |        |                      |
| Late-ID search activation race   |        |                      |
| Existing stale-search repair     |        |                      |
| Discovery eventual coverage      |        |                      |
| Affected-identity convergence    |        |                      |
| Manifest binding revalidation    |        |                      |
| Lease fencing                    |        |                      |
| Media integrity                  |        |                      |
| Crash recovery                   |        |                      |
| Authentication and authorization |        |                      |
| API compatibility                |        |                      |
| Fresh installation               |        |                      |
| Populated upgrade                |        |                      |
| Repair interruption and retry    |        |                      |

Use PASS, FAIL, UNVERIFIED, or BLOCKED.

An unexecuted mandatory correctness gate must not be reported as passed.

## H. Performance results validation

| Scenario | Claimed Baseline | Claimed Final | Independently Verified Result | Evidence Quality | Verdict |
| -------- | ---------------- | ------------- | ----------------------------- | ---------------- | ------- |

Explain any incomparable workloads, unsupported claims, missing measurements, or meaningful regressions.

Separate implementation completion from representative and larger-scale performance certification.

## I. Test quality and coverage

Document:

- Tests independently executed.
- Test results.
- Critical race reproductions.
- Important missing tests.
- Tests that do not establish their claimed behavior.
- Removed or weakened coverage.
- Required test additions.

## J. Migration and deployment readiness

Assess:

- Migration correctness.
- Existing-data compatibility.
- Repair procedures.
- Operational sequencing.
- Backup and recovery prerequisites.
- Mixed-version compatibility.
- Expected downtime based on available evidence.
- Production risks.
- Required post-deployment verification.

Explicitly identify whether the application can safely upgrade an existing Melodee installation.

## K. Architectural and maintainability assessment

Evaluate:

- Simplicity.
- Modularity.
- Dependencies.
- Resource ownership.
- Synchronization.
- Configuration.
- Error handling.
- Documentation.
- Long-term maintainability.

Identify existing and newly implemented decisions that should be preserved.

## L. Consolidated correction plan

If defects exist, produce a single actionable correction plan suitable for another continuous coding-agent operation.

| Order | Finding IDs | Required Correction | Dependencies | Required Tests | Acceptance Criteria |
| ----- | ----------- | ------------------- | ------------ | -------------- | ------------------- |

Prioritize:

1. Release blockers.
2. Correctness and security regressions.
3. Incomplete mandatory requirements.
4. Migration and repair defects.
5. Measured performance regressions.
6. Missing verification evidence.
7. Material maintainability concerns.

Distinguish mandatory corrections from optional improvements.

Do not turn the correction plan into another broad architectural redesign.

## M. Final acceptance statement

Explicitly answer:

1. Did the implementing agent satisfy the remediation specification?
2. Are all 21 work packages accounted for?
3. Are all 17 original findings adequately addressed?
4. Are the mandatory correctness defects actually resolved?
5. Can existing incorrect derived state be repaired?
6. Were any correctness, security, or durability guarantees weakened?
7. Is the representative workload measurably improved?
8. Are the reported performance claims credible?
9. Is the application safe to upgrade from the existing production version?
10. Is Melodee-rs ready for Release and Deployment Orchestration?

Provide separate conclusions for:

- Implementation completeness.
- Correctness verification.
- Representative workload verification.
- Larger-scale certification.
- Production upgrade readiness.
- Release readiness.

Do not combine these into one unsupported blanket approval.

## 22. Final release gate

A release recommendation of GO requires:

- All mandatory work package outcomes satisfied.
- No unresolved release-blocking defects.
- Concurrent genre correctness demonstrated.
- Existing genre aggregate repair demonstrated.
- Exact late-ID/old-generation acknowledgement race corrected.
- Search repair and activation correctness demonstrated.
- Discovery eventual coverage demonstrated.
- Durable identity reconciliation demonstrated.
- Manifest preparation and binding revalidation verified.
- Media integrity preserved.
- Security and API behavior preserved.
- No unbounded newly introduced resource queues.
- Safe fresh and populated migrations verified.
- Existing-data repair and recovery verified.
- Required repository build and test gates passed.
- Credible representative performance evidence.
- Complete operational upgrade instructions.
- All remaining limitations explicitly documented.

A limited inability to execute optional 10-million-track certification or extended soak testing does not automatically prevent release.

It does prevent claiming those operating scales or durations have been certified.

If mandatory outcomes are incomplete, recommend NO-GO.

If implementation appears correct but mandatory release evidence is missing, recommend NO-GO pending verification.

Do not perform the release or production deployment.

## 23. Final instruction

Review the actual implementation, not the intentions expressed in the specification or the confidence expressed by the implementing agent.

Be thorough, independent, skeptical, pragmatic, and evidence-driven.

Do not stop after discovering the first critical defect. Complete every independent review activity that remains possible.

Do not create another unnecessarily long remediation program if the implementation is already correct.

If a finding requires correction, make that correction sufficiently specific that a coding agent can implement and verify it without ambiguity.

The desired outcome is not merely an application that builds, passes tests, or produces attractive benchmark results.

The desired outcome is a Melodee-rs implementation that performs the right work efficiently, remains correct under concurrency and failure, preserves existing product behavior, and can be safely upgraded into production.

Produce:

`PERF-REMEDIATION-CODE-REVIEW.md`

Conclude with an explicit GO or NO-GO recommendation.

Do not implement corrections or perform Release and Deployment Orchestration.

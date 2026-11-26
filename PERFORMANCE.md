# Melodee Performance Engineering Standard

This document defines the enforceable performance, boundedness, and correctness expectations for
Melodee-rs. It is referenced by `AGENTS.md`. It is an engineering standard, not a benchmark report;
measurements live in `PERF-REMEDIATION-RESULTS.md` and the feature-parity evidence under
`design/evidence/`.

## 1. Workload model

| Tier | Shape | Use |
|---|---|---|
| Small fixtures | a handful of Releases, deterministic | correctness, races, migrations, failure injection |
| 100K catalog | fast plan/workflow iteration | query and workflow iteration |
| 560K–600K catalog | representative database performance | representative-scale claims |
| 1M catalog | first larger-scale bound check | plan stability |
| 5M / 10M / 25M | certification horizons | only with executed evidence and a resources record |

Representative shape includes: artists with missing and present external IDs, same-name distinct
artists, Releases with missing dates/genres/artwork, skewed genre popularity, duplicate terms,
ratings and ACLs, and both playable and alternate media assets. A representative *database* does
not require generating every audio file; a bounded real-media batch is used for media-path claims
and the separation is stated.

## 2. Invariants that performance work must preserve

- PostgreSQL is authoritative; Tantivy is a rebuildable projection.
- Public identifiers, content-addressed evidence, manifest provenance, lease fencing, and
  compatibility contracts are never traded for throughput.
- Same-name Artists remain distinguishable; manual and negative identity decisions retain force.
- Content integrity is proven by content, never by size/mtime/inode or application ownership.
- Commit-before-ack for the search outbox; revision fencing for incremental projection.
- Authentication strength, authorization, streaming admission, and no-follow path protections are
  not weakened for speed.

## 3. Proportional work

- Ordinary import cost is proportional to the affected Release(s) and their identity closure.
  Catalog-wide maintenance is explicit, bounded, resumable, and scheduled independently of every
  album import.
- A per-Track or per-Release catalog-wide operation requires an accepted requirement, a measured
  plan, and a documented backstop. `LIMIT` on a result does not prove bounded discovery cost.
- Background work has a durable cursor and eventual-coverage test. It must not reset to the
  beginning each pass and must not starve a work class behind a large population of another class.
- Idempotent re-imports perform no new global reconciliation work; outstanding historical work
  remains covered by explicit backstops.

## 4. Bounds and admission

- Every retained queue, cache, buffer, and collection has an explicit bound and idle behavior.
  Distinguish retained growth from a leak; both must be bounded.
- Expensive blocking work (filesystem traversal/hash, CPU media work, subprocesses, Tantivy
  open/commit) is admitted before spawn. `spawn_blocking` alone is not a resource design: its
  waiting queue is unbounded.
- Admission permits are held until the work actually stops, including cancellation cleanup.
  Cancellation of the async wrapper does not cancel running blocking work.
- External children have timeout, termination, wait/reap, output-drain, and cleanup behavior.
- Acquisition ordering for multiple permits/locks is documented; no circular waits.
- No unbounded backlog of pre-claimed work items waiting behind a resource permit.
- Do not detach publish/commit work from the fencing that owns it.

## 5. Database

- Transactions are small. Filesystem or subprocess work inside a write transaction requires an
  explicit correctness justification and, when reads are moved outside the transaction, a
  mandatory revalidation of work-item state, lease, and every prepared binding before use.
- High-frequency SQL has a predicate-aligned index proven by a production-shaped plan.
  `idx_scan = 0` never justifies dropping an index; equivalence of the complete definition
  (`pg_get_indexdef`, constraints, predicates, collations, opclasses) and representative plans do.
- Set-based writes are preferred over per-row loops; preserve null/duplicate/ordinal semantics,
  advisory lock key order, and required atomicity. Chunking must not expose a partial operation.
- Lease heartbeats are classified so they cannot queue behind the work whose leases they maintain.
  A lease renewal is not promised to be HOT-updatable while indexed lease fields change.
- History retention is bounded only where a retention requirement exists; otherwise provide a
  tested, non-destructive operator mechanism.

## 6. Search projection

- Activation is defined by a commit-safe cut: after activation, every catalog mutation committed
  before the cut is represented in the activated generation or superseded by an equal/newer
  authoritative revision, and every later commit remains durably deliverable. `max(id)` and an
  advancing id cursor do not prove this because ids are allocated before commit.
- The rebuilding generation tracks its own delivery obligations independently of ordinary
  `processed_at`/acknowledgement. An event acknowledged against the previous generation may still
  require replay.
- Catch-up is ordered and revision/delete aware; commit-before-ack is preserved.
- Generation retirement and reader/writer lifecycle respect the projection lease and multi-process
  ownership. Retired generations are not deleted while a live reader may use them.
- A rebuild validates authoritative content/revision/deletion state after writers quiesce; it does
  not require equality to a continuously changing database.

## 7. Interactive and streaming

- API, OpenSubsonic, Jellyfin, and Admin wire contracts, ordering, pagination, and status behavior
  are preserved. Golden fixtures cover changed routes.
- Authorization stays set-wise and fail-closed. No simplistic permission cache; any cache has an
  explicit key, byte/cardinality bound, freshness, and invalidation model.
- Streaming preserves GET/HEAD/range, no-follow protections, backpressure, client disconnect,
  per-user/global limits, cancellation, and shutdown. Reducing per-chunk task overhead must not
  occupy a blocking thread indefinitely per slow client.
- Rate limiting keeps exact multi-process admission and the global retained-key ceiling; a naive
  per-key upsert is not a valid replacement.

## 8. Benchmark protocol

1. Disposable, randomly named databases and temporary media roots created by the run. Verify the
   target before any destructive reset.
2. Optimized/release builds for every performance claim; record commit, toolchain, CPU/RAM/storage,
   PostgreSQL version and settings, exact configuration, dataset seed and distributions, media
   mix, worker counts, and cold/warm cache state.
3. Same dataset and intended operation state; prior runs must not turn imports into no-ops.
4. Explicit time/resource limits. A timeout is censored evidence, not an invented duration.
5. Several trials for short workflows; enough samples for latency percentiles (never p99 from five
   whole-run timings).
6. Separate cold/warm, logical buffers from physical reads, SQL time from transfer/allocation, and
   foreground latency from throughput.
7. `EXPLAIN (ANALYZE, BUFFERS)` and trigger profiling on representative present/absent/common/rare
   values; never force planner settings to manufacture a plan.
8. Memory/queue bounds under saturation and cancellation, not only at idle.
9. State sample counts, variability, measurement overhead, and bypassed subsystems.
10. Declare a noise-aware regression budget before final testing; do not relax it after a failure.

Every result is labelled: MEASURED IN THIS RUN, REPRODUCED CORRECTNESS, REPORTED BY VALIDATOR,
ANALYTICALLY DERIVED, PROPOSED TARGET, or NOT EXECUTED.

## 9. Migrations and repair

- Forward-only migrations under the ADR-0166 maintenance fence. Never edit a released migration.
  Ordinary SQLx migrations run inside a transaction, so `CREATE/DROP INDEX CONCURRENTLY` is not
  valid there; use the fenced forward build or an explicitly designed resumable online phase that
  inspects `pg_index.indisvalid`.
- Every schema change has a rollback/forward plan, a fresh-install test, and a populated-upgrade
  test with representative skew.
- Derived-data repair is a first-class deliverable: read-only check mode, idempotent apply mode,
  bounded/resumable batches, progress output, and a documented concurrency/maintenance requirement.
- Do not treat `IF NOT EXISTS` as proof that an existing object is valid or equivalent.

## 10. Change authorization and evidence

- A change that alters consistency, storage, or security semantics needs an accepted ADR.
- Mandatory correctness regressions (concurrent genre counts, late lower-id search activation,
  discovery coverage) run before and after any related optimization.
- Performance changes report before/after evidence, data/migration impact, API impact, and
  documentation impact. Architectural complexity requires measured justification.

# ADR-0216: Order recommendation projection locks and group statement changes

- **Date:** 2026-11-26
- **Status:** Proposed
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-PERF-003, PRD-SCALE-002, PERF-014

## Context

Recommendation popularity is maintained synchronously as play and catalog
relationships change. The original per-row triggers first allowed a play's
foreign key to acquire `KEY SHARE` on its Track and then upgraded to `FOR UPDATE`.
Two concurrent plays could deadlock during that upgrade. Related Track and
Artist corrections also updated shared recommendation rows in data-dependent
order, creating opposing lock orders. Raw SQL and migration/import writers can
write the same tables as the application, so an application-only lock is not a
complete protocol. Duplicate primary credits expose an existing inconsistency:
the old play trigger incremented an Artist once, while credit maintenance and
the discovery recount count each primary credit. The old user-state upsert also
failed when duplicate credits targeted the same Artist.

## Decision

Maintain popularity synchronously in the transaction that changes plays or
relationships. For each affected statement, acquire Track locks in UUID order
using `FOR NO KEY UPDATE`, then acquire affected existing recommendation rows in
`(entity_type, entity_id)` order before applying grouped deltas. `NO KEY UPDATE`
serializes relationship changes while remaining compatible with foreign-key
`KEY SHARE` checks. PostgreSQL transition tables supply the changed rows; no
catalog-wide recount, asynchronous queue, or global lock is required. Preserve
play and Release semantics and the nonnegative clamp. Artist recommendation
popularity uses the existing authoritative recount semantics: each primary
credit contributes its Track plays, including repeated credits for one Artist.
Per-user Artist play count increments once per distinct Artist. This corrects
the duplicate-credit inconsistency and changes the affected popularity result;
it is not a behavior-identical rewrite.

A scrobble first takes `ACCESS SHARE NOWAIT` table locks for the four binding
tables, then locks its Track and streams primary Artist IDs from a single
unordered forward-only cursor in pages of at most 256. It attempts Artist and
Release parent key-share locks with `NOWAIT`. If a parent is busy, it rolls
back the entire attempt before waiting for that bounded set in a separate
transaction, then retries with a fresh binding. It never waits for a parent
while holding the Track. The Track lock fences committed Release and primary
credit changes: the statement-level credit triggers also acquire the Track
before their transactions can commit. Each successful play and its derived
state still commit together. Repeated stale-binding retries are bounded and
return the existing database-error class after eight attempts; no partial play
is kept. Mere contention does not consume those attempts. Parent and Track
availability are probed with `NOWAIT`; unsuccessful probes release their
connection and retry with jitter. Process-wide admission limits read-only probes
to two, and each task retains its permit until its database statement and
rollback terminate even if its requester is cancelled. There is no total
deadline: a request can remain pending while a
correction holds a lock indefinitely, preserving existing play behavior. The
number of request futures waiting for admission is not bounded by this design
and remains an open resource-admission limitation.
Probe transactions use a one-second `lock_timeout` as a fallback for relation
locks; the active play transaction does not alter the session's setting.
The cursor has neither an ordering nor a distinct operation that could force
unbounded sort or rewind materialization. Actual plans remain a required
representative measurement.

Track mutations that may affect multiple existing rows must acquire their Track
locks in UUID order before issuing the update. Hard deletion must use the same
order. Multi-statement transactions that touch multiple sets of Tracks and
recommendation candidates must collect and lock their complete affected sets
before mutation; per-statement ordering alone is not proof of transaction-wide
ordering. Until all such writers are audited and tested, this proposal and its
migration remain unqualified.

Migration `0065` replaces trigger functions and must run inside the ADR-0166
maintenance fence. No historical migration is edited. Rollback across it uses a
verified pre-upgrade database backup and matching binary. Existing derived state
is retained. The normal bounded, idempotent discovery maintenance recount repairs
pre-existing popularity drift; the migration itself does not claim to repair it.

Migration `0066` removes the Artist and Observation foreign keys from the
reconciliation work queue only. Its UUIDs are non-owning hints: candidate
discovery resolves live identities with indexed joins and safely ignores stale
hints. The queue still retains captured names and external keys. Hard deletion
captures affected decision Artists' names and external keys in the deletion
transaction before removing evidence. This avoids acquiring Artist or
Observation FK locks after a Release mutation has locked Tracks, and it keeps
pending work rather than cascading it away on identity deletion. The Release
foreign key remains. The migration requires the same maintenance fence and
pre-upgrade backup; re-adding the constraints alone is not a safe rollback.

## Alternatives considered

### Lock in only the scrobble API

Rejected: administrative, import, and direct database writers can bypass that
entry point. The API may take an early compatible lock but cannot be the only
protection.

### Snapshot names and keys instead of retaining identity hints

Rejected: a relationship can acquire a new name, binding, or external key after
enqueue and before processing. Retaining UUID hints lets the worker discover the
current closure as well as facts already stored on the seed.

### Serialize every play using one global advisory lock

Rejected: one hot global lock couples unrelated Tracks and creates a
catalog-independent throughput bottleneck.

### Defer popularity updates asynchronously

Rejected: it changes synchronous freshness unless a durable equivalence and
recovery protocol is proved.

## Consequences

- Related changes may wait for the same Track and candidate rows; unrelated
  identities do not deliberately share a global lock.
- Grouped transition-table work and its database plans must be measured on
  realistic distributions, including large statements and skewed popularity.
- Raw bulk Track UPDATEs can lock tuples before an AFTER STATEMENT trigger sees
  its transition table. Writers therefore need sorted prelocking; the trigger
  alone cannot prove a deadlock-free arbitrary multi-statement transaction.
- Statement-level deltas assume previously consistent recommendation counters.
  Recovery and checks must use authoritative plays and current relationships.

## Validation

Before acceptance, require fresh and populated upgrade tests, exact popularity
checks against authoritative plays, raw and application concurrent inserts,
opposing Release/Artist corrections, multi-primary credits, multi-row and
multi-statement lock-order tests, rollback/retry, and representative concurrent
throughput and lock-wait measurements. Explicitly audit every writer and hard
or soft deletion path. The migration and tests remain in progress.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [Maintenance-fenced upgrades](ADR-0166-require-maintenance-fencing-for-blocking-schema-upgrades.md)

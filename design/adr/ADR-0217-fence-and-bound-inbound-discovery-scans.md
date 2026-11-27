# ADR-0217: Fence and bound inbound discovery scans

- **Date:** 2026-11-26
- **Status:** Proposed
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-PERF-003, PRD-SCALE-002

## Context

Inbound reconciliation formerly retained the complete source root, observed
paths and all active work items in process memory. Sources can be large, scans
can overlap across processes, and filesystem enumeration is not a database
snapshot. Interrupted scans must not classify an incompletely enumerated source
as missing or allow an expired owner to mutate a newer scan's state. Multiple
immediate-root names can resolve to the same canonical nested target.

## Decision

A per-source durable generation uses 16 filename-hash partitions with a
transaction-fenced owner lease. It stores bounded seen and completed partition
bitsets on the authoritative work item, instead of retaining a candidate spool.
A partition cursor advances only after a complete directory traversal. Candidate
failures remain unacknowledged; their partitions enter a durable retry bitset,
while the first pass continues through the other partitions. Later bounded retry
turns rotate through pending partitions with a backoff. Missing classification
waits until every partition succeeds. If an owner expires during a partition,
recovery clears every partition's seen bit in bounded pages before rescanning,
because a new process cannot prove the previous root occurrence still exists.
Seen bits from different alias partitions are retained during a live pass. A
separate done bit avoids repeating completed candidate processing after the
current snapshot has been revalidated. Missing-path updates and the
cursor for those updates commit in bounded, fenced transactions. A database
birth cutoff excludes work items concurrently created after the scan began from
premature missing classification; a later scan checks them. Removed source
leases are cleaned in bounded periodic maintenance even with ingestion disabled.

A one-shot scan waits at most 30 seconds for a busy scope then returns a typed
busy error; its caller can retry. Periodic scans skip a source still busy after
one second and proceed to later sources. The scheduler can be stopped while
waiting. Candidate processing follows filesystem partition order; the public
sorted source snapshot helper remains unchanged.

Recovery clears all prior seen bits through a durable, fenced keyset cursor
before observing the root again. The periodic scheduler yields after at most
four 128-row reset pages and resumes that reset under a new owner; completed
work markers are retained for revalidation. No old observation is trusted
across the reset, even if the source root changed. The public one-shot command
continues the reset in the same invocation. This bounds reset work in a
scheduler turn. Missing-path processing also yields after four 128-row pages;
each committed missing result records the scan generation. After a handoff,
a durable source-scoped sweep rechecks each representable path through the
newly pinned root before applying a fenced missing result. Errors are retried
per item; old lossy non-UTF-8 paths are deferred to a final full observation.
The sweep finishes with one full observation pass to compensate for root changes
between checks and database commits. Rows already marked in this generation are
skipped only while still `source-missing`; reappearing candidates are
revalidated, including previously completed ones. This protocol is pending
focused tests and measurement. Fixed-interval crash fairness for directory
iteration remains an open performance requirement.

Settling signatures encode the existing ordered metadata records as a
versioned BLAKE3 digest incrementally, rather than retaining a concatenated
copy of every record. Existing literal signatures remain comparable by hashing
the legacy bytes on demand, so an upgrade does not reset an unchanged Release
or rewrite acquisition evidence. New signatures can be visible in CLI manifest
diagnostics as versioned digests rather than literal metadata records; existing
stored signatures remain unchanged. These signatures describe settling
metadata; content verification at staging and publication continues to use its
existing cryptographic byte hashes. Snapshot entry collections remain an
unresolved bound.

## Alternatives considered

An in-memory set grows with the source. A persistent candidate spool also has
unbounded cardinality. A finite spool quota which rejects larger sources is not
an acceptable replacement for resumable progress. A lease checked outside the
mutation transaction admits stale owners. A sorted bounded selection by rescanning
the whole directory per page introduces quadratic work.

## Consequences

Each partition traverses the source root; the additional filesystem work must
be measured under realistic cardinalities. A recovered scan restarts its filesystem observations; done bits avoid repeating
already-completed processing after revalidation,
but an uninterrupted traversal is still required to complete that partition.
In particular, if every crash occurs before the iterator can reach a directory
tail, this protocol does not yet prove that the tail eventually progresses.
On Linux/Android and Apple targets, the pending implementation pins source and
candidate directory handles and resolves symlink components relative to those
handles. Windows uses a proposed capability-based path described by ADR-0218.
Apple and native Windows behavior have not been verified; unsupported targets
fail closed when the required secure filesystem operations are unavailable.
A root pathname recheck cannot be atomic
with the PostgreSQL commit of candidate effects; consumers can still observe
such effects before final root validation. That cross-resource fence remains
unresolved.
The in-development streaming snapshot producer transfers cancelled work to a
process-owned supervisor with four reserved reaper slots. A filesystem syscall
that never returns retains its producer and admission slot; cancellation does
not publish its output. Other slots can continue, and Tokio runtime teardown
does not own the reaper threads. The supervisor, teardown behavior, and the
four-slot resource choice still need final integration and load verification.

The implementation remains partial until that case and snapshot entry bounds
are resolved and measured. Work-item marker columns are fixed size. Partition
processing order differs from the prior lexicographic order.

Migration 0067 adds scan state and work-item markers. Apply with workers stopped
under ADR-0166. For rollback, stop writers and restore a pre-upgrade backup or
deploy the previous binary with its prior schema; do not remove SQLx migration
history. Existing work items and staging handoffs remain authoritative and are
not rewritten by the migration beyond additive marker defaults.

## Validation

Validate alias preservation across interrupted partitions, source-scope and
birth fencing, stale ownership, cancellation/restart, bounded transaction
pages, complete source traversal, fresh and populated upgrades, legacy
signature handoff and recovery, and representative throughput. This decision
and migration remain a work in progress until those checks pass.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [Performance standard](../../PERFORMANCE.md)

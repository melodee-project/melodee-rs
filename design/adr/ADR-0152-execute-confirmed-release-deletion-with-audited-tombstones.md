# ADR-0152: execute confirmed release deletion with audited tombstones

- **Date:** 2026-08-22
- **Status:** Accepted
- **Decision owners:** User-directed Release deletion correction
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-POLICY-001, PRD-POLICY-002, PRD-POLICY-006,
  PRD-POLICY-007, PRD-INGEST-010

## Context

ADR-0120 persisted an administrator's destructive authorization and a separate completion state,
but no runtime component executed the deletion. The Admin UI consequently presented a Delete
confirmation that left the Release in `NeedsReview` with pending completion. ADR-0145 separately
requires retained staged source for ordinary ingestion and prohibits automatic cleanup after
publication; that retention rule was incorrectly interpreted as prohibiting an explicit operator
deletion as well.

The catalog is highly connected and deliberately retains append-only policy, identity, import,
and compatibility evidence. Physically deleting the relational graph would destroy required audit
history and public-ID tombstones. Actual deletion must instead remove playable/source bytes and
retire the Release from every browse/playback projection while preserving forensic evidence.

## Decision

An administrator-confirmed Delete in the Release Review UI executes synchronously after the
durable authorization is committed:

1. Revalidate the sealed-manifest fingerprint and administrator identity/reason.
2. Resolve only the immutable publication-plan directory and durable staging-handoff directory
   below the configured canonical and staging roots.
3. Remove both directory trees with descriptor-relative no-follow operations. A missing tree is
   idempotent success; a symlink, special file, path traversal, missing root, or I/O error fails
   closed and leaves completion pending.
4. After both removals succeed, mark the Release catalog row `deleted`, advance the work item to
   `Deleted`, complete the policy decision, append catalog/work audit evidence containing relative
   paths and the source fingerprint, and enqueue search deletion operations for the Release and
   Tracks in one PostgreSQL transaction.
5. Keep Releases, Tracks, MediaAssets, imports, public IDs, policy evidence, and identity evidence
   as non-browsable tombstones. Client APIs and normal Admin Catalog listings must not return them.

This decision narrows ADR-0145: automatic staged-source deletion remains prohibited. Only an
explicit, typed, reasoned administrator action may remove retained staging data.

## Alternatives considered

### Delete only database rows

Rejected because it leaves playable/source bytes behind, violates the operator's expectation, and
cannot preserve the append-only foreign-key graph.

### Record authorization for an unimplemented future worker

Rejected because it was the existing misleading behavior. A separate worker can be introduced
later only if it preserves the same idempotent execution and failure contract.

## Rationale

Tombstones reconcile actual media removal with Melodee's public-ID and audit guarantees. Performing
the bounded operation immediately gives the administrator an honest success/failure result, while
the already durable pending authorization makes a failed attempt safely retryable.

## Consequences

### Positive

- Delete now means the Release is no longer playable or browseable and its managed bytes are gone.
- Audit, source fingerprint, policy evidence, and compatibility identifiers remain inspectable.
- Crash/retry after partial filesystem cleanup converges because missing directories are success.

### Negative / trade-offs

- PostgreSQL retains metadata tombstones, so Delete is not a privacy-erasure API.
- A filesystem failure can leave an authorized operation pending and requires an administrator retry.

### Follow-up work

- A future dedicated deletion worker may consume the same pending decision contract if synchronous
  execution becomes too slow at measured Release sizes.

## Validation

- Unit tests for safe-path validation, no-follow removal, and idempotent missing-tree retries.
- PostgreSQL tests for fingerprint/actor authorization, completed tombstones, audit evidence,
  aggregate correction, and search deletion outbox rows.
- Admin API and UI tests proving typed confirmation removes canonical/staged trees and returns a
  completed rather than pending result.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0120](ADR-0120-phase5-classification-novelty-and-policy-schema.md)
- [ADR-0145](ADR-0145-drain-inbound-into-durable-staging-before-ingestion.md)

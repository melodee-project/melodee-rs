# ADR-0145: drain inbound into durable staging before ingestion

- **Date:** 2026-08-15
- **Status:** Accepted
- **Decision owners:** Product owner
- **Supersedes:** ADR-0116 §Identity and admission, inbound source-disposition clause only
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-005, PRD-INGEST-006, PRD-INGEST-010, PRD-STORAGE-001

## Context

ADR-0116 correctly defines immutable Release IDs and convergent canonical publication, but its
inbound-retention rule treats an externally supplied source path as long-lived storage. That makes
reconciliation cost grow with every processed Release and prevents reuse of an inbound path.

Melodee needs four distinct storage responsibilities: inbound is a transient external filesystem
drop directory;
staging is durable Melodee-owned original source storage; the normalization work area is disposable
attempt scratch; canonical storage is the long-term published library. Physical staging is not the
logical ingestion state `Staged`.

## Decision

1. `MELODEE__INGEST__STAGING_ROOT` is required, absolute, durable, independently configured, and
   disjoint from every inbound root, work area, and canonical root whenever ingestion is enabled.
2. Once a settled candidate is leased for inspection, PostgreSQL records handoff intent and the
   candidate is acquired to `<staging-root>/releases/<work-item-id>/source`. Original inbound root
   identity, source-library identity, and relative path remain provenance; no configured absolute
   root is semantic manifest identity.
3. Same-filesystem acquisition uses a no-replace atomic directory rename. Cross-filesystem
   acquisition copies to a hidden directory under staging, streams BLAKE3 tree verification before
   and after copy, fsyncs, no-replace-finalizes, records a durable receipt, and only then removes
   inbound.
4. A durable handoff record distinguishes intent, staged receipt, and inbound removal. Recovery
   adopts only the deterministic final path for its work item; conflicting content fails closed.
5. Cleanup is guarded by the recorded directory generation and verified tree hash. A delayed worker
   cannot delete a new Release that reused the old path.
6. Inspection, normalization, validation diagnostics, review, and quarantine use staged source
   after acquisition. NeedsReview, Quarantined, Failed, and Published source retention are
   non-destructive; post-publication staging retention is a separate policy decision.
7. The `(source library, relative path)` uniqueness guard applies only while an inbound occurrence
   remains active. Once ownership is complete, the same inbound path creates a new work item.

## Alternatives considered

### Retain inbound indefinitely

Rejected because it makes inbound a second, unbounded library and forces every reconciliation
to traverse historical candidates.

### Use normalization work area as staging

Rejected because work output is attempt-isolated and disposable; it is not durable original-source
evidence for review, quarantine, or recovery.

## Rationale

The decision preserves ADR-0116's Release-centric canonical layout, no-clobber publication, and
manifest semantics while making bounded inbound scans and durable source custody explicit.

## Consequences

### Positive

- Processed Releases leave inbound, so new scans see only new arrivals.
- Recovery has PostgreSQL intent/receipt evidence rather than stale absolute source paths.
- Operators can review and retry durable original source bytes after restart.

### Negative / trade-offs

- Inbound mounts must allow the runtime UID to rename/remove acquired candidate directories.
- Cross-filesystem handoff reads source bytes more than once for verification.

### Follow-up work

- Define an explicit staged-source retention/authorized-delete policy separately; this ADR does
  not permit destructive post-publication cleanup.

## Validation

Validation must cover same-filesystem move, forced cross-filesystem copy, every intent/receipt/
cleanup crash boundary, source change during copy, lease loss, path reuse, delayed-cleanup races,
staging conflicts, restart from staging, review/quarantine retention, and a 200-Release inbound
drain followed by an empty reconciliation.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)

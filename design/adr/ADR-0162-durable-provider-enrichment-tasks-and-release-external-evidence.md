# ADR-0162: Durable provider enrichment tasks and release external evidence

- **Date:** 2026-08-25
- **Status:** Accepted
- **Decision owners:** Feature Completion Program Phase 2
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-EXT-001, PRD-EXT-002, PRD-EXT-003, PRD-EXT-004,
  PRD-ARTIST-005, PRD-ARTIST-012, PRD-RESOLVE-002, PRD-RESOLVE-005, PRD-WORK-003

## Context

ADR-0154 establishes the provider boundary and fail-degraded runtime, but does not define durable
task rows or Release-scoped external IDs. The existing cache is a response projection, not a
leaseable queue, and the catalog currently has only Artist external identities. Phase 2 needs a
restart-safe enrichment task and evidence shape without allowing providers to own Melodee
identity.

## Decision

1. Add an additive migration with `provider_enrichment_tasks`, keyed by Melodee entity (`artist`
   or `release`), provider, purpose, and hashed request key. Rows contain bounded status
   (`queued`, `running`, `succeeded`, `retry`, `failed`), attempt count, next-attempt time,
   lease owner/expiry, last error category/message, and timestamps. A unique task key makes
   scheduling idempotent.
2. Add append-only `provider_enrichment_attempts` rows with task, attempt number, provider,
   purpose, outcome, error category/message, started/completed timestamps, and optional cache key.
   Secrets and raw credentials never enter these rows.
3. Add `release_external_identities` with Release, provider, raw observed external ID, a trusted
   boolean, confidence, source, lock flag, and timestamps. Raw malformed values remain observations
   (`trusted=false`); only trusted rows participate in automatic Release convergence. The table
   does not make an external ID the canonical Release identity.
4. Lease claiming uses `FOR UPDATE SKIP LOCKED`, bounded batches, expiry recovery, and an explicit
   worker identity. Provider failure updates the task and attempt evidence but never changes media
   disposition or deletes catalog data.
5. Release/Artist merge and split operations retain all evidence rows and audit the predicate,
   actor, and evidence references. Provider results alone never force a merge.
6. Add `provider_identity_candidates` as an upserted observation projection for provider search
   candidates. It retains the local Artist, provider, external ID, query, rank, confidence, and
   bounded raw payload; it is review/diagnostics evidence only and is never read as a trusted
   identity predicate.
7. Add `releases.merged_into_release_id` plus append-only `release_merge_events` and
   `release_split_events`. A Release merge moves Tracks with an explicit original-ordinal map,
   retains the source Release and all observations, and marks the source as a redirect tombstone;
   split restores that map under the original source identity. Trusted embedded/provider evidence
   is required for the automated candidate scan, while locked or negative decisions suppress it.

## Alternatives considered

### Store enrichment tasks in the response cache

Rejected: cache freshness and task leasing have different lifecycles; conflating them loses retry
and operator diagnostics semantics.

Add provider-specific columns to Releases

Rejected: it makes external providers part of the catalog identity schema and cannot retain
multiple observations or provider-neutral provenance.

## Rationale

The additive shape reuses PostgreSQL as the source of truth, keeps response cache rebuildability,
and supplies the exact lease/audit primitives required by ADR-0154 and ADR-0160. It is bounded,
reversible, and independent of provider SDKs.

## Consequences

### Positive

- Enrichment survives process crashes and can be inspected without reading secrets.
- Release IDs can be retained and scored with the same evidence-only policy as Artist IDs.
- Duplicate Releases can be converged without deleting the source row or losing Track ordinals;
  the recorded map makes the operation reversible.

### Negative / trade-offs

- The migration adds tables and indexes; operators must use the documented forward migration and
  backup/restore rollback procedure.

### Follow-up work

- Implement the worker, provider composition, reconciliation pass, and migration/integrity tests
  in Feature-Completion Phase 2.

## Validation

Migration tests verify bounds, foreign keys, append-only attempt history, unique scheduling, and
lease recovery. Integration tests prove concurrent claims are exclusive, retries converge, cache
freshness suppresses calls, and malformed external IDs remain untrusted observations. Provider
fixtures prove failures never block import.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0154](ADR-0154-external-provider-enrichment-scope-and-runtime.md)
- [ADR-0160](ADR-0160-cross-release-artist-release-identity-reuse.md)

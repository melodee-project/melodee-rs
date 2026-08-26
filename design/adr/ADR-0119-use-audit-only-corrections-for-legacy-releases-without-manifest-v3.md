# ADR-0119: Use audit-only corrections for legacy Releases without manifest v3

- **Date:** 2026-08-10
- **Status:** Accepted
- **Decision owners:** User-directed Phase 4 implementation
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-008 through PRD-ARTIST-010, PRD-CATALOG-005,
  PRD-MIG-002, PRD-MIG-004

## Context

ADR-0117 permits migrated legacy Releases to exist without a Phase 3 work item, publication receipt,
or `catalog_imports` row. ADR-0118 defines a `CatalogSemanticOverlay` as an immutable explanation of
how one specific manifest v3 and publication plan became normalized catalog state. Its source
manifest ID, semantic hash, plan hash, and catalog-import ID are therefore mandatory evidence.

An Artist or AudioIdentity correction can affect both manifest-driven and legacy-only Releases. A
legacy-only Release has no truthful v3 source facts from which to construct ADR-0118's overlay.
Fabricating those facts would weaken the publication and migration evidence boundaries; refusing the
entire correction would make migrated identities unsafe to administer.

## Decision

- Corrections to a manifest-driven Release append and atomically select a new immutable
  `CatalogSemanticOverlay` exactly as ADR-0118 requires.
- Corrections to a legacy-only Release append the same immutable catalog audit event and
  transactional projection-outbox work, but do not create a synthetic semantic overlay.
- One correction transaction may affect both kinds of Release. It appends overlays for every
  manifest-driven Release and records the legacy Release IDs in the correction event/audit payload.
- Absence of a catalog import is the only reason an affected Release may omit a new overlay. A
  malformed or conflicting manifest-driven binding still fails closed.
- Phase 11 migration must create a versioned legacy catalog baseline before it can produce semantic
  overlays for migrated Releases. That baseline is migration evidence, not invented Phase 3
  publication evidence, and requires its own accepted schema/cutover decision.
- No correction moves, rewrites, re-encodes, or deletes source or canonical media.

## Alternatives considered

### Fabricate manifest, plan, or receipt identifiers

Rejected. Those values attest to Phase 3 observations and canonical publication. Synthetic values
would make an overlay appear to have evidence that never existed.

### Reject all corrections until Phase 11

Rejected. Public-ID mapping is proven in Phase 4, and a migrated catalog must retain safe audited
identity administration even before the final production cutover tooling exists.

### Make every overlay source field nullable

Rejected for Phase 4. It would weaken ADR-0118's manifest-driven invariant and conflate two different
provenance models. Phase 11 can define a separate legacy-baseline binding without relaxing v3
overlays.

## Consequences

Manifest-driven corrections remain fully reproducible from v3 plus durable decisions. Legacy-only
corrections remain explainable and projectable through append-only audit/outbox data, but do not have
a semantic-overlay snapshot until Phase 11 establishes a truthful migration baseline. Mixed
corrections must test both paths atomically.

## Validation

- PostgreSQL tests correct a manifest-driven Release and require a new overlay and unchanged v3,
  plan, receipt, and media paths.
- PostgreSQL tests correct a legacy-only Release and require audit/outbox evidence, no fabricated
  overlay/import, preserved public IDs, and unchanged media paths.
- A missing or inconsistent import binding on a manifest-driven Release continues to fail closed.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0118](ADR-0118-use-audioidentity-and-immutable-manifest-v3-semantic-overlays.md)
- Phase 4 work package

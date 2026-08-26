# ADR-0165: Materialize catalog genre serving facts

- **Date:** 2026-08-25
- **Status:** Accepted
- **Decision owners:** Repository maintainers (performance and scalability remediation)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SCALE-001, PRD-SCALE-002, PRD-PERF-003,
  PRD-PERF-005, PRD-PERF-006, PRD-API-M1-005, PRD-API-OS-005

## Context

Genre browse, filtering, recommendation, and compatibility responses currently recover genre
observations by joining `catalog_imports` to immutable manifest JSON and expanding every manifest's
`files` array. Several normal requests therefore deserialize and aggregate catalog-wide JSON at the
10-million-Track reference scale. The manifests remain authoritative import evidence, but they are
not an indexed serving model.

Native v1 and OpenSubsonic intentionally have different genre semantics. Native v1 treats the
trimmed observed genre string as one value and compares it case-insensitively. OpenSubsonic splits
the observed string at semicolons, preserves each displayed term, and uses the trimmed term for its
captured exact, case-sensitive filters. A projection must retain both meanings without sharing
adapter DTOs or rewriting immutable manifests.

## Decision

Add an authoritative PostgreSQL serving projection with two relations:

- `catalog_genre_terms` stores Track- and Release-keyed genre observations with an explicit
  `native` or `opensubsonic` surface, the exact display value, and generated trimmed/lowercase
  matching values.
- `catalog_genre_aggregates` stores song and distinct-Release counts by surface and exact display
  value for unrestricted catalog-wide genre lists.

Migration 0038 backfills both relations set-wise from existing manifest/catalog bindings. A
database trigger on `catalog_imports` projects future imports in the same transaction as catalog
publication, including imports performed by migration tooling rather than one Rust call path.
Statement-level transition-table triggers recompute only affected aggregate keys after term inserts
or deletes and Release status changes, so deletion, merge/deprecation, restoration, and retry remain
correct without row-order-dependent counters. Aggregates include only active Releases, matching the
existing native and OpenSubsonic serving contracts.

All normal API serving, discovery, and search-snapshot queries consume this projection instead of
manifest JSON. Restricted native users aggregate the indexed term rows under the existing library
ACL predicate; unrestricted and OpenSubsonic catalog-wide lists use the materialized aggregate.
PostgreSQL and immutable manifests remain authoritative, and the projection is rebuildable from
their catalog binding.

## Alternatives considered

### Parse manifest JSON in every adapter

Rejected. It preserves the current wire behavior but makes CPU, JSON expansion, grouping, and
temporary-memory work proportional to catalog size for normal requests.

### Collapse every adapter onto one normalized genre vocabulary

Rejected. Native v1 and OpenSubsonic have captured, different splitting and comparison behavior.
One vocabulary would be a compatibility change prohibited by ADR-0102 and ADR-0130.

## Rationale

The projection follows the existing PostgreSQL-authority/rebuildable-projection architecture,
removes JSON from hot paths, remains set-based, and preserves each adapter's contract explicitly.

## Consequences

### Positive

- Genre browse and filtering operate on narrow indexed rows rather than immutable JSON documents.
- Unrestricted genre counts are proportional to the number of genre values, not Track count.
- Import, retry, and Release deletion maintain the projection transactionally.

### Negative / trade-offs

- The catalog stores two surface-specific observations for a typical tagged Track.
- Import and deletion perform bounded aggregate maintenance for the affected genre values.
- Migration 0038 scans existing catalog manifests once to backfill the projection.

### Follow-up work

- A future operator command may expose an explicit projection rebuild, using the same set-based SQL,
  if production evidence shows that manual repair is useful. It is not required for correctness now.

## Validation

- Migration-from-empty and forward/backfill integrity tests.
- Catalog import idempotency and Release-deletion aggregate tests.
- Existing native v1, OpenSubsonic, and Jellyfin golden/route tests without wire changes.
- Query-shape tests rejecting manifest JSON expansion and `ORDER BY random()` on normal genre paths.
- Current- and multi-million-scale API performance evidence under PRD-PERF-005.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0102](ADR-0102-preserve-public-apis-as-separate-adapters.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0130](ADR-0130-phase-8-opensubsonic-authentication-extension-advertisement-and-streaming-corrections.md)

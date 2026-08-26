# ADR-0167: align native normalized search indexes

- **Date:** 2026-08-26
- **Status:** Accepted
- **Decision owners:** Repository maintainers (performance and scalability remediation)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SCALE-001, PRD-SCALE-002, PRD-PERF-001,
  PRD-PERF-003, PRD-PERF-005, PRD-API-M1-005

## Context

Migration 0018 added trigram GIN indexes over the display columns `artists.canonical_name`,
`artist_aliases.alias`, `releases.title`, and `tracks.title`. Native v1 browse, search, and
suggestion queries intentionally filter their normalized counterparts. PostgreSQL cannot use an
index on one expression for another column, so current-catalog acceptance at 560,000 Tracks
measured 326 ms p95 for an exact-suffix Track search against the proposed 50 ms budget.

Changing the queries to display columns would make the old indexes usable but would silently change
the normalization boundary preserved from the legacy service. Editing migration 0018 is prohibited
because applied SQLx migrations are immutable. Adding catalog-scale indexes is a schema and upgrade
availability decision governed by ADR-0166.

## Decision

Add migration 0039 with trigram GIN indexes on the exact normalized columns used by native v1:

- active/non-retired `artists.normalized_name`;
- `artist_aliases.normalized_alias`;
- active `releases.normalized_title`;
- `tracks.normalized_title`.

Keep the 0018 display-column indexes in place. Their removal needs production `pg_stat_user_indexes`
evidence across all adapters and is outside this correction. Apply 0039 only under the ADR-0166
maintenance fence; it uses ordinary migration-owned `CREATE INDEX` statements and is not claimed to
be online.

## Alternatives considered

### Rewrite native queries to display columns

Rejected. `ILIKE` would remain case-insensitive, but whitespace and any future normalization rules
would move from catalog-owned normalized facts to adapter-specific behavior.

### Replace the existing indexes

Rejected for this change. Dropping the display indexes without workload-wide usage evidence could
regress a different adapter, while keeping them is additive and rollback-safe.

## Rationale

The indexes match predicates exactly, preserve wire behavior, and remediate the measured scale gate
without adding a runtime dependency or duplicating normalization logic.

## Consequences

### Positive

- Native normalized substring predicates become indexable at catalog scale.
- Existing public route, DTO, ordering, pagination, and normalization behavior is unchanged.

### Negative / trade-offs

- Four additional GIN indexes consume disk and add catalog-import write amplification.
- The maintenance-fenced migration duration grows with catalog size.

### Follow-up work

- Review production index-usage statistics after an acceptance window before proposing removal of
  the older display-column indexes.

## Validation

- Migration-from-empty and exact-definition tests.
- `EXPLAIN` evidence that the normalized Track predicate uses the new GIN index.
- The release-mode 560,000-Track native API harness must meet its 50 ms browse/search p95 proposal.
- Existing native catalog compatibility tests must remain unchanged.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0166](ADR-0166-require-maintenance-fencing-for-blocking-schema-upgrades.md)

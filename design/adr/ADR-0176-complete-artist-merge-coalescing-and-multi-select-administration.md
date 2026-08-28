# ADR-0176: Complete Artist merge coalescing and multi-select administration

- **Date:** 2026-08-27
- **Status:** Accepted
- **Decision owners:** User-directed C# parity correction
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-002, PRD-ARTIST-004 through PRD-ARTIST-010,
  PRD-CATALOG-005, PRD-STORAGE-001, PRD-UI-005, PRD-UI-010

## Context

The Rust catalog already has an audited one-source Artist merge that retargets core identity and
catalog links. Its only Admin UI is a UUID form buried under Identity Review. The legacy C# product
instead lets an administrator select several Artists from the catalog, choose the survivor, inspect
the operation, and consolidate the selected identities.

The Rust operation also predates native user state, operator and provider artwork, provider
enrichment, and discovery chart links. Those newer Artist-owned rows are not all retargeted today,
so a technically successful merge can strand favorites, ratings, images, provider evidence, or
discovery links on a tombstone. Completing the parity capability crosses identity, catalog, user,
provider, UI, audit, and compatibility boundaries and therefore needs a durable decision.

PRD-ARTIST-010 requires a tombstone/redirect. PRD-STORAGE-001 forbids identity correction from
moving canonical media. Names remain non-unique, so the operation must be an explicit administrator
decision rather than a new automatic name rule.

## Decision

1. The existing single-source Admin API remains compatible. Add separate preview and multi-source
   apply operations; one through one hundred distinct, resolvable sources may be merged into one
   distinct, resolvable target.
2. Multi-source apply is one PostgreSQL transaction. Validation, every source merge, audit events,
   overlays, projections, and user/provider coalescing either all commit or all roll back.
3. Every source becomes a `merged` tombstone whose immutable public ID redirects to the selected
   target. Source canonical names become locked search aliases. Historical decisions, attempts, and
   audit events remain append-only and keep their original identity attribution.
4. Retarget ReleaseArtist, TrackArtist, Contribution, aliases, external identities, relationships,
   memberships, current observations, discovery chart links, and non-conflicting provider tasks.
   Rebuild Track participation, Artist aggregates, Release semantic overlays, and search outbox rows.
5. Native per-user Artist state is folded per user: boolean flags use logical OR, rating uses the
   maximum, play counts are summed, timestamps retain the earliest creation and latest activity,
   and source rows are removed after the target upsert.
6. Provider identity candidates move to the target with exact duplicates collapsed. Provider images
   move by provider; a target row wins unless source evidence has higher confidence or equal
   confidence with a newer observation. Conflicting provider-task histories remain attributed to
   the tombstone and active duplicates are terminally failed; non-conflicting tasks retarget.
7. Singular curated identity metadata is target-owned. Target canonical/sort name, type,
   disambiguation, active dates, and existing operator image win. When the target has no operator
   image, one source override is adopted deterministically; additional source overrides remain on
   their tombstones for recovery rather than being silently deleted. Preview reports these conflicts.
8. The Admin Artists table supplies page-scoped multi-selection. The merge dialog requires a target,
   nonblank reason, server-produced impact preview, and typed `MERGE` confirmation. It states that
   canonical media is not moved and identifies target-wins metadata/image behavior.
9. Bounds remain 1,000 affected Releases, 10,000 Tracks, and 50,000 identity rows. A selection over
   one hundred sources or any existing correction bound fails closed without partial writes.

## Alternatives considered

### Extend the old single-source UUID form only

Rejected because repeated requests can partially apply a multi-Artist decision and the workflow is
not discoverable from the catalog where duplicates are found.

### Delete source Artists and rewrite canonical media directories

Rejected because it destroys stable public identities, conflicts with PRD-ARTIST-010, and ports the
legacy filesystem-owned identity defect that PRD-STORAGE-001 explicitly rejects.

### Merge every scalar field automatically

Rejected because types, dates, and disambiguation can carry identity-distinguishing evidence. The
administrator-selected target is authoritative; sources remain tombstones and names become aliases.

## Rationale

This preserves the stronger Rust identity and storage invariants while restoring the productive C#
catalog workflow. A bounded preview plus one transaction gives the operator understandable impact
and prevents half-applied batches. Explicit folding rules make newer user/provider data predictable
instead of leaving it accidentally attached to an invisible source.

## Consequences

### Positive

- Duplicate Artists can be selected and merged where administrators encounter them.
- Releases, Tracks, user state, provider evidence, images, and projections converge consistently.
- Existing public IDs and API clients continue to resolve through durable redirects.
- No canonical media bytes or paths change.

### Negative / trade-offs

- Target selection is semantically important because scalar metadata and an existing target image
  win conflicts.
- OR/max folding preserves positive user state but cannot distinguish an explicit later clear from a
  default false/zero value because the schema has no per-field event history.
- Source-only operator images beyond the adopted image remain on tombstones for recoverability and
  are reported by preview rather than silently discarded.

### Follow-up work

- None required for the accepted scope.

## Validation

- Persistence integration tests seed multiple sources with Release/Track credits, aliases,
  relationships, memberships, user-state conflicts, artwork, provider candidates/images/tasks, and
  chart links; they prove one target, redirects, aggregate/projection refresh, and unchanged media.
- A forced late-source conflict proves the multi-source transaction rolls back all earlier sources.
- Admin API tests preserve the existing route and cover preview/apply authorization, validation,
  impact shape, and atomic response.
- Admin UI tests cover selection, target choice, impact preview, typed confirmation, success refresh,
  and inline errors.
- `make check`, the Admin UI e2e/visual gate, and docs checks are required before completion.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0160](ADR-0160-cross-release-artist-release-identity-reuse.md)
- [ADR-0175](ADR-0175-converge-repeated-primary-release-artist-credits.md)

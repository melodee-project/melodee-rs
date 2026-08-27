# ADR-0173: Reconcile repeated Track Artist credits within one Release

- **Date:** 2026-08-27
- **Status:** Accepted
- **Decision owners:** User-directed duplicate Track Artist correction
- **Supersedes:** None
- **Superseded by:** None; narrowed by ADR-0174's explicit compilation and conflict gates
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-010,
  PRD-ARTIST-011, PRD-RESOLVE-001, PRD-RESOLVE-002, PRD-RESOLVE-003,
  PRD-RESOLVE-005, PRD-CREDIT-001, PRD-CREDIT-002, PRD-CREDIT-005

## Context

ADR-0172 changed a compound Track billing string into ordered structured Track Artist observations.
An unmatched component correctly received a provisional Artist identity, but the set-based importer
allocated a new identity for every occurrence. If `Glockboyz Teejaee` was featured on four Tracks of
one `9000 Rondae` Release, Admin Catalog therefore showed four active `Glockboyz Teejaee` Artists.
The live failure also reproduced with repeated primary Track billing such as `Phil Kinley`.

The observations are separate facts and must remain separate. The defect is that repeated exact
credits inside one immutable Release were not co-resolved. A global name merge remains forbidden by
PRD-RESOLVE-003, and an identically named Release Artist or verified identity can make even a
same-Release match ambiguous. The correction also has to repair rows committed by older binaries
without deleting evidence or requiring a database reset.

## Decision

After strong external-ID reconciliation and before name-only review proposals, Melodee may
merge repeated provisional Artist identities when all of these conditions hold:

1. every identity resolves only Track-scoped `ObservedArtist` rows from exactly one shared Release;
2. every observation has a structured `TrackArtist` relationship and none of the identities has a
   `ReleaseArtist` relationship;
3. every identity and observation has the same exact nonblank normalized value;
4. at least two qualifying identities exist for the `(Release, normalized value)` pair; and
5. the group contains no mixed-scope or verified identity, locked current decision, active local
   do-not-match assertion, or different external IDs from the same provider; and
6. the Release has exactly one primary Release Artist, is not billed to `Various Artists`, and has
   no durable `compilation` classification or catalog-trait assertion.

The lowest stable UUID is the deterministic survivor. The operation uses the existing audited,
reversible Artist merge, records same-Release name and Release-context evidence, and writes
automated unlocked replacement decisions. It never deletes raw observations, predecessor
decisions, external identities, or semantic relationships. A bounded pass runs after successful
imports and once when the catalog scheduler starts, allowing durable duplicates from older binaries
to converge without a destructive migration or catalog reset.

This is a Release-scoped co-resolution predicate, not an Artist identity key. If the Release has
ambiguous primary billing, is a known compilation, or otherwise falls outside the track-only shape,
Melodee fails closed and leaves the identities for Identity Review. Exact-name matches outside this
contextual rule become proposals under ADR-0174 and never automatic merges.

## Alternatives considered

### Allocate one identity per Track occurrence

Rejected because it exposes repeated billing observations as repeated Melodee-owned Artists and
inflates catalog, search, and compatibility results.

### Reuse any equal normalized Artist globally

Rejected by PRD-RESOLVE-003. Different Artists can share a name, and the Release boundary is the
evidence that makes this correction acceptably narrow.

### Hide duplicate names only in Admin Catalog

Rejected because it would conceal incorrect identity relationships while leaving counts, search,
API results, and later matching decisions wrong.

### Rewrite identities in a one-way data migration

Rejected because startup reconciliation can use the normal audited merge/split path, preserves
operator decisions, and remains safe when later evidence introduces a conflict.

## Rationale

ADR-0170 already accepts exact normalized comparison inside an immutable Release when one matching
Release Artist is unambiguous. ADR-0172 keeps each structured credit as an identity-free observation
until resolution. This decision applies the same bounded evidence principle to repeated track-only
credits while adding stricter conflict gates and retaining the existing correction history. It fixes
the concrete catalog defect without turning a name string into global identity authority.

## Consequences

### Positive

- Repeated primary and featured Track credits inside one Release converge to one active Artist.
- Existing catalogs repair on scheduler startup without data deletion or a reset/re-import.
- Every raw occurrence and every predecessor decision remains available for provenance and split.
- Release Artist ambiguity and durable operator/provider conflicts fail closed.

### Negative / trade-offs

- Two different same-name Artists credited on different Tracks of one Release can be co-resolved if
  no stronger conflict has yet been recorded. The audited split and do-not-match assertion are the
  correction and future fence, matching ADR-0170's accepted consistently-wrong-tag trade-off.
- Reconciliation adds one bounded indexed candidate scan at startup and after successful imports.

### Follow-up work

- Monitor reconciliation audit events for unexpected same-Release same-name collisions in real
  catalogs; use Identity Review split and do-not-match controls when encountered.

## Validation

- PostgreSQL integration constructs the durable older-importer shape, verifies exactly one bounded
  candidate, reconciles it, and proves raw observation retention, semantic-link retargeting,
  evidence references, automated unlocked authority, audit payload, and idempotency.
- Ambiguous same-name Release Artist billing does not enter the candidate set.
- Provider-ID conflict, locked-decision, and active do-not-match tests prove each group fails closed.
- Known-compilation and multiple-primary-Release-Artist tests prove ambiguous Release contexts fail
  closed.
- Production scheduler integration proves startup repairs an older durable duplicate without a new
  import.
- The full repository gate verifies formatting, Clippy, Rust/Python/UI tests, PostgreSQL integration,
  build, and dependency policy.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0160](ADR-0160-cross-release-artist-release-identity-reuse.md)
- [ADR-0170](ADR-0170-reuse-uniquely-matching-release-artist-identities-for-tracks.md)
- [ADR-0172](ADR-0172-parse-track-billing-with-release-scoped-identity-evidence.md)
- [ADR-0174](ADR-0174-require-review-for-name-only-artist-identity-matches.md)

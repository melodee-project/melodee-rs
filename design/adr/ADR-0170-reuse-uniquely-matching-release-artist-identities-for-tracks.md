# ADR-0170: Reuse uniquely matching release artist identities for tracks

- **Date:** 2026-08-26
- **Status:** Accepted
- **Decision owners:** User-directed multi-Artist Release deduplication correction
- **Supersedes:** ADR-0153
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-011,
  PRD-RESOLVE-001, PRD-RESOLVE-002, PRD-RESOLVE-003, PRD-CREDIT-001, PRD-CREDIT-002

## Context

ADR-0153 allowed a Track observation to reuse its matching Release Artist identity only when the
manifest contained exactly one nonblank Release Artist. That fixed ordinary single-Artist Releases,
but it left a pathological result for valid multi-Artist Releases. If a Release contained `Artist A`
and `Artist A/Artist B`, every Track tagged exactly `Artist A` still received a separate provisional
Artist identity. One 15-Track Release therefore exposed 15 active `Artist A` rows in Admin Catalog:
one Release identity and fourteen redundant Track identities.

The raw observations are correct and must remain distinct. The defect is treating the presence of
any second Release Artist as ambiguity even when the Track value has exactly one exact match among
the Release's primary Artist observations. A durable correction is required because the choice
changes identity allocation for future imports and must auditably repair identities already created
by the old rule. Global name matching remains forbidden.

## Decision

Within one immutable Release import, a nonblank primary Track Artist observation reuses a Release
Artist identity when:

1. the Track has one primary Artist observation;
2. exactly one primary Release Artist observation has the same exact normalized value; and
3. the match is scoped to that Release import.

Additional Release Artists with different normalized values do not make that exact match ambiguous.
If zero or more than one Release Artist observation has the Track's normalized value, the importer
retains a distinct provisional identity. The normalized value is a same-Release comparison
attribute, never a global identity key.

Release and Track `ObservedArtist`, presentation, evidence, and decision rows remain distinct. The
shared identity decision records same-Release context and the existing high confidence. Existing
importer-created Track identities satisfying the new predicate are repaired by an additive data
migration: relationships and current decisions move to the uniquely matching Release Artist, the
source Artist becomes an audited merge tombstone, and no observation or append-only decision is
deleted. Reversal uses the existing audited Artist split operation; rollback of the migration means
restoring a pre-migration backup or explicitly splitting affected merges.

## Alternatives considered

### Keep ADR-0153's singleton-only rule

Rejected because it confuses unrelated additional billing with ambiguity in an exact Track-to-
Release observation match and produces unusable duplicate catalog rows.

### Merge equal names globally or only hide duplicates in Admin Catalog

Rejected. A global merge violates PRD-ARTIST-001 and PRD-RESOLVE-003 because distinct Artists can
share a name. UI-only grouping would conceal real duplicate identities while leaving catalog links,
counts, search results, and compatibility APIs incorrect.

## Rationale

The Release is the same bounded evidence context already accepted by ADR-0153. Requiring exactly one
matching Release observation makes the extension deterministic and conflict-gated without assuming
that the entire Release has singleton billing. It corrects the observed defect while preserving
Melodee-owned identities, raw provenance, and the bias against global false merges.

## Consequences

### Positive

- Multi-Artist Releases no longer create one active Artist row per matching Track.
- Exact raw observations and immutable decision history remain available.
- Existing affected catalogs converge automatically through a scoped, audited migration.

### Negative / trade-offs

- Two same-Release Artist observations that normalize equally remain ambiguous and do not reuse an
  identity automatically.
- A wrong but consistently repeated tag can share the Release identity; the audited split operation
  remains the correction path, as under ADR-0153.

### Follow-up work

- Rich structured parsing of combined billing strings remains outside this decision; separators in
  display text do not themselves create or merge identities.

## Validation

- Import integration: a Track exactly matching one of multiple Release Artist observations reuses
  that identity; a second nonmatching Release Artist remains distinct.
- Ambiguity integration: duplicate Release observations with the same normalized value do not
  trigger reuse.
- Forward-migration integration: old importer-created duplicates are retargeted, tombstoned, and
  audited while observations and predecessor decisions remain intact.
- Admin catalog integration: retired merge sources do not inflate active Artist browse counts.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0153](ADR-0153-reuse-unambiguous-release-artist-identity-for-matching-tracks.md)

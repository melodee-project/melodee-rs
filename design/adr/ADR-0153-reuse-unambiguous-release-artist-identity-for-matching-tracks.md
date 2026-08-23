# ADR-0153: reuse unambiguous release artist identity for matching tracks

- **Date:** 2026-08-22
- **Status:** Accepted
- **Decision owners:** User-directed Artist identity correction
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-011,
  PRD-RESOLVE-001, PRD-RESOLVE-002, PRD-RESOLVE-003

## Context

ADR-0117 correctly prohibits using normalized names as global Artist identity keys and biases weak
evidence toward provisional duplicates. The initial importer applied that protection independently
to every Release and Track observation, creating one Artist identity per occurrence. A three-Release
catalog with one consistently billed Artist per Release therefore displayed 25 Artists even though
it contained only three logical identities.

Raw occurrence provenance must remain distinct, but exact agreement inside one immutable Release
manifest is stronger evidence than global string similarity.

## Decision

The manifest importer reuses the Release Artist identity for a Track observation only when:

- the manifest has exactly one nonblank Release Artist observation;
- the Track has one nonblank primary Artist observation;
- their normalized values are exactly equal; and
- the resolution is scoped to that same Release import.

The Release and Track retain separate `ObservedArtist`, presentation, match-evidence, and match-
decision rows. The shared `ArtistId` is recorded as a confirmed automated decision with explicit
same-Release context evidence. Different values, multiple Release Artists, conflicting evidence,
and Artists from different Releases remain distinct provisional identities unless stronger
evidence or an administrator decision resolves them.

Existing importer-created duplicates satisfying the same narrow predicate are converted into
audited Artist merge tombstones and their current Track relationships are retargeted. Admin
browsing lists only provisional/verified identities, not merged/deprecated tombstones.

## Alternatives considered

### Globally merge equal normalized names

Rejected by PRD-RESOLVE-003 because unrelated Artists can share a name.

### Keep one provisional Artist per observation

Rejected because it confuses occurrence provenance with identity and produces unusable catalog
counts for ordinary single-Artist Releases.

## Rationale

The rule uses Release context as structured evidence while preserving the conservative global
identity boundary. Observations remain auditable and can later be split if conflicting evidence
appears.

## Consequences

### Positive

- Ordinary Releases expose one Artist identity rather than one per Track.
- Raw Release/Track strings and source pointers remain independently available.
- Same-name Artists in different Releases are still not automatically merged.

### Negative / trade-offs

- Incorrect source tags repeated consistently within a Release initially share an identity; the
  existing audited split operation remains the correction mechanism.

### Follow-up work

- Rich multi-Artist billing still requires structured manifest presentation beyond the current
  singleton-safe rule.

## Validation

- Import tests prove one identity plus separate observations for equal singleton billing.
- The 200-Release bounded import test proves identical names in separate Releases remain separate
  and identity reuse does not add lookup/N+1 behavior.
- Migration and Admin Catalog tests prove repaired merge tombstones do not inflate browse counts.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)

# ADR-0172: Parse track billing with Release-scoped identity evidence

- **Date:** 2026-08-27
- **Status:** Accepted
- **Decision owners:** User-directed Track Artist billing correction
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-RESOLVE-001,
  PRD-RESOLVE-002, PRD-RESOLVE-003, PRD-CREDIT-001, PRD-CREDIT-002,
  PRD-CREDIT-004, PRD-CREDIT-005, PRD-CREDIT-006

## Context

Inspection previously placed the union of Album Artist and Track Artist tags into the Release Artist
observation list. Catalog import then treated every list entry and every complete Track Artist string
as one identity candidate. A Release by `6arelyhuman` therefore acquired Release and Track identities
such as `6arelyhuman/Baby Jane`. A Track billed as
`9000 Rondae/Glockboyz Teejaee/OnFully/Roadrunner Glockboyz Tez` became one compound Artist rather
than one primary Artist and three featured Artists.

The catalog already models exact `DisplayArtist`, ordered `TrackArtist` relationships, roles, join
phrases, and identity-free `ParsedArtistCandidate` values. The defect is at the observation and
import boundaries. Separators are not universally safe identity delimiters: `/` is part of real
Artist names such as `AC/DC`, and `&` and `x` are even more ambiguous. PRD-CREDIT-005 and SPEC section
8.7 also forbid a parser from assigning or merging identity by itself.

## Decision

1. Inspection derives Release Artist observations from nonblank Album Artist tags. Track Artist tags
   are retained on their Tracks and in source-label evidence but do not become Release Artists. If
   every Album Artist tag is absent, the distinct Track Artist values are a fail-soft Release Artist
   fallback so otherwise valid local metadata can still enter review.
2. Catalog import recognizes case-insensitive, whitespace-bounded `featuring`, `feat.`, `feat`,
   `ft.`, `ft`, and `with` phrases. It preserves the exact source string as Track `DisplayArtist`,
   emits one identity-free candidate per component, retains each exact join phrase, marks the first
   component primary, and marks later components featured.
3. Slash-delimited Track billing is split only when the complete value is not itself a unique exact
   Release Artist match and at least one slash component uniquely matches a primary Release Artist
   observation on that same Release. Matching components are primary and the remaining components
   are featured. Otherwise the complete slash-containing value remains one literal candidate.
4. Each parsed component becomes its own `ObservedArtist`, evidence, and decision path. An exact
   unique same-Release Artist match may reuse that identity under ADR-0170; an unmatched component
   creates a distinct provisional identity. Names are never compared globally.
5. Embedded Artist external IDs are attached only when billing remains a single candidate. A
   multi-candidate string does not establish which component owns one unstructured external ID.
6. Existing imported catalogs are not automatically split. Verification, administrator edits, and
   literal slash names make an unconditional backfill unsafe. Development/test catalogs use the
   documented reset and re-import workflow; retained catalogs use Identity Review or a future
   previewable, operator-authorized repair workflow.

## Alternatives considered

### Split every separator globally

Rejected because it would corrupt literal Artist names and collaboration names. It also turns text
syntax into identity authority, contrary to PRD-CREDIT-005.

### Preserve every Track Artist value as one opaque Artist

Rejected because it reproduces the reported compound identities and discards the structured Track
billing that Melodee already models.

### Automatically rewrite every existing compound identity

Rejected because the historical string alone cannot prove that a slash was a delimiter, and an
existing Artist may have acquired verified evidence or administrator changes. A silent backfill
would be destructive identity inference.

## Rationale

Explicit phrases supply strong presentation syntax without claiming identity. Slash syntax becomes
safe enough only inside the bounded Release context where one component is already known as a
Release Artist. Separating candidate extraction from resolution preserves exact display fidelity,
same-name identity safety, and the established ADR-0170 reuse rule.

## Consequences

### Positive

- Release Artist lists no longer contain per-Track collaboration strings when Album Artist metadata
  is present.
- Featured Track Artists participate in catalog/search association projections with the
  `featured_artist` role.
- Exact capitalization, separators, spacing, and Track `DisplayArtist` text remain available.
- A single Track can expose any bounded number of ordered featured Artists.

### Negative / trade-offs

- Slash billing without a same-Release component remains literal and may require Identity Review.
- `&`, `+`, comma, and bare `x` syntax remains literal because the false-split risk is too high.
- Existing imported test data requires reset/re-import before the corrected structure appears.

### Follow-up work

- Consider an administrator-previewed repair workflow if production catalogs show a material volume
  of historical compound identities that cannot be handled through normal Identity Review.

## Validation

- Domain tests prove explicit and slash parsing preserves exact join phrases, fails closed on
  malformed separators, and produces candidates with no Artist identity.
- Media inspection tests prove Album Artist observations exclude differing Track Artist billing and
  retain the Track Artist fallback when Album Artist is absent.
- PostgreSQL import tests cover `6arelyhuman/Baby Jane`, the four-Artist `9000 Rondae` example,
  explicit `Featuring` plus `with`, featured participation roles, exact `DisplayArtist`, and a
  literal `$NOT/Jeht` Release Artist that must not split.
- The full repository quality gate validates API and compatibility surfaces remain unchanged.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0170](ADR-0170-reuse-uniquely-matching-release-artist-identities-for-tracks.md)

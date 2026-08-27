# ADR-0174: Require review for name-only Artist identity matches

- **Date:** 2026-08-27
- **Status:** Accepted
- **Decision owners:** User-directed same-name Artist identity correction
- **Supersedes:** ADR-0160 decision 1(b), decision 2, and the name-only portions of decisions 5 and 8
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-005,
  PRD-ARTIST-010, PRD-ARTIST-011, PRD-ARTIST-012, PRD-RESOLVE-001,
  PRD-RESOLVE-002, PRD-RESOLVE-003, PRD-RESOLVE-005

## Context

ADR-0160 allowed exact normalized-name equality plus the absence of known conflicts to merge
provisional Artists across Releases. That rule conflicts with the higher-authority
PRD-RESOLVE-003 invariant that string similarity alone never causes an automatic identity merge.
Absence of a recorded conflict is not positive proof of identity: two legitimate Artists can have
the same name, as with the British and American groups named `Nirvana`.

The catalog also needs a useful path for likely duplicates. Silently retaining every duplicate is
not sufficient, but collapsing identities before external or administrative evidence arrives loses
semantic correctness. Existing identity primitives already separate raw `ObservedArtist` facts,
non-authoritative proposals, authoritative current decisions, negative assertions, audited merges,
and reversible splits. The correction is therefore a policy and read-model change, not a schema
rewrite.

## Decision

1. Exact normalized-name equality is discovery evidence only. Across Releases it appends immutable
   `name_similarity` evidence and an automated, unlocked `proposed` decision for Identity Review.
   It never advances `artist_match_current`, retargets relationships, or writes a merge event.
2. Multiple active Artists may have the same canonical and normalized name. They keep distinct
   internal and public IDs, and an operator may set disambiguation text such as `UK psychedelic
   group` or `US grunge band`. Identity Review displays that disambiguation beside each candidate.
3. Cross-Release automatic Artist convergence is limited to a shared, well-formed trusted provider
   identifier (currently a MusicBrainz UUID at confidence 9000 or greater) or an explicit
   administrator merge. Even the provider-ID path fails closed for different IDs from the same
   provider, incompatible known Artist types, disjoint known active-date ranges, locked decisions,
   active do-not-match assertions, or disjoint known group-member sets. Area evidence is retained
   for review by the resolver; the canonical Artist table currently has no area field, so absence of
   area data can never make a merge safer.
4. Name proposals are idempotent per observation/candidate/current-decision epoch. A current
   imported provisional self-decision does not hide a newer alternative proposal from Identity
   Review. A later administrator confirmation, merge, active do-not-match assertion, or candidate
   deprecation closes or suppresses the proposal.
5. ADR-0173's same-Release Track-credit co-resolution remains allowed only as a contextual rule:
   identities must be provisional, track-only, confined to one Release, exact-name equal, unlocked,
   and free of negative/provider conflicts. The Release must have exactly one primary Release
   Artist and must not have `Various Artists` billing or durable `compilation` classification or
   catalog-trait evidence. Ambiguous and known-compilation Releases fail closed to proposals/review.
6. All automatic strong-ID and same-Release merges continue through the existing audited,
   reversible merge/split path and preserve raw observations, predecessor decisions, external
   evidence, and public-ID redirects.

## Alternatives considered

### Keep ADR-0160's conflict-gated exact-name merge

Rejected because no-conflict is not evidence of sameness and the behavior contradicts
PRD-RESOLVE-003. Reversibility mitigates an error after it happens but does not justify creating a
false merge.

### Never surface name-only candidates

Rejected because it leaves likely duplicates invisible and forces operators to discover them by
manual catalog browsing. A proposal is useful while remaining non-authoritative.

### Make canonical name unique with suffixes

Rejected because a string is not Artist identity. Disambiguation is presentation metadata, not a
key, and the canonical database must permit legitimate same-name Artists.

## Rationale

The decision directly restores the PRD evidence boundary: names find candidates, strong evidence or
an operator decides identity. It reuses the existing append-only evidence, proposal, negative,
merge, and split model, so the fix is reversible and requires no destructive data migration.

## Consequences

### Positive

- Legitimate same-name Artists remain separate by default.
- Likely duplicates remain visible and actionable in Identity Review.
- Trusted provider automation stays available at catalog scale with explicit conflict fences.
- Existing observations, public IDs, and audit history are preserved.

### Negative / trade-offs

- Catalogs without trusted IDs require operator review for cross-Release duplicates.
- A name shared by many Artists can create several bounded review candidates.
- Location conflicts cannot be enforced as a canonical SQL gate until Artist area data is modeled;
  they remain resolver/review evidence and never authorize automatic merging.

### Follow-up work

- If canonical Artist area data is added, define its conflict semantics in a migration-backed ADR
  before adding it to the strong-ID gate.

## Validation

- Domain tests prove an automated name-only confirmed decision is invalid.
- PostgreSQL integration tests prove cross-Release exact names remain two active Artists, conflicting
  provider IDs and active dates block strong automatic convergence, negative assertions and locked
  decisions suppress proposals, and known compilations block same-Release co-resolution.
- Catalog scheduler integration proves exact names create proposals visible in the attention queue
  without merge events, while a shared trusted MusicBrainz Artist ID still produces one audited
  merge.
- Persistence/API/UI tests prove candidate disambiguation reaches and renders in Identity Review.
- Existing same-name search tests prove distinct same-name identities remain independently
  searchable.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0160](ADR-0160-cross-release-artist-release-identity-reuse.md)
- [ADR-0173](ADR-0173-reconcile-repeated-track-artist-credits-within-one-release.md)

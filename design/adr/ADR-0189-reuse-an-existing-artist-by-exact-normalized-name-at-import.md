# ADR-0189: Reuse an existing Artist by exact normalized name at import

- **Date:** 2026-06-11
- **Status:** Accepted
- **Decision owners:** User-directed Artist identity parity work
- **Supersedes:** [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md) — only its "creates provisional Artists when no credible or locked match exists rather than merging by name" import rule
- **Superseded by:** None
- **Extends:** [ADR-0175](ADR-0175-converge-repeated-primary-release-artist-credits.md) (same conflict gates, applied to every credit form) and [ADR-0174](ADR-0174-require-review-for-name-only-artist-identity-matches.md) (name-only proposals still require review)
- **Related requirements:** PRD-ARTIST-001 through PRD-ARTIST-012, PRD-RESOLVE-001 through
  PRD-RESOLVE-005, PRD-CREDIT-001 through PRD-CREDIT-008, PRD-CATALOG-001 through PRD-CATALOG-005,
  PRD-INGEST-007 through PRD-INGEST-010

## Context

ADR-0117 decided two different things at once:

1. **Names are not identity keys.** Two Artists may share an exact canonical or normalized name, and
   nothing may make a name unique, authoritative, or a primary key.
2. **Catalog import creates a provisional Artist whenever no credible or locked match exists inside
   the Release being imported.**

The first statement is a durable identity invariant and remains correct. The second was an
implementation rule that diverged from the behaviour of the C# product Melodee replaces, and it
produced a visible defect.

The C# importer resolves credits against one lookup built from the existing catalog,
`existingArtistLookup.TryAdd($"name:{artist.NameNormalized}", existingArtist)`, and reuses it across
albums (`src/Melodee.Common/Jobs/LibraryInsertJob.cs`). A C# catalogue therefore cannot contain two
`2 Chainz` rows created by import: the second Release reuses the first Artist. The Rust importer
instead created one provisional Artist per Release and left cross-Release consolidation to the
Identity Review queue.

Measured on the development catalogue before this decision: **57 duplicate normalized-name groups
covering 132 Artist rows, 54 of the groups with byte-identical canonical names, and zero conflicting
external identities.** Every one of the 2,817 queued Identity Review proposals and 2,177 `confirmed`
per-Release decisions was bookkeeping for a duplicate the importer itself had just created; none was
evidence about a genuinely ambiguous artist. Operator-visible symptoms included split discographies,
fragmented counts, and an Identity Review queue that could never be cleared.

ADR-0175 later acknowledged the same defect from live evidence and converged the resulting rows
*after* import. That repaired existing catalogues but left the importer creating one duplicate row per
Release, so the queue kept refilling and every new release paid for a duplicate it did not need.

The defect was invisible to every gate because no test asserted "the same name on two Releases is one
Artist", and the divergent behaviour was documented as an accepted decision rather than a bug.

## Decision

**Import reuses an existing Artist when the normalized name is exactly equal and the reuse is
unambiguous.** A name is still not an identity key; it is a *reuse hint bounded by evidence*.

### Import-time reuse

For each credit resolved during a manifest import, reuse an existing Artist row when all of the
following hold. Otherwise create a new provisional Artist exactly as before.

- The existing row's `normalized_name` is exactly equal. No similarity, fuzzy, prefix, diacritic, or
  article-insensitive matching, and no alias matching: `The Coral` and `Coral` do not match.
- Exactly one non-tombstoned (`status IN ('provisional','verified')`, `merged_into_artist_id IS NULL`)
  Artist carries that normalized name. Two or more candidates refuse reuse and fall through to
  provisional creation plus review.
- The existing row has no disambiguation hint.
- The credit carries no embedded MusicBrainz Artist identifier, or the existing row's identifier for
  that provider is exactly equal. A conflicting trusted identifier refuses reuse.
- Artist type, active dates, and aliases are not rewritten by reuse.

### Post-import reconciliation of existing duplicates

Import-time reuse cannot repair a catalogue already written under ADR-0117, so the existing bounded
Artist reconciliation sweep additionally converges duplicate rows **for every credit form** — primary
Release credits and Track credits alike — when the whole active name group carries no differentiating
identity fact.

The existing guards are unchanged and are the reason this is not a name-based merge:

- every row in the active name group is provisional, disambiguation-free, and free of external
  identifiers, relationships, group memberships, and non-name aliases;
- every row carries only automated, unlocked decisions, and every observation owned by the group
  matches the name;
- no single Release credits two Artists with that name;
- no active negative assertion (`do_not_match`) exists for the name group;
- `artist_type`, `begin_date`, and `end_date` agree across the group.

Each convergence remains an append-only `artist_merge_events` row whose
`moved_relationships.reconciliation.evidence` names the predicate (`repeated_credit`), the scope
(`different_releases`), the role, `conflict_gated: true`, and the merged Artist id. It is reversible
through the existing audited split path. Rows that do not satisfy every guard stay separate and are
proposed for review, never merged.

### What this decision does not change

- `artists.normalized_name` remains non-unique and non-authoritative; SQL uniqueness is still
  forbidden.
- Same-name Artists remain fully representable, addressable by distinct public IDs, and preserved by
  every merge/split/public-ID rule in ADR-0117.
- Provider identities remain evidence, not keys.
- Manual decisions, locks, and negative assertions keep their current precedence.

## Alternatives considered

### Keep ADR-0117's rule and continue reconciling by review

Rejected. It manufactures duplicates at import and then asks an operator to collapse them, which
diverges from the replaced product for no benefit. The measured catalogue shows the queue was pure
self-inflicted backlog, not ambiguity resolution.

### Make `normalized_name` unique

Rejected and remains forbidden. Genuinely different artists share names (`Nirvana`, `Dune`), public
IDs and tombstones must survive, and a unique name would make migration and manual correction
impossible.

### Merge by name without the evidence guards

Rejected. Fuzzy or unconditional name merging would collapse `Nirvana` (UK psychedelic) into `Nirvana`
(US grunge) and destroy real identity.

### Reuse regardless of external identifiers

Rejected. Two same-name rows with different trusted MusicBrainz Artist ids are the strongest available
evidence that a name collision is a genuine homonym; reuse must refuse there.

## Rationale

The C# product reuses a name-matched Artist at import and cannot produce these duplicates, so
restoring that behaviour recovers feature parity for the workflow operators actually use, while the
evidence guards keep the identity model from regressing into name-keyed identity. Keeping
reconciliation as a second, guarded path means existing catalogues converge without a destructive
migration, and every convergence stays audited and reversible.

## Consequences

### Positive

- Import can no longer create a duplicate row for a name that is unambiguous in the catalogue.
- Existing duplicates converge through the already-audited reconciliation path, and the Identity
  Review queue stops accumulating self-inflicted proposals.
- Discographies, counts, and browse surfaces agree with the replaced product.
- Reuse reports the resolved Artist in `CatalogImportResult.artist_count` and refreshes the reused
  Artist's aggregates, so downstream counters stay truthful.

### Negative / trade-offs

- A genuinely new artist who happens to share an existing single name with no disambiguating fact is
  annexed into the existing row and later needs a manual split. This is the same risk the C# product
  takes, and the split path exists and is audited.
- Two code paths (import-time reuse, post-import reconciliation) must agree on the guards; the shared
  test matrix below is the mechanism that keeps them aligned.
- Reuse makes artist row counts depend on import order, so acceptance fixtures must not assume one
  Artist row per Release.

### Follow-up work

- Known-corpus coverage for both directions (equal names must converge; a name collision with
  conflicting provider identities must stay split).
- Provider enrichment continues to resolve identities with provider evidence after import; name reuse
  does not enqueue extra enrichment.

## Validation

- Import integration test: two Releases by one name yield one Artist carrying both Release and Track
  credits, one aggregate row (`release_count = 2`), no merge events, and no leftover name-only
  proposals.
- Guard test: a disambiguation hint present before the second import refuses reuse, leaving two
  provisional Artists and no convergence candidate.
- Legacy-catalogue test: two rows created under the old rule converge through
  `repeated_artist_credit_merge_candidates` plus `reconcile_artist`, remain audited as
  `repeated_credit` evidence, and are restored by `split_artist`.
- Conflicting embedded MusicBrainz identifiers keep same-name Artists apart, and a negative assertion
  or locked decision still suppresses both the recovery proposal and the convergence candidate.
- Bounded 200-Release acceptance scenario continues to assert exactly-once import, constant set-based
  statement counts, and no media reads.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0105](ADR-0105-postgresql-is-authoritative-state-store.md)
- [ADR-0110](ADR-0110-use-sqlx-and-explicit-sql.md)
- [ADR-0160](ADR-0160-cross-release-artist-release-identity-reuse.md)
- [ADR-0174](ADR-0174-require-review-for-name-only-artist-identity-matches.md)
- [ADR-0175](ADR-0175-converge-repeated-primary-release-artist-credits.md)
- ADR-0117's import rule superseded here corresponds to `crates/melodee-persistence/src/catalog.rs`
  (`reuse_existing_artists`) and `crates/melodee-catalog/src/importer.rs`
  (`reconcile_repeated_artist_credit_identities`).

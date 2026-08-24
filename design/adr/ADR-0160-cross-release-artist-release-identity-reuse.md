# ADR-0160: Cross-Release Artist/Release identity reuse and disposition

- **Date:** 2026-08-24
- **Status:** Accepted
- **Decision owners:** Feature Completion Program Phase 0 (explicitly user-assigned)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-010, PRD-ARTIST-011,
  PRD-RESOLVE-001, PRD-RESOLVE-002, PRD-RESOLVE-003, PRD-RESOLVE-005, PRD-CATALOG-001

## Context

ADR-0153 (Accepted) authorizes the manifest importer to **reuse the Release Artist identity for a
Track observation** only when the match is *unambiguous and scoped to the same Release import*:
exactly one nonblank Release Artist observation, exactly one nonblank Track primary Artist
observation, normalized values exactly equal, and the resolution confined to that Release. This
corrected the "25 Artists for a 3-Release catalog" duplication **within** a Release.

What ADR-0153 does **not** decide is the **cross-Release** case: the same logical Artist billed on
two different Releases (e.g., the same Artist string on Release A and Release B) currently
produces **two distinct provisional `Artist` identities** (one per Release import), because the
reuse rule is explicitly scoped to "that same Release import." The same applies to the **Release**
identity: two Releases that are the same logical release (same Artist + title + year, or the same
MusicBrainz Release ID) are not automatically recognized as one. This is the "cross-Release identity
disposition" entry decision: how and when two provisional identities from *different* Releases are
promoted to a single Melodee-owned identity, and what evidence and audit trail that promotion
requires.

The identity schema (ADR-0117) already provides the primitives: `Artist` (Melodee-owned identity),
`ObservedArtist` (raw occurrence with provenance), `ArtistMatch` (evidence + decision),
`ArtistMergeTombstone` (audited merge), and provider-scoped `artist_external_identities`
(evidence-only, PRD-ARTIST-005/012). PRD-RESOLVE-003 forbids using normalized names as a *global*
identity key and biases weak evidence toward provisional duplicates; PRD-RESOLVE-005 requires that
any identity consolidation be audited and reversible (a split operation exists). The gap is the
**policy** for cross-Release promotion, not the schema.

## Decision

1. **Cross-Release Artist promotion rule.** Two provisional `Artist` identities (from different
   Release imports) are promoted to a single identity when **any** of the following *strong*
   evidence predicates holds; each promotion is an audited, reversible decision:
   - **(a) External-ID evidence (primary).** Both identities carry an `artist_external_identity`
     row for the **same provider + same external ID** (e.g., the same MusicBrainz `mbid`). This
     is the strongest, provider-grounded evidence and is the default path for provider-enriched
     catalogs (PRD-ARTIST-005/012).
   - **(b) Unambiguous name evidence (secondary).** Both identities have exactly one
     nonblank primary name observation each, the normalized values are **exactly equal**, and
     there is **no conflicting evidence** (no other provisional identity with the same normalized
     name that has a different external ID, no `ArtistMatch` row marking the name as ambiguous).
     This is the ADR-0153 rule **generalized across Releases**, and it is the fallback when
     provider enrichment is unavailable.
   - **(c) Administrative decision.** An administrator explicitly merges the two identities
     (the existing audited merge operation); this is always allowed and overrides the
     provisional state.
2. **No global name-only merge.** Predicate (b) is **not** a blind global name merge: it is
   gated by the "no conflicting evidence" condition. If a third provisional identity with the same
   normalized name exists and carries a *different* external ID (or an admin has marked the name
   ambiguous), (b) does **not** fire; the identities remain provisional pending (a) or (c).
   This preserves PRD-RESOLVE-003 (normalized names are never a *global* identity key).
3. **Cross-Release Release (album) promotion.** Two provisional `Release` identities are promoted
   to a single Release when:
   - **(a)** both carry an `artist_external_identity`/release-external-ID row for the same
     provider + same release ID (e.g., the same MusicBrainz release ID); **or**
   - **(b)** the `(primary Artist identity after Artist promotion, normalized release title,
     release year)` triple is exactly equal and there is no conflicting evidence.
   Track-level identity is preserved across the Release promotion (Tracks are not re-keyed; their
   `release_id` is retargeted to the surviving Release in an audited transaction).
4. **Audit and reversibility.** Every promotion writes an `ArtistMergeTombstone` (or
   `ReleaseMergeTombstone`) row recording: the two source identities, the surviving identity, the
   predicate that fired (a/b/c), the evidence rows (external IDs / name observations), the
   deciding principal (automated vs. admin), and the timestamp. The existing audited **split**
   operation (PRD-RESOLVE-005) remains the reverse: a promotion can be split, restoring the two
   provisional identities and their `ObservedArtist`/evidence rows. A promotion is **never**
   destructive: no `ObservedArtist`, `ArtistMatch`, or `artist_external_identity` row is deleted.
5. **Idempotency / set-based.** The promotion is a set-based, idempotent pass over provisional
   identities: running it twice does not re-merge an already-merged pair. It is triggered at
   import (a post-import reconciliation pass) and on demand (an admin "reconcile identities"
   action), not on every single observation.
6. **Provider scope.** Predicate (a) uses provider evidence only as *evidence*; it never makes a
   provider the canonical authority (PRD-ARTIST-012). The surviving identity is a Melodee-owned
   `Artist`/`Release`; the external ID is a provenance row on it.
7. **Scope boundary.** This ADR governs **Artist and Release** identity promotion. Track
   identity, Contribution, and Disposition are unaffected (Tracks are retargeted to the surviving
   Release; their own identity is not merged across Releases). MediaAsset identity is unaffected
   (a file is its own MediaAsset regardless of which Release it is cataloged under).
8. **Admin visibility.** The admin SPA (Feature Completion Phase 2, ARTIST workstream) surfaces
   provisional cross-Release identities that *could* be promoted under (a) or (b) as "candidate
   merges," with the evidence (external IDs / name) displayed, so an operator can review and
   approve (or split) a promotion. The promotion itself is automated when (a) or (b) fires without
   conflict; the admin surface is the oversight and correction path.

## Alternatives considered

### Keep one provisional identity per Release (ADR-0153 scope only)

Rejected by the user-assigned product direction: the cross-Release identity disposition is an
entry decision the feature-completion program must make. Leaving the same Artist as N provisional
identities (one per Release) is the pre-ADR-0153 duplication problem, now at the Release level.

### Blind global normalized-name merge

Rejected by PRD-RESOLVE-003: unrelated Artists (and unrelated Releases) can share a name; a
blind global merge would collapse distinct identities. Predicate (b) is gated by the "no
conflicting evidence" condition, so it is a *conservative generalization* of ADR-0153, not a
blind merge.

### Defer cross-Release promotion to an admin-only operation

Rejected: provider-grounded promotion (predicate a) is safe to automate (same provider + same
external ID is strong evidence), and requiring an admin to click-merge every same-Artist-across-
Releases pair would be unusable at catalog scale. Automation for (a)/(b) with admin oversight and
a reversible split is the right balance.

## Rationale

The identity schema (ADR-0117) and the ADR-0153 reuse rule already establish that
*occurrence provenance stays distinct* and *identity is Melodee-owned*. What is missing is the
cross-Release promotion policy. Using provider external IDs as the primary (strongest) evidence
and the ADR-0153 name rule (generalized, conflict-gated) as the secondary keeps the decision
consistent with the existing conservative identity boundary, while the audited, reversible merge
tombstone + split operation satisfies PRD-RESOLVE-005. This is a *policy* decision on top of
existing schema; it does not change the schema, the ADR-0117 invariants, or the provider-
evidence-only rule.

## Consequences

### Positive

- The same Artist across Releases is one Melodee-owned identity (not N provisional identities),
  closing the cross-Release identity disposition.
- Provider-grounded promotion is automated and safe; name-based promotion is conservative and
  conflict-gated.
- Every promotion is audited and reversible; no occurrence provenance is lost.

### Negative / trade-offs

- A mis-promotion (two distinct Artists sharing a name, no external ID to disambiguate) is
  possible under (b); the split operation is the correction, and the "no conflicting evidence"
  gate minimizes (but does not eliminate) this risk.
- The promotion pass adds an import/reconciliation step and an admin oversight surface.

### Follow-up work

- Feature Completion Phase 2 (ARTIST workstream): the cross-Release promotion pass (predicate a:
  external-ID join; predicate b: conflict-gated name join); the `ReleaseMergeTombstone` row and
  Release-promotion pass; the audited split reverse; the admin "candidate merges" review screen;
  property tests (idempotency, reversibility, no-conflict-gate).

## Validation

- Artist promotion (a): two provisional Artists with the same MusicBrainz external ID are
  promoted to one; the tombstone records predicate (a) and the external-ID evidence; the split
  restores two provisional identities.
- Artist promotion (b): two provisional Artists with exactly equal normalized names and no
  conflicting evidence are promoted; a third provisional Artist with the same name but a
  different external ID blocks promotion (identities remain provisional).
- Release promotion: two provisional Releases with the same (Artist-after-promotion, title,
  year) are promoted; Tracks are retargeted to the surviving Release in an audited transaction.
- Idempotency: running the promotion pass twice does not re-merge an already-merged pair.
- No destructive row loss: every promotion leaves all `ObservedArtist`, `ArtistMatch`, and
  `artist_external_identity` rows intact.
- No existing identity behavior changes (ADR-0117/0153 within-Release behavior is unchanged;
  golden diff).

## References

- [PRD.md](../../PRD.md) §13 (PRD-ARTIST-001/002/010/011), §14 (PRD-RESOLVE-001/002/003/005),
  §19 (PRD-CATALOG-001)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
  (identity/contribution schema; occurrence provenance distinct)
- [ADR-0153](ADR-0153-reuse-unambiguous-release-artist-identity-for-matching-tracks.md)
  (within-Release reuse rule; this ADR generalizes it across Releases)
- `design/evidence/phase-4/` (identity schema evidence)

# ADR-0175: Converge repeated primary Release Artist credits

- **Date:** 2026-08-27
- **Status:** Accepted
- **Decision owners:** User-directed correction after live `4Trey` and `Alice Cooper` catalog evidence
- **Supersedes:** ADR-0174 decision 1 for repeated primary Release Artist context
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-010,
  PRD-ARTIST-011, PRD-RESOLVE-001, PRD-RESOLVE-002, PRD-RESOLVE-003,
  PRD-RESOLVE-004, PRD-RESOLVE-005

## Context

ADR-0174 corrected unsafe global name merging by retaining every cross-Release exact-name match as
a separate provisional Artist. Live validation exposed the opposite failure mode: every Release
import created another indistinguishable primary Artist. The development catalog contained two
active `4Trey` rows and fourteen active `Alice Cooper` rows. Each row was provisional, carried the
same exact normalized primary Release credit, and had no external ID, disambiguation, type, date,
relationship, membership, or manual evidence that could distinguish it.

Merely adding Identity Review proposals did not make the catalog usable and encoded the wrong
expected behavior in tests. At the same time, normalized names cannot become global identity keys:
legitimate homonyms such as the British and American Artists named `Nirvana` must remain distinct
when differentiating evidence is present. The durable policy therefore needs positive Release-role
context plus group-wide conflict gates, audit, and reversibility.

## Decision

1. Melodee automatically consolidates provisional Artists when the same exact normalized value is
   recorded as a primary Artist credit on different non-deleted cataloged Releases and the complete
   active
   same-name group is indistinguishable.
2. The complete group must be provisional and controlled only by unlocked automated decisions.
   The group is rejected when any member has an external identity, nonblank disambiguation,
   differentiating alias, membership, relationship, administrator or locked decision, active
   do-not-match assertion, mismatched observation, different Artist type, different active dates,
   or ambiguous same-Release primary billing. Strong shared provider-ID reconciliation remains a
   separate, earlier predicate.
3. Exact name plus repeated primary Release role is persisted as `name_similarity` and
   `release_context` evidence. This is not string similarity alone and does not make normalized
   name unique. Names that lack this context or fail any gate remain separate and reviewable under
   ADR-0174.
4. The oldest eligible Artist ID is the deterministic survivor. Every source becomes a redirect;
   observations, Release and Track credits, public IDs, predecessor decisions, and audit history
   are preserved through the existing reversible merge/split operation. The survivor remains
   provisional.
5. Reconciliation runs after successful imports and once at catalog-scheduler startup, including
   repair of duplicates created by older binaries. Candidate reads and mutation scope remain
   bounded and idempotent.

## Alternatives considered

### Keep every exact-name identity until manual review

Rejected because it produced fourteen indistinguishable Alice Cooper rows in ordinary catalog
browsing and requires an operator to repair every Release import manually.

### Make normalized Artist name globally unique

Rejected because a string is not Artist identity and legitimate same-name Artists exist. A unique
constraint would also make later disambiguation and safe split workflows substantially harder.

### Merge every exact normalized name

Rejected because it would collapse known homonyms and ignore higher-quality conflicting evidence.
The selected predicate requires primary Release context and fails closed for the entire name group
as soon as differentiating evidence exists.

## Rationale

Primary Release billing is durable catalog context identified by PRD-RESOLVE-002, so the decision
does not weaken PRD-RESOLVE-003 into name-only matching. Group-wide gates preserve the bias against
false merges, while provisional status, redirects, immutable observations, audit events, and split
support keep the repair reversible when source metadata lacks a real-world distinction.

## Consequences

### Positive

- Repeated ordinary Release imports no longer create indistinguishable Artist-list rows.
- Existing conflict-free duplicates converge at startup without a database reset.
- Provider IDs, disambiguation, dates, types, relationships, membership, and operator decisions
  protect legitimate homonyms.
- Every consolidation remains visible, audited, and reversible.

### Negative / trade-offs

- Two real same-name Artists with identical primary billing strings and no differentiating metadata
  are unknowable from the available source facts and may share one provisional identity. Adding an
  external ID or disambiguation before reconciliation prevents that; an audited split corrects it
  afterward.
- Conservative group-wide gates may leave a true duplicate for Identity Review when only one row
  has partial enrichment.

### Follow-up work

- None required for the bounded correction. Future area metadata should join the differentiating
  evidence gates when it becomes canonical Artist data.

## Validation

- PostgreSQL integration covers contextual convergence and audit, different provider IDs, active
  dates, disambiguation, negative assertions, manual locks, and same-Release ambiguity.
- Scheduler integration imports and repairs the concrete two-`4Trey` case and a fourteen-Artist
  Alice Cooper startup backlog whose Releases are retained as deprecated, leaving one provisional
  survivor and audited redirects.
- Existing Identity Review tests continue to cover proposals and candidate disambiguation for
  names that do not satisfy this predicate.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0160](ADR-0160-cross-release-artist-release-identity-reuse.md)
- [ADR-0173](ADR-0173-reconcile-repeated-track-artist-credits-within-one-release.md)
- [ADR-0174](ADR-0174-require-review-for-name-only-artist-identity-matches.md)

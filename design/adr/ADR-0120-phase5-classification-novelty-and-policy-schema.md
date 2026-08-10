# ADR-0120: Phase 5 classification, novelty, and policy schema

- **Date:** 2026-08-10
- **Status:** Accepted
- **Decision owners:** Phase 5 implementation
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-CLASS-001, PRD-CLASS-002, PRD-CLASS-003, PRD-POLICY-001,
  PRD-POLICY-002, PRD-POLICY-003, PRD-POLICY-004, PRD-POLICY-005, PRD-POLICY-006,
  PRD-POLICY-007, PRD-POLICY-008

## Context

Phase 4 established the authoritative PostgreSQL catalog with Melodee-owned Artist, Release,
Track, MediaAsset, and AudioIdentity identities, plus durable evidence/decision/audit relationships.
The legacy C# server inferred classification (e.g. "Greatest Hits") from filename regexes and could
trigger destructive deletion behavior based on directory names or single metadata fields.

Phase 5 must replace that with explainable, non-destructive-first classification and policy:

- `ReleaseTrait` must be multi-valued (not a single mutually-exclusive type).
- Classification must retain evidence and confidence per trait.
- Evidence must follow a hierarchy: admin decisions > trusted embedded metadata > structured tags
  > catalog analysis > release title > directory name > parent directory name. Directory/path naming
  alone must never authorize destructive action.
- Policy must evaluate classification + Track novelty + catalog context and return a disposition
  (Accept, Review, Ignore, Quarantine, Delete).
- Greatest Hits collections must use catalog novelty analysis, not title-only regex.
- A durable decision/audit record must exist before any destructive action.
- Admin UI must support testing a proposed policy against the existing catalog without applying it.

## Decision

### Classification

- A Release has zero or more `ReleaseTrait` values in a `BTreeSet<ReleaseTrait>`. The model
  supports multiple traits simultaneously (e.g. `Studio + Compilation`, `GreatestHits + Remaster`).
- Each trait carries zero or more `ClassificationEvidence` records with: an `EvidenceSource`
  dimension (the hierarchy), a `ClassificationEvidenceKind`, a confidence (basis points 0–10000),
  and an operator-safe detail string.
- `EvidenceSource` ordering:
  1. `Administrator` (locked manual decision)
  2. `EmbeddedExternalId` (trusted embedded provider type)
  3. `StructuredTags` (Looney ID3 `RELEASETYPE` or equivalent structured field)
  4. `CatalogContext` (existing catalog knowledge about this Release/Artist)
  5. `ReleaseTitle` (text pattern in the Release title)
  6. `DirectoryName` (basename of the inbound directory)
  7. `ParentDirectoryName` (parent directory basename)
- `DirectoryName` and `ParentDirectoryName` are the weakest sources. A trait supported only by
  directory evidence must have confidence below the default destructive threshold and must never
  alone produce `Delete`.

### Novelty

- Each Track is classified against the existing catalog by its `AudioIdentityId` and `MediaAsset`
  byte hash:
  - `ExactDuplicate` — same stored canonical bytes (shared `MediaAsset`).
  - `KnownAudioIdentity` — same underlying `AudioIdentity`, possibly different encoding.
  - `AlternateRecording` — distinct `AudioIdentity` from a known one (live/acoustic/re-record/remix).
  - `NewTrack` — no catalog audio identity match.
  - `Uncertain` — insufficient evidence (unknown fingerprint or missing identity).
- `NoveltyEvaluation` aggregates per-Track results into counts: `exact_duplicate_count`,
  `known_audio_identity_count`, `alternate_recording_count`, `new_track_count`, `uncertain_count`,
  and `total_track_count`.

### Policy

- A `PolicyRuleSet` is versioned (`version: u32`) and named. It contains ordered rules.
- Each `PolicyRule` has: a stable string identifier, a predicate over `ReleaseClassification`,
  `NoveltyEvaluation`, and existing disposition, and a resulting `Disposition`.
- Default built-in rules implement PRD-POLICY-006:
  - `GreatestHits && new_track_count == 0 && alternate_count == 0 && uncertain_count == 0` →
    `Ignore` or `Delete` per operator-configured preference.
  - `GreatestHits && (new_track_count > 0 || alternate_count > 0)` → `Review` or `Accept` per
    operator preference.
  - `GreatestHits && uncertain_count > 0` → `Review`.
  - Corrupt/unreadable media → `Quarantine`.
  - Explicit administrator override → chosen outcome.
- Non-GreatestHits traits (Bootleg, Promo, Live, etc.) remain configurable and receive no
  guessed destructive defaults.
- A policy evaluation returns a `PolicyEvaluation` containing the disposition, matched rule IDs,
  a human-readable evidence summary, and potential destructive impact count.

### Durable decisions and dry-run

- Before any destructive action, a `ReleasePolicyDecision` row is persisted with: Release ID,
  policy rule set version, matched rule identifiers, disposition, evidence summary, source
  fingerprint, actor (`System` or `Administrator { actor }`), timestamp, and a `completed` flag
  (default false). Completion is recorded separately after successful execution.
- `PolicyTestSummary` supports dry-run: counts Releases matched by each rule, existing dispositions,
  representative sample Release IDs, and total potential destructive impact. No decision rows are
  written during dry-run.

### PostgreSQL schema

- `release_classifications` — one row per Release, referencing the Release ID and `PolicyRuleSetId`.
- `release_classification_evidence` — one row per evidence item, referencing the classification.
- `track_novelty` — one row per Track within a classified Release.
- `release_novelty_evaluations` — one row per Release aggregate.
- `policy_rule_sets` — versioned rulesets with JSONB predicate definitions.
- `policy_decisions` — durable decision records.
- `policy_decision_audit` — append-only audit of every decision change.
- `policy_test_runs` — dry-run result snapshots.

### melodee-policy crate

- Owns classification, novelty, and policy evaluation logic as pure, testable functions.
- Depends on `melodee-domain` and `melodee-persistence` for catalog reads.
- Does not depend on filesystem deletion, HTTP, or web frameworks.

## Alternatives considered

### Reuse the legacy single AlbumType enum

Rejected. PRD-CLASS-001 explicitly forbids replacing the legacy single mutually-exclusive type
with another single type. A `BTreeSet<ReleaseTrait>` supports all required multi-valued combinations.

### Auto-merge identical directory names

Rejected. PRD-CLASS-003 and SPEC-INV-008 require that directory/path naming alone never authorize
destructive action. Directory evidence is the weakest tier and carries low confidence.

### Title-only regex for Greatest Hits

Rejected by PRD-POLICY-003. The rule must use catalog novelty analysis. Title text is evidence
tier 5, not a decision key.

### Delete without a durable decision record

Rejected by PRD-POLICY-007 and SPEC-INV-008. Deletion occurs only after a persisted
`ReleasePolicyDecision` row exists.

## Rationale

The schema separates what Melodee believes (classification with evidence) from what Melodee
wants to do (policy disposition), making decisions explainable and auditable. Multi-valued traits
match real-world Releases. The evidence hierarchy ensures the weakest signals cannot override the
strongest. Novelty analysis against AudioIdentity prevents deleting collections with genuinely new
or alternate material. Versioned rulesets and dry-run support let operators test policy changes
before activation.

## Consequences

### Positive

- Classification is explainable: every trait has evidence source, kind, and confidence.
- Redundant Greatest Hits collections can be safely ignored/deleted when all Tracks are
  `ExactDuplicate` or `KnownAudioIdentity`.
- Genuinely new or alternate material is never silently deleted (routes to Review/Accept).
- Directory/path naming alone cannot trigger destructive disposition.
- Durable decision/audit records protect against unsafe deletions.
- Dry-run mode enables policy testing without risk.

### Negative / trade-offs

- The policy engine adds a new crate and crate boundary.
- Classification and novelty must run against the full catalog for accurate novelty assessment,
  which is a read-heavy operation.

### Follow-up work

- Phase 5 implements and validates the schema/classifier/policy/dry-run under this decision.
- Phase 10 (Admin UI) consumes the explainable decision data.
- Phase 11 (migration) preserves legacy classifications through audit records.

## Validation

- Domain unit tests for multi-valued traits, evidence hierarchy ordering, novelty categorization,
  and policy rule evaluation.
- PostgreSQL tests for classification/evidence persistence, novelty evaluation storage, durable
  decision records before destructive state, dry-run results, and audit append-only invariants.
- Twenty acceptance tests covering PRD-CLASS-001–003 and PRD-POLICY-001–008.
- Differential tests against legacy Greatest Hits behavior to prove safe rejection and protected
  new/alternate material.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0118](ADR-0118-use-audioidentity-and-immutable-manifest-v3-semantic-overlays.md)
- [ADR-0119](ADR-0119-use-audit-only-corrections-for-legacy-releases-without-manifest-v3.md)

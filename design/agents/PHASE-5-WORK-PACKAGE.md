# Phase 5 Work Package — Classification, Novelty, and Policy

## Objective

Replace scattered regex-based validation, classification, and deletion behavior with explainable
classification evidence, Track-level novelty analysis, and versioned policy rules that return a
disposition. Implement dry-run capability, durable decision/audit records before destructive
execution, operator overrides, and a safe default policy that can reject redundant Greatest Hits
collections while protecting genuinely new or alternate material.

## Authority

In precedence order:

1. ADR-0100 through ADR-0119 (foundational, storage, identity, media, and Phase 4 decisions).
2. This phase's ADR-0120 (classification, novelty, and policy schema) once accepted.
3. SPEC invariants 001–009 and sections 7, 11, 15–17, 22–25, and 28.
4. PRD-CLASS-001 through PRD-CLASS-003, PRD-POLICY-001 through PRD-POLICY-008, and supporting
   requirements PRD-PRINCIPLE-005/006/008, PRD-FP-001/002/004/005, PRD-CATALOG-001–005,
   PRD-STORAGE-001–004, PRD-INGEST-006–010, PRD-ARCH-006, PRD-SCALE-001/002.
5. `IMPLEMENTATION.md` Phase 5 and `design/agents/DEFINITION-OF-DONE.md`.

## Assumptions fixed before coding

- Phase 4 catalog is authoritative: Releases, Tracks, AudioIdentities, AudioIdentityEvidence,
  AudioIdentityMatchDecisions, MediaAssets, and byte hashes/fingerprints are durable and immutable.
  Phase 5 consumes them as evidence; it never reopens canonical or source media.
- `ReleaseTrait` is the multi-valued classification vocabulary (already defined in
  `melodee-domain/src/release.rs`). Phase 5 adds classification evidence, confidence, and the
  evidence hierarchy, plus novelty categories and policy rules/dispositions/decisions.
- The Phase 3/4 ingestion state machine and `Disposition` enum already exist in
  `melodee-domain/src/ingestion.rs`. Phase 5 produces a `Disposition` through policy evaluation and
  persists a durable `ReleasePolicyDecision` before any destructive action.
- Directory/path naming is weak, low-hierarchy evidence. It may contribute to a trait hypothesis
  with low confidence but can never alone authorize destructive disposition.
- Destructive actions (Delete) require: (a) a durable decision/audit row, (b) an explicit operator or
  policy authorization, (c) source fingerprint, and (d) are recorded as completed separately.
- Phase 5 introduces a new `melodee-policy` application crate. It owns classification, novelty, and
  disposition rule logic. It does not own filesystem deletion implementation — that remains a
  separate destructive-action boundary with its own safety contract.

## Allowed scope

- Framework-independent domain types in `melodee-domain` for classification evidence/hierarchy,
  Track novelty categories/evidence/aggregate evaluation, versioned policy rules/rule sets, durable
  policy decisions, and policy test/dry-run results.
- Additive PostgreSQL migration `0008_phase5_policy.sql` and SQLx repositories for classification
  evidence, novelty evaluation, policy rule sets, durable decisions, and dry-run test results.
- `melodee-policy` crate implementing: a classification engine (evidence collection + multi-valued
  traits), a novelty analysis engine (Track novelty categories against catalog), a policy engine
  (versioned rules -> disposition), and dry-run/test-against-catalog support.
- CLI `Policy` subcommands for test/dry-run, decision inspection, and operator override.
- Twenty acceptance tests covering all Phase 5 PRD requirements.
- Traceability updates, evidence, and affected documentation.

## Forbidden scope

- Filesystem deletion execution. Phase 5 authorizes `Delete` as a disposition and records the
  durable decision/audit; actual file removal is a separate destructive-action boundary.
- Tantivy schema, search, or MQL (Phase 6).
- Native Melodee API v1, OpenSubsonic, Jellyfin, authentication, streaming, UI, or other
  later-phase product implementation (Phase 7+).
- Production migration/cutover, user/application-state migration, or library relocation (Phase 11).
- Any broker, Redis, Elasticsearch/OpenSearch, GraphQL, microservices, or C# service translation.

## Implementation slices

### 5.1 Domain vocabulary and invariants

- New ID types: `ReleaseClassificationId`, `TrackNoveltyId`, `PolicyRuleSetId`,
  `PolicyRuleId`, `ReleasePolicyDecisionId`.
- `EvidenceSource` — evidence hierarchy dimension (administrator, embedded external ID/metadata,
  structured tags, catalog analysis, release title, directory name, parent directory name).
- `ClassificationEvidence` — one evidence record (trait, source, dimension, confidence, detail).
- `ReleaseClassification` — traits (multi-valued `BTreeSet<ReleaseTrait>`), evidence list, overall
  confidence.
- `TrackNoveltyCategory` — `ExactDuplicate`, `KnownAudioIdentity`, `AlternateRecording`, `NewTrack`,
  `Uncertain`.
- `TrackNovelty` — per-Track novelty assessment (category, evidence, confidence).
- `NoveltyEvidenceKind` — evidence dimensions for novelty (byte hash, fingerprint, catalog context).
- `NoveltyEvaluation` — aggregate counts per category for a Release.
- `PolicyRuleSet` — versioned ruleset with default-rule identifiers.
- `PolicyEvaluation` — evaluation result (disposition, matched rules, evidence summary, impact).
- `ReleasePolicyDecision` — durable decision record (policy version, disposition, rule ID/version,
  evidence summary, source fingerprint, actor, timestamp, completion state).
- `PolicyTestSummary` — dry-run aggregate (matched count, existing dispositions, samples, destructive
  impact).

### 5.2 Database schema and persistence

- Migration creates: `release_classifications`, `release_classification_evidence`,
  `track_novelty`, `release_novelty_evaluations`, `policy_rule_sets`, `policy_decisions`,
  `policy_test_runs`, and an append-only `policy_decision_audit` table.
- SQLx repositories for classification storage, novelty evaluation persistence, policy rule
  management, decision recording, and dry-run query support.

### 5.3 Classification engine

- Collect evidence from manifest observations, embedded metadata, and catalog context.
- Produce multi-valued `ReleaseClassification` with explainable evidence and confidence.
- Evidence hierarchy: directory/path evidence is weakest; administrator decisions are strongest.

### 5.4 Novelty analysis engine

- For each Track, classify against existing catalog audio identities and byte hashes.
- Produce `NoveltyEvaluation` aggregating counts per `TrackNoveltyCategory`.
- Uncertainty never auto-deletes.

### 5.5 Policy engine

- Evaluate `PolicyRuleSet` rules against `ReleaseClassification`, `NoveltyEvaluation`, and existing
  dispositions.
- Return `Disposition` with matched rule identifiers and evidence summary.
- Default rules implement PRD-POLICY-006 logic (GreatestHits + novelty counts).
- Dry-run mode evaluates without persisting decisions or triggering destructive actions.

### 5.6 CLI integration

- `melodee policy test` — dry-run against catalog, prints summary.
- `melodee policy evaluate <release-id>` — evaluate policy for one Release.
- `melodee policy decide <release-id> <accept|review|ignore|quarantine|delete>` — operator override.

## Exit criteria

- Redundant Greatest Hits collections can be rejected/ignored safely when all Tracks are
  `ExactDuplicate` or `KnownAudioIdentity` and none are `NewTrack`/`AlternateRecording`/`Uncertain`.
- A collection with genuinely new or alternate material is never silently deleted; it routes to
  `Review` or `Accept`.
- Directory-name/path evidence alone cannot trigger destructive disposition; it can only contribute
  low-confidence trait hypotheses.
- Policy decisions are explainable: matched rule IDs, evidence summary, and disposition provenance are
  persisted and inspectable.
- A durable decision/audit record exists before any destructive action.
- Dry-run/test mode shows matched counts, existing dispositions, representative samples, and
  potential destructive impact without applying changes.

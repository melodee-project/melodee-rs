# ADR-0196: Expose duplicate candidate scans and Release identity correction in the Admin API

- **Date:** 2026-11-14
- **Status:** Accepted
- **Decision owners:** Catalog curation, Admin API
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-ADMIN-002, PRD-ARTIST-010, PRD-ARTIST-011,
  PRD-RESOLVE-002, PRD-RESOLVE-003, PRD-CATALOG-001

## Context

Operators can already resolve observed Artist strings, merge/split Artists, and review a
Release's catalog comparison through `/api/admin/v1`. The durable catalog also has Release
merge and reversible split operations (`merge_release`, `split_release_merge`), trusted Release
provider evidence (`release_external_identities`), and a non-authoritative `Proposed` identity
outcome.

That capability was not reachable by an external curation harness:

- no endpoint returned duplicate Artist or Release **candidates** with evidence and blockers, so
  an agent had to page the whole catalog and invent its own scoring;
- Release merge/split had no HTTP surface, even though the store operations were audited and
  tested;
- Release detail did not expose provider evidence, the merge tombstone target, or merge history;
- the Admin API could only execute a locked confirmation, not record an evidence-backed
  **proposal** for human review;
- automated callers had no way to state which harness acted.

AI harnesses and coding agents need the same conservative evidence model as the resolver. Giving
them direct identity writes or unstated heuristics would violate the catalogue's identity rules:
names are not identity keys (PRD-ARTIST-001), provider candidates are evidence only
(PRD-ARTIST-012), and similarity must never automatically merge identities (PRD-ARTIST-011,
ADR-0174).

## Decision

1. Add read-only Admin API candidate scans:
   - `GET /api/admin/v1/catalog/duplicates/artists`
   - `GET /api/admin/v1/catalog/duplicates/releases`
   Each is bounded and paginated, sorted by an advisory score, and returns the two entities,
   the evidence facts with weights, and explicit blockers. Scans are set-based SQL with a
   server-side execution budget; they never write identity rows and never authorize a merge.
   Name-only pairs always carry a `name_only` blocker. Conflicting trusted provider identifiers,
   disambiguation/type conflicts, and Release date or track-count mismatches are blockers, not
   silent vetoes. Release evidence includes trusted provider identifiers, shared audio identities,
   and shared normalized Track titles with near-equal durations so a duplicate with independently
   derived audio identities remains visible.
2. Add audited Release identity correction endpoints:
   - `POST /api/admin/v1/catalog/releases/merge/preview`
   - `POST /api/admin/v1/catalog/releases/merge`
   - `POST /api/admin/v1/catalog/releases/{id}/split`
   They wrap the existing durable store operations. Merge requires a non-blank reason and the
   administrator's public id as the audit actor; the source becomes a redirect tombstone and all
   Tracks, observations, contributions, media, and audit evidence are retained. Split reverses
   exactly one recorded merge event using its immutable Track-ordinal map.
3. Extend Release detail with `externalIdentities` (trusted/untrusted provider evidence),
   `mergeEvents` (auditable merge history and the exact event id needed for split), and
   `mergedIntoReleaseId` (the surviving Release for a tombstone).
4. Add a proposal endpoint:
   `POST /api/admin/v1/identity/observations/{id}/propose`.
   It records immutable evidence rows and a non-locked `Proposed` `ArtistMatchDecision` with
   automated authority. It does not advance the authoritative decision pointer or change any
   Release/Track credit; an administrator still confirms, creates a new Artist, or rejects the
   candidate. Proposals are rejected once an authoritative decision exists.
5. Accept an optional `X-Melodee-Agent` header as advisory attribution. It is trimmed, bounded,
   stripped of control characters, recorded in tracing and automated proposal authority, and
   never used for authorization. The administrator JWT remains the only authorization gate.
6. Do not add or change any native-v1, OpenSubsonic, or Jellyfin route. These are Rust-native
   Admin API surfaces under `/api/admin/v1` only.

## Alternatives considered

### Let each harness page the catalog and score duplicates externally

Rejected: it pushes catalog-scale scans, unstable heuristics, and repeated candidate logic into
every harness; it also makes the evidence inconsistent between operators and agents.

### Allow the candidate scan to merge on a high score

Rejected: this contradicts PRD-ARTIST-011 and ADR-0174. A score is advisory evidence; execution
stays an explicit, audited operation with a reason.

### Expose raw SQL or the database to agents

Rejected: it bypasses the administrator gate, audit trail, and bounded request model.

### Reuse the existing `confirm` resolution for agent proposals

Rejected: `confirm` is a locked authoritative decision. An agent must be able to contribute
evidence without claiming authority; the separate `Proposed` path preserves that distinction.

## Rationale

This is the smallest surface that makes agent-assisted curation real without changing the
catalog's authority model. Candidates and proposals are additive and read-mostly; Release
merge/split reuse already-tested durable operations; every mutation remains administrator-gated
and audited; and the identity invariants stay in one place rather than being reimplemented by
each harness.

## Consequences

### Positive

- Operators and AI harnesses share one evidence model, one candidate ranking, and one audit trail.
- Release corrections can be previewed, applied, and reversed through the API.
- An agent can contribute proposals without any ability to change identity authority.
- No compatibility contract is touched; native clients see no change.

### Negative / trade-offs

- Candidate scores are advisory heuristics and need tuning against real catalogues; the
  evidence/blocker vocabulary is now a semi-public Admin API contract.
- The scan's SQL joins are more expensive than a single browse query. Limits, the 10-second
  execution budget, and supporting indexes bound the cost.
- Split requires the exact merge event id; callers must read `mergeEvents` first.

### Follow-up work

- Add an Admin API OpenAPI/JSON Schema document and, if justified, an MCP tool adapter.
- Consider a persistent service-account capability that can read and propose but not execute
  merges; the current implementation still uses an administrator JWT.
- Tune evidence weights and add shared-release/co-credit signals once observed against the
  known catalogue corpus.

## Validation

- `apps/melodee-server/tests/admin_duplicates_api.rs` covers the administrator gate, candidate
  evidence and blockers, score filtering and paging, Release preview/merge/split over real
  PostgreSQL, tombstone detail fields, agent proposal visibility, non-authoritative authority,
  and rejection after confirmation.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` and the repository
  prechecks must pass.
- The public Admin API reference documents every new route and its advisory semantics.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0174](ADR-0174-require-review-for-name-only-artist-identity-matches.md)
- [ADR-0162](ADR-0162-durable-provider-enrichment-tasks-and-release-external-evidence.md)
- [Admin API reference](../../docs/src/content/docs/api/admin-api.md)

# ADR-0133: Phase 9 party mode and jukebox parity waiver

- **Date:** 2026-08-13
- **Status:** Accepted
- **Decision owners:** Phase 9 work package (delegated product decision; see Context)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-PARTY-001, PRD-PARTY-002, PRD-PARTY-003, PRD-PARTY-004, PRD-USER-004

## Context

The frozen legacy baseline (commit `4028b58d…`) does **not compile** the Party Mode or Queue
controllers: 35 routes (32 Party, 3 Queue) return empty 404 at runtime, captured as promoted
negative goldens under `compatibility/melodee-api-v1/golden/native-excluded/` (coverage-matrix
discrepancy D6). OpenSubsonic `jukeboxControl` is an intentional captured 410, and legacy C#
ADR-0001 records that jukebox was never implemented. ADR-0123 fixed Phase 7/9 scope: Phase 7
preserved the observable absence exactly, and reactivating Party Mode requires a superseding ADR
plus a client compatibility plan.

Phase 9's exit gate (PRD §56) is "the feature-parity matrix has no unapproved gaps" and explicitly
permits "an explicit approved ADR/release waiver" for a gap. Parity means reproducing what clients
can observe at the frozen baseline; no shipping client (Android 1.8.0, MeloAmp, Feishin, any
OpenSubsonic client) can depend on routes that return 404/410 at the baseline.

The Phase 9 work assignment (2026-08-13) delegated the waiver-or-build product decision to the
executing agent with a documented recommendation for the waiver path
(`design/agents/PHASE-9-PLAN.md` §3).

## Decision

Phase 9 **waives** Party Mode and jukebox implementation as parity obligations:

1. The frozen native 404 absence for the 35 Party/Queue routes and the OpenSubsonic
   `jukeboxControl` 410 remain the correct, tested behavior. The pinned negative test matrices
   (`native_catalog_api.rs` excluded-route cases, `opensubsonic/negative.rs`) stay green and
   unchanged.
2. PRD-PARTY-001, PRD-PARTY-002, and PRD-USER-004 are satisfied by **preserved intentional
   absence** with this ADR as the approved waiver. PRD-PARTY-003 (playback backend management) and
   PRD-PARTY-004 (historical ADR review) remain separately satisfied by Phase 7/8 evidence.
3. Server-side playback backend/endpoint management implemented in Phase 7
   (`native/endpoints.rs`, `playback-backend/*`) is unaffected; it is a distinct, shipped
   capability.
4. Future Party Mode is a **post-parity feature**, not a compatibility obligation. It requires its
   own ADR chain: product decision, schema ADR, sessions/queue/moderation design, conditional
   `jukeboxControl` design, and a client compatibility plan, per the ADR-0123 reactivation clause.

## Alternatives considered

### Build Party Mode and jukebox in Phase 9

Benefits: closes the largest "not implemented" domain; legacy ADR-0007 shows Party Mode was once
aspirational. Costs: it is a *new feature*, not parity — the baseline exposes nothing to be
compatible with; it would be the largest work package in the phase (sessions, collaborative queues,
moderation, backend orchestration, schema, client plan) with zero differential evidence to
validate against; and it would delay every other Phase 9 domain. Rejected because it confuses
parity with feature growth and has no captured contract to satisfy.

### Leave the routes unpinned

Benefits: none. Costs: losing the 35-case negative test matrix would let the frozen absence drift
silently. Rejected; the absence is a tested contract.

## Rationale

Parity is defined against observable baseline behavior (PRD §6). The baseline's observable Party
Mode and jukebox behavior is "absent." Preserving that absence with pinned tests plus an approved
waiver satisfies Gate G at the lowest risk and keeps the option to build the feature later without
any migration burden (no client can have state against routes that never existed).

## Consequences

### Positive

- Phase 9 exit gate is achievable without speculative feature work.
- The negative contract remains executable and cannot drift.
- PRD-PARTY-001/002 and PRD-USER-004 close with accepted evidence.

### Negative / trade-offs

- Users who read legacy ADR-0007 and expected Party Mode must wait for a post-parity feature
  effort; this is recorded honestly in the parity matrix and Phase 9 report.

### Follow-up work

- None required for parity. Any future Party Mode proposal starts a new ADR chain.

## Validation

- `apps/melodee-server/tests/native_catalog_api.rs` — 35-case excluded-route matrix stays green.
- `apps/melodee-server/src/opensubsonic/negative.rs` and Phase 8 differential replay — intentional
  410/501 matrix stays green.
- `design/requirements/FEATURE-PARITY-MATRIX.md` — Party Mode and Jukebox rows reference this ADR
  as the approved waiver.

## References

- [PRD.md](../../PRD.md) §34, §56
- [ADR-0123](ADR-0123-phase-7-scope-overlap-handling-for-party-requests-podcasts-shares-analytics.md)
- [Legacy ADR-0001 / ADR-0007](legacy/README.md)
- `compatibility/melodee-api-v1/golden/native-excluded/`
- `design/agents/PHASE-9-PLAN.md` §3

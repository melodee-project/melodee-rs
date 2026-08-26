# ADR-0140: Retire the legacy /jukebox web route

- **Date:** 2026-08-14
- **Status:** Accepted
- **Decision owners:** Phase 10
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-WEB-001, PRD-WEB-002, PRD-PARTY-001 (waived), PRD-PARTY-002 (waived)

## Context

The Phase 9 Blazor page inventory (historical artifact removed, 63 pages /
68 routes from the frozen legacy baseline) classified the legacy `/jukebox` page as a DROP
candidate: jukebox capability was never implemented in the legacy server (legacy ADR-0001), the
OpenSubsonic jukebox endpoints are an intentional frozen 410, and ADR-0133 waived party/jukebox
parity. The inventory rule requires each DROP to have its own accepted ADR. The Rust server has
never served `/jukebox`; this ADR records the retirement decision so the classification becomes
final.

## Decision

The legacy Blazor `/jukebox` route is **retired** in the rewrite: no replacement page is built,
no redirect is added, and the route remains absent from the Rust server. The frozen OpenSubsonic
`jukeboxControl` 410 behavior (ADR-0133, `opensubsonic/negative.rs`) is unchanged.

## Alternatives considered

### Rebuild a jukebox page

The capability itself is waived (ADR-0133); building a UI for a nonexistent backend would be a
fake affordance and a new feature, not parity.

### Keep the route as a stub

A stub page would advertise a capability that does not exist; honest absence is preferable.

## Rationale

The page has no working backend in the legacy baseline, no client can depend on it, and the
capability decision is already made. The cheapest honest disposition is retirement.

## Consequences

### Positive

- Web parity classification becomes final for this route (PRD-WEB-002).

### Negative / trade-offs

- If party/jukebox is ever reactivated (post-parity feature), a new UI will be designed under its
  own ADR chain.

### Follow-up work

- None.

## Validation

- `grep` evidence: no `/jukebox` route exists in the Rust server or admin SPA.
- Phase 9 checklist 11 and the feature-parity matrix reference this ADR for the DROP row.

## References

- [ADR-0133](ADR-0133-phase-9-party-mode-and-jukebox-parity-waiver.md)
- Blazor page inventory (historical artifact removed)
- [PRD.md](../../PRD.md) §43

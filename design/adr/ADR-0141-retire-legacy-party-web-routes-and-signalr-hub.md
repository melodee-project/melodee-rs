# ADR-0141: Retire the legacy /party web routes and SignalR hub

- **Date:** 2026-08-14
- **Status:** Accepted
- **Decision owners:** Phase 10
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-WEB-001, PRD-WEB-002, PRD-PARTY-001..004 (waived), PRD-USER-004
  (waived)

## Context

The Phase 9 Blazor page inventory classified `/party` and `/party/{SessionApiKey}` as DROP
candidates: party-mode capability is waived by ADR-0133 (the frozen baseline does not compile the
party controllers; the native routes are pinned as intentional 404 absence). Retiring
`/party/{SessionApiKey}` also retires the legacy server's only SignalR hub (`/party-hub`) and the
`PartyHub`/`PartyModeService` components. The inventory rule requires an accepted ADR per DROP.
The two routes are one feature surface and are covered by this single ADR.

## Decision

The legacy Blazor routes `/party` and `/party/{SessionApiKey}` are **retired** in the rewrite,
together with the `/party-hub` SignalR hub and the party UI components. No replacement is built
and no redirect is added; the Rust server never served these routes. The frozen native-API party
absence (pinned 404 goldens, ADR-0123/ADR-0133) is unchanged.

## Alternatives considered

### Rebuild party mode

Waived by ADR-0133; reactivation is a post-parity feature requiring a superseding ADR chain,
schema ADR, and client plan — not a web-parity preservation task.

### Preserve the SignalR hub for future use

A hub with no supported capability is attack surface and maintenance cost with zero consumers.

## Rationale

Honest absence matches the accepted capability waiver; retiring the hub removes the last SignalR
dependency from the product surface.

## Consequences

### Positive

- Web parity classification becomes final for both routes (PRD-WEB-002).
- No SignalR-equivalent realtime infrastructure is needed in the Rust deployment shape
  (consistent with ADR-0111's no-broker posture).

### Negative / trade-offs

- Future party-mode reactivation must rebuild both API and UI under its own ADRs.

### Follow-up work

- None for Phase 10/11.

## Validation

- `grep` evidence: no `/party*` routes or SignalR-equivalent hubs exist in the Rust server or
  admin SPA; frozen `native-excluded` goldens remain green.

## References

- [ADR-0133](ADR-0133-phase-9-party-mode-and-jukebox-parity-waiver.md)
- [ADR-0123](ADR-0123-phase-7-scope-overlap-handling-for-party-requests-podcasts-shares-analytics.md)
- [Blazor page inventory](../evidence/phase-9/blazor-page-inventory.md)
- [PRD.md](../../PRD.md) §43

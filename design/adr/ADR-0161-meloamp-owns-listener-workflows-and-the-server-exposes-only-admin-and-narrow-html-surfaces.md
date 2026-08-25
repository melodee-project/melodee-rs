# ADR-0161: MeloAmp owns listener workflows and the server exposes only admin and narrow HTML surfaces

- **Date:** 2026-08-25
- **Status:** Accepted
- **Decision owners:** Product owner, Feature Completion Program Phase 2
- **Supersedes:** [ADR-0155](ADR-0155-end-user-web-ui-surface-and-architecture.md)
- **Superseded by:** None
- **Related requirements:** PRD-WEB-001, PRD-WEB-002, PRD-UI-002, PRD-TEST-008,
  PRD-API-ADMIN-003

## Context

The frozen C# application includes a listener-facing Blazor application, while the Rust rewrite
already has a complete Carbon Admin UI at `/admin`. MeloAmp is the maintained listener/curator
client and already owns browsing, search, playback, playlists, ratings, recommendations, and
related user workflows. Maintaining a second listener SPA in the server would duplicate product
surface, client release work, and compatibility testing without improving the server contract.

ADR-0155 accepted the opposite product direction before MeloAmp ownership was made explicit. Its
historical body remains useful evidence, but its route obligation is no longer current authority.

## Decision

1. MeloAmp owns listener browsing, search, playback, playlists, ratings, recommendations, and
   related user workflows. Native-v1 remains the server contract consumed by MeloAmp.
2. The Rust server owns APIs, ingestion, PostgreSQL-backed storage, streaming, operational
   behavior, and the full Admin UI served at `/admin`.
3. A full `web-ui/` SPA at `/` is not a v1 server obligation. The server must not add a duplicate
   listener application or duplicate listener screens.
4. Narrow browser-only HTML pages may exist only where a public link or security flow genuinely
   requires HTML, such as an anonymous public-share landing or password-reset completion. They
   are bounded pages, not an application shell.
5. MeloAmp compatibility and acceptance journeys replace per-screen proof for the removed server
   listener SPA. Existing native-v1, OpenSubsonic, and Jellyfin contracts remain independent.

## Alternatives considered

### Build the duplicate server listener SPA

Rejected: MeloAmp already owns the listener experience. A second SPA would duplicate routes,
state, localization, accessibility, and client acceptance burden.

### Serve the legacy Blazor application

Rejected: it contradicts the ground-up Rust rewrite and would add a second server runtime.

## Rationale

This preserves a single listener product, keeps the Rust server focused on durable APIs and
operations, and retains the already-shipped Admin UI without changing any wire compatibility
surface. It is an explicit approved relocation of WEBUI-001 to MeloAmp, not an unrecorded absence.

## Consequences

### Positive

- Removes the duplicate SPA obligation and its active backlog blocker.
- MeloAmp is the listener application; `/admin` is the server UI.

### Negative / trade-offs

- Narrow HTML flows still need their own bounded security and compatibility tests if introduced.

### Follow-up work

- Remove stale `web-ui/` instructions from active plans and record WEBUI-001 as
  `DIFFERENT - APPROVED` with MeloAmp as the replacement surface.

## Validation

Repository validation must find no active instruction to create `web-ui/`. The parity matrix and
requirement traceability must identify MeloAmp as the approved replacement, and MeloAmp native-v1
smoke/acceptance remains the compatibility evidence for listener journeys. No server listener SPA
build is expected.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0155](ADR-0155-end-user-web-ui-surface-and-architecture.md) (historical superseded decision)
- MeloAmp repository (read-only client reference)

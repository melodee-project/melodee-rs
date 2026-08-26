# ADR-0155: End-user web UI product surface and architecture

- **Date:** 2026-08-24
- **Status:** Superseded
- **Decision owners:** Feature Completion Program Phase 0 (explicitly user-assigned)
- **Supersedes:** None
- **Superseded by:** [ADR-0161](ADR-0161-meloamp-owns-listener-workflows-and-the-server-exposes-only-admin-and-narrow-html-surfaces.md)
- **Related requirements:** PRD-WEB-001, PRD-WEB-002, PRD-UI-002, PRD-TEST-008, PRD-API-ADMIN-003,
  PRD-AUTH-003, PRD-STREAM-001, PRD-PARTY-003, PRD-USER-001

## Context

The C# Melodee Blazor app is a full **end-user** web UI, not just an admin console. Its route
inventory (frozen oracle `4028b58d…`) includes user surfaces the Rust server does not serve:
`/` (dashboard), `/search`, `/musicplayer`, `/stats`, `/activity/nowplaying`, `/share/{id}` (public
share landing), `/about`, `/user/{id}`, plus account flows `/login`, `/account/register`,
`/account/profile`, `/account/reset-password`, `/account/forgot-password`, onboarding
`/onboarding`, and feature pages `/party`, `/party/{session}`, `/jukebox`, `/requests`, `/charts`.
Party collaboration is driven by a SignalR `PartyHub` (`src/Melodee.Blazor/Hubs/PartyHub.cs`).

Rust currently serves only the admin SPA at `/admin` (ADR-0138/0139). The user-assigned product
direction makes the end-user web UI a **first-class v1 surface** (WEBUI-001 is an implementation
obligation, not an accepted absence). The existing React + TypeScript + Carbon + Vite + i18n +
theming + RTL + static-serving foundations (ADR-0109/0138) must be reused where appropriate.
The native-v1, OpenSubsonic, and Jellyfin wire DTOs must **not** be merged or casually redesigned
(ADR-0102; AGENTS.md compatibility rules).

## Decision

1. **Surface and route ownership.** A second SPA, `web-ui/`, served at `/` (site root) by the same
   Rust server (reusing ADR-0138's `tower-http` `fs` static-serving pattern). Route ownership:
   - End-user SPA owns `/` and all non-`/admin` public and authenticated routes (dashboard,
     search/MQL, music player, stats, now-playing, public share landing `/share/{id}`, about,
     user profile, account register/login/reset/forgot, onboarding, party/jukebox/requests/charts
     where preserved).
   - Admin SPA continues to own `/admin` exclusively.
   - The two SPAs are separate Vite projects (separate builds, separate `pnpm` workspaces) sharing
     a common component/theme/i18n package. They are not one app with two prefixes.
2. **Session/auth.** The end-user SPA authenticates through the existing native-v1 cookie
   sign-in/refresh/sign-out flow (ADR-0139 pattern, `native/auth.rs`) with same-origin deployment.
   No Bearer tokens in web storage. Account registration/profile/reset reuse the existing native-v1
   account endpoints (PRD-AUTH-003). Route guards are UX-only; the server enforces authorization
   per SPEC §20.
3. **Public share landing.** `/share/{id}` is served as an anonymous, authenticated-by-share-token
   page (no account required to view a share). It consumes the existing native share endpoints and
   does not require an admin principal.
4. **API consumption.** The SPA consumes **native-v1** endpoints only (search, catalog, streaming
   URLs, playlists, charts, party, requests, shares, auth, now-playing). It never calls
   OpenSubsonic or Jellyfin wire surfaces directly, and no new shared wire DTO is introduced across
   the three adapters (ADR-0102).
5. **Realtime.** Party/jukebox collaboration uses a **SSE** (Server-Sent Events) or
   WebSocket-compatible push channel exposed as a native-v1 route; the legacy SignalR `PartyHub`
   semantics (presence, queue, now-playing) are preserved over that channel. (SignalR-specific
   transport is replaced; the user-visible collaboration behavior is preserved.)
6. **Player.** The music player uses native streaming endpoints (PRD-STREAM-001..003) with
   Range/seek semantics already captured; no new streaming contract.
7. **Non-goals / preserved absences.** No admin functionality is duplicated into the end-user SPA.
   Party Mode/jukebox rows waived under ADR-0133 remain waived unless that ADR is revisited; this
   ADR preserves the *web routes* for surfaces the server actually implements.
8. **Accessibility / responsive / i18n / theming / RTL** follow the admin UI standards
   (PRD-UI-REF, ADR-0137/0138): Carbon + Melodee shared components, all visible text localized,
   light/dark/system/custom themes, RTL, keyboard-accessible, responsive breakpoints, visual
   regression.

## Alternatives considered

### One unified SPA for admin + end-user

Rejected: different trust boundaries (admin principal vs. end-user/share-token auth), different
release cadence, and the admin SPA is already a shipped, gated surface (ADR-0139). Two SPAs sharing
a common package keep each surface independently testable.

### Serve the Blazor app as-is

Rejected: contradicts the ground-up rewrite and the React/Carbon decisions (ADR-0109); Blazor
Server has its own runtime and would add a second server process.

### End-user UI on OpenSubsonic/Jellyfin wire

Rejected: the native-v1 API is the first-party contract with full feature coverage (charts, party,
shares, onboarding); the compat adapters are for third-party clients and lack several user
workflows.

## Rationale

A separate end-user SPA on the native-v1 API reuses every accepted foundation (tooling, session
strategy, design system, static serving) while preserving the compatibility adapters' independence
and the admin surface's authorization posture. It directly satisfies PRD-WEB-001/002 by preserving
currently-implemented web capabilities in a modern surface with an explicit, ADR-governed route map.

## Consequences

### Positive

- Closes the parity-matrix end-user-web-UI row with a concrete, ADR-governed surface and route map.
- Reuses the entire accepted toolchain; no new runtime dependency (SSE is a native-v1 route).
- Share landing and account flows become real v1 capabilities rather than silent absences.

### Negative / trade-offs

- Two SPAs double the JS supply-chain and e2e surface; shared-package discipline is required to
  avoid divergence.
- A native push channel (SSE/WebSocket) is new server surface; it must be frozen-capture-diffed
  for any overlap with existing party/jukebox routes.

### Follow-up work

- Feature Completion Phase 3 (end-user web UI): scaffold `web-ui/`, shared package, auth wiring,
  per-route implementation + Playwright/visual suites, and the SSE push channel. Each preserved
  route needs a capture or legacy-source behavioral document.

## Validation

- Server test: `GET /` serves the end-user SPA and deep links return its `index.html`; `/admin`
  and all API routes are unaffected (route-shadowing test, ADR-0138 pattern).
- SPA e2e (Playwright): login → authenticated navigation; anonymous `/share/{id}` renders;
  expired session → redirect to login; non-admin cannot reach `/admin`.
- Native-v1 golden diff: no existing native route/DTO/status changes.
- Accessibility/visual-regression suites per PRD-TEST-008.

## References

- [PRD.md](../../PRD.md) §43 (PRD-WEB-001/002), §38 (PRD-UI-002), §21 (PRD-API-M1-006),
  §22 (PRD-STREAM-001), §25 (PRD-API-ADMIN-003), §26 (PRD-AUTH-003), §27 (PRD-USER-001),
  §34 (PRD-PARTY-003)
- [ADR-0138](ADR-0138-admin-spa-tooling-and-static-serving.md), [ADR-0139](ADR-0139-admin-spa-session-strategy-and-admin-api-gating.md)
- [ADR-0109](ADR-0109-use-carbon-for-admin-ui.md), [ADR-0137](ADR-0137-localization-stack-and-rtl-preservation.md)
- [ADR-0102](ADR-0102-preserve-public-apis-as-separate-adapters.md)
- Blazor route inventory (withdrawn evidence report
  `design/evidence/phase-9/blazor-page-inventory.md`)

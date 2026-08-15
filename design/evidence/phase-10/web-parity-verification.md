# Phase 10 — Web parity final verification (PRD-WEB-002)

Date: 2026-08-15. Verifies the preliminary Phase 9 classification
(`design/evidence/phase-9/blazor-page-inventory.md`: 63 pages / 68 routes, KEEP 54 / REPLACE 6 /
DROP-candidate 3) is now final, with every REPLACE landed and every DROP covered by its own
accepted ADR.

## REPLACE items — landed

| Legacy route | Disposition | Phase 10 surface | Evidence |
|---|---|---|---|
| `/musicplayer` | Vestigial standalone route retired; in-browser playback capability preserved as embedded preview | `AudioPreview` shared component: Release Review track breakdown (pre-publication preview endpoint), Track/Release detail (signed stream URLs). Full listening remains with native/OpenSubsonic/Jellyfin clients per `design/ui/README.md` | `admin-ui/src/components/AudioPreview.tsx`; `admin_review.rs` preview endpoint; `admin_catalog.rs` `streamUrl` |
| `/Error` | Stock ASP.NET template replaced | Localized, themed `ErrorState` (notFound / forbidden / unexpected) + `RouteErrorBoundary` inside the app shell | `admin-ui/src/components/ErrorState.tsx`, `RouteErrorBoundary.tsx`; e2e `not-found.spec.ts` |
| `/onboarding` | Redesigned API-first | Onboarding wizard at `/admin/onboarding` over the Phase 9 stores (six preflight checks, remediation, fail-closed completion, bypass detection) | `admin-ui/src/screens/onboarding/OnboardingWizard.tsx`; `admin_onboarding.rs`; PRD-SETUP-001/002 tested |
| `/onboarding/blocking` | Same | Guard states expressed via `GET /onboarding/status` (`completed`/`bypassed`/`canComplete`) | `onboarding.rs` |
| `/admin/scripts` | Redesigned API-first on Boa (ADR-0135) | Settings → Scripts tab: list by event, enable/disable, run history, test-run | `admin-ui/src/screens/settings/`; `admin_scripts.rs` |
| `/admin/scripts/{EventName}` (Monaco/Jint editor) | No Monaco/Jint analog per ADR-0135 | Script source editing via the Settings → Scripts surface over `PUT /scripts/{event}/{name}` | `admin_scripts.rs` |

## DROP items — retired by accepted ADRs

| Legacy route | ADR | Notes |
|---|---|---|
| `/jukebox` | ADR-0140 (Accepted) | Capability waived by ADR-0133; frozen OS jukebox 410 unchanged |
| `/party` | ADR-0141 (Accepted) | Capability waived by ADR-0133; native party routes remain pinned 404 |
| `/party/{SessionApiKey}` | ADR-0141 (Accepted) | Retires the only SignalR hub (`/party-hub`) and party components |

## KEEP items — surfaces verified

The 54 KEEP pages' capabilities are preserved on the rewrite's surfaces: admin/curation pages
(`/admin/*`, `/data/*` grids and details) → the admin SPA's Processing/Review/Identity/Catalog/
Policies/System/Users/Settings screens; account/profile/auth → the frozen native API v1; browsing
and playback → native clients, OpenSubsonic, and Jellyfin adapters (Phases 7–8 accepted);
public share → native share routes; requests/podcasts/radio/charts → Phase 7–9 API surfaces with
admin management under Settings. Legacy service-layer authorization mismatches flagged by the
inventory (`/data/users`, `/data/useredit/*`, `/usergroup/{key}`, `/admin/mediaartists`) are
closed by ADR-0139: every `/api/admin/v1` route is `AdminPrincipal`-gated server-side.

## Conclusion

All REPLACE items have shipped surfaces, all DROP candidates have accepted ADRs, no legacy route
was removed without the documented ADR path. PRD-WEB-002 is satisfied; its status moves to
`tested` with this document as evidence.

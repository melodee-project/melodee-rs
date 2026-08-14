# PRD-REQUEST-002 surface inventory (WP-01, 2026-08-13)

PRD-REQUEST-002: "Requests must be represented in both parity APIs/UI surfaces where currently
exposed."

## Finding: the frozen legacy build exposes requests only on the native v1 surface

Method: searched the three captured route inventories for request/comment/activity endpoints.

- **Native v1** — `compatibility/melodee-api-v1/contracts/route-inventory.json` contains 11
  request routes under `/api/v1/requests*` (`RequestsController`): list/create, activity,
  activity/unread, get/update/delete `{id}`, comments list/create, complete, seen. These are the
  only parity-surface request routes in the frozen build.
- **OpenSubsonic** — `design/agents/PHASE-8-OPENSUBSONIC-ROUTE-MATRIX.json` (81 routes) contains
  no request/comment/activity endpoint. The only "request" matches are unrelated rationale text
  (credential-free 302 redirects) and the captured community goldens under
  `compatibility/opensubsonic/golden/community/` cover radio, scan, and shares only — no request
  captures exist.
- **Jellyfin** — `design/agents/PHASE-8-JELLYFIN-ROUTE-MATRIX.json` (60 routes) contains no
  request/comment/activity endpoint, and `compatibility/jellyfin/golden/` has no request capture
  directory.
- **Admin UI** — Phase 10 scope (per `AGENTS.md` phase boundary and the Phase 9 plan); the legacy
  Blazor UI requests page (`pages/RequestsPage.tsx` evidence in the route inventory) is a UI
  surface, not a frozen HTTP contract.

Consequence: "both parity APIs/UI surfaces where currently exposed" reduces to the native v1 API
today. No OpenSubsonic or Jellyfin request endpoints may be added without the 4-step
contract-change process (compatibility test, product decision, accepted ADR, migration/client
plan); none is proposed. The native implementation is already differentially evidenced
(`apps/melodee-server/tests/native_community_api.rs` against
`compatibility/melodee-api-v1/golden/native-community/`).

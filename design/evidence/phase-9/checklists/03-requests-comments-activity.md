# Checklist 03 — Requests, comments, activity (WP-01)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Request lifecycle CRUD | PRD-REQUEST-001 | ✅ | `native_community_api.rs`; migration 0012 |
| Comments + activity/unread/seen | PRD-REQUEST-001 | ✅ | `native_community_api.rs` |
| Auto-completion/matching at catalog commit | PRD-REQUEST-001 | ✅ | `native_community_phase7.rs` (atomic ID-backed completion) |
| Requests on all surfaces where legacy exposes them | PRD-REQUEST-002 | ✅ | `design/evidence/phase-9/request-002-surface-inventory.md`: frozen build exposes requests ONLY on the native v1 surface (11 `/api/v1/requests*` routes in `route-inventory.json`; zero request/comment/activity routes in the 81-route OS and 60-route JF matrices and no request goldens on either adapter). "Where currently exposed" = native-only today; admin UI = Phase 10. No OS/JF request endpoints added (frozen contracts unchanged). |

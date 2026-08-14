# Checklist 07 — Internet radio (WP-06)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| OpenSubsonic radio CRUD (admin-gated) | PRD-RADIO-001/002, PRD-API-OS-004 | ✅ | Phase 8 `os-radio-*` differential evidence |
| Admin-API radio list/read (any authenticated user) | PRD-RADIO-001 | ✅ | `apps/melodee-server/src/admin_radio.rs` (`GET /api/admin/v1/radio/stations[/{id}]`); tests `apps/melodee-server/tests/admin_radio_api.rs` (`station_lifecycle_via_admin_api`, `mutations_are_admin_gated_listing_is_authenticated`). Note: the frozen native API v1 has no radio routes, so per WP-06 scope the surface is the Admin API over the shared store (ADR-0134 §1), not the frozen native adapter |
| Admin-API radio create/update/delete (admin-gated) | PRD-RADIO-002 | ✅ | `crates/melodee-persistence/src/radio.rs` (`RadioStationStore` over `opensubsonic_radio_stations`); gating matches the captured OS gate (`is_admin` or `Administrator` role); tests `admin_radio_api.rs` (401/403, role-admin case) and `crates/melodee-persistence/tests/radio_store.rs` (round-trip, bounds, integrity) |
| Stations survive migration | PRD-RADIO-002, PRD-MIG-003 | ✅ | ID stability: store issues ids once (v7) and never regenerates them; migration 0020 immutability trigger verified by `radio_store.rs::public_identity_is_immutable_at_the_database`; cross-surface visibility (admin ↔ OS same rows/ids) proven by `admin_radio_api.rs::admin_and_opensubsonic_surfaces_share_one_store`. Phase 11 owns production import |

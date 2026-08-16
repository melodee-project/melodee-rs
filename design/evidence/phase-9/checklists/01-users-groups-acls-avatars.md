# Checklist 01 — Users, groups, library ACLs, profiles, avatars (WP-03)

> Correction note (2026-08-16, R1): rows below cited a
> `user_admin_phase9.rs` test file that does not exist. The cited coverage
> lives in `apps/melodee-server/tests/admin_access_api.rs` (HTTP-level group
> CRUD, fail-closed delete, ACL effects, user create/update/disable) and
> `crates/melodee-persistence/tests/user_group_admin.rs` (store level). The
> citations have been corrected in place; the claimed coverage was verified
> against those files.

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Native auth flows (JWT/refresh/cookie/Google/reset/logout) | PRD-AUTH-001/002 | ✅ | `auth_phase7.rs`, `native_auth_api.rs`; Phase 7 report |
| Roles/permissions and account states | PRD-AUTH-003, PRD-AUTH-006 | ✅ | `auth_phase7.rs`, `native_catalog_api.rs` |
| Library ACL enforcement (fail-closed) | PRD-AUTH-004 | ✅ | `native_stream_acl.rs`, `native_catalog_api.rs` ACL cases |
| Secret-safe handling | PRD-AUTH-005, PRD-AUTH-007 | ✅ | AUTH-007 closed by WP-08: export redacts every secret to a fixed marker with a header warning; import names secrets only, never accepts the marker as a value (fail-closed), and records bounded audit evidence — `crates/melodee-persistence/src/settings.rs`, `apps/melodee-server/tests/admin_config_api.rs`, `crates/melodee-persistence/tests/settings.rs` |
| Group CRUD + membership admin API | PRD-AUTH-003/004 | ✅ | `/api/admin/v1/groups…` (`admin_access.rs` over shared `group_admin.rs`); `apps/melodee-server/tests/admin_access_api.rs` (`group_lifecycle_via_http`, `membership_and_binding_changes_drive_acl_enforcement`), `crates/melodee-persistence/tests/user_group_admin.rs` (store level) |
| Admin user management beyond list | PRD-AUTH-003 | ✅ | `POST/PUT /api/admin/v1/users` (create with bcrypt, flags/roles, disable/enable with session revocation); `apps/melodee-server/tests/admin_access_api.rs` (`create_user_round_trips_without_secret_leakage`, `update_user_flags_and_disable_enable`, `password_reset_rotates_credential_and_revokes_sessions`) |
| Avatar read (OS getAvatar) | PRD-API-OS-004 | ✅ | Phase 8 differential replay; WP-03 rewired read path to `native_user_avatars` (shape identical): `opensubsonic_media_api.rs::get_avatar_reads_phase9_avatar_store` |
| Avatar upload/change + `avatarLastChanged` | PRD-USER-001; ADR-0134 §2 | ✅ | Migration `0022_phase9_user_admin.sql` (`native_user_avatars`); native `/api/v1/user/me/avatar` GET/POST/DELETE (magic-byte + pipeline bounded); `native_avatar_api.rs`; `avatarLastChanged` from avatar `updated_at`: `opensubsonic_conditional_api.rs::get_user_avatar_last_changed_reflects_avatar_store` |
| Profile surface (`/user/me`) | PRD-USER-001 | ✅ | `native_auth_api.rs` |
| OS user management stays intentional 501 | PRD-AUTH-003 | 🚫 | WP-13 decision record (no contract change) |

## Device profiles (WP-10) — closest fit; device profiles have no dedicated checklist

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Device profile CRUD + default behavior parity | PRD-DEVICE-001; ADR-0134 §5 | ✅ | Migration `0026_phase9_device_profiles.sql`; `device_profiles.rs` store; admin `/api/admin/v1/device-profiles…` + `…/{id}/default`; `device_profiles.rs` (persistence), `admin_device_profiles_api.rs`; legacy inventory `../device-profiles-legacy-inventory.md` |
| No false transcoding-control claim | PRD-DEVICE-002 | ✅ | Store has no stream-negotiation consumer; module/DTO rustdoc + `docs/.../api/admin-api.md` state metadata-only with an explicit caution |
| Profile-driven stream selection | PRD-DEVICE-003 | 🚫 deferred | Post-parity per ADR-0134 §5; requires its own ADR + API contract (status-overrides note) |

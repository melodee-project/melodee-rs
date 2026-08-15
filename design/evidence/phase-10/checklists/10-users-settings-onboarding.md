# Checklist 10 — Users & Access, Settings, Onboarding, Quarantine/History

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Users list + create/update + password set/reset (admin-gated) | PRD-API-ADMIN-002, PRD-WEB-002 | ✅ | Backend API complete: `apps/melodee-server/tests/admin_access_api.rs` (`list_users_is_paginated_filtered_sorted_and_gated`, `password_reset_rotates_credential_and_revokes_sessions`, `create_user_round_trips_without_secret_leakage`, `update_user_flags_and_disable_enable`); store: `UserAdminStore::list_users`/`set_password` + `UserSort` in `crates/melodee-persistence/src/user_admin.rs`. UI: `admin-ui/src/screens/users/UsersScreen.tsx` (server-paged/sorted/filtered `EntityTable`, create/edit/set-password modals, lock-disable-enable); unit `admin-ui/src/screens/users/UsersScreen.test.tsx`; e2e `admin-ui/e2e/users-access.spec.ts` (exact request bodies, 3 viewports, ar-SA rtl smoke) |
| Groups/members/library bindings UI over Phase 9 admin API | PRD-WEB-002 | ✅ | `admin-ui/src/screens/users/GroupsPanel.tsx` (groups table, create/rename/delete, detail modal with member add/remove and library bind/unbind by id); unit `admin-ui/src/screens/users/UsersScreen.test.tsx` (groups tab); e2e `admin-ui/e2e/users-access.spec.ts`. Note: bindings are bind-by-id because the admin API exposes no library picker endpoint |
| Settings CRUD over 190-key registry; config export/import preview UX | PRD-CONFIG-001, PRD-CONFIG-002 | ✅ | `admin-ui/src/screens/settings/ConfigSection.tsx` (settings browser grouped by key prefix, secret redaction surfaced via `__MELODEE_SECRET_REDACTED__`, config export download, import with dry-run preview of applied/added/changed/unchanged/secrets/unknown/warnings before apply); unit `admin-ui/src/screens/settings/SettingsScreen.test.tsx`; e2e `admin-ui/e2e/settings.spec.ts`. Note: the admin API has no per-key mutation endpoint — the browser is honestly read-only and mutation is import-only (`apps/melodee-server/src/admin_config.rs`) |
| Onboarding wizard over tested onboarding stores (fail-closed, bypass) | PRD-SETUP-001, PRD-SETUP-002 | ✅ | `admin-ui/src/screens/onboarding/OnboardingWizard.tsx` (six preflight checks, ProgressIndicator + per-check remediation, fail-closed completion gating on `canComplete`, 409 re-evaluation surface, completed/bypassed redirect); strict-gated overview banner in `admin-ui/src/screens/overview/OverviewScreen.tsx` (`completed === false && bypassed === false` only); unit `admin-ui/src/screens/onboarding/OnboardingWizard.test.tsx`, `admin-ui/src/screens/overview/OverviewScreen.test.tsx`; e2e `admin-ui/e2e/onboarding.spec.ts` (fail-closed, completion redirect, 409, already-completed redirect, rtl smoke; 3 viewports) |
| Quarantine + decision history views | PRD-INGEST-010, SPEC §20 | ✅ | `admin-ui/src/screens/processing/WorkItemStateTable.tsx` (shared state-pinned work-item table with inspector deep link), `QuarantineScreen.tsx` (`state=quarantined`), `HistoryScreen.tsx` (terminal states cataloged/indexed/ignored/deleted switcher); unit `admin-ui/src/screens/processing/QuarantineHistory.test.tsx`; e2e `admin-ui/e2e/quarantine.spec.ts` (state pin, deep link to inspector, history switcher; 3 viewports) |
| Notification templates + test-send UI; scripts list UI; radio admin UI | PRD-WEB-002 REPLACE items | ✅ | `admin-ui/src/screens/settings/NotificationsSection.tsx` (template list/edit per key+locale, test-send, delivery log), `ScriptsSection.tsx` (scripts list, wired events, create/edit/enable/delete, test-run, run history), `RadioSection.tsx` (stations CRUD/enable-disable); unit `admin-ui/src/screens/settings/SettingsScreen.test.tsx`; e2e `admin-ui/e2e/settings.spec.ts` (3 viewports, rtl smoke) |
| Device profiles admin UI (metadata-only, no stream-selection claim) | PRD-DEVICE-001/002 | ✅ | `admin-ui/src/screens/settings/DeviceProfilesSection.tsx` (profiles list, create/edit/delete, set-default; copy is metadata-only — no stream-selection behavior claimed); unit `admin-ui/src/screens/settings/SettingsScreen.test.tsx`; e2e `admin-ui/e2e/settings.spec.ts` |

## Verification

All admin-ui gates green (2026-08-15): `pnpm check` (tsc) ✓, `pnpm lint` (0 errors, 2
pre-existing fast-refresh warnings) ✓, `pnpm i18n:check` ✓, `pnpm test` (35 files / 195
tests) ✓, `pnpm build` ✓, `pnpm test:e2e` (138 tests across desktop-1600x1200,
tablet-768x1024, mobile-375x812) ✓.

## Deviations and decisions

- Admin-object deletes (group/station/profile/script/template) use Carbon danger Modals
  rather than `DestructiveActionDialog`, because those endpoints do not accept the dialog's
  reason field.
- Library bindings are bind-by-id; no library picker endpoint exists in the admin API.
- Settings mutation is import-only; no per-key mutation endpoint exists.
- UI-SPEC §8 mobile overflow fixes landed in the same change: the onboarding
  ProgressIndicator and checks StructuredList scroll inside their own containers
  (`melodee-onboarding-steps-scroll`, `melodee-onboarding-checks-scroll`), and the
  users/groups tables scroll inside `melodee-table-scroll` (a wider page broke mobile
  hit-testing under Chrome's expanded layout viewport).

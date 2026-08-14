# Checklist 12 — CLI and maintenance (WP-05)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Existing: doctor/migrate/work/ingest/catalog/policy/search/charts | PRD-CLI-001 | ✅ | `apps/melodee-cli`; Phase 2–6 evidence |
| Commands coordinate via PostgreSQL/leases | PRD-CLI-002, PRD-WORK-004 | ✅ | shared WorkItemStore; lease tests |
| User bootstrap/recovery | PRD-CLI-001 | ✅ | `melodee users bootstrap-admin/disable/enable` (env-var password, fail-closed first admin); `crates/melodee-persistence/tests/user_group_admin.rs`; `apps/melodee-cli/tests/users_groups.rs` |
| Group management | PRD-CLI-001, PRD-AUTH-004 | ✅ | `crates/melodee-persistence/src/group_admin.rs` + `melodee groups list/create/delete/add-user/remove-user/bind-library/unbind-library` (delete fails closed with members/bindings); tests in `user_group_admin.rs` + `apps/melodee-cli/tests/users_groups.rs` |
| Config validate/export | PRD-CLI-001, PRD-CONFIG-001/002 | ⬜ | WP-08 |
| Notification test-send | PRD-NOTIF-003 | ⬜ | WP-09 |
| Worker-pool pause/resume with durable history | PRD-WORK-005 | ✅ | Migration 0023 `worker_pool_state` + `worker_pool.rs`; pause gates in podcast/policy/discovery workers; `melodee worker-pools list/pause/resume`; tests: `worker_pool_state.rs`, `apps/melodee-server/tests/worker_pools.rs`, `apps/melodee-cli/tests/worker_pools.rs` |
| Scheduled-task triggers share durable leases | PRD-WORK-001/003 | 🔶 | WP-04 chart scheduling closes the remaining gap |

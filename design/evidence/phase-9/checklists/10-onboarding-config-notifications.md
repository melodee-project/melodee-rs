# Checklist 10 — Onboarding, config import/export, notifications/templates (WP-08, WP-09)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| 190-key legacy configuration inventory | Phase 0 gap; PRD-CONFIG-001 | ⬜ | WP-08: generate from legacy `SettingRegistry.cs` via oracle |
| First-run onboarding preflight + admin account | PRD-SETUP-001 | ⬜ | WP-08: `onboarding_state` (ADR-0134 §3) + CLI/server flow |
| Migration bypass of onboarding | PRD-SETUP-002 | ⬜ | WP-08: completion-state check |
| Settings manageable via CLI; env/file + DB layering | PRD-CONFIG-001/003 | ⬜ | WP-08: `server_settings` + `config` CLI |
| Config import/export with preview/validation + secret handling | PRD-CONFIG-002, PRD-AUTH-007 | ⬜ | WP-08: redacted export, explicit secret warnings on import |
| Notification templates storage/rendering | PRD-NOTIF-001 | ⬜ | WP-09: `notification_templates` (ADR-0134 §4) |
| FOSS delivery, fail-degraded, no secret logging | PRD-NOTIF-002 | ⬜ | WP-09: transport + `notification_deliveries` audit |
| CLI test-send + externalized text | PRD-NOTIF-003 | ⬜ | WP-09 + WP-05 CLI |

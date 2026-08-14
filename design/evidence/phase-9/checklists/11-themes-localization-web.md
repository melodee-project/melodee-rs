# Checklist 11 — Themes, localization, user-facing web parity (WP-14)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Theme CRUD/import/export/rescan/per-user/asset serving | §7 inventory row | ✅ | Phase 7 `themes-*` goldens; `native_discovery_api.rs` |
| Blazor page inventory classified (keep/replace/drop) | PRD-WEB-001 | ✅ | WP-14: `design/evidence/phase-9/blazor-page-inventory.md` — 63 pages / 68 routes inventoried from the frozen oracle (commit `4028b58d`) with per-page auth level and capability; preliminary classification KEEP 54 / REPLACE 6 / DROP-candidate 3 |
| Web capability surface moves documented; ADR before route removal | PRD-WEB-002 | ✅ | WP-14: classification outcome in `blazor-page-inventory.md` (per-page rationale; component follow-ons); ADR-0109 Phase 9 amendment (Carbon accepted, shell validation = Phase 10 entry); no route removed in Phase 9 — each DROP candidate requires its own ADR + the 4-step contract-change process per AGENTS.md |
| Themes/appearance (system/light/dark) UI | PRD-THEME-001..004 | ➡ Phase 10 | Index phase tags; ADR-0109 amendment (shell validation = Phase 10 entry) |
| Localization/i18n + RTL | PRD-I18N-001..006 | ➡ Phase 10 | Index phase tags; confirmed out of Phase 9 exit gate |

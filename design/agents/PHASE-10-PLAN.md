# Phase 10 Plan — Admin UI

Status: **Complete (2026-08-15).** All five waves executed; consolidated evidence and gate
results are in [design/evidence/phase-10/PHASE-10-REPORT.md](../evidence/phase-10/PHASE-10-REPORT.md).
Wave 0 decisions resolved: ADR-0137 (localization stack: i18next + react-i18next), ADR-0138
(tooling: Vite/Vitest/Playwright + embedded static serving at `/admin`), ADR-0139 (cookie
session over native-v1 auth + uniform `AdminPrincipal` gating of diagnostics endpoints);
Wave 4 added ADR-0140/0141 (legacy web-route retirements), ADR-0142 (admin browse indexes),
ADR-0143 (catalog artwork overrides). Metadata-editing scope decided: Phase 10 shipped read-only
catalog detail + curation/identity/policy decisions; full metadata edit forms defer to a
follow-on ADR (see §3). API explorer deferred to Phase 11+.

This document is the review + plan for full Phase 10 completion.

Sources: `IMPLEMENTATION.md` §13, `PRD.md` §25/§38–44/§56 (Phase 10) + §53 planned-ADR table,
`SPEC.md` §5–7/§19.4/§20–23, `ADR-0109`, `design/ui/DESIGN-SYSTEM.md`, `design/ui/MOCKUPS.md`,
`design/ui/reference/` (eight canonical mockups), `design/requirements/REQUIREMENTS-INDEX.md`,
`design/requirements/FEATURE-PARITY-MATRIX.md`, `design/requirements/status-overrides.json`,
`design/evidence/phase-9/PHASE-9-REPORT.md` + `checklists/11-themes-localization-web.md`,
`design/evidence/phase-9/blazor-page-inventory.md`, `docs/AGENTS.md`,
`apps/melodee-server/src/admin*.rs`, `docs/src/content/docs/api/admin-api.md`, and a full workspace
survey (2026-08-14).

---

## 1. Current-state review findings

### 1.1 What Phases 0–9 already closed (do not rebuild)

Phase 10 is a **consumer of established backend surfaces**, not a re-platforming of them:

- **Admin API `/api/admin/v1` groups already implemented and tested** (`apps/melodee-server/src/`):
  ingest work-item diagnostics (counts, bounded listing, detail, events, leases — `admin.rs`),
  users/groups/library-ACL administration (`admin_access.rs`), internet radio CRUD
  (`admin_radio.rs`), device profiles (`admin_device_profiles.rs`), notification
  templates/deliveries/test-send (`admin_notifications.rs`), config export/import with preview +
  redaction (`admin_config.rs`), onboarding status/complete (`admin_onboarding.rs`), script
  registry/runs/test-run (`admin_scripts.rs`). Conventions are settled: stable error envelope
  `{code, message, correlationId}`, `limit`/`offset` with server-side clamping (default 50, max
  500), camelCase DTOs, `AdminPrincipal` Bearer gating with 401/403.
- **Backend legs of the Phase-10-deferred requirements are tested**: PRD-SETUP-001/002 (onboarding
  preflight/bypass — the graphical wizard is explicitly "Phase 10 admin UI over the same stores"
  per the parity matrix), PRD-CONFIG-001..003 (settings CRUD/resolver/import-export), PRD-POLICY-008
  (dry-run engine + `policy_test_runs` audit; only the UI leg is in-progress), PRD-WORK-002
  (queue/lease/worker-pool state; only the UI leg is deferred), PRD-WEB-001 (63-page Blazor
  inventory, KEEP 54 / REPLACE 6 / DROP-candidate 3).
- **SPA auth raw material**: Phase 7 native auth (authenticate/refresh/logout, cookie variants,
  credential-revision revocation), CORS layer for the native app, signed stream URLs for cataloged
  tracks, image endpoints, `GET /api/v1/admin/users` (list-only).
- **Approved visual intent**: eight 1600×1200 mockups (`design/ui/reference/`), the written design
  system (`DESIGN-SYSTEM.md` — tokens, status semantics, four page patterns, destructive-action and
  accessibility invariants), and accepted `ADR-0109` (React + TypeScript + Carbon, DataTable first,
  TanStack only via proven-gap ADR).

### 1.2 Phase 9 handoff items (owned by Phase 10)

- **Checklist 11's two ➡ rows**: PRD-THEME-001..004 and PRD-I18N-001..006 — the entire
  theming/appearance and localization/RTL requirement set.
- **ADR-0109 shell validation = Phase 10 entry criteria**: Processing Queue, Release Review, and
  Artist Identity Review shells in Carbon with light/dark, one translated locale, RTL smoke,
  keyboard navigation, and responsive behavior. **Evidence must be recorded in the ADR before
  Phase 10 is accepted.** This gates closeout, so the three shells are sequenced first (Wave 2).
- **PRD-WEB-002 is `implemented`, not `tested`**: the WP-14 classification is *preliminary pending
  Phase 10 ratification*; the three DROP candidates (`/jukebox`, `/party`, `/party/{key}` — the
  latter retires the only SignalR hub) each require their own ADR before any route removal, and the
  REPLACE items (`/onboarding*`, `/admin/scripts*`, `/musicplayer`, `/Error`) become real screens.
- **Auth-mismatch fixes**: legacy `/data/users`, `/data/useredit/*`, `/usergroup/{key}`,
  `/admin/mediaartists` relied on service-layer (not route-level) authorization — the redesign must
  enforce admin route auth (flagged in the Blazor inventory).
- **Durable artwork wiring** (`images.rs::resolve_image`, captured 404 classes) is tracked with the
  artwork pipeline "with Phase 10 UI residue" — confirm an owning WP at Wave 0.
- **Per-user locales for notifications** (matrix row) land with localization.

### 1.3 Genuine Phase 10 greenfield (zero implementation today)

| Area | State today |
|---|---|
| Admin SPA | `admin-ui/` contains only a README; no Node tooling outside `docs/`; no package.json, no build, no tests. |
| Static SPA serving | None. `tower-http` enabled with `["cors", "trace"]` only — no `fs`/`ServeDir`; SPEC §6 allows "served by the server or a simple reverse proxy" (undecided). |
| Release Review API | All missing over HTTP: manifest/evidence retrieval, validation review results, policy decisions + audit, operator decide/override, reprocess/retry of failed work items (stores exist: `get_manifest`, `get_decision`, `apply_admin_override`, …). |
| Artist Identity Review API | All missing: observed-artist/match listing, merge/split mutations (`catalog_identity.rs` has full store support incl. audit reasons; exercised only in tests). |
| Policy rule CRUD/editor API | None: rule sets exist in persistence but default rules are compiled into `melodee-policy`; no versioned rule management endpoints (SPEC §19.4 names "policies" in-scope). |
| System/Workers/Search API | No HTTP for worker-pool pause/resume, search index status/rebuild, aggregated dashboard stats (CLI-only today). |
| Server-side tables | Admin lists are limit/offset bare arrays — **no total counts, no sort parameters, no pagination envelope**; native catalog lists have `orderBy` but are a frozen legacy-vocabulary contract that must not be reshaped for the UI. |
| Diagnostics auth | `/api/admin/v1/ingest/*` and `/system/info` are **unauthenticated** today (handlers lack `AdminPrincipal`). |
| Review audio preview | Signed stream URLs exist only for published tracks; nothing streams a work item's pre-publication audio (mockup 03 shows inline preview in Release Review). |
| Admin user breadth | No `GET /api/admin/v1/users` list (native-only), no admin password set/reset, no delete. |
| Localization assets | No i18n framework, no string extraction, 20-culture inventory (PRD-I18N-001) unharvested from the legacy Blazor resources. |
| UI test infrastructure | No Vitest/Playwright/visual-regression tooling; `make check` and the Makefile have no JS step; no admin-ui CI workflow. |

### 1.4 Tracking/documentation gaps that block the Phase 10 exit gate

Phase 10 exit = *"All canonical admin workflows are complete and accepted"* (PRD §56) plus
deliverables: Carbon shell, eight canonical screens, theming/light/dark, localization/RTL,
responsive/accessibility testing, visual regression suite. Blocking gaps:

1. **ADR-R020 (localization stack and RTL preservation)** is listed Proposed in PRD §53 with **no
   ADR file** — i18next + react-i18next is the PRD-preferred stack but is unratified; every localized
   screen depends on this decision.
2. **Build/runtime tooling undecided**: no ADR or documented dependency decision for the SPA build
   tool (Vite assumed), test runner, Playwright, or static-serving mechanism; all are new production
   dependencies requiring the FOSS gate (`TECHNOLOGY-BASELINE.md`) and an ADR or documented decision.
3. **PRD §39 stale instruction**: line ~1798 says mockups "should be copied into
   `docs/ui/reference/`" — `docs/AGENTS.md` forbids publishing mockups as product screenshots.
   Resolve by PRD correction (mockups stay in `design/ui/reference/`).
4. **Responsive breakpoints undefined** anywhere; `DESIGN-SYSTEM.md` mandates responsive behavior
   without numbers. The PRD-UI-REF-003 Melodee UI specification must define them (Carbon grid
   breakpoints are the implied default).
5. **24 Phase-10-tagged requirements are all `planned`**: PRD-UI-001..010, PRD-UI-REF-001..003,
   PRD-THEME-001..004, PRD-I18N-001..006, PRD-API-ADMIN-001..003; plus PRD-TEST-008 (UI testing,
   phases `0,all`). None has started; all must reach terminal status at closeout.
6. **Parity matrix Phase-10 rows** are `Planned`/`In progress` (appearance, localization, RTL,
   metadata editing, artist/release/track browsing-editing, artwork management, staging/manual
   review, diagnostics/Doctor, job history/trigger/pause-resume, themes, API explorer) — they are
   the exit-gate checklist and must be reconciled as WPs land.

### 1.5 Decisions Phase 10 cannot make unilaterally

- **Metadata-editing scope**: the matrix lists "Metadata editing", "Artist browsing/editing/merge",
  "Release browsing/editing" as phase `4,10` `In progress` with "admin UI remain Phase 10", and the
  Blazor KEEP list includes `/data/*` edit pages — but `IMPLEMENTATION.md` §13's work list describes
  read/decision workflows only. **Product decision required**: full edit forms vs. read-only detail +
  curation decisions in Phase 10.
- **SPA serving strategy**: embedded static serving (tower-http `fs`, new dependency feature) vs.
  reverse-proxy-only. SPEC §6 permits both; pick one as the supported default.
- **SPA auth/session UX**: cookie vs. Bearer storage, refresh handling, login screen behavior —
  rides on the frozen Phase 7 native auth contract; must not change that contract.
- **Diagnostics endpoint gating**: adding `AdminPrincipal` to `/api/admin/v1/ingest/*` changes a
  shipped (Rust-native, non-frozen) surface — allowed, but must be explicit and documented; the
  pre-release dev-server caveat in `api/admin-api.md` gets resolved by this decision.
- **DROP-candidate routes**: `/jukebox`, `/party`, `/party/{key}` each need their own ADR (WP-14
  residue; ADR-0133 waives capability, not route retirement).
- **TanStack Table**: deferred per ADR-0109 until a proven Carbon DataTable gap; any adoption needs
  its own ADR.
- **API explorer** (matrix `Planned`, phases `7,8,10`): decide Phase 10 scope or defer with an
  explicit note.

---

## 2. Execution strategy

Phase 10 is executed in five waves. Wave 0 is mandatory gating work; Wave 1 builds the application
foundation; Wave 2 delivers the three ADR-0109 validation shells **first** (phase entry criteria);
Waves 3–4 complete the screen inventory and cross-cutting quality. Backend API gaps (§1.3) are
delivered as co-equal WPs inside the waves that need them — the admin API is a free Rust-native
surface (Phase 9 report: "not frozen contracts"), so extending it needs no contract-change process,
only the normal schema/ADR discipline.

Every WP follows the agent execution protocol (IMPLEMENTATION §16): requirement IDs + ADRs first,
acceptance tests before implementation, smallest safe slice, `cargo fmt --all` +
`make check` green for Rust changes and the new JS gate for UI changes, `/docs` updated in
the same change.

### Wave 0 — Unblock, decide, specify (no screens yet)

| # | Item | Deliverable |
|---|---|---|
| W0-1 | **Decision bundle** (see §3): localization stack ADR (ADR-R020), SPA build/test tooling ADR or documented dependency decision (Vite, Vitest, Playwright, exact pins, Node 22/pnpm 11.20.0 matching `docs/`), SPA serving ADR (embedded vs proxy), SPA auth/session decision, diagnostics-gating decision, metadata-editing scope decision, API-explorer disposition | Accepted ADRs/decisions before dependent WPs start |
| W0-2 | **Melodee UI specification** (PRD-UI-REF-003): design tokens + Carbon overrides, app-shell dimensions, density, status semantics codification (from DESIGN-SYSTEM.md), shared-component contracts, interaction patterns, **numeric responsive breakpoints**, accessibility rules, agent UI rules | `design/ui/` spec document; closes the breakpoint gap |
| W0-3 | **Legacy localization harvest**: extract the 20-culture resource inventory from the frozen Blazor oracle (source of existing translations), classify reusable vs. admin-console-new strings; i18n key scheme + English fallback semantics (PRD-I18N-006) | Translation seed assets + coverage report |
| W0-4 | **Screen/workflow checklists**: one parity checklist per canonical screen + the shell/theming/i18n cross-cutters under `design/evidence/phase-10/checklists/`, each mapping screen → requirement IDs → mockup → API endpoints → tests | 10+ checklists seeding WP acceptance criteria |
| W0-5 | **PRD correction**: fix the §39 `docs/ui/reference/` instruction (mockups stay in `design/ui/`); confirm durable-artwork wiring and API explorer have owning WPs; assign the three DROP-candidate ADRs to Wave 4 | PRD/docs consistency; no orphan follow-ups |

### Wave 1 — Application foundation (needs W0-1/W0-2)

| WP | Scope | Requirements |
|---|---|---|
| WP-01 | **Toolchain + scaffold**: `admin-ui/` Vite + React + TypeScript + Carbon package; exact-pinned deps, committed lockfile, `packageManager` pin; ESLint/format config; base routing; `pnpm` scripts; `allowBuilds` treatment for install-script deps | PRD-UI-002, ADR-0109 |
| WP-02 | **CI + local gate**: path-filtered `.github/workflows/admin-ui.yml` modeled on `docs.yml` (Node 22, pnpm 11, frozen lockfile); `make check`/Makefile JS step (typecheck, lint, unit tests, production build) so the gate is executable locally and in CI | PRD-TEST-008 (partial) |
| WP-03 | **Static serving + dev wiring**: per W0-1 decision — embedded `ServeDir` (tower-http `fs` feature, dependency decision) with SPA fallback, or documented proxy config; dev-server proxy for `/api`; CORS posture for admin routes settled | SPEC §6 |
| WP-04 | **App shell**: persistent collapsible left nav with the canonical IA (Overview; PROCESSING: Processing/Review Queue/Quarantine/History; CATALOG: Artists/Releases/Tracks/Identity Review; POLICIES: Release Rules/Matching Rules; SYSTEM: Storage/Workers/Search/Users & Access/Settings/Diagnostics); header global search entry, user/language/appearance/notifications/server-status controls; routing skeleton with empty states | PRD-UI-001/004/006, DESIGN-SYSTEM.md shell rules |
| WP-05 | **Auth/session UX**: login screen against native auth; token storage + refresh per W0-1; route guards incl. the legacy auth-mismatch fixes (admin enforcement on users/groups/identity screens); 401/403 handling; logout | PRD-WEB-002, SPEC §20 |
| WP-06 | **Theming system**: Carbon theme tokens + Melodee token layer; System/Light/Dark appearance × theme packs (server-side theme-pack API integration); high-contrast/reduced-motion respect; logical-properties CSS discipline | PRD-THEME-001..004 |
| WP-07 | **Localization framework**: i18next + react-i18next per accepted ADR; string extraction discipline; Intl date/time/number formatting; `dir=rtl` derivation; key validation + English fallback; harvested translations loaded; Arabic/Persian RTL smoke harness | PRD-I18N-001..006 |
| WP-08 | **Shared components**: `<AppShell/> <PageHeader/> <MetricCard/> <StatusTag/> <EntityAvatar/> <ArtistLink/> <ReleaseLink/> <TrackLink/> <ProcessingStateTag/> <EvidenceItem/> <EvidenceSummary/> <DecisionPanel/> <InspectorPanel/> <EntityTable/> <EmptyState/> <HealthIndicator/> <AuditTimeline/> <AudioPreview/> <DestructiveActionDialog/>` — with component/story tests; server-side table primitives (sort/filter/page state in shareable URLs) | PRD-UI-003/006/008/010, §40 component list |
| WP-09 | **API client layer**: typed client for `/api/admin/v1` (hand-written against documented contracts; no codegen dependency without a decision); error-envelope handling; correlation-id surfacing; bounded retry; pagination-envelope conventions | PRD-API-ADMIN-001..003 |
| WP-10 | **Admin API hardening + table support**: gate `/api/admin/v1/ingest/*` + `/system/info` behind `AdminPrincipal` (per W0-1); add total-count pagination envelopes and per-endpoint sort whitelists; `GET /api/admin/v1/users` list + admin password set/reset | PRD-API-ADMIN-002, PRD-UI-010, SPEC §23 |

### Wave 2 — ADR-0109 validation shells (phase entry criteria; highest priority screens)

Each shell ships with its required backend WP and the full validation matrix: **light/dark, one
translated locale, RTL smoke rendering, keyboard navigation, responsive behavior**.

| WP | Screen + backend | Requirements |
|---|---|---|
| WP-11 | **Processing Queue**: dense server-paged table, state filter chips with counts, bulk toolbar, right Release Inspector, queue position preserved on navigation; backend: extend work-item listing (filters, counts-by-state, sort whitelist) — builds on WP-10 | PRD-UI-006/010, PRD-INGEST-010, ADR-0109 validation #1 |
| WP-12 | **Release Review**: evidence/confidence display, catalog comparison (Known/New/Alternate/Uncertain), track breakdown with inline audio preview, decision actions (Accept/Quarantine/Ignore/Delete with impact dialog), policy match + recommendation, review timeline. Backend: manifest/evidence endpoints, policy decision + audit read, operator decide/override mutation (audited, authorized), reprocess/retry, **pre-publication review audio preview** (operator-gated, bounded) | PRD-UI-007/008, PRD-POLICY-008 (UI leg), SPEC §20 audit, ADR-0109 validation #2 |
| WP-13 | **Artist Identity Review**: unresolved-identities queue, incoming observation vs. candidate comparison with confidence, evidence summary, actions (Confirm Match / Create New Artist / Leave Provisional / Never Match — negative assertion), recent-decisions feed. Backend: observed-artist/match listing, confirm/create/negative-assertion mutations, merge/split endpoints over `catalog_identity.rs` stores (audit reason strings preserved) | PRD-API-ADMIN-002, SPEC §20, ADR-0109 validation #3 |
| W2-4 | **Record ADR-0109 shell-validation evidence** in the ADR (screenshots, keyboard/RTL/locale/responsive test results) — without this, Phase 10 cannot be accepted | ADR-0109 acceptance |

### Wave 3 — Remaining canonical screens and parity surfaces

| WP | Screen + backend | Requirements |
|---|---|---|
| WP-14 | **Overview dashboard**: KPI cards, attention queue, activity chart, system health, mini processing table; backend: aggregated dashboard/stats endpoint | PRD-UI-001, SPEC §22 |
| WP-15 | **Artist Detail + Release Detail**: tabbed workspaces (Overview/Releases/Appearances/Contributions/Relationships/Identity/Images/History; Tracks/Artists/Contributors/Files/Classification/Provenance), catalog browsing tables (35k+/50k+/560k+ server-side), image/artwork management surface (consumes durable-artwork wiring) | PRD-UI-005/010, matrix artwork + browsing rows |
| WP-16 | **Policies/Rule Editor**: rule list (type/priority/matches/enabled), condition/action builder, "Test against catalog" with representative samples + destructive-impact preview; backend: versioned policy rule CRUD + dry-run/test-run endpoints over `melodee-policy` | PRD-POLICY-008, SPEC §19.4 |
| WP-17 | **System/Workers/Diagnostics**: worker-pool table (queued/active/throughput/age/retries/leases), pause/resume, health panels (PostgreSQL/Tantivy/storage/FFmpeg/API surfaces), recent failures, search index status/rebuild trigger, doctor/system check; backend: worker-pool + search-admin + diagnostics aggregation endpoints | PRD-WORK-002 (UI leg, closes the deferral), PRD-INGEST-010, SPEC §22 |
| WP-18 | **Quarantine + History views**, **Users & Access** (users list, groups/members/library bindings, device profiles, notification templates/test-send, scripts, radio admin), **Settings** (settings CRUD over the 190-key registry, config export/import with preview + redaction UX) | PRD-CONFIG-001/002, PRD-WEB-002 REPLACE items |
| WP-19 | **Onboarding wizard**: guided first-run UI over the tested onboarding stores (six preflight checks with remediation, fail-closed completion, bypass state) — the WP-14 REPLACE for `/onboarding*`; script-editor surface decision per ADR-0135 (the legacy Monaco/Jint editor has no analog) | PRD-SETUP-001/002, PRD-WEB-002 |

### Wave 4 — Quality gates, contract residue, closeout

| # | Item |
|---|---|
| W4-1 | **Accessibility + responsive evidence**: keyboard-complete navigation/actions, focus order, labeled tables/dialogs/toasts/audio controls, no hover-only info, artwork alt text; responsive verification at the W0-2 breakpoints across all eight canonical screens |
| W4-2 | **Visual regression suite** (PRD-UI-REF-002, PRD-TEST-008): Playwright screenshot baselines for canonical screens in light + selected dark + RTL variants; real React screenshots supersede the mockups as references |
| W4-3 | **Localization/RTL verification**: all 20 cultures load with fallback semantics; full translated-locale pass on at least one non-English locale; Arabic + Persian RTL verification on canonical screens (PRD-I18N-005) |
| W4-4 | **DROP-candidate ADRs** (`/jukebox`, `/party`, `/party/{key}` + SignalR hub retirement) and final PRD-WEB-002 verification: every KEEP/REPLACE/DROP classification ratified, every surface move documented → promote PRD-WEB-002 to `tested` |
| W4-5 | **Performance evidence**: server-side table endpoints at catalog scale (SPEC §23 budgets; bounded memory, indexed pagination); SPA bundle/boot budget defined and measured |
| W4-6 | **Documentation sweep**: replace every `:::caution[Planned documentation]` admin-UI aside in `/docs` (admin/overview, processing, identity-review, policies, system-diagnostics, scripting-hooks, quick-start, library-management/*) with shipped-behavior docs incl. real screenshots; new theming + localization pages; `api/admin-api.md` updated for all new endpoint groups and the auth-gating change |
| W4-7 | **Closeout**: all checklists green; all 24 Phase-10 requirement IDs + PRD-TEST-008 + PRD-POLICY-008/PRD-WORK-002 UI legs terminal; parity matrix Phase-10 rows reconciled; `design/evidence/phase-10/PHASE-10-REPORT.md` per the release-gate evidence format; ADR-0109 evidence confirmed recorded; `AGENTS.md` phase boundary advanced |

---

## 3. Decision bundle (W0-1) — recommended positions

| Decision | Recommendation | Authority |
|---|---|---|
| Localization stack (ADR-R020) | **i18next + react-i18next** (PRD-preferred, MIT, in the technology baseline); ICU/MessageFormat plural support; harvest legacy resources as translation seeds (W0-3) | New ADR closing the PRD §53 gap |
| Build/test tooling | **Vite + Vitest + Playwright**, exact pins, Node 22 / pnpm 11.20.0 (match `docs/`); FOSS gate per TECHNOLOGY-BASELINE | ADR or documented dependency decision |
| SPA serving | **Embedded static serving** via tower-http `fs` with SPA fallback as the supported default (single-executable deployment story, SPEC §6); reverse proxy documented as an alternative | ADR or documented dependency decision (new feature flag on an existing dep) |
| SPA auth/session | **Cookie-based session** riding the frozen Phase 7 native auth contract (cookie sign-in/out variants already exist); no contract change; admin screens require `isAdmin` | Decision note; Phase 7 contract untouched |
| Diagnostics gating | **Gate `/api/admin/v1/ingest/*` and `/system/info` behind `AdminPrincipal`**; `/healthz`/`/readyz` stay anonymous. The admin API is a Rust-native non-frozen surface, so this is a hardening change, not a contract break; update `api/admin-api.md` in the same change | WP-10, documented |
| Metadata-editing scope | **Phase 10 ships read-only catalog detail + curation/identity/policy decisions; full metadata edit forms defer to a follow-on ADR.** Rationale: IMPLEMENTATION §13's work list is read/decision workflows; edit forms are the largest scope swing and need their own write-contract design (legacy edit pages are service-layer-authorized and need redesign anyway). If product wants editing in Phase 10, it becomes WP-20 with its own acceptance tests. | Product decision; matrix rows updated either way |
| API explorer | **Defer to Phase 11+** with an explicit matrix note; ship accurate `/docs` API pages instead | Matrix note |
| TanStack Table | Keep deferred; Carbon DataTable until a proven gap (ADR-0109 mechanism) | ADR-0109 |
| DROP candidates | Approve the three per-route ADRs in Wave 4 (capability already waived by ADR-0133) | 3 small ADRs |
| Matching Rules nav entry (PRD-UI-004) | **Removed at closeout, not placeholder-shipped**: no matching-rule model exists (the engine has exactly one rule model — Release policy rules — and identity matching is evidence-based); a future matching-rule engine would restore the entry. Recorded with checklist 08 🚫 row; System → Search/Storage/Diagnostics placeholder routes replaced by real screens in the same closeout | Product decision, recorded here |

## 4. Cross-cutting requirements for every WP

- **Compatibility discipline:** the admin API is free to extend, but Phase 7/8 frozen contracts
  (native v1, OpenSubsonic, Jellyfin) must not be reshaped to serve the UI — the SPA consumes them
  as-is or the admin API gains Rust-native endpoints (PRD-API-ADMIN-003). Any intentional
  404/410/501 change still requires the 4-step contract-change process.
- **Schema changes:** policy rule CRUD and any new UI-support tables need their own migration +
  rollback/forward plan + data-integrity tests + accepted schema ADR.
- **Security:** every data-changing action is authorized server-side; destructive and identity
  decisions carry administrator identity + reason in the audit record (SPEC §20); secrets stay
  redacted in config export UX; review audio preview is operator-gated and bounded.
- **Testing:** component/story tests for shared components; Playwright e2e for canonical workflows;
  visual regression in light/dark/RTL; API endpoint suites follow the established per-surface test
  file + disposable-PostgreSQL harness (`TestDb`, rebuilt routers, `oneshot`).
- **Gate discipline:** `cargo fmt --all` + `make check` for Rust changes; the WP-02 JS gate
  (typecheck/lint/unit/build, later + Playwright) for UI changes; both green before any WP is done.
- **Docs:** `/docs` updated in the same PR for every user-visible WP; mockups never published as
  product screenshots (`docs/AGENTS.md`); real screenshots replace mockups as references once
  accepted (PRD-UI-REF-002).
- **Traceability:** `status-overrides.json` + `extract_requirements.py` per WP; parity matrix rows
  reconciled as screens land; checklists updated continuously, not at closeout.

## 5. Sequencing summary and critical path

```
W0-1 (decisions) ──► W0-2 (UI spec) ──► Wave 1 (WP-01..10 foundation, partially parallel)
W0-3 (i18n harvest) ─┘                      │
                                            ▼
                              Wave 2 (WP-11..13 validation shells + W2-4 ADR-0109 evidence)
                                            │
                                            ▼
                              Wave 3 (WP-14..19 remaining screens, parallelizable)
                                            │
                                            ▼
                              Wave 4 (W4-1..7 quality + closeout)
```

Critical path: **W0-1 decisions → WP-04/WP-08 shell+components → WP-11..13 validation shells →
W2-4 ADR-0109 evidence → W4-7 closeout**. The heaviest engineering is WP-12 (Release Review backend
+ decision UX) and WP-16 (policy rule CRUD + editor); the heaviest evidence burden is Wave 4's
accessibility/visual/localization matrix.

Suggested PR slicing (per IMPLEMENTATION §15): one WP per PR at most; Wave 0 as three PRs
(W0-1 ADRs, W0-2 spec, W0-3..W0-5 harvest/checklists/corrections).

## 6. Risks

1. **Metadata-editing scope swing** (§1.5): read-only vs. full edit forms is the largest scope
   variable — resolve in W0-1 before any catalog screen work.
2. **ADR-0109 evidence is a hard acceptance gate**: the three validation shells carry a fixed
   quality matrix (light/dark × translated locale × RTL × keyboard × responsive); under-scoping the
   matrix blocks phase acceptance regardless of the remaining screens.
3. **20-culture localization** is volumetric work: harvested legacy translations will not cover
   admin-console strings; incomplete locales rely on fallback (PRD-I18N-006), which must be
   evidenced honestly rather than claimed as full translation.
4. **Backend gaps are broader than the screen list suggests** (§1.3): Release Review, Identity
   Review, policy CRUD, and worker/search admin endpoints are new API surface area with audit and
   authorization obligations — they are co-equal WPs, not afterthoughts, and WP-12/WP-16 carry
   schema-ADR potential.
5. **Pre-publication audio preview** touches source/work-area files: needs careful authorization,
   path-safety, and bounded-range handling; the safe fallback is waveform/metadata-only preview
   with a documented limitation.
6. **New JS supply chain**: the repo's dependency discipline (FOSS gate, lockfiles, deny checks)
   has no JS equivalent yet; WP-01/WP-02 must establish it (lockfile, audit tooling) before
   dependency sprawl.
7. **Playwright flakiness/visual-baseline drift**: visual regression needs deterministic data
   fixtures and viewport pinning, or W4-2 becomes a recurring failure source (cf. the Phase 9
   timing-sensitive test flake).

---

## Review report (per AGENTS.md required workflow)

- **Files changed:** `design/agents/PHASE-10-PLAN.md` (this document; new planning artifact).
- **Requirements satisfied:** none implemented — this is the planning artifact only.
- **ADRs applied/proposed:** proposes future ADRs for the localization stack (closing PRD §53's
  ADR-R020 gap), SPA build/test tooling, SPA static serving, the three DROP-candidate route
  retirements, and (conditionally) metadata editing; proposes decision notes for SPA auth/session
  and diagnostics-endpoint gating; relies on accepted ADR-0109 and records its validation as Phase
  10 entry criteria per the Phase 9 amendment.
- **Tests run:** none (planning only). Repository state verified by read-only survey (2026-08-14).
- **Compatibility impact:** none. Plan explicitly forbids reshaping frozen Phase 7/8 contracts for
  the UI and routes the three DROP-candidate route removals through per-route ADRs.
- **Data/migration impact:** none now; §4 enumerates the schema-ADR obligations Phase 10 will incur
  (policy rule CRUD at minimum).
- **Documentation impact:** Updated — this plan under `design/agents/` (internal engineering
  docs). No `/docs` (public) changes: no user-visible behavior changed. The PRD §39 stale
  `docs/ui/reference/` instruction is documented here; its correction is scheduled as W0-5 rather
  than done in this planning change.
- **Remaining risks:** §6.

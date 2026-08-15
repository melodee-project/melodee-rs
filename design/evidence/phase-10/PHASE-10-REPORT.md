# Phase 10 Report — Admin UI

**Status: COMPLETE (2026-08-15).** Executed per
[design/agents/PHASE-10-PLAN.md](../../agents/PHASE-10-PLAN.md) (five waves) under accepted ADRs
0109 (Carbon; validation evidence recorded in the ADR), 0137 (localization stack), 0138 (SPA
tooling + static serving), 0139 (session strategy + admin API gating), 0140/0141 (legacy
jukebox/party web-route retirements), 0142 (admin browse indexes), and 0143 (catalog artwork
overrides + frozen image-endpoint precedence).

## Exit criteria (IMPLEMENTATION §13 / PRD §56)

| Exit criterion | Result | Evidence |
|---|---|---|
| Canonical workflows match approved information hierarchy | ✅ All eight canonical screens implemented per UI-SPEC + mockups' IA | `admin-ui/src/screens/`; checklists 03–10 (all ✅) |
| Destructive and identity decisions are safe and explainable | ✅ Reason-required, audited, fail-closed; typed confirmation + impact display for delete-class | `DestructiveActionDialog`; `admin_review.rs`/`admin_identity.rs`; e2e coverage |
| Existing supported languages and direction behavior preserved | ✅ 20 cultures; ar-SA/fa-IR RTL verified on canonical screens | `i18n-harvest.md`; `e2e/localization.spec.ts`; RTL visual baselines |
| UI does not require proprietary component tiers | ✅ Carbon (Apache-2.0) only; TanStack never needed (no proven gap) | `admin-ui/package.json`; ADR-0109 |
| Carbon shell | ✅ | checklist 01 |
| Eight canonical screens | ✅ Overview, Processing Queue, Release Review, Artist Identity Review, Artist Detail, Release Detail, Policies/Rule Editor, System/Workers/Diagnostics (+ Track detail, Quarantine/History, Users/Settings, Onboarding) | checklists 03–10 |
| Theming/light/dark | ✅ System/Light/Dark × theme packs | checklist 01; `e2e/theme-packs.spec.ts` |
| Localization/RTL | ✅ | checklist 02 |
| Responsive/accessibility testing | ✅ axe WCAG 2.2 A/AA per screen; 375/768/1600 viewports | `accessibility-visual-report.md` |
| Visual regression suite | ✅ Committed baselines, light/dark/RTL, deterministic mocks | `e2e/visual.spec.ts-snapshots/` |

**PRD §56 exit: "All canonical admin workflows are complete and accepted." — met.**

## Wave summary

- **Wave 0** — decision bundle (ADR-0137/0138/0139), `design/ui/UI-SPEC.md` (PRD-UI-REF-003, incl.
  previously undefined numeric breakpoints), legacy resx harvest (84 mapped keys × 19 cultures),
  ten screen checklists, PRD §39 stale mockup-copy instruction corrected.
- **Wave 1** — Vite/React/TS/Carbon scaffold; `admin-ui.yml` CI + local JS gate in
  `scripts/check.sh`; embedded SPA serving at `/admin` (tower-http `fs`); app shell, cookie auth
  (native-v1 contract untouched), theming, i18n framework, 18 shared components, typed API client;
  admin API hardening: diagnostics endpoints uniformly `AdminPrincipal`-gated (ADR-0139),
  pagination envelopes + sort whitelists, admin users list + password reset.
- **Wave 2** — the three ADR-0109 validation shells (Processing Queue, Release Review, Artist
  Identity Review) with their backend surfaces (review queue/detail/decision/reprocess/preview;
  identity unresolved/detail/resolve/never-match/decisions/merge/split); validation evidence
  recorded in ADR-0109.
- **Wave 3** — Overview, catalog browsing + Artist/Release/Track detail, Policies rule editor
  (immutable versioned rule sets, test-against-catalog), System/Workers/Diagnostics (pool
  pause/resume, search rebuild), Quarantine/History, Users & Access, Settings (config
  export/import, notifications, scripts, radio, device profiles), Onboarding wizard; backend:
  dashboard aggregation, policy rule CRUD/test, worker/search/system endpoints, catalog
  browse/detail endpoints.
- **Wave 4** — accessibility + responsive evidence (incl. a real app-wide shell remount defect
  found and fixed), visual regression baselines, localization verification, DROP ADRs
  (0140/0141), performance evidence + index remediation (ADR-0142: 13 full indexes; identity
  queue 365 ms → 1.9 ms at synthetic 200k scale), SPA code-split to budget (240.9 kB → 172.3 kB
  gzip initial), signed-URL catalog audio preview, artwork overrides (ADR-0143), full `/docs`
  sweep (11 planned asides replaced, 8 real screenshots), closeout.

## Gate evidence (2026-08-15, `./scripts/check.sh` green end-to-end)

- Repository/doc validation: **44 ADRs, 8 mockups, 239 requirements** — pass; requirements index
  current.
- Python capture tests: pass. `cargo fmt --check`, `clippy -D warnings`: clean.
- **Rust tests: 904 passed, 0 failed** (workspace, all features) — up from 834 at Phase 9.
- **Admin UI: 232 unit/component tests, 259 Playwright e2e runs (3 viewports), 0 failures**;
  `pnpm check/lint/i18n:check/build` green; frozen lockfile.
- Docs: `pnpm check`, `check:links`, `build` green (40 pages). `cargo deny check`: ok.

## Requirement closeout

All 24 Phase-10-tagged requirements terminal: PRD-UI-001..010, PRD-UI-REF-001..003,
PRD-THEME-001..004, PRD-I18N-001..006, PRD-API-ADMIN-001..003 → `tested` (PRD-UI-009
`implemented` — command palette covers navigation entry per its "desirable" scope); PRD-TEST-008
→ `tested`; Phase-10 legs of PRD-POLICY-008, PRD-WORK-002 → `tested`; PRD-WEB-002 → `tested`
(final ratification: [web-parity-verification.md](web-parity-verification.md)). Parity matrix:
all Phase-10 rows `Complete` except **Metadata editing** (`In progress`, deliberately — full
metadata edit forms deferred by the Phase 10 decision log; read-only detail + curation decisions
shipped; follow-on ADR required).

## Compatibility impact

No native-v1/OpenSubsonic/Jellyfin DTO, status, or auth behavior changed. ADR-0143 deliberately
extends artwork read behavior: native `GET /images/{id}/{size}`, OpenSubsonic `getCoverArt`, and
Jellyfin `Items`/`Artists` image routes serve a stored Artist/Release override when one exists;
Jellyfin then falls back to immutable publication-plan artwork. Each surface preserves its
captured no-artwork fallback when neither source exists. Party/jukebox absence is preserved
(ADR-0133) with legacy web routes retired by ADR-0140/0141.

## Post-close artwork adapter completion

The initial ADR-0143 evidence proved the native endpoint precedence branch, but an independent
integration review found that the separate OpenSubsonic and Jellyfin image handlers had retained
their previous no-artwork-only resolver paths. This was corrected without merging wire DTOs or
altering their no-override captured branches:

- `opensubsonic/media.rs` now serves Artist/Release overrides through `getCoverArt`, with Song
  identifiers resolving their Release override; `opensubsonic_media_api.rs`
  `get_cover_art_catalog_overrides_are_served_for_artist_release_and_song` proves all three
  identifiers.
- `jellyfin/images.rs` now applies the override → publication-plan ordering through the bounded,
  no-follow direct-play read boundary, and renders stored PNG/JPEG bytes through the shared
  bounded image processor for its documented size buckets. Its ETag is content-derived so an
  immediate replacement cannot produce a stale `304`.
- `jellyfin_media_api.rs`
  `images_serve_artist_and_release_overrides_on_all_catalog_routes` proves both route families,
  album/track inheritance, the indexed form, size processing, HEAD, and conditional caching;
  `images_fall_back_to_published_canonical_artwork` proves the Artist/Release/Track fallback
  through a real immutable plan and no-follow canonical root.

## Data/migration impact

Two additive migrations: `0029_phase10_admin_browse_indexes.sql` (13 full indexes, ADR-0142;
rollback = documented DROP INDEX) and `0030_phase10_catalog_entity_images.sql` (artwork override
table, ADR-0143; rollback = DROP TABLE). Both have data-integrity tests; no destructive changes.

## Known limitations (honest)

1. Metadata edit forms are not part of Phase 10 (decision log); catalog detail is read-only with
   curation/identity/policy/artwork operations.
2. Policy rule-set versions authored in the UI are durable immediately but adopted by workers on
   restart/redeploy (no hot reload — documented in `api/admin-api.md`).
3. Ad-hoc (unstored) policy test runs execute but are not persisted (`policy_test_runs` FK);
   persisting them needs a schema ADR.
4. Theme packs map onto the `--melodee-*` token layer only (UI-SPEC §2); legacy pack component
   selectors are not applied.
5. Artist browse ILIKE improved but remains planner-bound at extreme scale (342 ms → 115 ms at
   100k synthetic); the OR/EXISTS flattening and release `artistId` EXISTS rewrite are documented
   code-level follow-ups in `performance-report.md` §4.
6. Non-English locales cover harvested legacy strings; admin-console vocabulary falls back to
   English until translated (ADR-0137 fallback is the designed behavior; coverage report:
   `i18n-harvest.md`).
7. Visual baselines are Chromium-on-Linux only; deliberate visual changes require
   `--update-snapshots` with PR review.
8. Closeout residue resolved: the four remaining placeholder routes are gone. System → Search,
   Storage, and Diagnostics are real screens (shared `SearchPanel`/`LeasesPanel` extracted from
   the Workers screen — no duplicated logic). The PRD-UI-004 Policies → Matching Rules nav entry
   was removed rather than placeholder-shipped: no matching-rule model exists (identity matching
   is evidence-based); a future matching-rule engine would restore it (decision log §3,
   checklist 08 🚫 row). Per-path storage capacity and MQL query tooling remain CLI/native-API
   surfaces and are stated as such on the screens.

## Documentation impact

- **Updated** — `/docs`: admin section rewritten for the shipped UI (overview, processing,
  release review, identity review, policies, system diagnostics, users & access, scripting),
  new `admin/settings.md` + `admin/appearance-and-localization.md`, quick-start onboarding
  wizard, `api/admin-api.md` fully updated, 8 real-UI screenshots. Internal: this report, ten
  checklists, UI-SPEC, harvest/accessibility/performance/web-parity evidence docs,
  PHASE-10-PLAN status, ADRs 0109/0137–0143.

## Remaining risks

- Phase 11 entry: metadata-editing ADR (if product wants edit forms), provider integrations
  (ADR-0136 deferral), container images — all tracked with owners in the parity matrix.
- JS supply chain is now gated in CI (`pnpm audit --prod`, frozen lockfile) but is a new
  operational surface to watch.

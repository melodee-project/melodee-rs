# Phase 10 W4 — Accessibility, Visual Regression, and Localization/RTL Evidence

- **Date (UTC):** 2026-08-15
- **Scope:** W4-1 accessibility e2e (axe + keyboard), W4-2 visual regression baselines,
  W4-3 localization/RTL verification for the admin SPA (`admin-ui/`).
- **Related requirements/ADRs:** PRD-UI-REF-003, PRD-I18N-001…006, UI-SPEC §8 (responsive)
  and §9 (accessibility); ADR-0109 (shell validation), ADR-0137 (i18n), ADR-0138 (static
  serving), ADR-0139 (route guard).

## 1. Method

Three new Playwright suites, all running against a shared deterministic mock API
(`admin-ui/e2e/mockApi.ts`): every admin endpoint stubbed with stable fixtures,
notifications/scripts/radio/device-profiles as empty lists, a fixed clock
(`FIXED_NOW = 2026-08-14T12:30:00Z`) installed via `addInitScript`, and a seeded-session
helper (`seedPreferences`) that sets localStorage preferences *if absent* so mid-test
locale switching via reload works. `gotoScreen` waits for the `html lang` attribute, a
resolved header health state, and the footer locale marker (≥1056px widths).

- **Accessibility** (`admin-ui/e2e/accessibility.spec.ts`): axe-core 4.13 (bundled via
  axe-playwright 2.2.2) against all 11 canonical screens (`CANONICAL_SCREENS`) with WCAG
  tags `wcag2a`, `wcag2aa`, `wcag21a`, `wcag21aa`, `wcag22aa` (best-practice rules excluded
  intentionally), plus a keyboard smoke test per screen (skip link is the first tab stop,
  Tab reaches the primary action, focus is visible). 2 tests × 11 screens × 3 projects
  (1600×1200, 768×1024, 375×812) = **66 tests**.
- **Visual** (`admin-ui/e2e/visual.spec.ts`): full-page screenshot baselines for 8
  canonical screens × {light, dark, rtl(ar-SA)} at 1600×1200, plus 3 screens at 375×812
  light. `expect.toHaveScreenshot` configured with `maxDiffPixelRatio: 0.01`,
  `animations: 'disabled'`, `caret: 'hide'`. Baselines committed at
  `admin-ui/e2e/visual.spec.ts-snapshots/` (27 PNGs, `*-linux` platform suffix).
- **Localization/RTL** (`admin-ui/e2e/localization.spec.ts`, desktop project): a
  20-culture boot loop asserting per-culture harvest/fallback behavior with console-error
  collection; a ja-JP pass over all 11 canonical screens; and an ar-SA + fa-IR RTL pass
  asserting `dir="rtl"` and a mirrored layout (nav bounding box right of viewport center)
  on all 11 screens. 4 tests.

## 2. Accessibility results

**66/66 passing** across the three canonical viewports. Four real violation classes were
found by the axe runs and fixed at the source:

| # | axe rule | Root cause | Fix |
|---|---|---|---|
| 1 | `listitem` (WCAG 1.3.1) | Carbon `SwitcherItem` renders an `<li>` but sat bare inside `HeaderPanel` | Wrapped language/appearance panel items in `<ul class="melodee-shell__panel-list">` (`AppShell.tsx`, style in `tokens.css`) |
| 2 | `definition-list` / `dlitem` (WCAG 1.3.1) | Release-review facts grid rendered `<dt>`/`<dd>` without a `<dl>` wrapper | `ReleaseReviewScreen.tsx` restructured so the grid element *is* the `<dl>`; full-width facts use `.melodee-review-fact--full` |
| 3 | `scrollable-region-focusable` (WCAG 2.1.1) | `#main-content` scrolls at small viewports but was not keyboard-focusable | New `useScrollRegionFocus()` hook (`src/components/scrollRegions.ts`): MutationObserver on `document.body` (React replaces the `#main-content` DOM node) plus resize and `document.fonts.ready` listeners; called once in `AppShell` |
| 4 | (not axe — e2e flake investigation) | Screen-local state (save notices, editor drafts) wiped on every navigation and shell re-render | See §3 |

### 3. HeaderContainer remount defect (found via W4 flake investigation)

The pre-existing `policies.spec.ts` flake ("list → edit → save replaces the rule at the
same position") was root-caused to Carbon's `HeaderContainer`: it treats its `render` prop
as a **component type** (`jsx(Children, …)`, `PropTypes.elementType`). AppShell passed an
inline arrow closure, so every AppShell re-render — each location change, the initial
health-poll resolution — produced a new component type at that position, forcing React to
**unmount and remount the entire routed screen subtree** (Content → Suspense → Outlet →
screen). Instrumented evidence: `location.key` changed per `setSearchParams` navigation
and the screen's unmount/mount effects fired ~25 ms after each new-key render, destroying
ephemeral state (the success notice was set, then lost to the remount).

Fix: `AppShell.tsx` no longer uses `HeaderContainer` at all — the render callback ignored
both injected props (expansion is shell-owned per the existing code comment), so the
fragment is returned directly. This was an app-wide defect affecting every screen's local
state across every navigation, not merely a test flake. Post-fix verification:
`e2e/policies.spec.ts --repeat-each=6` → 90/90 passed (previously ~3/60 failure rate), and
the save-notice assertion is now stable under parallel load.

## 4. Visual regression inventory

27 committed baselines (`admin-ui/e2e/visual.spec.ts-snapshots/`):

| Screen | light | dark | rtl (ar-SA) | mobile light |
|---|---|---|---|---|
| overview | ✅ | ✅ | ✅ | ✅ |
| processing-queue | ✅ | ✅ | ✅ | ✅ |
| release-review | ✅ | ✅ | ✅ | ✅ |
| artist-detail | ✅ | ✅ | ✅ | — |
| release-detail | ✅ | ✅ | ✅ | — |
| identity-review | ✅ | ✅ | ✅ | — |
| policies | ✅ | ✅ | ✅ | — |
| system-workers | ✅ | ✅ | ✅ | — |

Tolerance: `maxDiffPixelRatio: 0.01` with animations disabled and the caret hidden; the
mock API plus fixed clock make the captures deterministic (stable across 3 back-to-back
full-suite runs). Regenerate deliberately with:

```bash
pnpm test:e2e -- --update-snapshots
```

Baselines carry the `-linux` platform suffix; runners on other platforms must generate
their own baselines (standard Playwright behavior).

## 5. Localization / RTL results

`pnpm i18n:check` coverage at time of recording (1125 en-US keys):

| Locale | Coverage | | Locale | Coverage |
|---|---|---|---|---|
| en-US (source) | 1125/1125 (100%) | | nl-NL | 84/1125 (7.5%) |
| ar-SA (RTL) | 84/1125 (7.5%) | | pl-PL | 83/1125 (7.4%) |
| cs-CZ | 84/1125 (7.5%) | | pt-BR | 84/1125 (7.5%) |
| de-DE | 80/1125 (7.1%) | | ru-RU | 84/1125 (7.5%) |
| es-ES | 81/1125 (7.2%) | | sv-SE | 84/1125 (7.5%) |
| fa-IR (RTL) | 84/1125 (7.5%) | | tr-TR | 84/1125 (7.5%) |
| fr-FR | 84/1125 (7.5%) | | uk-UA | 84/1125 (7.5%) |
| id-ID | 83/1125 (7.4%) | | vi-VN | 83/1125 (7.4%) |
| it-IT | 81/1125 (7.2%) | | zh-CN | 84/1125 (7.5%) |
| ja-JP | 84/1125 (7.5%) | | ko-KR | 84/1125 (7.5%) |

E2E findings (all 4 tests passing, desktop project):

- **20-culture boot loop:** every culture boots via localStorage + reload with zero
  console errors; harvested keys render translated (e.g. `nav.artists` is genuinely
  translated in all 19 non-English locales), unharvested keys (e.g.
  `overview.attention.title`) prove the ADR-0137 en-US fallback end-to-end.
- **ja-JP canonical-screen pass:** all 11 screens render without console errors.
- **RTL mirrored layout:** ar-SA and fa-IR render every canonical screen with
  `dir="rtl"` and the nav bounding box right of the viewport center.

Two genuine i18n boot bugs were found by this suite and fixed in `src/i18n/index.ts`:

1. Boot direction used `i18n.resolvedLanguage` instead of `i18n.language` — a persisted
   ar-SA/fa-IR preference booted LTR until the first interaction. Now
   `applyDocumentDirection(i18n.language || …)`.
2. The `languageChanged` event fires synchronously during `init()` before the bundle-load
   listener attaches, so a persisted non-English locale booted with English strings. Boot
   now calls `ensureLocale(i18n.language)` explicitly.

Unit coverage added in `src/i18n/i18n.test.ts`: `Intl.PluralRules` shapes for fr/ja/ru/ar,
the i18next ru-RU plural bundle, per-locale number grouping (incl. fr-FR narrow no-break
spaces), and datetime shapes. 11/11 i18n tests pass (199 total unit tests).

**Limitations (unchanged from the harvest report):** non-English coverage is the ~84-key
legacy-harvested set (~7.5% of the catalog); several cultures contain
`[NEEDS TRANSLATION] X` placeholder values (e.g. cs-CZ `actions.refresh`); full
translation is a content effort outside this wave. See
[i18n-harvest.md](i18n-harvest.md).

## 6. Gate results

All run in `admin-ui/` unless noted:

| Gate | Result |
|---|---|
| `pnpm check` (tsc --noEmit) | ✅ clean |
| `pnpm lint` | ✅ 0 errors (2 pre-existing react-refresh warnings) |
| `pnpm i18n:check` | ✅ structure OK |
| `pnpm test` (vitest) | ✅ 199/199 |
| `pnpm build` | ✅ built |
| `pnpm test:e2e` ×3 back-to-back | ✅ 235 passed + 62 skipped, zero failures, each run |
| `python3 scripts/validate_repository.py` (repo root) | ✅ passed |

## 7. Remaining risks

- Visual baselines are desktop/mobile Chrome-only; no cross-browser matrix yet.
- axe best-practice rules are intentionally excluded; WCAG 2.2 A/AA is the gate.
- Screenshot tolerance (1% of pixels) would not catch a subtle single-token color drift;
  deliberate visual changes still require regenerating and reviewing baselines in the PR.

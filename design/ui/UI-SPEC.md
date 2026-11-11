# Melodee UI Specification

Status: **Accepted for Admin UI implementation (2026-08-14). Amended 2026-08-15** by ADR-0184
(reference alignment and Melodee surface system): §1 token inventory, §2 override scope, §3 shell
dimensions and status placement, §6 component contracts, and the new §6.1 primitive layer.
Satisfies PRD-UI-REF-003. Authoritative for implemented behavior alongside `DESIGN-SYSTEM.md`;
mockups remain visual intent only (PRD-UI-REF-001). Governing ADRs: ADR-0109 (React + TypeScript +
Carbon), ADR-0137 (localization/RTL), ADR-0138 (tooling/serving), ADR-0139 (session/gating),
ADR-0184 (surface system).

## 1. Design tokens

Carbon tokens (`@carbon/react` themes) are the default. The **Melodee token layer** is the set of CSS
custom properties layered on Carbon themes (`src/theme/tokens.css`), never raw hex values in
components:

| Token family | Purpose |
|---|---|
| `--melodee-status-success/-info/-warning/-error/-provisional/-neutral` | Status semantics (§5) |
| `--melodee-artwork-surface` | Contrasting surface behind artwork |
| `--melodee-density-compact/-default` | Row density overrides for data-heavy tables |
| `--melodee-brand-accent`, `--melodee-link-contrast` | Brand accent (links, primary nav active state) |
| `--melodee-surface`, `--melodee-surface-border`, `--melodee-surface-border-strong`, `--melodee-surface-muted` | Card and table surfaces (ADR-0184) |
| `--melodee-radius-card/-control/-pill` | Corner radii |
| `--melodee-space-1..8`, `--melodee-grid-gap` | Spacing scale |
| `--melodee-font-page-title/-card-title/-label/-metric/-micro` | Typography scale |
| `--melodee-chip-{success,info,warning,error,provisional,neutral}-{surface,ink}` | Status chip tints (§5) |
| `--melodee-progress-fill`, `--melodee-progress-track` | Progress bars |
| `--melodee-shell-background/-active/-text/-muted` | Navigation chrome |

A theme pack defines values for both light and dark Carbon themes (g10/g100); appearance mode and
theme pack are independent axes (PRD-THEME-002). High-contrast and reduced-motion media queries are
respected and never overridden.

## 2. Carbon token overrides

Overrides are limited to the token layer above. Border radius comes from
`--melodee-radius-*` rather than Carbon's default 0; density uses Carbon's compact rows where
listed in §4.

**Component CSS is permitted only through the §6.1 primitive layer.** A screen may not carry its
own color, radius, shadow, spacing, or typography declarations; it composes primitives and layout
containers, and every value those emit originates from a custom property in §1 (ADR-0184). Carbon
component internals are never forked — where a reference treatment has no Carbon equivalent (filter
pills, status chips, progress bars), Melodee owns a primitive built on Carbon tokens.

## 3. App shell dimensions

- Left navigation: 256 px expanded, 48 px collapsed (Carbon UI shell side nav), persistent.
- Header: 48 px (Carbon UI shell header) containing one inset global search entry (`⌘K`/`Ctrl+K`),
  a compact server status indicator, notifications, language selector, appearance toggle, and user
  menu. The language control shows the current locale name at ≥ lg; the user control shows avatar
  and username at ≥ lg (ADR-0184).
- Brand block: monogram tile + `melodee` wordmark + `ADMIN` micro-label; the collapsed rail shows
  the monogram only.
- Content region: max width none (data-heavy), **24 px padding at ≥ lg** (was 16 px), 16 px below.
- Inspector panel: 400 px right rail rendered as a `Surface` card (dismissible; content reflows).
- Footer strip: status line inside the side nav (`Server online`, locale, appearance, collapse).
  The footer is the canonical server-status location; the header carries a compact mirror.
- Mockup proportions of 220 px navigation and 72 px header are directional only; the dimensions
  above are binding.

## 4. Density

- Default: Carbon default density.
- Queue/triage tables (Processing Queue, Review Queue, Quarantine, worker pools): compact density
  via Carbon's compact DataTable size; user-switchable density control per table, persisted in the
  URL/view state.
- Virtualization is an optimization only; server-side paging is the correctness mechanism
  (PRD-UI-010).

## 5. Status semantics (binding)

Status is **never color-only**: label + icon + accessible text always.

| Meaning | Carbon/tag treatment | Token |
|---|---|---|
| Complete, healthy, accepted | Green tag / success icon | `--melodee-status-success` |
| Active, processing, indexing | Blue tag / in-progress icon | `--melodee-status-info` |
| Waiting, review, uncertain | Amber/warning tag | `--melodee-status-warning` |
| Failed, rejected, danger | Red tag / error icon | `--melodee-status-error` |
| Provisional, identity work | Purple (distinct secondary accent) | `--melodee-status-provisional` |
| Disabled, ignored | Gray/neutral tag | `--melodee-status-neutral` |

Invariants: classification ≠ failure; disposition ≠ classification; quarantine is distinct from
failed processing and ignored content.

## 6. Shared components

The §40 PRD component set is implemented under `admin-ui/src/components/` and is the only
sanctioned way to render their concepts: `AppShell, PageHeader, MetricCard, StatusTag,
EntityAvatar, ArtistLink, ReleaseLink, TrackLink, ProcessingStateTag, EvidenceItem,
EvidenceSummary, DecisionPanel, InspectorPanel, EntityTable, EmptyState, HealthIndicator,
AuditTimeline, AudioPreview, DestructiveActionDialog`. Contracts:

- `EntityTable`: server-side sort/filter/page only; URL-reflected state; compact density option;
  accessible caption + sorted-column announcement.
- `StatusTag`/`ProcessingStateTag`: enforce §5 (icon + label + tooltip/description).
- `DestructiveActionDialog`: requires affected-counts, storage impact, reason text, matched
  rule, quarantine/ignore alternative display, and explicit typed confirmation for delete-class
  actions (PRD-UI-008).
- `AudioPreview`: labeled transport controls, no autoplay, keyboard operable.
- `DecisionPanel`: ordered choices with the recommended action visually primary; destructive
  choices are never visually equal to safe ones.

### 6.1 Melodee surface and primitive layer (ADR-0184)

Screens compose these primitives; they do not hand-author surfaces. Each has a Vitest contract
test and is exported from `components/index.ts`.

| Primitive | Contract |
|---|---|
| `Surface` | The one card treatment: `title`, `subtitle`, `actions`, `footer`, `density`, `tone`. Provides the heading level and the plain/inset variants. |
| `PageHeader` | Page title, subtitle, inline-end action region, optional breadcrumb line. |
| `HeroCard` | Entity hero: media slot, title, subtitle, `chips`, `facts`, `actions`. One hero per screen. |
| `MetricCard` | KPI: `label`, `value`, `delta` (text + tone), `status`, `hint`. |
| `StatStrip` | Row of numeral+caption cells (detail screens). |
| `StatusChip` | Reference chip tint; icon + label + visually-hidden description (§5). |
| `ProgressBar` | `value` 0–100 with textual value, `stage` label, `<progress>` semantics. |
| `FilterPills` | Real buttons with `aria-pressed`, arrow-key navigation, count badges. |
| `KeyValueList` | Term/value/derived-detail pairs; replaces per-screen fact grids. |
| `EvidenceTileGrid` | Compact evidence counts with tone. |
| `TimelineList` | Ordered audit/decision events with tone markers. |
| `DecisionActionList` | Ordered actions, exactly one primary, destructive tinted, each with a reason line. |
| `TabNav` | Underlined tab bar, `role="tablist"`, arrow-key navigation, RTL-safe. |
| `EntityTable` | As above, plus column presets (`progress`, `chip`, `stacked`, `amount`). |

Rules: exactly one primary action per screen region; no color-only meaning; no raw hex,
radius, shadow, or font-size declaration outside the token layer; layout-only rules live in
`screens.css` and may not set color or type.

## 7. Interaction patterns

- **Queue + inspector**: selection opens the right inspector without losing queue position or
  filters; `Enter` opens full detail; bulk selection swaps the toolbar for explicit bulk actions.
- **Review workspace**: evidence first, choices last; original observations always shown alongside
  resolved values.
- **Command palette** (`⌘K`): power-user navigation to screens/entities; accessible, and never the
  only path to a feature (PRD-UI-009).
- **Notifications/toasts**: Carbon toast notifications with accessible roles; destructive outcomes
  link to the audit record.
- **URLs**: meaningful filter/sort/page state is shareable via query parameters.

## 8. Responsive behavior (breakpoints)

Carbon grid breakpoints, binding for all screens:

| Name | Width | Behavior |
|---|---|---|
| `sm` | 320–671 px | Single column; side nav overlays; inspector becomes full-width drawer; tables switch to stacked cards for ≤ 4-column datasets or horizontal scroll otherwise |
| `md` | 672–1055 px | Side nav collapsed to icons by default; inspector overlays content |
| `lg` | 1056–1311 px | Full shell; inspector docks at 400 px |
| `xl`/`max` | ≥ 1312 px | Full shell; mockup layouts (1600×1200) target this band |

Every canonical screen is verified at 375×812 (mobile), 768×1024 (tablet), and 1600×1200
(desktop) in the visual-regression suite.

## 9. Accessibility

- Keyboard-complete navigation and actions; visible focus; logical focus order.
- Tables: accessible captions, sort announcements, row-selection labels.
- Dialogs/toasts/progress: Carbon accessible patterns; no hover-only information.
- Artwork: meaningful alt text or marked decorative.
- Audio preview: labeled controls, no autoplay.
- Target: WCAG 2.2 AA; verified with axe-core checks in the e2e suite on every canonical screen.

## 10. Agent-specific UI rules

1. No component library other than Carbon without an ADR (TanStack Table only via proven-gap ADR
   per ADR-0109).
2. No colors/spacing/radii/shadows outside §1–§2 tokens.
3. No per-page navigation models; the shell IA is fixed (PRD-UI-004).
4. No emoji as icons (Carbon icons only).
5. No catalog-scale client-side pagination (PRD-UI-010).
6. Never expose PostgreSQL/filesystem internals as product concepts (PRD-UI-005); the Release
   lifecycle is `Discovered → Processing → Review → Cataloged`.
7. Admin UI language is `Release`/`Track`, never `Album`/`Song` (AGENTS.md product language).
8. Destructive actions are never visually equal to normal actions (PRD-UI-008).
9. All visible text goes through the i18n layer (ADR-0137); no string concatenation of translated
   fragments; logical CSS properties only (RTL).
10. Do not redesign canonical workflows without approval; mockup text is not data (PRD-UI-REF-001).
11. New screens compose §6.1 primitives; a screen that declares color, radius, shadow, or font
    size directly is a review failure (ADR-0184).

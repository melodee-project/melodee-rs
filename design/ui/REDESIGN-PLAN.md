# Admin UI Redesign and Refactor Plan

Status: **Proposed (2026-08-15).** Requires operator approval before implementation because
`design/ui/UI-SPEC.md` §10.10 forbids redesigning canonical workflows without approval, and the
work below amends UI-SPEC §1–§3, regenerates the PRD-UI-REF-002 visual baselines, and adds additive
admin-API fields.

Scope owner: Admin UI (`admin-ui/`), with additive field work in `apps/melodee-server/src/`.
Reference: `design/ui/reference/01..08-*.png` (visual intent per PRD-UI-REF-001).
Governing authority: `design/adr/` → `SPEC.md` → `PRD.md` (PRD-UI-001..010, PRD-UI-REF-001..003) →
this plan.

---

## 1. How this review was performed

1. Read the eight canonical mockups in `design/ui/reference/` and sampled their rendered geometry
   and colors (section 3) rather than eyeballing them.
2. Verified the committed visual baselines are the *true* current UI, not stale: the whole
   Playwright suite was executed against the working tree at `ce2384f`
   (`pnpm test:e2e` → **307 passed, 62 skipped**), including all 27
   `admin-ui/e2e/visual.spec.ts-snapshots/*` baselines. The screenshots in section 4 are therefore
   authenticated current-state evidence.
3. Audited the implementing source: `admin-ui/src/shell/AppShell.tsx`,
   `admin-ui/src/theme/tokens.css` (1 131 lines), the 21 shared components, 40+ screens, and the
   admin DTOs in `apps/melodee-server/src/admin*.rs` to determine which reference content is
   renderable from real data and which is not.

## 2. Verdict

The Admin UI is a **functionally complete, accessible, localized, server-paged Carbon application**
that renders the domain model honestly. It is **not the designed product** in
`design/ui/reference/`. The gap is compositional, not cosmetic:

| Area | Assessment |
|---|---|
| Shell / brand | Different visual language. Reference: monogram tile + `melodee` wordmark + `ADMIN` subtitle, 72 px header with a single inset search field, text language selector, avatar + username, and a status strip in the nav footer. Current: SVG spectrum wordmark, 48 px header with 5 icon-only actions plus a green health pill in the header, 9 nav items with different icon vocabulary. |
| Overview | Reference answers "what needs attention" with a *two-series line chart*, attention rows with severity icon + colored count chips, and a *release-level* queue table. Current renders a 3-bar activity chart, plain link rows, and a state/count summary table. |
| Processing queue | Reference: filter *pills* above a card containing search + Filters/Columns/Density + Bulk actions, then a dense table with Release/Artist/State/Progress/Size and a separate inspector *card*. Current: Carbon `ContentSwitcher` with a black active segment, a table titled "Processing queue work items" with raw UUID sublines, and a bordered (not carded) inspector rail. |
| Release review | Reference: hero card with artwork + fact strip + `NEEDS REVIEW` chip, then Classification & Evidence / Catalog Comparison / Track Breakdown / Decision Actions / Policy Match / Review Timeline as discrete cards. Current: stacked `melodee-review-card` blocks with flat typography, decision choices rendered as outlined buttons, and a timeline table. |
| Identity review | Reference: three-pane workspace — carded observation queue with count chips, candidate cards with a green "best" treatment and large confidence numerals, evidence-summary tile grid, red-tinted "Never Match Candidate". Current: table-based queue, plain panels, no candidate card treatment, no evidence tiles. |
| Artist detail | Reference: 112 px avatar hero with `VERIFIED` chip and two-column facts, five blue-numeral stat cards, underlined tab bar, then Biography+Aliases / Top Releases / Relationships columns. Current: small hero, four plain stat cards, tabs whose first body is a raw aliases table only. |
| Release detail | Reference: artwork hero with Play sample / Open storage and a Tracks·Duration·Format·Catalog# strip, tabs, Tracks table + Release Information and Media Files rail. Current: the metadata-transform form appears **above** the entity hero, and Overview is a right-aligned key/value list. |
| Policies | Reference: rule list card (Policy/Type/Priority/Matches/Status) beside a full editor card with condition rows, action rows, "Test against catalog", and a match-count chip. Current: different column set (Disposition/Matches), editor is a plain panel, condition builder is a 4-column grid without the reference's inline operators. |
| System / Workers | Reference: five KPI cards, Worker Pools table with Avg time + Throughput, System Health rail, Recent Failures, and a Diagnostics action list. Current: four KPI cards, pools table, health rail, recent failures, leases, search index, and diagnostics links spread across three stacked panels. |
| Data availability | **Six reference elements cannot be rendered from current DTOs** — see §6. Fabricating them is prohibited (AGENTS.md, UI-SPEC §5), so §7 schedules additive backend fields. |

Root cause: the reference mockups were treated as *information-hierarchy* guidance only
(PRD-UI-REF-001), so each screen was built as the minimum Carbon composition that exposes the DTO.
No shared *surface* vocabulary (card, hero, stat strip, filter pill, progress cell, evidence tile,
timeline) was ever built, so `melodee-panel` + Carbon defaults became the whole visual system.

## 3. Measured reference facts (sampled from the PNGs)

These are the values the redesign targets. Mockups are not pixel specifications (PRD-UI-REF-001);
proportional relationships and the palette are binding, absolute sizes are directional where they
conflict with UI-SPEC §3.

### 3.1 Surfaces and color

| Element | Sampled value | Current token/behavior | Delta |
|---|---|---|---|
| Page background | `#f4f4f4` | `--cds-background` (g10 `#f4f4f4`) | none |
| Card surface | `#ffffff` | `--cds-layer` | none |
| Card border | `#eaeaea`–`#eeeeee` | `--cds-border-subtle` `#e0e0e0` | **too dark**; add `--melodee-surface-border` |
| Card shadow | none (flat 1 px border) | none | keep flat; no shadow debt |
| Card radius | ~8 px | `--melodee-surface-radius: 0.5rem` (8 px) | none |
| Primary text | `#161616` | `--cds-text-primary` | none |
| Secondary/label text | `#525252` | `--cds-text-secondary` | none |
| Positive/delta text | `#0f62fe` | `--cds-link-primary` | use accent, not success |
| Nav background | `#07131f` | `--melodee-shell-background: #091117` | **shift** to `#07131f` |
| Nav active row | `#102334` + 3 px accent rail | `--melodee-shell-active: #173044` | **lighter than reference** |
| Nav group label | `#6f8193`, 11 px, tracked caps | `--melodee-shell-muted: #a8b7c5` | **too bright** |
| Table header row | `#f4f4f4` | Carbon data-table header | retint |
| Row divider | `#eaeaea`–`#f6f6f6` | `--cds-border-subtle` | **too dark** |
| Warning chip (REVIEW / attention) | fill `#fff2e8`, ink `#ff832b` | `color-mix(…warning 25%…)`, Carbon `Tag` | **replace tint model** |
| Selected filter pill | fill `#edf5ff`, ink `#0f62fe` | Carbon `ContentSwitcher` selected = `#161616` fill | **replace component treatment** |
| Progress fill / track | `#0f62fe` / `#d9d9d9` | no progress primitive exists | **new token pair** |

### 3.2 Geometry (mockup pixels at 1600×1200)

| Element | Reference | UI-SPEC §3 | Resolution |
|---|---|---|---|
| Nav width | 220 px | 256 px | keep **256 px** (spec wins; mockup is proportionally narrower) |
| Header height | 72 px | 48 px | keep **48 px** shell header; adopt the mockup's *composition*, not its height |
| Content gutter | 28 px | 16 px at ≥ lg | adopt **24 px**, amend UI-SPEC §3 |
| Card gap | 20 px | — | adopt **16–20 px** via `--melodee-grid-gap` |
| KPI card | 200×124 px | — | target ≈124 px min-height, 6-up at xl |
| Page title | ~32 px/700 | 2 rem/600 | adopt **2 rem/700, tighter tracking** |
| Dense row height | ~56 px | compact density | adopt 48–56 px, keep user density control |

## 4. Evidence: current state (baselines verified at HEAD)

| Screen | Current baseline | Reference |
|---|---|---|
| Overview | `e2e/visual.spec.ts-snapshots/overview-light-desktop-1600x1200-linux.png` | `01-overview-dashboard.png` |
| Processing queue | `processing-queue-light-desktop-1600x1200-linux.png` | `02-processing-queue.png` |
| Release review | `release-review-light-desktop-1600x1200-linux.png` | `03-release-review.png` |
| Identity review | `identity-review-light-desktop-1600x1200-linux.png` | `04-artist-identity-review.png` |
| Artist detail | `artist-detail-light-desktop-1600x1200-linux.png` | `05-artist-detail.png` |
| Release detail | `release-detail-light-desktop-1600x1200-linux.png` | `06-release-detail.png` |
| Policies | `policies-light-desktop-1600x1200-linux.png` | `07-policies-rule-editor.png` |
| System workers | `system-workers-light-desktop-1600x1200-linux.png` | `08-system-workers-diagnostics.png` |

## 5. Decisions required before coding

| # | Decision | Recommendation | Blocking? |
|---|---|---|---|
| D1 | **Navigation IA.** The mockups show `MANAGEMENT { Policies, Identity }` and `SYSTEM { Workers, Search, Storage, Users, Settings }` — no `History`, no `Diagnostics`, no `Users & Access`. PRD-UI-004 (authority 3) specifies `POLICIES { Release Rules, Matching Rules }`, `CATALOG { …, Identity Review }`, `SYSTEM { …, Diagnostics }`. | Keep **PRD-UI-004 IA** verbatim (mockup text may not override product architecture, PRD-UI-REF-001). Restyle only: group labels, icon vocabulary, active treatment, spacing. Record the mockup deviation in UI-SPEC §3. | No — resolved by authority |
| D2 | **Server status placement.** UI-SPEC §3 puts the status indicator in the header; mockups put "Server online" only in the nav footer. | Keep both (header chip + footer strip) and amend UI-SPEC §3 to say the footer is canonical and the header chip is a compact mirror. | No |
| D3 | **Shell dimensions** follow §3.2. | Adopt 24 px gutter, reference palette, keep 256/48. Amend UI-SPEC §3. | No |
| D4 | **Missing data.** Priority, per-item progress %, per-item byte size, Release/Artist on queue rows, KPI deltas, a second activity series, worker "Avg time", storage-warning count. | Implement §7 additive fields; **omit** any element whose data cannot be produced and declare the omission in `design/ui/DESIGN-SYSTEM.md` + the screen's docs. Never render invented values. | **Yes** for the queue and overview screens |
| D5 | **"Progress" semantics.** The ingestion state machine is discrete; the mockup shows 72 %/45 %. | Render **stage progress** derived from a single declared state→stage table (a documented deterministic mapping, same class as `ProcessingStateTag`), never a fabricated elapsed-time estimate. | No |
| D6 | **Tab set per detail screen.** Artist: Overview/Releases/Appearances/Contributions/Relationships/Identity/Images/History; Release: Overview/Tracks/Artists/Contributors/Files/Classification/Provenance/History. | Adopt exactly the existing tab order (already spec-aligned); the reference's `Images` tab maps to the existing artwork manager. | No |
| D7 | **Order of hero vs. task panels** on Release Detail (mockup puts the entity hero first, the transform panel is a task). | Hero first; metadata transforms move into the Overview tab body below the fact strip. | No |
| D8 | **ADR.** This program changes visual language, adds a component contract layer, and adds admin-API fields. | Create `ADR-0184-admin-ui-reference-alignment-and-surface-system.md` (Proposed → Accepted) with supersede notes for any UI-SPEC clause changed. | **Yes** |

## 6. Data-gap register (what the mockups show but the API cannot answer)

| Reference element | Screen | Available today | Plan |
|---|---|---|---|
| `+12 today`, `8 active now`, `5 new`, `+38 this week` | Overview KPIs | `workItems.completedLast24h`, `failedLast24h`, `countsByState`; **no catalog deltas** | add `deltas` block to `DashboardDto` (§7.1); drop catalog week-deltas if the cheap query is not acceptable |
| Two-series activity line chart (`Releases processed`, `Tracks indexed`) | Overview | one series `activity.completedPerHour` | add `indexedPerHour` (§7.1) |
| `Storage warning 1` attention row | Overview | `health.storage.status === 'degraded'` (no count) | render severity row without a count |
| Queue `Release`, `Artist` | Processing queue | `WorkItemDto` has paths only; `catalog_imports` joins to `releases` (already used by the review queue) | add `releaseTitle`/`artistName` to the list DTO (§7.2) |
| Queue `Priority` | Processing queue | not modeled | **omit**; replace with `Attempts` (already present) — record in DESIGN-SYSTEM |
| Queue `Progress` | Processing queue | state machine only | client-side stage mapping (D5) |
| Queue `Size` | Processing queue | `MediaAssetDto.byteSize` exists only after import | add `mediaBytes` when bound, else honest "—" (§7.2) |
| Worker `Avg time` | System workers | `oldestQueuedAgeSeconds`, `throughputPerMinute`; no mean processing time | add `avgProcessingSeconds` (§7.3) or relabel column `Oldest queued` |
| `Uptime 12d 4h` | System workers | `GET /api/admin/v1/diagnostics` `uptimeSeconds` | reuse existing endpoint (no backend work) |
| Artist `Members` | Artist detail | `memberships` section total | reuse (no backend work) |

## 7. Workstreams

Sequencing is deliberate: tokens → shell → shared components → screens. Each later phase relies on
the earlier phase's primitives, and every phase leaves the tree green (`make check-fast` +
`pnpm test:e2e` for carried-over assertions).

### W0 — Decisions and guardrails

- Accept D1–D7 in the PR description; draft and accept `ADR-0184`.
- Amend `design/ui/UI-SPEC.md` (§1 tokens, §2 overrides, §3 shell dimensions + status placement,
  §4 density, §6 component contracts) and `design/ui/DESIGN-SYSTEM.md` (surface system, omitted
  mockup affordances and why).
- Split `admin-ui/src/theme/tokens.css` (1 131 lines, one file) into:
  `tokens.css` (color/space/radius/typography custom properties),
  `surfaces.css` (card, panel, hero, stat strip, table skin, rails),
  `shell.css` (brand/header/nav/footer),
  `screens.css` (layout-only rules, no colors).
  Import order in `src/theme/index.scss`. **No visual change in this phase** — pure move.

Acceptance: `git diff` shows only file moves; all 27 baselines unchanged; `make check-fast` green.

### W1 — Token and surface layer

Add to the token layer (both `g10` and `g100`, plus high-contrast overrides):

```css
--melodee-surface: var(--cds-layer);
--melodee-surface-border: #eaeaea;          /* dark: #393939 */
--melodee-surface-border-strong: #d9d9d9;
--melodee-surface-muted: #f4f4f4;           /* table header, inset strips */
--melodee-radius-card: 0.5rem;
--melodee-radius-pill: 999px;
--melodee-radius-control: 0.25rem;
--melodee-space-1: 0.25rem; … --melodee-space-8: 2rem;
--melodee-grid-gap: 1.25rem;
--melodee-shell-background: #07131f;
--melodee-shell-active: #102334;
--melodee-shell-muted: #6f8193;
--melodee-progress-track: #d9d9d9;
--melodee-progress-fill: var(--cds-link-primary);
--melodee-chip-warning-surface: #fff2e8;
--melodee-chip-warning-ink: #ff832b;
--melodee-chip-info-surface: #edf5ff;
--melodee-chip-info-ink: #0f62fe;
```

Plus a typography scale (`--melodee-font-page-title`, `--melodee-font-card-title`,
`--melodee-font-label`, `--melodee-font-metric`, `--melodee-font-micro`) so screens stop setting
`font-size` inline.

Acceptance: token parity test (`src/theme/packs.test.ts` extended) proves every new token exists in
light, dark, and high-contrast; no component still consumes a raw hex; `prefers-contrast` /
`prefers-reduced-motion` still authoritative.

### W2 — Shell alignment

`admin-ui/src/shell/AppShell.tsx` + `shell.css`:

1. **Brand block.** Replace the spectrum wordmark with the reference composition: 32 px rounded
   monogram tile, `melodee` wordmark, `ADMIN` 10 px tracked subtitle. Collapsed rail shows the tile
   only. Keep the existing `<img>` assets as the source (no new binary unless assets are missing);
   if a monogram asset is absent, author `public/brand/melodee-mark-tile.svg` (SVG only, no raster).
2. **Header.** One inset search field (`#f8f8f8` fill, 1 px `#eaeaea` border, 36 px, `⌘K` hint on
   the trailing edge) replacing the current bordered command button; right cluster becomes
   `globe + "English"` · notifications · appearance icon · `avatar + username` (username text at
   ≥ lg only). Keep every action's accessible name and the `HeaderPanel` switchers.
3. **Nav.** Keep PRD-UI-004 IA. Restyle: 15 px labels, 2 px inset accent rail on the active row,
   `#102334` active fill, 11 px tracked group labels in `#6f8193`, tighter vertical rhythm.
   Replace the icon set with reference-matching Carbon glyphs (one glyph per item, see §8).
4. **Footer strip.** `● Server online` left, locale + appearance right, collapse control; header
   health chip stays as a compact mirror (D2).
5. **Mobile.** Keep the off-canvas rail; the reference has no mobile mockup, so UI-SPEC §8 governs.

Acceptance: `smoke.spec.ts`, `localization.spec.ts`, `theme-packs.spec.ts` green; keyboard order
unchanged (skip link → menu → search → nav → content); axe clean at 375/768/1600.

### W3 — Shared component library pass

Every one of these replaces hand-rolled markup in multiple screens, so they come before the screen
work. New or reworked modules under `admin-ui/src/components/` (each exported through `index.ts`,
each with a Vitest sibling):

| Component | Contract |
|---|---|
| `Surface` / `SectionCard` (replaces raw `melodee-panel` usage) | `title?, subtitle?, actions?, footer?, density?, as?, children` — one card treatment, one heading level policy |
| `MetricCard` (rework) | `label, value, delta?: {text, tone}, status?, hint?` — reference layout: 13 px label, 32 px numeral, accent delta line |
| `StatStrip` | centered numeral + caption cells (Artist detail, Release detail) |
| `HeroCard` | `media, title, subtitle, chips, facts[], actions` — one hero for Artist, Release, Review, Identity |
| `StatusChip` (rework of `StatusTag`) | reference chip tint model; keeps icon-in-label + visually-hidden description (UI-SPEC §5 non-negotiable) |
| `ProgressBar` | `value: 0..100, label, stage` — value announced as text, `<progress>` semantics |
| `FilterPills` (replaces `ContentSwitcher` in queues) | `items[{key,label,count}], value, onChange` — real buttons with `aria-pressed`, pill skin |
| `KeyValueList` | `items[{term, value, detail?}]` — replaces the 36 `melodee-fact` + 35 `melodee-review-fact` usages |
| `EvidenceTileGrid` | `tiles[{label, value, tone}]` |
| `TimelineList` | `events[{at, title, detail, tone}]` |
| `DecisionActionList` | ordered actions with one primary, one destructive tinted variant, and reasons (replaces outlined-button stack) |
| `TabNav` | underline tab bar with `role="tablist"`, sticky-safe, RTL-safe |
| `DataTableSkin` (extends `EntityTable`) | header retint, row divider, density, column presets (`progress`, `amount`, `chip`, `stacked-primary`) |

Acceptance: component unit tests cover status semantics, Pills keyboard model, ProgressBar
`aria-valuenow` text, and TabNav arrow-key navigation; `grep` shows no raw `melodee-fact`,
`melodee-panel`, or `<ContentSwitcher>` left in screens.

### W4 — Canonical screen redesigns

Each screen is its own reviewable change with its own acceptance criteria and baseline update.

#### W4.1 Overview (`screens/overview/`)
- 6 KPI cards: label / numeral / tone-colored delta (`Inbound +12 today`, `Processing 8 active now`,
  `Needs Review 5 new`, catalog counts with week deltas *only if* W6.1 lands).
- Activity card: **two-series SVG line chart** (Releases processed, Tracks indexed) with gridlines,
  legend, `role="img"` label, and the existing visually-hidden data table for both series.
  Geometry lives in `overview.ts` (`layoutLines`) and unit-tests without a DOM.
- Needs attention card: rows = severity icon + label link + count chip tinted by severity.
- System health card: dot + name + right-aligned verdict + measured value (`12 ms`, `1.8M docs`,
  `4.83 TB`) in a 3-column grid; `disabled` stays neutral.
- Current queue card: Release / Artist / State / Progress / Added columns from W6.2, one row per
  active item, `Open the processing queue` footer link; falls back to the state-count table when
  the enriched endpoint is unavailable.

Acceptance: `overview.spec.ts` extended for two series + deltas + queue rows; chart has a text
alternative; axe clean; mobile stacks KPI cards 2-up (not 1-up at 118 px).

#### W4.2 Processing queue (`screens/processing/`)
- Row 1: filter pills (`All 1,248`, per-state counts) — `FilterPills`, counts from
  `getWorkItemCounts`.
- Card toolbar: search box (server-side `query`-equivalent is not available for work items — search
  filters the current page **or** is deferred; decide in W6.2), `Filters`, `Columns`, `Density`, and
  a right-aligned `Bulk actions` primary button shown only with a selection.
- Table: `Release` (title + path subtitle), `Artist`, `State` chip, `Progress` (stage bar + %),
  `Added` (relative + title), `Size`; checkbox column; priority omitted (D4).
- Inspector: convert to a `Surface` card containing artwork/title/artist/state chip, a
  `KeyValueList` (state, progress, source, tracks, duration, format, fingerprint), a `Next action`
  block, and primary/secondary buttons.

Acceptance: `processing-queue.spec.ts` asserts pill counts, selection→bulk-bar swap, inspector
fields, and keyboard row activation; URL state unchanged (`state`, `sort`, `page`, `pageSize`,
`item`).

#### W4.3 Release review (`screens/review/ReleaseReviewScreen.tsx`)
- Hero card: artwork, `Artist — Release`, `NEEDS REVIEW` chip, Tracks/Year/Source/Traits facts.
- Body cards in reference order: Classification & Evidence (evidence rows with confidence chips),
  Catalog Comparison (labeled bars: Known / New / Alternate / Uncertain with counts and %),
  Track Breakdown (Track, Existing Match, Result chip, Duration, Preview).
- Rail cards: Decision Actions (`Accept Release` primary → `Quarantine` → `Ignore` → `Delete`
  tinted destructive, each with its reason line), Policy Match (rule chip, recommendation,
  reason), Review Timeline (`TimelineList`).
- Keep `DestructiveActionDialog` for Delete (PRD-UI-008) — the tinted button never bypasses it.

#### W4.4 Identity review (`screens/identity/IdentityReviewScreen.tsx`)
- Three carded panes: observation queue (avatar + name + release + count chip, selected row tinted),
  observation detail (`Incoming observation`, candidate cards), actions rail.
- Candidate cards: best candidate gets the success-tinted treatment, confidence rendered as a large
  numeral in the candidate's tone, facts row (country / years / releases) from evidence.
- `Create New Artist` as a dashed/bordered "add" card; evidence summary as `EvidenceTileGrid`
  (Alias/Year/Release/External/Negative counts derived from `evidence` groups).
- Actions rail: `Confirm Match` (primary), `Create New Artist`, `Leave Provisional`,
  `Never Match Candidate` (error-tinted).

#### W4.5 Artist detail (`screens/catalog/ArtistDetailScreen.tsx`)
- Hero: 112 px avatar, name, disambiguation, `VERIFIED` chip, 2-column fact grid
  (Type/Formed/Origin/Genres/External IDs).
- `StatStrip`: Releases, Tracks, Members, Appearances, Aliases (Members from `memberships` total).
- `TabNav` with the reference order; Overview body = Biography + Aliases (chips with type captions) /
  Top Releases (artwork thumb + title + year + track count) / Relationships & Members (avatar, name,
  role, years). All existing tabs keep their tables.

#### W4.6 Release detail (`screens/catalog/ReleaseDetailScreen.tsx`)
- Hero: 176 px artwork, title, `Artist · Year · Type`, `CATALOGED` chip, `Play sample` (existing
  `AudioPreview`) + `Open storage`, and a fact strip (Tracks / Duration / Format / Catalog #).
- Tabs unchanged; Overview = Tracks table (left) + Release Information and Media Files rail (file
  name, size, format chip from `mediaAssets`).
- Metadata transforms move into a collapsed `Surface` under the Overview fact strip (D7).

#### W4.7 Policies (`screens/policies/`)
- Left card: search + `Create Policy` + table (Policy / Type / Priority / Matches / Status) with the
  selected row tinted and the reference's `Enabled`/`Disabled` chip.
- Right card: editor with `Name`, `Description`, `Conditions` rows (`field`, operator, value,
  remove), `+ Add condition`, `Actions` (Disposition / Record decision / Allow override),
  `Test against catalog` with the match-count chip, and `Cancel` / `Save Policy`.
- Existing policy-engine semantics and destinations are unchanged — this is presentation plus the
  condition-row ergonomics already implemented.

#### W4.8 System / Workers (`screens/system/SystemWorkersScreen.tsx`)
- 5 KPI cards (Workers, Queued, Completed 24 h, Failed 24 h, Uptime from the diagnostics endpoint).
- Worker Pools table: Pool / Workers / Running / Queued / Avg time / Throughput / Status / Actions.
- System Health rail (`SystemHealthPanel` re-skinned to the 3-column row layout).
- Recent Failures card (worker id, message, time).
- Diagnostics card: four action rows (Run system check, View structured logs, Performance metrics,
  Database metrics) wired to the existing endpoints; `Active Leases` and `Search Index` stay as
  cards below.

### W5 — Secondary screen consistency pass

Same shell/surface vocabulary applied to `ReviewQueueScreen`, `HistoryScreen`, `QuarantineScreen`,
`CatalogListScreen` (+ Artists/Releases/Tracks), `TrackDetailScreen`, `StorageScreen`,
`SearchScreen`/`SearchPanel`, `UsersScreen`/`GroupsPanel`, `SettingsScreen` (+ sections),
`DiagnosticsScreen`, `LeasesPanel`, `OnboardingWizard`, `LoginScreen`, and `ErrorState` 404.
No layout invention — each reuses `Surface`, `HeroCard`, `KeyValueList`, `TabNav`, `EntityTable`.

### W6 — Backend enablers (additive, admin API only)

The admin API is Rust-native and not a frozen compatibility surface (see
`admin-ui/src/api/types.ts` header comment), but each change still needs: DTO docs in
`docs/src/content/docs/api/admin-api.md`, an integration test, and a UI consumption test.
No existing field is removed or retyped.

1. **`DashboardDto`** — add `deltas { inboundToday, newForReview, catalogAdditions7d { artists,
   releases, tracks } }` and `activity.indexedPerHour: [{hour, count}]`. Source: bounded counts over
   `created_at`/`completed_at`; single set-based query each, no per-row loops.
2. **Work-item list DTO** — add `releaseTitle`, `artistName`, `mediaBytes` via
   `release_work_items LEFT JOIN catalog_imports LEFT JOIN releases` in the same `list_page` query
   (the review queue already proves the join). Keep the single-query, server-paged contract; assert
   the query plan stays a single index-backed join in the integration test.
3. **`WorkerPoolDto`** — add `avgProcessingSeconds` (mean over completed items in the window) or, if
   the aggregate is too expensive at scale, document the column as `Oldest queued` and skip the
   field. Decide with a measured query, not by guessing.
4. **`DashboardAttentionDto`** — optional `storageWarnings` count if a cheap predicate exists.

Gate: `cargo test -p melodee-server` + `make test-integration` green; `docs/.../admin-api.md`
updated; no OpenSubsonic/Jellyfin/native route touched (compatibility isolation).

### W7 — Evidence, documentation, gates

- Regenerate all 27 `e2e/visual.spec.ts-snapshots/*` baselines (light/dark/RTL × 8 screens + 3
  mobile) in one commit **after** each screen's own baseline update, so a reviewer can read the
  visual diff per screen.
- Regenerate `docs/src/content/docs/admin/images/*.png` (8 files, 1600×1200) from the same captures;
  they must stay byte-identical in dimension and be refreshed in the same change as the screens
  (PRD-UI-REF-002: real screenshots are the reference).
- Extend `e2e/accessibility.spec.ts` keyboard smoke for the new primitives (`TabNav` arrow keys,
  `FilterPills` pressed state, `ProgressBar` value text).
- Extend `e2e/localization.spec.ts` for RTL on every new component (logical properties only).
- i18n: `pnpm i18n:check` must stay green; new keys added to `en-US.json` only (other locales may
  fall back, the check reports but does not fail).
- Update `design/ui/UI-SPEC.md`, `design/ui/DESIGN-SYSTEM.md`, `design/ui/MOCKUPS.md` (note the
  mockups become historical intent), and the requirement index rows for PRD-UI-REF-002/003.
- Public docs: `docs/src/content/docs/admin/overview.md`, `processing.md`, `policies.md`,
  `system-diagnostics.md`, `identity-review.md`, `appearance-and-localization.md`, and
  `docs/src/content/docs/library-management/release-review.md` — every screenshot caption and any
  description of a screen's layout/columns.
- Gates: `cargo fmt --all`, `make check`, `make admin-ui-e2e`, `python3 scripts/do-prechecks.py`.

## 8. Navigation icon mapping (PRD-UI-004 IA preserved)

| Item | Reference glyph intent | Carbon icon |
|---|---|---|
| Overview | gauge | `Dashboard` |
| Processing | arrows cycle | `Renew` |
| Review Queue | task list | `TaskView` |
| Quarantine | container | `DataVis_1`→`Archive` |
| History | clock | `History` |
| Artists | person | `UserAdmin`→`User` |
| Releases | discs | `List`→`Catalog` |
| Tracks | note | `Music` |
| Identity Review | fingerprint | `Identification` |
| Release Rules | shield/rule | `Rule` |
| System group | — | `DataBase` (Storage), `Renew` (Workers), `Search`, `UserMultiple` (Users & Access), `Settings`, `Activity` (Diagnostics) |

Final glyph choices are recorded in `UI-SPEC.md` §3 so screens and shell cannot drift.

## 9. Test and evidence strategy

| Layer | Evidence |
|---|---|
| Unit (Vitest) | New primitives (`ProgressBar`, `FilterPills`, `TabNav`, `KeyValueList`, `EvidenceTileGrid`, `MetricCard` delta) + `overview.ts` line geometry + stage-progress mapping |
| Component | Existing sibling `*.test.tsx` pattern extended for every reworked component |
| e2e behavioural | 18 spec files already assert DOM contracts; extend per screen (pills, inspector, tabs, evidence tiles, chart series) |
| Visual | 27 regenerated baselines; `maxDiffPixelRatio 0.01` retained |
| Accessibility | axe WCAG 2.2 A/AA on 8 screens × 3 viewports + keyboard smoke |
| RTL/mobile | `ar-SA` + `fa-IR` baselines; 375×812 baselines for overview/queue/review |
| Backend | Integration tests for the joined list query and dashboard deltas; `EXPLAIN` assertion |

## 10. Risks

| Risk | Mitigation |
|---|---|
| Baseline churn hides regressions | One screen per commit; baseline updated in the same commit as its screen; per-screen diff review |
| Scope creep into IA/product decisions | D1–D8 decided up front; IA frozen to PRD-UI-004 |
| Backend join degrades queue latency at catalog scale | Single indexed join + `EXPLAIN` assertion in the integration test; fall back to path-only rows if the budget is missed, and record the omission |
| Carbon `ContentSwitcher`/`Tag` divergence becomes a maintenance fork | Replace, don't fork: `FilterPills`/`StatusChip` are Melodee-owned primitives over Carbon tokens; no Carbon internals overridden |
| RTL regressions from new absolute geometry | Logical properties only; RTL baselines gate the change |
| Docs screenshots drift | Regenerated in the same change; dimension check in `scripts/validate_repository.py` already guards PNG size |
| Losing honest absence (mockup-only numbers creeping in as fake data) | §6 register is authoritative: no element ships without a real source |

## 11. Exit criteria

1. All eight canonical screens render the reference's composition, hierarchy, and chip/typography
   language using only token-layer values.
2. `PRD-UI-004` IA unchanged; `PRD-UI-005/006/007/008/009/010` invariants provably intact by their
   existing tests.
3. Zero fabricated values: every displayed datum traces to an API field.
4. 27 visual baselines + 8 docs images regenerated; axe clean; RTL and mobile baselines green.
5. `make check`, `make admin-ui-e2e`, and `python3 scripts/do-prechecks.py --include-e2e` green.
6. `UI-SPEC.md`, `DESIGN-SYSTEM.md`, `MOCKUPS.md`, the requirement index, and the six admin docs
   pages updated; `ADR-0184` accepted.
7. No new production dependency; Carbon remains the only component library.

## 12. Suggested commit sequence

```
1  design: ADR-0184 + UI-SPEC/DESIGN-SYSTEM amendments (approval gate)
2  admin-ui: split theme stylesheets (no visual change)
3  admin-ui: token + surface vocabulary
4  admin-ui: shell alignment (brand, header, nav, footer)
5  admin-ui: surface/card primitives + KeyValueList + StatusChip
6  admin-ui: MetricCard, StatStrip, HeroCard, ProgressBar, FilterPills, TabNav
7  admin-ui: EvidenceTileGrid, TimelineList, DecisionActionList, EntityTable skin
8  admin-ui: Overview redesign (+ baselines)
9  server+admin-ui: dashboard deltas & second activity series
10 admin-ui: Processing queue redesign (+ inspector card)
11 server+admin-ui: enriched work-item list fields
12 admin-ui: Release review redesign
13 admin-ui: Identity review redesign
14 admin-ui: Artist detail redesign
15 admin-ui: Release detail redesign
16 admin-ui: Policies list + editor redesign
17 admin-ui: System/Workers redesign (+ diagnostics uptime)
18 admin-ui: secondary screens consistency pass
19 docs: admin pages + regenerated screenshots
20 design: requirement traceability + closeout evidence
```

## 13. Documentation impact

- `design/ui/UI-SPEC.md` — §1 token table, §2 override scope, §3 shell dimensions and status
  placement, §4 density, §6 component contracts (approved amendment).
- `design/ui/DESIGN-SYSTEM.md` — surface system, chip semantics, omitted mockup affordances.
- `design/ui/MOCKUPS.md` — mark mockups as historical intent, real screenshots supersede.
- `design/adr/ADR-0184-…` — new accepted decision.
- `design/requirements/REQUIREMENTS-INDEX.md` — PRD-UI-REF-002/003 evidence pointers.
- `/docs/src/content/docs/admin/*` + `library-management/release-review.md` and their `images/*.png`
  — every screen description and screenshot.
- `docs/src/content/docs/api/admin-api.md` — the additive `DashboardDto`/work-item/worker-pool
  fields from §7.

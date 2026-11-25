# ADR-0184: admin UI reference alignment and Melodee surface system

- **Date:** 2026-08-15
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-UI-001, PRD-UI-002, PRD-UI-004, PRD-UI-005, PRD-UI-006,
  PRD-UI-007, PRD-UI-008, PRD-UI-009, PRD-UI-010, PRD-UI-REF-001, PRD-UI-REF-002, PRD-UI-REF-003

## Context

The Admin UI (`admin-ui/`) is functionally complete: every canonical screen renders real
data, is localized in twenty cultures, supports light/dark/system appearance and RTL, is
server-paged, and passes axe-core WCAG 2.2 AA. It is nevertheless **not the designed
product** in `design/ui/reference/01..08-*.png`.

An audit of the eight canonical screens against those mockups (sampling rendered geometry
and palette, and re-running the full Playwright suite to authenticate the committed visual
baselines as current-state evidence) found the divergence is compositional rather than
cosmetic:

- the shell brand block, header composition, and navigation treatment differ in structure,
  not only in spacing;
- six of eight screens are missing the mockups' composition entirely (entity hero cards,
  stat strips, tabbed bodies, carded right rails, two-series activity chart, filter pills,
  release-level queue rows, candidate cards with confidence numerals, evidence tile grids);
- the only shared surface vocabulary is a single `.melodee-panel` class plus Carbon
  defaults, so each screen re-invented its own layout with `melodee-fact`,
  `melodee-review-card`, `melodee-metric-card` and similar one-off classes.

Root cause: PRD-UI-REF-001 correctly declares mockups to be information-hierarchy guidance
rather than pixel specifications, and PRD-UI-REF-003/UI-SPEC.md were written from that
premise. The result was a spec that sanctioned *tokens* but never a *surface system*, so
"no one-off page styles" was unenforceable: there was nothing shared to extend.

A durable decision is needed because the remedy changes the visual contract of every screen,
introduces a shared primitive layer, adds additive admin-API fields, and regenerates the
PRD-UI-REF-002 baselines that currently act as the regression gate.

## Decision

1. **The Melodee surface system is the only sanctioned visual vocabulary.** A token-driven
   layer (`src/theme/tokens.css`, `surfaces.css`, `shell.css`, `screens.css`) plus a shared
   primitive set (`Surface`, `HeroCard`, `StatStrip`, `MetricCard`, `StatusChip`,
   `ProgressBar`, `FilterPills`, `KeyValueList`, `EvidenceTileGrid`, `TimelineList`,
   `DecisionActionList`, `TabNav`, `EntityTable`) replaces ad-hoc per-screen styling.
   UI-SPEC.md §2 is amended: component CSS outside Carbon's tokens is permitted **only**
   through these primitives, and every value they emit originates from a CSS custom property
   in the token layer. No Carbon component internals are forked.

2. **The mockups' visual language is adopted; their product architecture is not.** The
   navigation information architecture stays exactly PRD-UI-004 (including `History`,
   `Identity Review` under Catalog, `POLICIES`, and `Diagnostics`). The mockups'
   `MANAGEMENT` grouping, omitted `History`/`Diagnostics` entries, and `Album`/`Song`
   vocabulary are rejected under PRD-UI-REF-001 and the product-language rule.

3. **Shell dimensions stay UI-SPEC §3** (256 px expanded / 48 px collapsed navigation, 48 px
   header). The mockups' 220 px/72 px proportions are treated as directional. The content
   gutter becomes 24 px at ≥ lg (was 16 px). Server status is reported once, by the header
   indicator; the navigation footer carries only locale, appearance, and the collapse control, so
   the shell never shows the same status twice.

   **Amendment (2026-11-24).** The expanded navigation footer now shows the running server
   version below the locale. Operators can identify the build from any screen. The appearance
   preference remains available in the header menu.

4. **No fabricated values.** Every element the mockups show must trace to an API field. Where
   the data does not exist, the element is omitted and recorded in `DESIGN-SYSTEM.md` — never
   simulated. Concretely, the queue `Priority` column is omitted (no priority concept exists
   in the domain); `Progress` is a documented deterministic *stage* mapping, not a time
   estimate; KPI deltas and the second activity series require the additive
   `DashboardDto` fields; queue `Release`/`Artist`/`Size` require the `catalog_imports` join.

5. **Backend enablers are additive only.** New admin-API fields (`DashboardDto.deltas`,
   `activity.indexedPerHour`, `WorkItemDto.releaseTitle/artistName/mediaBytes`, and a
   `query` list filter) are added without removing, renaming, or retyping an existing field.
   The admin API is Rust-native and not a frozen compatibility surface, but no
   OpenSubsonic, Jellyfin, or native Melodee API v1 route is touched.

6. **Baselines follow the screens.** The 27 committed visual baselines and the 8 committed
   `/docs` admin screenshots are regenerated per screen in the same change as that screen, so
   the visual gate keeps detecting regressions within the new design instead of pinning the
   old one.

## Alternatives considered

### Alternative A — restyle Carbon defaults only

Keep UI-SPEC §2 as written and improve screens by choosing better Carbon components and
tokens. Cost is lowest and no ADR would be needed. Rejected because it cannot express the
reference's chip, pill, progress, hero, or stat-strip language at all; the measured gap is
compositional, so token-only work would leave six of eight screens visibly unlike the
reference — the exact outcome this decision exists to correct.

### Alternative B — adopt a second component library or a charting library

Vendoring a chart library for the activity panel and a richer table library would shorten
implementation. Rejected: ADR-0109 and UI-SPEC §10 forbid a second component library without
a proven-gap ADR, the activity chart is a two-series line plot that hand-authored SVG renders
accessibly with a text alternative, and Carbon DataTable already satisfies the server-side
paging contract.

### Alternative C — chase pixel fidelity to the mockups

Rejected on authority: PRD-UI-REF-001 makes mockups visual intent, several mockup labels are
generated hallucinations, and following them literally would contradict PRD-UI-004 and the
`Release`/`Track` product language.

## Rationale

The measured facts make the trade-off explicit rather than aesthetic. The reference palette
differs from the implemented one by a small, enumerable set of values (`#eaeaea` surface
borders vs Carbon's `#e0e0e0`, `#07131f`/`#102334`/`#6f8193` navigation vs the current
`#091117`/`#173044`/`#a8b7c5`, `#fff2e8`+`#ff832b` warning chips, `#edf5ff`+`#0f62fe`
selected pills, `#0f62fe`/`#d9d9d9` progress). Those are token changes. What cannot be fixed
by tokens is the absence of shared surface primitives, and that absence is what produced both
the visual gap and the existing one-off-class sprawl.

Making the primitives the sanctioned vocabulary resolves both problems at once, keeps Carbon
as the only component library, keeps every accessibility guarantee already proven by the axe
suite, and gives future screens something to extend instead of something to re-invent.

## Consequences

### Positive

- Eight canonical screens render the reference's hierarchy and visual language while keeping
  the PRD-mandated navigation and terminology.
- One card, hero, chip, pill, progress, fact-list, tab, and table treatment replaces dozens of
  one-off class families; new screens inherit the design instead of re-deriving it.
- Status semantics become stricter, not looser: `StatusChip` keeps icon + label +
  visually-hidden description while adopting the reference's tint model.
- The visual-regression gate keeps working across the change because baselines move with
  their screens.

### Negative / trade-offs

- A larger one-time diff than a token-only pass, and every screen's baseline changes.
- The primitive layer is now a maintained surface: adding a screen that bypasses it is a
  review failure, and primitive contract changes ripple across all screens.
- The additive admin-API fields require an integration test each and make the dashboard and
  queue list queries slightly wider (one indexed join, measured before acceptance).
- Mockup affordances without data (`Priority`, worker mean processing time) are permanently
  absent or substituted (relabelled `Oldest queued`), so the implemented screens will never be
  pixel-identical to the mockups. This is intended.

### Follow-up work

- `design/ui/REDESIGN-PLAN.md` workstreams W0–W7.
- Requirement traceability for PRD-UI-REF-002/003 after the new baselines land.
- Regenerate the eight `/docs` admin screenshots in the same change as the screens they show.

## Validation

- `admin-ui/e2e/visual.spec.ts`: 27 regenerated baselines, light/dark/RTL plus mobile, 1 %
  tolerance retained.
- `admin-ui/e2e/accessibility.spec.ts`: axe-core WCAG 2.2 A/AA across 8 screens × 3
  viewports, plus keyboard smoke extended to `TabNav`, `FilterPills`, and `ProgressBar`.
- `admin-ui/e2e/localization.spec.ts` and the `ar-SA`/`fa-IR` baselines: logical properties
  only, RTL parity for every new primitive.
- Vitest unit coverage for each primitive (status semantics, pill keyboard model, progress
  value text, tab arrow keys, chart geometry) and for the stage-progress mapping.
- `cargo test -p melodee-server` plus `make test-integration` for the additive DTO fields and
  the joined queue query, including an `EXPLAIN` assertion that the queue list remains a
  single index-backed join.
- `make check`, `make admin-ui-e2e`, `python3 scripts/do-prechecks.py --include-e2e`.

## References

- [PRD.md](../../PRD.md) — PRD-UI-001..010, PRD-UI-REF-001..003
- [SPEC.md](../../SPEC.md)
- [UI-SPEC.md](../ui/UI-SPEC.md)
- [DESIGN-SYSTEM.md](../ui/DESIGN-SYSTEM.md)
- [MOCKUPS.md](../ui/MOCKUPS.md)
- [REDESIGN-PLAN.md](../ui/REDESIGN-PLAN.md)
- [design/ui/reference/](../ui/reference/)
- ADR-0109 (React + TypeScript + Carbon), ADR-0137 (localization/RTL), ADR-0138 (tooling and
  static serving), ADR-0139 (session and gating)

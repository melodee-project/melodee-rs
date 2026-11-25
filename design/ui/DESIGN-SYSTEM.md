# Melodee Admin Design System

## Product character

Modern, polished, calm, data-rich, simple, and intuitive. The UI explains what Melodee is doing, what
requires an operator decision, and why. It must not feel like a generic CRUD generator or a second
consumer player.

## Foundation

- Carbon React components and Carbon tokens are the default implementation primitives.
- A Melodee token layer may override brand, status, artwork-surface, and density tokens without
  bypassing Carbon accessibility behavior.
- No second component library or proprietary grid is allowed.
- Carbon DataTable is the initial table solution. TanStack Table is deferred until an accepted ADR
  demonstrates requirements Carbon cannot satisfy reasonably.

## Global shell

- Persistent collapsible left navigation.
- Global search/command entry in the header.
- User, language, appearance, notifications, and server status controls in the header; locale,
  running server version, and collapse in the navigation footer.
- Server status is reported once, in the header. Never duplicate it in the rail.
- Primary navigation groups: Overview; Processing; Catalog; Policies; System.
- Consumer playback features are not primary navigation. Track preview is available where curation
  needs audio evidence.

## Page patterns

### Overview

Answer “what needs attention?” first, then activity, throughput, health, and recent decisions.

### Queue plus inspector

Use a server-paged dense table and a right inspector for high-throughput triage. Selection replaces the
normal toolbar with explicit bulk actions. A full detail page remains available.

### Review workspace

Show entity summary, evidence, confidence, catalog comparison, policy match, timeline, and ordered
choices. Preserve original observations alongside resolved values.

### Detail workspace

Use tabs for Overview, Releases/Tracks, Contributions, Identity, Files, Provenance, Images, and History
as appropriate. Avoid one enormous editable form.

## Status semantics

Status is never color-only. Use label, icon, and accessible description.

| Meaning | Semantic treatment |
|---|---|
| Complete/healthy/accepted | Success |
| Active/processing/indexing | Informational |
| Waiting/review/uncertain | Warning |
| Failed/rejected/danger | Error |
| Provisional/identity work | Distinct secondary accent |
| Disabled/ignored | Neutral |

Classification is not a failure state. Disposition is not classification. Quarantine is distinct from
failed processing and ignored content.

## Themes and appearance

- Preserve System, Light, and Dark appearance choices.
- Support administrator-defined Melodee theme packs through stable semantic tokens.
- Do not store raw component CSS as the primary theme contract.
- Respect high-contrast and reduced-motion preferences.
- Nested surfaces may use contrasting Carbon themes when artwork/detail hierarchy benefits.

## Localization and direction

- All visible strings use the localization layer.
- Preserve the current supported culture inventory during migration.
- Locale controls dates, times, numbers, units, plural forms, collation, and text direction.
- Arabic and Persian must retain RTL behavior; components and custom CSS use logical properties.
- Never concatenate translated fragments to build sentences.

## Data scale

- Search, filter, sort, and paginate on the server for 35k+ Artists, 50k+ Releases, and 560k+ Tracks.
- Keep URLs/shareable state for meaningful filters and selected saved views.
- Virtualization is an optimization, not a substitute for server-side data access.

## Destructive actions

Before Delete, show affected Releases/Tracks/files, estimated storage impact, matched rule/reason,
audit outcome, and quarantine/ignore alternative. Require explicit confirmation and authorization.

## Accessibility

- Keyboard-complete navigation and actions.
- Visible focus and logical focus order.
- Accessible table labels, status text, validation, dialogs, toasts, and progress.
- No hover-only information.
- Artwork has meaningful alt text or is decorative.
- Audio preview has labeled controls and does not autoplay.

## Agent rule

Do not imitate hallucinated text or data from a mockup. Implement the information architecture and
visual language using the domain/API specifications and shared components.

## Surface system (ADR-0184)

The reference mockups define a visual language that Carbon defaults do not express: flat white
cards with a hairline `#eaeaea` border and 8 px radius on a `#f4f4f4` page, tinted status chips
(fill + ink pair, no ring), selected filter pills in `#edf5ff`/`#0f62fe`, blue progress bars on a
`#d9d9d9` track, entity hero cards with a media slot, numeral-focused stat strips, and underlined
tabs.

These are implemented once, as the §6.1 primitives in `design/ui/UI-SPEC.md`, driven entirely by
CSS custom properties. Screens compose primitives; they do not declare color, radius, shadow, or
font size. Layout-only rules live in `src/theme/screens.css`.

Measured reference values (sampled from `design/ui/reference/`) that the token layer encodes:

| Element | Value |
|---|---|
| Page background | `#f4f4f4` |
| Card surface / border | `#ffffff` / `#eaeaea` |
| Table header / row divider | `#f4f4f4` / `#eaeaea` |
| Primary / secondary text | `#161616` / `#525252` |
| Positive delta accent | `#0f62fe` |
| Navigation background / active / muted | `#07131f` / `#102334` / `#6f8193` |
| Warning chip | `#fff2e8` fill, `#ff832b` ink |
| Info chip / selected pill | `#edf5ff` fill, `#0f62fe` ink |
| Progress fill / track | `#0f62fe` / `#d9d9d9` |

Shell dimensions remain UI-SPEC §3 (256/48 px navigation, 48 px header, 24 px content gutter); the
mockups' 220/72 px proportions are directional. Navigation information architecture remains
PRD-UI-004 — the mockups' `MANAGEMENT` grouping, omitted `History`/`Diagnostics` entries, and
`Album`/`Song` labels are rejected under PRD-UI-REF-001 and the product-language rule.

### Intentionally absent mockup affordances

Never simulate these; the data does not exist:

| Mockup element | Reason | Substitution |
|---|---|---|
| Queue `Priority` column | No priority concept exists in the domain | `Attempts` occupies the column position |
| Queue numeric `Progress` percent as time estimate | The ingestion state machine is discrete | Deterministic *stage* progress from the documented state map |
| Queue `Size` before import | Bytes are known only once media assets exist | Em dash until `mediaBytes` is bound |
| Overview catalog week deltas | No historical catalog counters | Rendered only when `DashboardDto.deltas` reports them |
| Worker `Avg time` | No mean-processing-time aggregate | Column relabelled `Oldest queued` |
| `Storage warning 1` count | Health reports a verdict, not a count | Severity row without a count |

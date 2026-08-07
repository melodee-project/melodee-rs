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
- User, language, appearance, notifications, and server status controls in the header/footer areas.
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

# ADR-0137: Localization stack and RTL preservation

- **Date:** 2026-08-14
- **Status:** Accepted
- **Decision owners:** Phase 10
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-I18N-001, PRD-I18N-002, PRD-I18N-003, PRD-I18N-004, PRD-I18N-005,
  PRD-I18N-006, PRD-UI-001

## Context

The Phase 10 admin SPA must preserve the legacy culture inventory (20 cultures: en-US, de-DE,
es-ES, fr-FR, it-IT, ja-JP, pt-BR, ru-RU, zh-CN, ar-SA, nl-NL, pl-PL, tr-TR, id-ID, ko-KR, vi-VN,
fa-IR, uk-UA, cs-CZ, sv-SE), localize every user-visible string, format dates/times/numbers per
locale, and retain RTL for Arabic and Persian. PRD §53 listed this decision as planned ADR-R020 but
no ADR file existed; PRD-I18N-002 names i18next + react-i18next as the preferred stack, and
`design/requirements/TECHNOLOGY-BASELINE.md` already records i18next (MIT) as an approved FOSS
dependency. Without a ratified stack decision, no localized screen work can start.

## Decision

Use **i18next + react-i18next** (MIT) as the admin SPA localization framework.

- All user-visible strings are externalized into per-locale JSON resources keyed by stable,
  namespaced keys; no translated-fragment concatenation — use i18next interpolation and ICU-style
  plural forms.
- **en-US is the source and fallback locale.** Missing keys in any other locale fall back to
  English (PRD-I18N-006); a CI check fails on keys present in en-US but structurally invalid
  elsewhere, and reports (not fails on) untranslated keys.
- Date/time/number/unit formatting uses standards-based `Intl` APIs driven by the active locale
  (PRD-I18N-004).
- Text direction is derived from the locale (`ar-SA`, `fa-IR` → `dir="rtl"` on the document root);
  components and custom CSS use logical properties so layout mirrors automatically
  (PRD-I18N-005).
- The 20-culture inventory is preserved (PRD-I18N-001). Legacy Blazor resource strings are
  harvested as translation seeds where they map to identical semantics; admin-console strings with
  no legacy analog ship English-complete and fall back in other locales until translated.
- Locale selection is a user preference stored server-side with the existing profile/preferences
  mechanisms, with browser-locale detection as the pre-login default.

## Alternatives considered

### FormatJS (react-intl)

Strong ICU support, but a heavier runtime and a second message-syntax mental model; i18next is the
PRD-named preference and already in the technology baseline.

### Lingui

Compile-time extraction is attractive, but adds a macro/build-plugin chain and a smaller ecosystem;
not named by the PRD.

### Hand-rolled dictionaries

No plural/formatting/RTL ecosystem, no fallback machinery; reinvents solved problems.

## Rationale

i18next is the PRD-preferred, MIT-licensed, actively maintained stack with first-class React
bindings, pluralization, fallback chains, and lazy locale loading. Ratifying the PRD preference
closes the ADR-R020 gap with the lowest decision risk.

## Consequences

### Positive

- Unblocks all localized screen work with an approved FOSS stack.
- Fallback semantics make incomplete translations safe to ship.
- Logical-properties discipline keeps RTL maintenance near-zero per component.

### Negative / trade-offs

- Legacy harvested translations cover consumer-surface strings; admin-console vocabulary
  (Review/Quarantine/Disposition/…) will be English-only in non-source locales at first, visible as
  English fallback.

### Follow-up work

- Harvest report and seeded locale resources (Wave 0, W0-3).
- RTL smoke verification on canonical screens (Wave 4, W4-3).

## Validation

- CI locale-key validation script (missing/invalid key detection, fallback report).
- Rendered smoke of canonical screens under `ar-SA` with `dir="rtl"` asserted in Playwright.
- Unit tests for plural forms and Intl formatting under non-English locales.

## References

- [PRD.md](../../PRD.md) §42, §53 (ADR-R020)
- [SPEC.md](../../SPEC.md) §21
- [design/ui/DESIGN-SYSTEM.md](../ui/DESIGN-SYSTEM.md)
- [TECHNOLOGY-BASELINE.md](../requirements/TECHNOLOGY-BASELINE.md)

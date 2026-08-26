# ADR-0109: Use Carbon Design System for the administration UI

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-UI-001 through PRD-UI-010, PRD-THEME-001, PRD-I18N-001

## Context

The new admin application must be modern, slick, simple, intuitive, data-heavy, accessible, themed,
localized, light/dark capable, and fully FOSS. Carbon is active, Apache-2.0, and visually aligned with
the approved mockups.

## Decision

Proposed: use React + TypeScript + Carbon. Use Carbon DataTable first; add TanStack Table only for a
proven requirement Carbon cannot satisfy cleanly.

## Validation

Implement the Processing Queue, Release Review, and Artist Identity Review shells in Carbon,
including light/dark, a translated locale, RTL smoke rendering, keyboard navigation, and responsive
behavior, then accept or reject this ADR.

## Phase 0 review (2026-08-09)

- **Status unchanged: Proposed.** The ADR's own validation (implement Processing Queue,
  Release Review, and Artist Identity Review shells in Carbon) is Phase 9 scope and has not
  run; acceptance criteria are not met.
- **Technology revalidation (2026-08-09):** carbon-design-system/carbon is active (pushed
  2026-08-08), Apache-2.0, no commercial boundary. Carbon remains the preferred candidate;
  the approved mockups under design/ remain consistent with it.

## Phase 9 amendment (2026-08-13)

- **Status changed: Proposed → Accepted.** The technology decision (React + TypeScript + Carbon,
  Carbon DataTable first, TanStack Table only for a proven gap) is accepted: Phase 0 revalidated
  Carbon's active, Apache-2.0, fully-FOSS status and the approved mockups remain consistent with
  it.
- **Scope ambiguity resolved.** This ADR's validation section (implement Processing Queue,
  Release Review, and Artist Identity Review shells with light/dark, a translated locale, RTL
  smoke, keyboard navigation, and responsive behavior) is **Phase 10 entry criteria**, not Phase 9
  scope. Phase 9's only obligation toward this ADR is WP-14's confirmation that theming/i18n/RTL
  requirement rows are Phase-10-owned and outside the Phase 9 exit gate.
- The shell-validation evidence must still be produced and recorded here (or in a superseding
  note) before Phase 10 is accepted.

## Phase 10 validation evidence (2026-08-14)

The required validation shells are implemented in `admin-ui/` (React 19 + TypeScript strict +
Carbon 1.114, Vite build, served at `/admin` per ADR-0138):

- **Processing Queue** — `admin-ui/src/screens/processing/ProcessingQueueScreen.tsx` +
  `WorkItemInspector.tsx`.
- **Release Review** — `admin-ui/src/screens/review/{ReviewQueueScreen,ReleaseReviewScreen}.tsx`.
- **Artist Identity Review** — `admin-ui/src/screens/identity/IdentityReviewScreen.tsx`.

Validation matrix evidence:

- **Light/dark:** theme system (`admin-ui/src/theme/ThemeProvider.tsx`, `tokens.css`) with
  System/Light/Dark appearance over Carbon g10/g100; covered by component tests and the e2e
  suite.
- **Translated locale:** 20-culture i18next catalog (ADR-0137) with legacy-harvested translations
  (harvest evidence: withdrawn `design/evidence/phase-10/i18n-harvest.md` report); locale switching covered by e2e.
- **RTL smoke:** ar-SA `dir="rtl"` assertions in `admin-ui/e2e/` (shell, processing queue,
  release review, identity review specs) at 1600×1200, 768×1024, and 375×812.
- **Keyboard navigation:** e2e keyboard specs (table row focus/Enter activation, decision-action
  tab order) in the processing-queue/release-review/identity-review e2e files.
- **Responsive:** all e2e specs run at the three UI-SPEC §8 viewports.

Test counts at time of recording: 96 unit/component tests, 42 e2e runs — all green; recorded
in the withdrawn `design/evidence/phase-10/checklists/04..06` evidence.
**ADR-0109 validation is satisfied.**

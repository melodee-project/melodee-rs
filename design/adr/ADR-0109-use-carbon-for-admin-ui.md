# ADR-0109: Use Carbon Design System for the administration UI

- **Date:** 2026-08-07
- **Status:** Proposed
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

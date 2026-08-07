---
applyTo: "admin-ui/**"
---

# Carbon admin UI instructions

- Use Carbon Design System components and tokens first; do not add another component library.
- Canonical mockups define visual intent. `design/ui/DESIGN-SYSTEM.md` defines implementation rules.
- Admin UI language uses Release and Track. Compatibility API names remain in adapter documentation.
- All visible text is localized. Preserve light, dark, system, custom themes, and supported RTL
  cultures.
- Use server-side search/filter/sort/pagination for catalog-scale data.
- Prefer an inspector panel for fast review and a full page for deep work.
- Destructive actions must show scope, reason, impact, and recovery alternative.
- Provide keyboard, focus, screen-reader, loading, empty, error, and reduced-motion behavior.
- Do not add TanStack Table until Carbon DataTable is proven insufficient by an accepted ADR.

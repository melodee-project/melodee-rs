# ADR-0138: Admin SPA tooling and static serving

- **Date:** 2026-08-14
- **Status:** Accepted
- **Decision owners:** Phase 10
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-UI-002, PRD-TEST-008, PRD-API-ADMIN-001

## Context

ADR-0109 accepted React + TypeScript + Carbon for the admin UI but deliberately did not choose the
build tool, test runner, browser-test harness, or how built assets reach the browser. `admin-ui/`
has no tooling; the repository's only Node project is `docs/` (Astro, pnpm 11, Node 22). SPEC §6
permits "static admin SPA assets served by the server or a simple reverse proxy". The server
currently enables `tower-http` with `["cors", "trace"]` only — no `fs` feature. New production
dependencies require an ADR or documented dependency decision and must pass the FOSS gate.

## Decision

- **Build tool:** Vite (MIT) with `@vitejs/plugin-react`; TypeScript strict mode. Exact-pinned
  dependencies, committed `pnpm-lock.yaml`, `packageManager` pinned to pnpm 11, Node >= 22.12 —
  matching the `docs/` toolchain conventions.
- **Unit/component tests:** Vitest (MIT) + React Testing Library.
- **Browser/e2e/visual tests:** Playwright (Apache-2.0) with its own config; visual-regression
  screenshots for canonical screens in light/dark/RTL variants (PRD-UI-REF-002).
- **Static serving:** the Rust server embeds and serves the built SPA via `tower-http`'s `fs`
  feature (`ServeDir` + SPA fallback to `index.html`) mounted at `/admin`, as the supported
  default single-executable deployment. A reverse proxy serving the `admin-ui/dist` output remains
  a documented alternative. Enabling the `fs` feature on the existing `tower-http = "=0.7.0"`
  workspace dependency is the only Cargo change — no new crate.
- **CI:** a path-filtered `admin-ui.yml` workflow modeled on `docs.yml` (pnpm/action-setup,
  setup-node 22, `pnpm install --frozen-lockfile`, typecheck/lint/unit/build, Playwright job), and
  a local `make admin-ui-check` hook added to `scripts/check.sh` so the JS gate is executable
  locally.

## Alternatives considered

### Reverse-proxy-only serving

Avoids the `tower-http/fs` feature, but forces every deployment to run a second HTTP layer and
breaks the single-executable deployment story SPEC §6 prefers as default.

### Webpack/rollup-direct, Jest, Cypress

Webpack is heavier than Vite for an SPA of this size; Jest duplicates a transform pipeline Vitest
gets free from Vite; Cypress is Apache-2.0 but Playwright has better multi-locale/RTL viewport and
screenshot tooling and headless CI ergonomics.

### Rust-native frontend (Leptos/Yew/Dioxus)

Would avoid a second toolchain but contradicts accepted ADR-0109 (React + Carbon) and the approved
mockups/design system.

## Rationale

Vite + Vitest + Playwright is the minimal, fully-FOSS, actively maintained toolchain that satisfies
PRD-TEST-008 (unit/component + e2e + visual regression) with one mental model. Embedded static
serving keeps the deployment contract "one server executable" while leaving the proxy path open.

## Consequences

### Positive

- Single-command local gate covering both Rust and JS.
- One executable serves API + SPA; no mandatory reverse proxy.
- Deterministic CI via frozen lockfile and pinned toolchain.

### Negative / trade-offs

- The repository now carries two dependency ecosystems; JS supply-chain auditing (pnpm audit in
  CI) is a new obligation.
- The server binary grows by the size of the embedded/served assets; rebuilds require the SPA
  bundle to be present (build script or pre-built `dist`).

### Follow-up work

- Playwright browser installation documented for contributors (`pnpm exec playwright install`).
- JS dependency audit step in `admin-ui.yml`.

## Validation

- `scripts/check.sh` runs the JS gate and fails on typecheck/lint/test/build errors.
- CI `admin-ui.yml` passes on a clean checkout with `--frozen-lockfile`.
- `GET /admin` serves the SPA and deep links (e.g. `/admin/processing/queue`) return `index.html`;
  API routes are unaffected (route-shadowing test).

## References

- [PRD.md](../../PRD.md) §38, §40, §56
- [SPEC.md](../../SPEC.md) §6, §21
- [ADR-0109](ADR-0109-use-carbon-for-admin-ui.md)

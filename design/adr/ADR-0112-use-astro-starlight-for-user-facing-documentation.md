# ADR-0112: Use Astro Starlight for user-facing documentation

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD documentation/distribution goals; FOSS dependency gate
  (ADR-0103)

## Context

The legacy documentation approach is Jekyll-oriented. User documentation must remain inside
`/docs` in the Melodee repository, stay Markdown-first, and publish through GitHub Pages built
by GitHub Actions. Coding agents are primary maintainers, so the authoring model must be
conventional and easy to keep correct. The architecture must support multiple languages and RTL
locales, light/dark appearance, and fully static search — all without proprietary hosted
services and with every dependency active and 100% FOSS (ADR-0103). The documentation model
keeps public docs (`/docs`) strictly separate from internal engineering material (`/design`)
and compatibility evidence (`/compatibility`).

## Decision

Proposed: use Astro Starlight as the documentation framework, embedded as a self-contained
pnpm project under `/docs`, and deploy its static build to GitHub Pages through the official
GitHub Pages Actions flow (`.github/workflows/docs.yml`).

Selected and verified versions (npm registry, 2026-08-07):

| Package | Version | License | Purpose |
| --- | --- | --- | --- |
| astro | 7.2.0 | MIT | Static site framework |
| @astrojs/starlight | 0.41.7 | MIT | Documentation theme/toolkit |
| @astrojs/check | 0.9.10 | MIT | Content/frontmatter/TS diagnostics |
| starlight-links-validator | 0.25.2 | MIT | Internal link validation at build |
| sharp | 0.35.3 | Apache-2.0 | Image processing for Astro assets |
| typescript | 6.0.3 | Apache-2.0 | Type checking (peer of @astrojs/check) |
| pnpm | 11.20.0 | MIT | Package manager (lockfile committed) |

Search is Starlight's built-in Pagefind integration: a fully static, FOSS index that works on
GitHub Pages with no external service.

## Alternatives considered

### Continue Jekyll

Jekyll is the incumbent and builds natively on GitHub Pages with zero workflow setup. However
its classic theme ecosystem provides weaker built-in support for the required feature set
(multilingual/RTL architecture, light/dark, static search, modern accessibility), the Ruby
toolchain adds a second language ecosystem to a repository that already standardizes Node for
the admin UI, and GitHub's own Pages gem pins outdated Jekyll versions for the zero-config path.
Keeping it means owning more custom plumbing for capabilities Starlight provides by default.

### Docusaurus

Active, MIT, feature-rich (versioning, i18n, search plugins), and React-based like the planned
admin UI. It is a heavier runtime (client-side React SPA for every page), its i18n and
versioning machinery exceeds current needs, and its authoring/configuration surface is larger
for agents to keep consistent. Static-first Astro output better matches a documentation site.

### VitePress

Active, MIT, fast, and simple, with good Markdown ergonomics. Its multilingual/RTL support and
theme system are less comprehensive than Starlight's for documentation-specific needs
(e.g. built-in Pagefind search, doc-focused sidebar/locale model), and its Vue orientation
diverges from the repository's React admin UI stack. A reasonable fallback if Starlight stalls.

### MkDocs Material

Active, MIT, excellent Markdown ergonomics and search, widely deployed. It introduces a Python
documentation toolchain (the repository already has Python, but the docs site then depends on
PyPI packaging and the Material theme's Insiders open-core model for some features). Core
features we need are FOSS, but the paid Insiders tier creates exactly the open-core pressure
ADR-0103 steers away from.

## Rationale

Starlight gives the required capabilities by default — Markdown-first authoring, light/dark
appearance, multilingual and RTL-capable architecture, static Pagefind search, accessible
components — while emitting static HTML that deploys trivially to GitHub Pages. It is actively
maintained, MIT-licensed end to end, has no paid tier, and its conventional content structure
(`src/content/docs/`) is easy for coding agents to extend without custom plumbing. The Node +
pnpm toolchain aligns with the planned admin UI stack.

## Consequences

### Positive

- Markdown remains the primary authoring surface; MDX is opt-in per page.
- Search, theming, light/dark, i18n/RTL architecture work without external services.
- Pull-request validation (`pnpm check`, `pnpm check:links`, `pnpm build`) catches broken
  links, invalid frontmatter, and type errors before deployment.
- Documentation changes become normal product work: user-facing documentation impact is part
  of the Definition of Done, and docs updates ship in the same change as behavior changes.

### Negative / trade-offs

- A Node/pnpm toolchain now lives under `/docs`; CI must install it deterministically and the
  lockfile must stay committed and frozen.
- Generated output (`docs/dist`, `docs/.astro`) must never be committed (enforced via
  `.gitignore`).
- Starlight upgrades occasionally change configuration APIs (observed during adoption:
  pre-0.39 sidebar autogenerate syntax removed); upgrades are deliberate, version-pinned steps.
- Relative internal links are required (Pages base path); enforced by `pnpm check:links`.

### Follow-up work

- Accept or reject this ADR after the first deployed GitHub Pages build is reviewed.
- Populate planned-behavior placeholder pages as Rust rewrite phases implement features.
- Re-evaluate OpenAPI reference rendering (fully FOSS) when the Melodee API documentation
  outgrows prose pages; record a separate decision if a generator is adopted.

## Validation

- `pnpm install --frozen-lockfile`, `pnpm check`, `pnpm check:links`, and `pnpm build` succeed
  from a clean checkout (executed 2026-08-07; 34 pages built, Pagefind index generated, all
  internal links valid).
- `.github/workflows/docs.yml` builds on pull requests and deploys through the official Pages
  actions with minimal permissions and deployment concurrency control.
- The link checker was negative-tested: it fails the build on a deliberately broken link.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0103: Require fully FOSS dependencies](ADR-0103-require-fully-foss-dependencies.md)
- [docs/README.md](../../docs/README.md)
- [docs/AGENTS.md](../../docs/AGENTS.md)

## Phase 0 ratification (2026-08-09)

- **Status change:** Proposed -> Accepted (Issue 10 ratification authority).
- **Evidence:** the decision is already implemented and continuously verified: `/docs` is a
  self-contained Starlight project; `.github/workflows/docs.yml` runs `pnpm check`,
  `pnpm check:links`, and a build with link validation on every change; the site deploys to
  GitHub Pages.
- **Technology revalidation (2026-08-09):** npm registry — astro 7.2.0 (MIT) and
  @astrojs/starlight 0.41.7 (MIT) are the exact versions recorded in this ADR and remain the
  current releases; Pagefind search is fully static/FOSS. No commercial boundary.

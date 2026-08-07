# Documentation AGENTS.md — rules for working under `/docs`

Mandatory reading before changing anything in this directory. The repository root
`AGENTS.md` still applies; this file adds documentation-specific rules.

## What `/docs` is

`/docs` is the public, user-facing Melodee documentation site (Astro Starlight, deployed to
GitHub Pages). It is the only place in this repository for public product documentation.

Boundary:

```text
/docs            Public, user-facing documentation (this site)
/design          Internal architecture, requirements, ADRs, design, UI references
/compatibility   Captured external API contracts and compatibility evidence
```

Never move internal engineering material (`PRD.md`, `SPEC.md`, `IMPLEMENTATION.md`, ADRs, test
artifacts, agent instructions, work packages) into `/docs`. Never publish generated design
mockups from `design/ui/reference` as product screenshots.

## Audience

Public documentation is written for:

- Melodee users;
- self-hosters;
- administrators;
- client developers;
- API consumers.

Internal engineering commentary does not belong here.

## Documentation is product work

Documentation is not optional cleanup after implementation. Any user-observable change must
cause the implementation agent to evaluate whether documentation must change, including:

- new features or changed workflows;
- new settings, changed defaults, new configuration;
- new CLI commands or changed CLI syntax;
- new APIs or API behavior changes;
- compatibility changes (Melodee API v1, OpenSubsonic, Jellyfin);
- installation/deployment changes;
- migration or upgrade requirements;
- authentication or permissions changes;
- UI changes that affect instructions;
- troubleshooting-relevant behavior;
- deprecated or removed functionality.

### Same-change principle

When implementation changes user-visible behavior, documentation is changed in the **same
PR/change set**. Do not create a future documentation TODO merely to claim a feature is
complete.

### Documentation evaluation rule

Every implementation task must explicitly answer:

```text
Documentation impact:
- None — with reason
or
- Updated — list files
```

A bare `None` without a reason is not sufficient for user-visible work.

## Honesty rules

- A capability must exist before public documentation says it exists. `PRD.md`, `SPEC.md`,
  `IMPLEMENTATION.md`, Proposed ADRs, roadmap items, and mockups are **not** evidence of shipped
  behavior.
- Documentation for unimplemented behavior must be clearly marked planned/development (use a
  `:::caution[Planned documentation]` aside) or must not exist.
- If documentation and implementation disagree, record the discrepancy; do not guess.
- Commands shown to users must be valid, current, copy/pasteable, supported by Melodee, and
  tested where practical. Do not invent commands from anticipated future tooling.
- Screenshots must represent implemented UI.

## Terminology

Core product language is **Artist**, **Release**, **Track**. Use `Album`/`Song` only when
documenting Melodee API v1 fields that preserve those names, OpenSubsonic, Jellyfin
compatibility, or quoting client-facing legacy terminology. Never silently rename compatibility
API fields in documentation.

## Authoring rules

- Markdown first. Use MDX only where a real interactive or visual requirement cannot be
  expressed in Markdown/Starlight components.
- Use Starlight's conventional content structure (`src/content/docs/`); do not create custom
  loaders.
- **Internal links are relative to the current file** (e.g. `./concepts/`,
  `../installation/docker/`). Never use root-absolute internal links — they break under the
  GitHub Pages base path. `pnpm check:links` validates them.
- New settings must link to configuration documentation. New APIs must link to API
  documentation. Troubleshooting content should link to the relevant feature/configuration
  pages.
- Images go in `public/images/` (or beside the content file) and must be referenced by working
  relative links. Do not embed textual UI strings into images.

## Localization and RTL

English is the source locale under `src/content/docs/`. Additional locales get their own
`src/content/docs/<locale>/` tree and a `locales` entry in `astro.config.mjs`; RTL languages set
`dir: 'rtl'`. Write translation-friendly source: avoid locale-specific formatting assumptions
and text embedded in images. Do not break the multilingual architecture.

## Commands

```bash
pnpm install --frozen-lockfile   # install (CI uses exactly this)
pnpm dev                         # local dev server
pnpm check                       # Astro/TypeScript diagnostics
pnpm check:links                 # relative link + asset validation
pnpm build                       # production build (also runs build-time link validation)
```

All of the above must pass before a docs change is considered done. Generated output
(`docs/dist/`, `docs/.astro/`) is never committed.

## Dependencies

All documentation dependencies must be active, modern, and 100% FOSS (see
`design/requirements/TECHNOLOGY-BASELINE.md` for the recorded versions and licenses). No
analytics, hosted search, hosted comments, proprietary themes, or closed-source build services.
Search is Starlight's built-in Pagefind — fully static, no external service.

## Visual design

Use Starlight's design system and accessibility behavior first. Melodee branding is applied
through logo, favicon, restrained palette overrides in `src/styles/custom.css`, and light/dark
appearance. Do not build a custom documentation UI framework and do not reproduce the Carbon
admin UI inside the documentation site.

# Melodee public documentation

This directory contains the public, user-facing Melodee documentation site. It is an
[Astro Starlight](https://starlight.astro.build) project. `.github/workflows/docs.yml` always
validates and builds it; it deploys to GitHub Pages only when the repository supports Pages and
the `MELODEE_PAGES_ENABLED` repository variable is explicitly enabled.

Internal engineering documentation — requirements, architecture, ADRs, work packages,
implementation notes, UI references — lives under [`/design`](../design) and must not be moved
here. Captured API contracts live under [`/compatibility`](../compatibility).

Agents and contributors changing files under `/docs` must read [`AGENTS.md`](AGENTS.md) first.

## Requirements

- Node.js ≥ 22.12
- pnpm 11 (via Corepack: `corepack enable`, or see <https://pnpm.io/installation>)

## Install

```bash
cd docs
pnpm install --frozen-lockfile
```

The lockfile (`pnpm-lock.yaml`) is committed and is the only dependency source of truth. Never
install with npm or yarn.

## Run locally

```bash
pnpm dev
```

This starts the Astro dev server with live reload and prints the local URL.

## Validate

Run all checks before submitting a change:

```bash
pnpm audit        # production and development dependency advisories
pnpm check        # Astro/TypeScript diagnostics (content schemas, frontmatter, TS errors)
pnpm check:links  # relative internal link and image-asset validation
pnpm build        # production build; also runs starlight-links-validator and Pagefind
```

A change is valid only when all four commands succeed. From the repository root,
`make docs-site-check` installs the frozen lockfile and runs these same checks.

## Build output

`pnpm build` writes the static site to `docs/dist/`. This directory (and `docs/.astro/`) is
generated output, is listed in the repository `.gitignore`, and must never be committed.

## Deployment

`.github/workflows/docs.yml` builds the site on every pull request that touches `docs/` and on
the corresponding pushes to `main`. The workflow installs Node and pnpm deterministically, runs
`pnpm install --frozen-lockfile`, `pnpm check`, `pnpm check:links`, `pnpm audit`, and `pnpm build`.

To enable GitHub Pages publication on a repository and plan that support it:

1. In **Settings → Pages**, select **GitHub Actions** as the publishing source.
2. Create the repository Actions variable `MELODEE_PAGES_ENABLED` with value `true`.
3. For a project Pages URL, keep the workflow's `DOCS_SITE` and `DOCS_BASE` values. For a custom
   domain, set `DOCS_SITE` to that domain, `DOCS_BASE` to `/`, and add the domain's `CNAME` file
   under `public/`.

When that variable is not `true`, the workflow intentionally skips `configure-pages`, artifact
upload, and deployment while retaining the complete documentation quality gate. This keeps the
repository green on plans that do not offer GitHub Pages.

## Navigation

The sidebar is configured explicitly in `astro.config.mjs` under `sidebar`. Top-level groups
autogenerate from their content directories; page order within a group is set with
`sidebar.order` in each page's frontmatter.

## Adding a page

1. Create a Markdown file under `src/content/docs/<section>/my-page.md`.
2. Add frontmatter:

   ```markdown
   ---
   title: My Page
   description: One-line summary shown in search and link previews.
   sidebar:
     order: 3
   ---
   ```

3. Write content in plain Markdown. Use MDX (`.mdx`) only when Markdown cannot express the
   requirement.
4. Link to other pages with **relative** links (e.g. `./other-page/`, `../section/page/`).
   Root-absolute internal links break under the Pages base path and fail validation.
5. Run the validation commands above.

## Adding images

Place images in `public/images/` and reference them relative to the page, or place them beside
the content file. Keep text out of images (localization). Images must show implemented UI only —
never generated design mockups from `design/ui/reference`.

## Localization

English is the source locale at the content root. To add a language, create
`src/content/docs/<locale>/` and register the locale in `astro.config.mjs` (`locales`), setting
`dir: 'rtl'` for right-to-left languages. Starlight provides localized chrome, and Pagefind
search remains fully static for every locale. Write source content so it is
translation-friendly.

## Migrating legacy documentation

No Jekyll documentation tree exists in this repository; the legacy C# server's documentation
lives in the legacy repository. When legacy user-facing content is brought over, migrate it
incrementally rather than for parity:

1. Inventory the source page and verify the behavior it describes still ships.
2. Convert to Starlight-compatible Markdown/frontmatter in the appropriate section.
3. Preserve useful URLs where reasonably possible; add redirects only where necessary and
   supported by the deployment.
4. Do not carry forward obsolete or inaccurate documentation; report content whose current
   behavior cannot be verified instead of guessing.

## Search

Search is Starlight's built-in Pagefind integration: a fully static, FOSS index generated at
build time. It works on GitHub Pages with no external service and no configuration.

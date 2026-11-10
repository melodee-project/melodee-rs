// @ts-check
import starlight from '@astrojs/starlight';
import { defineConfig } from 'astro/config';
import starlightLinksValidator from 'starlight-links-validator';

// Melodee public documentation site.
//
// A supported GitHub Pages deployment can provide DOCS_SITE and DOCS_BASE for
// the project Pages URL (https://<owner>.github.io/<repo>/). The workflow
// always validates and builds docs, but publication is opt-in because some
// repository plans do not support Pages. If a custom domain is configured,
// set DOCS_SITE to that domain and DOCS_BASE to '/' in the workflow and add
// the domain's CNAME file under docs/public/.
const site = process.env.DOCS_SITE ?? 'https://melodee-project.github.io';
const base = process.env.DOCS_BASE ?? '/melodee-rs';

export default defineConfig({
  site,
  base,
  integrations: [
    starlight({
      title: 'Melodee',
      description:
        'User documentation for the Melodee music server: installation, library management, clients, features, administration, and APIs.',
      logo: {
        src: './public/logo.svg',
        alt: 'Melodee',
      },
      favicon: '/favicon.svg',
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/melodee-project/melodee-rs',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/melodee-project/melodee-rs/edit/main/docs/',
      },
      // Localization architecture: English is the source locale served from
      // the content root. Additional locales are added here with their own
      // `src/content/docs/<locale>/` trees; RTL languages set `dir: 'rtl'`.
      // Example:
      //   ar: { label: 'العربية', lang: 'ar', dir: 'rtl' },
      defaultLocale: 'root',
      locales: {
        root: { label: 'English', lang: 'en', dir: 'ltr' },
      },
      // Starlight's built-in Pagefind search is fully static and FOSS; it
      // works on GitHub Pages without any external service.
      // The 404 page is a content entry (`src/content/docs/404.md`) so its
      // text is translatable with the rest of the docs; disabling Starlight's
      // injected 404 avoids a duplicate `/404` route.
      disable404Route: true,
      pagefind: true,
      customCss: ['./src/styles/custom.css'],
      sidebar: [
        { label: 'Home', link: '/' },
        {
          label: 'Getting Started',
          items: [{ autogenerate: { directory: 'getting-started' } }],
        },
        {
          label: 'Installation',
          items: [{ autogenerate: { directory: 'installation' } }],
        },
        {
          label: 'Library Management',
          items: [{ autogenerate: { directory: 'library-management' } }],
        },
        {
          label: 'Clients',
          items: [{ autogenerate: { directory: 'clients' } }],
        },
        {
          label: 'Features',
          items: [{ autogenerate: { directory: 'features' } }],
        },
        {
          label: 'Administration',
          items: [{ autogenerate: { directory: 'admin' } }],
        },
        {
          label: 'API',
          items: [{ autogenerate: { directory: 'api' } }],
        },
        {
          label: 'Troubleshooting',
          items: [{ autogenerate: { directory: 'troubleshooting' } }],
        },
      ],
      // Internal content links are written relative to the current file so
      // they survive base-path changes (project Pages now, custom domain
      // later). Astro rewrites them with the configured base at build time.
      // Relative links are validated by `pnpm check:links`
      // (docs/scripts/check-relative-links.mjs); the plugin still validates
      // any absolute internal links, hashes, and locale consistency.
      plugins: [starlightLinksValidator({ errorOnRelativeLinks: false })],
    }),
  ],
});

/// <reference types="vitest/config" />
import { createRequire } from 'node:module';
import { dirname } from 'node:path';

import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// Carbon's SCSS emits @font-face URLs with the webpack-style `~@ibm/plex`
// prefix. Vite does not strip `~`; alias it to the real @ibm/plex install
// (a dependency of @carbon/styles, resolved from that package's context so
// pnpm's strict node_modules layout is honored).
const require_ = createRequire(import.meta.url);
const carbonStylesPkg = require_.resolve('@carbon/styles/package.json', {
  paths: [dirname(require_.resolve('@carbon/react/package.json'))],
});
const plexRoot = dirname(
  require_.resolve('@ibm/plex/package.json', { paths: [dirname(carbonStylesPkg)] }),
);

// The SPA is served by the Rust server at /admin (ADR-0138), so the base path is
// fixed and deep links fall back to index.html server-side.
export default defineConfig({
  base: '/admin/',
  plugins: [react()],
  css: {
    preprocessorOptions: {
      scss: {
        // Carbon 1.x stylesheets still use the deprecated Sass if() function
        // form. quietDeps silences deprecation notices originating in
        // dependency stylesheets only; a deprecation in this repository's own
        // SCSS still fails loudly.
        quietDeps: true,
      },
    },
  },
  build: {
    // lightningcss (Vite 8's bundled CSS minifier) rejects Carbon's
    // @position-try nested rules, and esbuild is not bundled by rolldown-vite.
    // Ship unminified CSS until upstream handles the syntax.
    cssMinify: false,
  },
  resolve: {
    alias: [{ find: /^~@ibm\/plex/, replacement: plexRoot }],
  },
  server: {
    port: 5173,
    proxy: {
      '/api': { target: 'http://127.0.0.1:8080', changeOrigin: true },
      '/images': { target: 'http://127.0.0.1:8080', changeOrigin: true },
      '/healthz': { target: 'http://127.0.0.1:8080', changeOrigin: true },
      '/readyz': { target: 'http://127.0.0.1:8080', changeOrigin: true },
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    css: false,
  },
});

import { defineConfig, devices } from '@playwright/test';

// No @types/node in this project; only CI detection is needed here.
declare const process: { env: Record<string, string | undefined> };

/**
 * E2E gate (PRD-TEST-008, ADR-0138). Runs against a production build served
 * by `vite preview` at the real /admin base path; the API is mocked with
 * page.route() so no live server is required.
 *
 * Canonical viewports per UI-SPEC §8: 1600×1200 desktop, 768×1024 tablet,
 * 375×812 mobile.
 */
export default defineConfig({
  testDir: './e2e',
  // The live suites under e2e/live drive a real server and are run separately
  // (`make audit-live`, playwright.live.config.ts); this mocked gate excludes them.
  testIgnore: /live\//,
  timeout: 30_000,
  // The default worker count is half the host CPUs (12 here), which can
  // exhaust a GitHub runner and kill the single Vite preview process halfway
  // through this 351-case cross-viewport suite. Four workers complete within
  // the CI budget while keeping the server stable.
  workers: 4,
  fullyParallel: true,
  // Keep the concise live output while also producing the report that the CI
  // workflow uploads on failure. Without the HTML reporter, a passing upload
  // could contain no report or failed-image attachments for diagnosis.
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: 'http://127.0.0.1:4173/admin/',
    // Visual fixtures include fixed UTC timestamps. Pin the browser timezone
    // to the canonical snapshot timezone so host/runner settings cannot move
    // rendered date-time text.
    timezoneId: 'America/Chicago',
    trace: 'retain-on-failure',
  },
  expect: {
    toHaveScreenshot: {
      // Anti-alias tolerance (W4-2): up to 1% of pixels may differ to absorb
      // sub-pixel text-rendering jitter; anything structural fails. See
      // e2e/visual.spec.ts for the determinism contract.
      maxDiffPixelRatio: 0.01,
      animations: 'disabled',
      caret: 'hide',
    },
  },
  webServer: {
    // Bind 127.0.0.1 explicitly: the default `localhost` may resolve to ::1
    // while the readiness probe hits 127.0.0.1.
    command: 'python3 ../scripts/admin_preview.py',
    url: 'http://127.0.0.1:4173/admin/',
    reuseExistingServer: !process.env.CI,
    timeout: 180_000,
  },
  projects: [
    {
      name: 'desktop-1600x1200',
      use: { ...devices['Desktop Chrome'], viewport: { width: 1600, height: 1200 } },
    },
    {
      name: 'tablet-768x1024',
      use: { ...devices['Desktop Chrome'], viewport: { width: 768, height: 1024 } },
    },
    {
      name: 'mobile-375x812',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 375, height: 812 },
        isMobile: true,
        hasTouch: true,
      },
    },
  ],
});

import { defineConfig, devices } from '@playwright/test';

// No @types/node in this project; only env access is needed here.
declare const process: { env: Record<string, string | undefined> };

/**
 * Live Admin UI audit (no API mocks) — see e2e/live/.
 *
 * This project is intentionally separate from `playwright.config.ts`: the default
 * gate mocks the Admin API so it can run without a server, which is precisely the
 * seam that let contract drift through. This config drives a real running
 * instance and a real administrator session.
 *
 *   MELODEE_LIVE_URL=http://127.0.0.1:8080/admin \
 *   MELODEE_LIVE_USERNAME=... MELODEE_LIVE_PASSWORD=... \
 *     pnpm exec playwright test --config playwright.live.config.ts
 *
 * No `webServer`: the instance must already be up (`make up`). Tests skip
 * themselves when credentials are absent, so an ad-hoc run is never a false pass.
 */
export default defineConfig({
  testDir: './e2e/live',
  timeout: 60_000,
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
  use: {
    baseURL: process.env.MELODEE_LIVE_URL ?? 'http://127.0.0.1:8080/admin',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    // Authenticate once (the API rate-limits sign-in), then reuse the session.
    { name: 'live-setup', testMatch: /auth\.setup\.ts/ },
    {
      name: 'live-desktop',
      dependencies: ['live-setup'],
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1600, height: 1200 },
        storageState:
          process.env.MELODEE_LIVE_STATE ?? '/tmp/melodee-live-auth-state.json',
      },
    },
  ],
});

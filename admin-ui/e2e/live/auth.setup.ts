import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';

import { expect, test as setup } from '@playwright/test';

/**
 * Live-suite authentication setup.
 *
 * The Admin API rate-limits sign-in, so a suite that signs in per test exhausts
 * the budget and starts seeing `429`. Standard Playwright fix: authenticate once,
 * persist the storage state, and let every test start already signed in.
 *
 * Runs as a dependency of the `live-desktop` project; see playwright.live.config.ts.
 */

const LIVE_API = process.env.MELODEE_LIVE_API_URL ?? 'http://127.0.0.1:8080';
const USERNAME = process.env.MELODEE_LIVE_USERNAME ?? '';
const PASSWORD = process.env.MELODEE_LIVE_PASSWORD ?? '';
export const STATE_PATH =
  process.env.MELODEE_LIVE_STATE ?? '/tmp/melodee-live-auth-state.json';

setup('authenticate', async ({ request }) => {
  setup.skip(!USERNAME || !PASSWORD, 'needs MELODEE_LIVE_USERNAME / MELODEE_LIVE_PASSWORD');

  const response = await request.post(`${LIVE_API}/api/v1/auth/cookie/sign-in`, {
    data: { userName: USERNAME, password: PASSWORD },
  });
  expect(response.status(), 'admin sign-in').toBe(200);

  const cookies = (await request.storageState()).cookies;
  expect(cookies.length, 'session cookie issued').toBeGreaterThan(0);

  mkdirSync(dirname(STATE_PATH), { recursive: true });
  writeFileSync(
    STATE_PATH,
    JSON.stringify({ cookies, origins: [] }, null, 2),
    { mode: 0o600 },
  );
});

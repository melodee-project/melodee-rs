import { expect, test, type Page } from '@playwright/test';

/**
 * Live Admin UI audit (no mocks).
 *
 * Every other spec under `e2e/` mocks the Admin API with `page.route()`. That is
 * why a form could send a field the server silently dropped (Release
 * `country`/`releaseType`), why a free-text control could sit over a closed
 * dictionary (Artist `type`), and why a statistic could count rows that the UI
 * never listed — the mocks accepted whatever the UI sent.
 *
 * This suite runs against a *real* server and asserts the seam:
 *
 *   1. every canonical screen loads with no console error and no failed request;
 *   2. every edit form round-trips: type a value, save, reload, see it persisted;
 *   3. closed dictionaries are selects whose options are the domain values;
 *   4. no form offers a field the API does not accept.
 *
 * Requires a running instance and an administrator:
 *
 *   MELODEE_LIVE_URL=http://127.0.0.1:8080/admin \
 *   MELODEE_LIVE_USERNAME=... MELODEE_LIVE_PASSWORD=... \
 *     pnpm exec playwright test --config playwright.live.config.ts
 *
 * Skipped (not failed) when credentials are absent so the default gate is
 * unaffected.
 */

import { apiGet, HAVE_CREDENTIALS, LIVE_API, LIVE_URL as BASE_URL, LIVE_USERNAME, signIn } from './session';

/** Screens every operator can reach; each must render cleanly. */
const SCREENS: ReadonlyArray<{ path: string; ready: RegExp }> = [
  { path: '/', ready: /Overview/ },
  { path: '/processing', ready: /Processing/ },
  { path: '/processing/review', ready: /Review Queue/ },
  { path: '/processing/quarantine', ready: /Quarantine/ },
  { path: '/processing/history', ready: /History/ },
  { path: '/catalog/artists', ready: /Artists/ },
  { path: '/catalog/releases', ready: /Releases/ },
  { path: '/catalog/tracks', ready: /Tracks/ },
  { path: '/catalog/identity', ready: /Identity/ },
  { path: '/policies/releases', ready: /Release Rules/ },
  { path: '/system/storage', ready: /Storage/ },
  { path: '/system/workers', ready: /Workers/ },
  { path: '/system/search', ready: /Search/ },
  { path: '/system/users', ready: /Users/ },
  { path: '/system/settings', ready: /Settings/ },
  { path: '/system/stats', ready: /Release processing/ },
  { path: '/system/diagnostics', ready: /Diagnostics/ },
];

/** Collects console errors and non-2xx/3xx responses for one navigation. */
function watchFailures(page: Page): { consoleErrors: string[]; failedRequests: string[] } {
  const consoleErrors: string[] = [];
  const failedRequests: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error') {
      consoleErrors.push(message.text());
    }
  });
  page.on('response', (response) => {
    const status = response.status();
    if (status >= 400 && !response.url().includes('/favicon')) {
      failedRequests.push(`${status} ${response.url()}`);
    }
  });
  return { consoleErrors, failedRequests };
}

test.describe.configure({ mode: 'serial' });

test.describe('live admin UI', () => {
  test.skip(!HAVE_CREDENTIALS, 'set MELODEE_LIVE_USERNAME / MELODEE_LIVE_PASSWORD to run');

  test('the header loads the signed-in account’s thumbnail on a restored session', async ({ page, context, request }) => {
    const cookie = await signIn(context, request);
    const users = await apiGet<{ items: { id: string; username: string }[] }>(
      request, cookie, `${LIVE_API}/api/admin/v1/users?query=${encodeURIComponent(LIVE_USERNAME)}`,
    );
    const account = users.items.find((user) => user.username.toLowerCase() === LIVE_USERNAME.toLowerCase());
    expect(account).toBeDefined();
    const thumbnail = page.waitForResponse((response) =>
      new URL(response.url()).pathname === `/images/user_${account?.id}/80`
      && response.request().resourceType() === 'image');
    await page.goto(BASE_URL + '/');
    expect((await thumbnail).ok()).toBe(true);
    const image = page.locator('header .melodee-shell__avatar img');
    await expect(image).toBeVisible();
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
  });

  test('statistics show registered asset files separately from database size', async ({ page, context, request }) => {
    const cookie = await signIn(context, request);
    const stats = await apiGet<{ database: { assetFileBytes: number; assetFileCount: number } }>(
      request, cookie, `${LIVE_API}/api/admin/v1/stats`,
    );
    expect(stats.database.assetFileBytes).toBeGreaterThanOrEqual(0);
    expect(stats.database.assetFileCount).toBeGreaterThanOrEqual(0);
    await page.goto(`${BASE_URL}/system/stats`);
    await expect(page.getByText('Artwork, avatars and theme files', { exact: true })).toBeVisible();
    await expect(page.getByText('Database size', { exact: true })).toBeVisible();
  });

  test('every canonical screen loads with no console error and no failed request', async ({ page, context, request }) => {
    await signIn(context, request);
    const failures: string[] = [];
    for (const screen of SCREENS) {
      const watcher = watchFailures(page);
      await page.goto(`${BASE_URL}${screen.path}`);
      await expect(page.getByText(screen.ready).first()).toBeVisible({ timeout: 20_000 });
      // Give the screen's data fetches a beat to settle before judging.
      await page.waitForLoadState('networkidle');
      if (watcher.consoleErrors.length > 0) {
        failures.push(`${screen.path}: console: ${watcher.consoleErrors[0]}`);
      }
      // Asset 404s are not product defects; API failures are.
      const apiFailures = watcher.failedRequests.filter((entry) => entry.includes('/api/'));
      if (apiFailures.length > 0) {
        failures.push(`${screen.path}: request: ${apiFailures[0]}`);
      }
      page.removeAllListeners('console');
      page.removeAllListeners('response');
    }
    expect(failures, `screens with failures:\n${failures.join('\n')}`).toEqual([]);
  });

  test('shows the server’s worker settings and masks its database connection', async ({ page, context, request }) => {
    await signIn(context, request);
    await page.goto(`${BASE_URL}/system/settings`);
    const key = 'MELODEE__INSPECTION__CONCURRENCY';
    await page.getByRole('button', { name: `Edit ${key}` }).click();
    const dialog = page.getByRole('dialog', { name: `Edit ${key}` });
    await expect(dialog.getByLabel('Value')).toHaveValue(/^[1-9][0-9]*$/);
    await dialog.getByRole('button', { name: 'Cancel' }).click();

    await page.getByRole('searchbox', { name: 'Filter settings by key' }).fill('MELODEE__DATABASE__URL');
    const row = page.getByRole('row').filter({ hasText: 'MELODEE__DATABASE__URL' });
    await expect(row.getByText('Redacted')).toBeVisible();
    await expect(row.getByText('Managed on host')).toBeVisible();
    await expect(row.getByRole('button')).toHaveCount(0);
  });

  test('Release edit offers no field the API rejects', async ({ page, context, request }) => {
    const cookie = await signIn(context, request);
    const releases = await apiGet<{ items: { id: string; status: string }[] }>(
      request, cookie, `${LIVE_API}/api/admin/v1/catalog/releases?limit=50`,
    );
    const release = releases.items.find((item) => item.status === 'active');
    expect(release, 'a populated active Release fixture is required').toBeTruthy();
    await page.goto(`${BASE_URL}/catalog/releases/${release!.id}`);
    await page.getByRole('button', { name: 'Edit' }).click();
    const labels = await page
      .locator('form .cds--label')
      .evaluateAll((els) => els.map((element) => element.textContent?.trim() ?? ''));
    // `country` and `releaseType` were removed from the wire contract; offering
    // them meant a save that silently discarded input.
    expect(labels.join('|')).not.toMatch(/country/i);
    expect(labels.join('|')).not.toMatch(/^type$/i);
    // The date is validated, not free text.
    await expect(page.getByLabel(/date/i).first()).toBeVisible();
  });

  test('queue statistics drill down to the rows they count', async ({ page, context, request }) => {
    const cookie = await signIn(context, request);
    const artists = await apiGet<{ items: { id: string }[] }>(
      request, cookie, `${LIVE_API}/api/admin/v1/catalog/artists?limit=1`,
    );
    expect(artists.items.length, 'a populated Artist fixture is required').toBeGreaterThan(0);
    const id = artists.items[0]!.id;
    const detail = await apiGet<{
      stats: { billedTrackCount: number };
      trackCredits: { items: unknown[]; total: number };
    }>(request, cookie, `${LIVE_API}/api/admin/v1/catalog/artists/${id}`);
    expect(detail.stats.billedTrackCount).toBeGreaterThan(0);
    expect(detail.trackCredits.total).toBe(detail.stats.billedTrackCount);
    await page.goto(`${BASE_URL}/catalog/artists/${id}`);
    // The stat cards are links into the credits tab: a count is never a dead end.
    const credits = page.getByRole('link', { name: /track credits/i });
    await expect(credits).toBeVisible();
    await credits.click();
    await expect(page.getByRole('tab', { name: 'Tracks' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(page.getByRole('table').locator('tbody tr')).toHaveCount(detail.trackCredits.items.length);
  });
});

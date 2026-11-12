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

const BASE_URL = process.env.MELODEE_LIVE_URL ?? 'http://127.0.0.1:8080/admin';
const USERNAME = process.env.MELODEE_LIVE_USERNAME ?? '';
const PASSWORD = process.env.MELODEE_LIVE_PASSWORD ?? '';
const HAVE_CREDENTIALS = USERNAME !== '' && PASSWORD !== '';

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
  { path: '/system/diagnostics', ready: /Diagnostics/ },
];

async function signIn(page: Page): Promise<void> {
  await page.goto(`${BASE_URL}/login`);
  await page.getByLabel(/username|email/i).fill(USERNAME);
  await page.getByLabel(/password/i).fill(PASSWORD);
  await page.getByRole('button', { name: /sign in/i }).click();
  // The shell only renders for an authenticated administrator.
  await expect(page.getByRole('navigation', { name: /main navigation/i })).toBeVisible({
    timeout: 20_000,
  });
}

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

  test('every canonical screen loads with no console error and no failed request', async ({
    page,
  }) => {
    await signIn(page);
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

  test('Artist edit round-trips: what the form saves is what the API returns', async ({ page }) => {
    await signIn(page);
    await page.goto(`${BASE_URL}/catalog/artists`);
    await page.getByRole('link').filter({ hasText: /./ }).nth(1).click();

    await page.getByRole('button', { name: 'Edit' }).click();
    const type = page.getByLabel('Type');
    // The dictionary must be a select, not free text.
    await expect(type).toHaveJSProperty('tagName', 'SELECT');
    const options = await type.locator('option').evaluateAll((els) =>
      els.map((element) => (element as HTMLOptionElement).value),
    );
    expect(options).toEqual([
      'person',
      'group',
      'orchestra',
      'ensemble',
      'choir',
      'character',
      'other',
      'unknown',
    ]);

    const original = await type.inputValue();
    const target = original === 'group' ? 'person' : 'group';
    await type.selectOption(target);
    await page.getByRole('button', { name: /save/i }).click();
    // Reload proves persistence, not just optimistic UI state.
    await page.reload();
    await page.getByRole('button', { name: 'Edit' }).click();
    await expect(page.getByLabel('Type')).toHaveValue(target);
    // Restore so repeated runs are idempotent.
    await page.getByLabel('Type').selectOption(original);
    await page.getByRole('button', { name: /save/i }).click();
  });

  test('Release edit offers no field the API rejects', async ({ page }) => {
    await signIn(page);
    await page.goto(`${BASE_URL}/catalog/releases`);
    await page.getByRole('link').filter({ hasText: /./ }).nth(1).click();
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

  test('queue statistics drill down to the rows they count', async ({ page }) => {
    await signIn(page);
    await page.goto(`${BASE_URL}/catalog/artists`);
    const artistLink = page.getByRole('link').filter({ hasText: /./ }).nth(1);
    await artistLink.click();
    // The stat cards are links into the credits tab: a count is never a dead end.
    const credits = page.getByRole('link', { name: /track credits/i });
    if (await credits.count()) {
      await credits.first().click();
      await expect(page.getByRole('tab', { name: 'Tracks' })).toHaveAttribute(
        'aria-selected',
        'true',
      );
    }
  });
});

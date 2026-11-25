import { readFile } from 'node:fs/promises';
import { expect, test, type Page } from '@playwright/test';

/**
 * Shell smoke: renders the canonical IA in English and honors RTL when the
 * operator switches to ar-SA (PRD-I18N-005). The admin API is mocked — no
 * live server is required.
 */

async function mockAdminApi(page: Page) {
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === '/api/admin/v1/system/info') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ service: 'melodee', version: '1.2.3-rc.4', status: 'ok' }),
      });
      return;
    }
    if (url.pathname === '/api/admin/v1/dashboard') {
      // The shell lands on the overview dashboard; an empty catalog
      // is a valid server response.
      const subsystem = { status: 'ok', detail: 'ok' };
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          generatedAt: '2026-08-14T12:00:00Z',
          catalog: { artists: 0, releases: 0, tracks: 0, mediaAssets: 0, pendingSearchOutbox: 0 },
          workItems: { countsByState: [], failedLast24h: 0, completedLast24h: 0 },
          attention: {
            unresolvedIdentities: 0,
            pendingPolicyDecisions: 0,
            failedProcessing: 0,
            quarantined: 0,
          },
          health: {
            status: 'ok',
            postgresql: subsystem,
            tantivy: subsystem,
            storage: subsystem,
            ffmpeg: subsystem,
            podcastStorage: { status: 'disabled', detail: 'podcast worker disabled' },
            disk: subsystem,
            workers: subsystem,
            apiSurfaces: [],
          },
          activity: { completedPerHour: [] },
        }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ items: [], total: 0, limit: 50, offset: 0 }),
    });
  });
}

test.beforeEach(async ({ page }) => {
  await mockAdminApi(page);
  // Deterministic locale: start every run in English.
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('serves the Melodee favicon from a nested Admin URL', async ({ page }) => {
  await page.goto('/admin/processing/queue');
  const icon = page.locator('head link[rel="icon"]');
  await expect(icon).toHaveAttribute('href', '/admin/favicon.ico');
  const response = await page.request.get('/admin/favicon.ico');
  expect(response.status()).toBe(200);
  expect(response.headers()['content-type']).toMatch(/^image\/(x-icon|vnd\.microsoft\.icon)\b/);
  const original = await readFile(new URL('../../assets/icons/icon.ico', import.meta.url));
  expect((await response.body()).equals(original)).toBe(true);
});

test('renders the shell navigation groups in English', async ({ page }) => {
  await page.goto('/');

  // UI-SPEC §8: below lg the side nav starts collapsed to icons/overlay;
  // expand it via the header control before asserting labels.
  const expandButton = page.getByRole('button', { name: 'Expand navigation' }).first();
  if ((page.viewportSize()?.width ?? 1600) < 1056) {
    await expect(expandButton).toBeVisible();
    await expandButton.click();
  }

  const nav = page.getByRole('navigation', { name: 'Main navigation' });
  await expect(nav.getByText('Overview').first()).toBeVisible();
  await expect(nav.getByText('Processing').first()).toBeVisible();
  await expect(nav.getByText('Catalog').first()).toBeVisible();
  await expect(nav.getByText('Policies').first()).toBeVisible();
  await expect(nav.getByText('System').first()).toBeVisible();
  const footer = nav.locator('.melodee-shell__footer');
  await expect(footer.getByText('Version 1.2.3-rc.4', { exact: true })).toBeVisible();
  await expect(footer.getByText('System', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Appearance: System' })).toBeVisible();
  await footer.getByRole('button', { name: 'Collapse navigation' }).click();
  await expect(footer.getByText('Version 1.2.3-rc.4')).toHaveCount(0);

  await expect(page.locator('html')).toHaveAttribute('dir', 'ltr');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en-US');
});

/**
 * Shell chrome invariants (ADR-0184). These are asserted structurally, not only
 * by the visual baselines: the screenshot tolerance (1% of pixels) is wider than
 * a status line or a brand glyph, so a duplicated status or the wrong brand mark
 * could otherwise pass unnoticed.
 */
test('server status is reported once, by the header, and the rail shows the badge', async ({
  page,
}) => {
  await page.goto('/');

  // One status indicator, in the header.
  const headerStatus = page.locator('.melodee-shell__health');
  await expect(headerStatus).toBeVisible();
  await expect(headerStatus).toContainText(/server status unknown|server online/i);

  // Server health has a single indicator, even when the footer shows the version.
  const footer = page.locator('.melodee-shell__footer');
  await expect(footer).toBeVisible();
  await expect(footer).not.toContainText(/server online|server offline|checking server status/i);

  // The brand mark is the official circular badge, not an interim monogram.
  await expect(page.locator('.melodee-shell__brand-mark')).toHaveAttribute(
    'src',
    '/admin/brand/melodee-circular-badge.svg',
  );
  // Count the status indicators in the whole shell: exactly one.
  await expect(page.locator('.melodee-shell__health')).toHaveCount(1);
});

test('the footer follows the running server across a failed poll and recovery', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 1056, 'expanded desktop navigation');
  await page.clock.install();
  await page.goto('/');
  const footer = page.locator('.melodee-shell__footer');
  await expect(footer).toContainText('Version 1.2.3-rc.4');
  await page.route('**/api/admin/v1/system/info', (route) => route.fulfill({ status: 503 }));
  await page.clock.fastForward(30_000);
  await expect(page.locator('.melodee-shell__health')).toContainText(/server offline/i);
  await expect(footer).not.toContainText('Version');
  await page.route('**/api/admin/v1/system/info', (route) => route.fulfill({
    json: { service: 'melodee', version: '1.2.3', status: 'ok' },
  }));
  await page.clock.fastForward(30_000);
  await expect(footer.getByText('Version 1.2.3', { exact: true })).toBeVisible();
});

test('switching to ar-SA sets dir=rtl on the document root', async ({ page }) => {
  await page.goto('/');

  await page.getByRole('button', { name: /^Language:/ }).click();
  await expect(page.getByLabel('Deutsch — Preview', { exact: true })).toContainText('Preview');
  await expect(page.getByLabel('English (US)', { exact: true })).not.toContainText('Preview');
  await expect(page.getByText('Preview languages use English where translations are missing.')).toBeVisible();
  // SwitcherItem entries are anchors without href (roleless); click by text.
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
});

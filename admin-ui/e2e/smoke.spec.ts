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
        body: JSON.stringify({ service: 'melodee', version: 'test', status: 'ok' }),
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

  // The rail footer carries locale, appearance, and collapse only.
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

test('switching to ar-SA sets dir=rtl on the document root', async ({ page }) => {
  await page.goto('/');

  await page.getByRole('button', { name: /^Language:/ }).click();
  // SwitcherItem entries are anchors without href (roleless); click by text.
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
});

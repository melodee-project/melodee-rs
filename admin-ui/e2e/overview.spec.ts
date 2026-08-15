import { expect, test, type Page } from '@playwright/test';

/**
 * Overview dashboard e2e (WP-14). The admin API is mocked with page.route()
 * — no live server is required. Runs at all three canonical viewports
 * (UI-SPEC §8) via the playwright projects, plus the ar-SA dir=rtl smoke.
 */

const DASHBOARD = {
  generatedAt: '2026-08-14T12:00:00Z',
  catalog: {
    artists: 35214,
    releases: 50482,
    tracks: 560291,
    mediaAssets: 600000,
    pendingSearchOutbox: 12,
  },
  workItems: {
    countsByState: [
      { state: 'discovered', count: 3 },
      { state: 'ready', count: 5 },
      { state: 'needs_review', count: 2 },
      { state: 'failed', count: 1 },
    ],
    failedLast24h: 4,
    completedLast24h: 128,
  },
  attention: {
    unresolvedIdentities: 23,
    pendingPolicyDecisions: 14,
    failedProcessing: 1,
    quarantined: 8,
  },
  health: {
    status: 'ok',
    postgresql: { status: 'ok', detail: 'postgresql answered a bounded probe' },
    tantivy: { status: 'ok', detail: 'active search generation validated' },
    storage: { status: 'ok', detail: 'storage paths writable' },
    ffmpeg: { status: 'ok', detail: 'ffmpeg 8.1' },
    apiSurfaces: [
      { surface: 'native', status: 'serving' },
      { surface: 'opensubsonic', status: 'serving' },
      { surface: 'jellyfin', status: 'serving' },
      { surface: 'admin', status: 'serving' },
    ],
  },
  activity: {
    completedPerHour: [
      { hour: '2026-08-14T10:00:00Z', count: 5 },
      { hour: '2026-08-14T11:00:00Z', count: 9 },
      { hour: '2026-08-14T12:00:00Z', count: 7 },
    ],
  },
};

async function mockAdminApi(page: Page) {
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const json = (body: unknown) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
    if (pathname === '/api/admin/v1/system/info') {
      await json({ service: 'melodee', version: 'test', status: 'ok' });
      return;
    }
    if (pathname === '/api/admin/v1/dashboard') {
      await json(DASHBOARD);
      return;
    }
    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ code: 'not_found', message: 'not found', correlationId: null }),
    });
  });
}

test.beforeEach(async ({ page }) => {
  await mockAdminApi(page);
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('loads the dashboard with KPIs, attention, health, activity, and queue summary', async ({
  page,
}) => {
  await page.goto('/');

  // KPI cards from the dashboard DTO.
  await expect(page.getByText('35,214')).toBeVisible();
  await expect(page.getByText('560,291')).toBeVisible();
  await expect(page.getByText('Failed (24h)')).not.toBeVisible();

  // Needs attention panel with links into the owning screens.
  const attention = page.locator('section', { has: page.getByRole('heading', { name: 'Needs attention' }) });
  await expect(attention.getByText('Unresolved artist identities')).toBeVisible();
  await expect(attention.getByText('23')).toBeVisible();
  await expect(attention.getByRole('link', { name: 'Failed processing' })).toBeVisible();
  await expect(attention.getByRole('link', { name: /Quarantined/ })).toHaveAttribute(
    'href',
    '/admin/processing/review?state=quarantined',
  );

  // System health panel: subsystem rows with textual verdicts.
  const health = page.locator('section', { has: page.getByRole('heading', { name: 'System health' }) });
  await expect(health.getByText('PostgreSQL', { exact: true })).toBeVisible();
  await expect(health.getByText('Tantivy search index')).toBeVisible();
  await expect(health.getByText('OpenSubsonic')).toBeVisible();

  // Accessible activity chart.
  await expect(
    page.getByRole('img', { name: 'Bar chart of completed work items per hour' }),
  ).toBeVisible();

  // Mini queue table with a link into the processing queue.
  await expect(page.getByRole('table', { name: 'Work items by state' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Open the processing queue' })).toBeVisible();
});

test('attention rows navigate to the owning screens', async ({ page }) => {
  await page.goto('/');
  const attention = page.locator('section', { has: page.getByRole('heading', { name: 'Needs attention' }) });
  await attention.getByRole('link', { name: /Unresolved artist identities/ }).click();
  await expect(page).toHaveURL(/\/catalog\/identity/);
});

test('rtl smoke: ar-SA renders the dashboard with dir=rtl', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('35,214')).toBeVisible();

  await page.getByRole('button', { name: 'Language', exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
  // Untranslated admin vocabulary falls back to English; the chrome mirrors.
  await expect(page.getByText('35,214')).toBeVisible();
  await expect(
    page.getByRole('img', { name: 'Bar chart of completed work items per hour' }),
  ).toBeVisible();
});

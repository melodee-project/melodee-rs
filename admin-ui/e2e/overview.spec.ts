import { expect, test, type Page } from '@playwright/test';

/**
 * Overview dashboard e2e. The admin API is mocked with page.route
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
    podcastStorage: { status: 'disabled', detail: 'podcast worker disabled' },
    disk: { status: 'ok', detail: 'tightest storage root has sufficient free space' },
    workers: { status: 'ok', detail: 'no stale leases; worker queues are healthy' },
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
    if (pathname === '/api/admin/v1/ingest/work-items') {
      // One bound queue row: the preview shows the Release, its artist, and
      // the stage progress (ADR-0184).
      await json({
        items: [
          {
            id: '11111111-1111-4111-8111-111111111111',
            apiKey: '33333333-3333-4333-8333-333333333333',
            sourceLibraryId: null,
            targetLibraryId: null,
            sourcePath: '/library/incoming/Business as Usual',
            currentPath: '/library/incoming/Business as Usual',
            targetPath: null,
            sourceFingerprint: null,
            manifestHash: null,
            state: 'publishing',
            stateReason: null,
            attemptCount: 1,
            nextAttemptAt: null,
            leaseOwner: null,
            leaseAcquiredAt: null,
            leaseExpiresAt: null,
            lastErrorCode: null,
            lastErrorMessage: null,
            correlationId: '44444444-4444-4444-8444-444444444444',
            discoveredAt: '2026-08-14T10:00:00Z',
            firstStartedAt: null,
            lastAttemptAt: null,
            updatedAt: '2026-08-14T10:05:00Z',
            createdAt: '2026-08-14T10:00:00Z',
            completedAt: null,
            releaseId: '55555555-5555-4555-8555-555555555555',
            releaseTitle: 'Business as Usual',
            artistName: 'Men at Work',
            mediaBytes: 33600000,
          },
        ],
        total: 1,
        limit: 5,
        offset: 0,
      });
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
    page.getByRole('img', { name: 'Line chart of Releases processed and Tracks indexed per hour' }),
  ).toBeVisible();

  // Release-level queue preview with a link into the processing queue.
  const queue = page.getByRole('table', { name: 'Current processing queue' });
  await expect(queue).toBeVisible();
  await expect(queue.getByText('Men at Work')).toBeVisible();
  await expect(
    queue.getByRole('progressbar', { name: /processing stage/ }).first(),
  ).toBeVisible();
  await expect(page.getByRole('link', { name: 'Open the processing queue' })).toBeVisible();
});

test('attention rows navigate to the owning screens', async ({ page }) => {
  await page.goto('/');
  const attention = page.locator('section', { has: page.getByRole('heading', { name: 'Needs attention' }) });
  await attention.getByRole('link', { name: /Unresolved artist identities/ }).click();
  await expect(page).toHaveURL(/\/catalog\/identity/);
});

for (const activation of ['click', 'keyboard'] as const) {
  test(`Needs review card opens the filtered items using ${activation}`, async ({ page }) => {
    await page.route('**/api/admin/v1/dashboard', (route) => route.fulfill({
      json: {
        ...DASHBOARD,
        workItems: {
          ...DASHBOARD.workItems,
          countsByState: [{ state: 'needs_review', count: 1 }, { state: 'failed', count: 3 }],
        },
      },
    }));
    await page.route('**/api/admin/v1/review/queue?*', (route) => route.fulfill({
      json: {
        items: [{
          id: '11111111-1111-4111-8111-111111111111',
          state: 'needs_review',
          currentPath: '/library/incoming/Release needing review',
          sourcePath: '/library/incoming/Release needing review',
          attemptCount: 1,
          createdAt: '2026-08-14T10:00:00Z',
          updatedAt: '2026-08-14T10:05:00Z',
          lastErrorMessage: null,
        }],
        total: 1, limit: 50, offset: 0,
        countsByState: [{ state: 'needs_review', count: 1 }, { state: 'failed', count: 3 }],
      },
    }));
    await page.goto('/');
    const card = page.getByRole('link', { name: /Needs review/ });
    await expect(card.getByText('1', { exact: true })).toBeVisible();
    await expect(card).toHaveAttribute('href', '/admin/processing/review?state=needs_review');
    const reviewRequest = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return url.pathname === '/api/admin/v1/review/queue' && url.searchParams.get('state') === 'needs_review';
    });
    if (activation === 'keyboard') {
      await card.focus();
      await expect(card).toBeFocused();
      await card.press('Enter');
    } else {
      await card.getByText('1', { exact: true }).click();
    }
    await reviewRequest;
    await expect(page).toHaveURL(/\/admin\/processing\/review\?state=needs_review$/);
    await expect(page.getByText('Release needing review', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: /Needs review/ })).toHaveAttribute('aria-pressed', 'true');
  });
}

test('rtl smoke: ar-SA renders the dashboard with dir=rtl', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('35,214')).toBeVisible();

  await page.getByRole('button', { name: /^Language:/ }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
  // Untranslated admin vocabulary falls back to English; the chrome mirrors.
  await expect(page.getByText('35,214')).toBeVisible();
  await expect(
    page.getByRole('img', { name: 'Line chart of Releases processed and Tracks indexed per hour' }),
  ).toBeVisible();
});

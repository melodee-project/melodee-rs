import { expect, test, type Page } from '@playwright/test';

/**
 * System / Workers / Diagnostics e2e (WP-17). The admin API is mocked with
 * page.route() — no live server is required. Runs at all three canonical
 * viewports (UI-SPEC §8) via the playwright projects. Covers the pools
 * table, the pause/resume lease-coordinated control flow with mocked POSTs,
 * the search rebuild 409 surface, and the ar-SA dir=rtl smoke.
 */

const POLICY_POOL = {
  name: 'policy',
  paused: false,
  pausedBy: null,
  pausedAt: null,
  resumeRequestedAt: null,
  updatedAt: '2026-08-14T12:00:00Z',
  metricsAvailable: true,
  queued: 7,
  running: 2,
  oldestQueuedAgeSeconds: 95,
  completedLastHour: 60,
  throughputPerMinute: 1,
  failedLast24h: 0,
};

const DISCOVERY_POOL = {
  ...POLICY_POOL,
  name: 'discovery',
  metricsAvailable: false,
  queued: null,
  running: null,
  oldestQueuedAgeSeconds: null,
  completedLastHour: null,
  throughputPerMinute: null,
  failedLast24h: null,
};

const HEALTH = {
  status: 'ok',
  postgresql: { status: 'ok', detail: 'postgresql answered a bounded probe' },
  tantivy: { status: 'ok', detail: 'active search generation validated' },
  storage: { status: 'ok', detail: 'storage paths writable' },
  ffmpeg: { status: 'ok', detail: 'ffmpeg 8.1' },
  apiSurfaces: [
    { surface: 'native', status: 'serving' },
    { surface: 'admin', status: 'serving' },
  ],
};

const FAILURE = {
  id: '11111111-1111-4111-8111-111111111111',
  apiKey: '33333333-3333-4333-8333-333333333333',
  sourceLibraryId: null,
  targetLibraryId: null,
  sourcePath: '/library/incoming/Business as Usual',
  currentPath: '/library/incoming/Business as Usual',
  targetPath: null,
  sourceFingerprint: null,
  manifestHash: null,
  state: 'failed',
  stateReason: null,
  attemptCount: 5,
  nextAttemptAt: null,
  leaseOwner: null,
  leaseAcquiredAt: null,
  leaseExpiresAt: null,
  lastErrorCode: 'checksum_mismatch',
  lastErrorMessage: 'checksum mismatch',
  correlationId: '44444444-4444-4444-8444-444444444444',
  discoveredAt: '2026-08-14T10:00:00Z',
  firstStartedAt: null,
  lastAttemptAt: null,
  updatedAt: '2026-08-14T10:05:00Z',
  createdAt: '2026-08-14T10:00:00Z',
  completedAt: null,
};

const SEARCH_STATUS = {
  enabled: true,
  projection: {
    pending: 3,
    failed: 0,
    leased: 1,
    oldestAvailableAt: null,
    lastProcessedOutboxId: 42,
    activeGenerationId: '55555555-5555-4555-8555-555555555555',
    latestRebuildStatus: null,
    latestRebuildError: null,
  },
  generation: { artists: 100, releases: 200, tracks: 300, total: 600, schemaVersion: 1 },
  generationErrorCode: null,
  generationError: null,
};

const LEASE = {
  workItemId: '66666666-6666-4666-8666-666666666666',
  apiKey: '77777777-7777-4777-8777-777777777777',
  state: 'inspecting',
  leaseOwner: 'worker-3',
  leaseAcquiredAt: '2026-08-14T11:55:00Z',
  leaseExpiresAt: '2026-08-14T12:05:00Z',
  lastActivityAt: '2026-08-14T11:56:00Z',
};

const DASHBOARD = {
  generatedAt: '2026-08-14T12:00:00Z',
  catalog: { artists: 1, releases: 2, tracks: 3, mediaAssets: 4, pendingSearchOutbox: 0 },
  workItems: {
    countsByState: [{ state: 'failed', count: 1 }],
    failedLast24h: 3,
    completedLast24h: 4231,
  },
  attention: {
    unresolvedIdentities: 0,
    pendingPolicyDecisions: 0,
    failedProcessing: 1,
    quarantined: 0,
  },
  health: HEALTH,
  activity: { completedPerHour: [] },
};

async function mockAdminApi(
  page: Page,
  options: { rebuildConflict?: boolean } = {},
): Promise<{ pauseCalls: string[]; resumeCalls: string[] }> {
  const pauseCalls: string[] = [];
  const resumeCalls: string[] = [];
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (pathname === '/api/admin/v1/system/info') {
      await json({ service: 'melodee', version: 'test', status: 'ok' });
      return;
    }
    if (pathname === '/api/admin/v1/dashboard') {
      await json(DASHBOARD);
      return;
    }
    if (pathname === '/api/admin/v1/system/health') {
      await json(HEALTH);
      return;
    }
    if (pathname === '/api/admin/v1/workers/pools') {
      await json({ items: [POLICY_POOL, DISCOVERY_POOL], total: 2, limit: 2, offset: 0 });
      return;
    }
    if (pathname === '/api/admin/v1/workers/pools/policy/pause') {
      pauseCalls.push('policy');
      await json({ ...POLICY_POOL, paused: true, pausedBy: 'admin-1' });
      return;
    }
    if (pathname === '/api/admin/v1/workers/pools/policy/resume') {
      resumeCalls.push('policy');
      await json(POLICY_POOL);
      return;
    }
    if (pathname === '/api/admin/v1/workers/failures') {
      await json({ items: [FAILURE], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === '/api/admin/v1/search/status') {
      await json(SEARCH_STATUS);
      return;
    }
    if (pathname === '/api/admin/v1/search/rebuild') {
      if (options.rebuildConflict) {
        await json(
          { code: 'rebuild_in_progress', message: 'an unfinished rebuild exists', correlationId: null },
          409,
        );
      } else {
        await json(
          {
            jobId: '88888888-8888-4888-8888-888888888888',
            status: 'pending',
            requestedBy: 'admin-1',
          },
          202,
        );
      }
      return;
    }
    if (pathname === '/api/admin/v1/ingest/leases') {
      await json([LEASE]);
      return;
    }
    await json({ code: 'not_found', message: 'not found', correlationId: null }, 404);
  });
  return { pauseCalls, resumeCalls };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('loads the system screen with pools, health, failures, search, and leases', async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto('system/workers');

  const poolsTable = page.getByRole('table', { name: 'Worker pools and queue metrics' });
  await expect(poolsTable).toBeVisible();
  await expect(poolsTable.getByText('policy')).toBeVisible();
  await expect(poolsTable.getByText('discovery')).toBeVisible();
  // Pools without a durable queue table report honest absence, not zeros.
  await expect(poolsTable.getByText('Not reported').first()).toBeVisible();

  // KPI cards.
  await expect(page.getByText('Completed (24h)')).toBeVisible();
  await expect(page.getByText('4,231')).toBeVisible();

  // Health panel, failures, search status, leases.
  await expect(page.getByRole('heading', { name: 'System Health' })).toBeVisible();
  await expect(
    page.getByRole('table', { name: 'Recent failed work items' }).getByText('checksum mismatch'),
  ).toBeVisible();
  await expect(page.getByText('Indexed documents')).toBeVisible();
  await expect(
    page.getByRole('table', { name: 'Active worker leases' }).getByText('worker-3'),
  ).toBeVisible();

  // Diagnostics: honest note that logs/performance remain CLI surfaces.
  await expect(page.getByText('Logs and performance remain CLI surfaces')).toBeVisible();
});

test('pause requires confirmation and both pause/resume POST to the pool endpoints', async ({
  page,
}) => {
  const { pauseCalls, resumeCalls } = await mockAdminApi(page);
  await page.goto('system/workers');

  const poolsTable = page.getByRole('table', { name: 'Worker pools and queue metrics' });
  const policyRow = poolsTable.locator('tbody tr', { hasText: 'policy' });
  await policyRow.getByRole('button', { name: 'Pause' }).click();

  // The pause is gated behind a localized confirmation modal.
  const dialog = page.getByRole('dialog', { name: 'Pause pool policy' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Pause' }).click();
  await expect.poll(() => pauseCalls.length).toBe(1);
  // The status tag carries icon + label + accessible description (UI-SPEC
  // §5), so assert the durable audit line and the flipped action.
  await expect(policyRow.getByText('Paused by admin-1')).toBeVisible();
  await expect(policyRow.getByRole('button', { name: 'Resume' })).toBeVisible();

  // Resume is a direct action on the now-paused row.
  await policyRow.getByRole('button', { name: 'Resume' }).click();
  await expect.poll(() => resumeCalls.length).toBe(1);
  await expect(policyRow.getByRole('button', { name: 'Pause' })).toBeVisible();
});

test('surfaces the 409 rebuild-in-progress conflict', async ({ page }) => {
  await mockAdminApi(page, { rebuildConflict: true });
  await page.goto('system/workers');

  await page
    .locator('section', { has: page.getByRole('heading', { name: 'Search Index' }) })
    .getByRole('button', { name: 'Rebuild search index' })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Rebuild the search index' });
  // The confirmation states the rebuild is long-running; no fake progress.
  await expect(dialog.getByText(/long-running/)).toBeVisible();
  await dialog.getByRole('button', { name: 'Rebuild search index' }).click();

  await expect(page.getByText('A rebuild is already in progress.')).toBeVisible();
});

test('a failure row navigates to the processing inspector', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('system/workers');

  const failuresTable = page.getByRole('table', { name: 'Recent failed work items' });
  await failuresTable.getByText('/library/incoming/Business as Usual').click();
  await expect(page).toHaveURL(/\/processing\?state=failed&item=11111111/);
});

test('rtl smoke: ar-SA renders the system screen with dir=rtl', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('system/workers');
  await expect(page.getByRole('table', { name: 'Worker pools and queue metrics' })).toBeVisible();

  await page.getByRole('button', { name: 'Language', exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
  await expect(page.getByRole('table', { name: 'Worker pools and queue metrics' })).toBeVisible();
});

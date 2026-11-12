import { expect, test, type Page } from '@playwright/test';

/**
 * Quarantine + History e2e (PRD-INGEST-010): the quarantined work
 * items table pins state=quarantined server-side, rows link to the review
 * workspace, and the inspector opens via URL state. History switches
 * between terminal states server-side. The admin API is mocked.
 */

const ITEM = {
  id: '11111111-1111-4111-8111-111111111111',
  apiKey: '33333333-3333-4333-8333-333333333333',
  sourceLibraryId: null,
  targetLibraryId: null,
  sourcePath: '/library/incoming/Business as Usual',
  currentPath: '/library/incoming/Business as Usual',
  targetPath: null,
  sourceFingerprint: null,
  manifestHash: null,
  state: 'quarantined',
  stateReason: 'operator decision',
  attemptCount: 2,
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
  completedAt: '2026-08-14T10:06:00Z',
};

async function mockAdminApi(page: Page): Promise<{ states: (string | null)[] }> {
  const states: (string | null)[] = [];
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (pathname === '/api/admin/v1/system/info') {
      await json({ service: 'melodee', version: 'test', status: 'ok' });
      return;
    }
    if (pathname === '/api/admin/v1/ingest/work-items') {
      states.push(url.searchParams.get('state'));
      await json({ items: [ITEM], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === `/api/admin/v1/ingest/work-items/${ITEM.id}`) {
      await json(ITEM);
      return;
    }
    if (pathname === `/api/admin/v1/ingest/work-items/${ITEM.id}/events`) {
      await json([
        {
          id: 1,
          workItemId: ITEM.id,
          eventType: 'transitioned',
          fromState: 'needs_review',
          toState: 'quarantined',
          reasonCode: 'operator_decision',
          workerId: null,
          correlationId: '55555555-5555-4555-8555-555555555555',
          createdAt: '2026-08-14T10:05:00Z',
          metadata: null,
        },
      ]);
      return;
    }
    if (pathname === '/api/admin/v1/onboarding/status') {
      await json({
        completed: true,
        bypassed: false,
        canComplete: true,
        completedBy: 'admin',
        checks: [],
      });
      return;
    }
    await json({ code: 'not_found', message: 'not found', correlationId: null }, 404);
  });
  return { states };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('quarantine pins state=quarantined and links to the review workspace', async ({ page }) => {
  const { states } = await mockAdminApi(page);
  await page.goto('processing/quarantine');

  const table = page.getByRole('table', { name: 'Quarantined work items' });
  await expect(table.getByText('Business as Usual')).toBeVisible();
  await expect(table.getByText('quarantined')).toBeVisible();
  expect(states).toContain('quarantined');
  expect(states).not.toContain(null);

  await expect(table.getByRole('link', { name: 'Open review' })).toHaveAttribute(
    'href',
    `/admin/processing/review/${ITEM.id}`,
  );
});

test('the inspector opens via URL state with the event history', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('processing/quarantine');

  await page.getByRole('table', { name: 'Quarantined work items' }).getByText('Business as Usual').click();
  await expect(page).toHaveURL(/item=11111111/);
  await expect(page.getByText(/needs_review → quarantined/)).toBeVisible();
});

test('history switches terminal states server-side', async ({ page }) => {
  const { states } = await mockAdminApi(page);
  await page.goto('processing/history');

  const table = page.getByRole('table', { name: 'Processing and disposition history' });
  await expect(table.getByText('Business as Usual')).toBeVisible();
  // History defaults to `indexed`, the successful terminal state.
  expect(states.some((state) => state === 'indexed' || state === 'cataloged')).toBe(true);

  await page.getByRole('button', { name: 'Ignored' }).click();
  await expect.poll(() => states).toContain('ignored');
  await expect(page).toHaveURL(/state=ignored/);
});

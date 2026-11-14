import { expect, test } from '@playwright/test';

import { HAVE_CREDENTIALS, LIVE_API, LIVE_URL, apiGet, signIn } from './session';

function assertTimestamp(value: unknown): void {
  expect(typeof value, 'timestamp wire type').toBe('string');
  expect(value as string, 'UTC RFC 3339 timestamp').toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
  expect(Number.isFinite(Date.parse(value as string)), 'browser can parse timestamp').toBe(true);
}

test.describe('live processing timestamps', () => {
  test.skip(!HAVE_CREDENTIALS, 'live administrator credentials are required');

  test('real work-item and event timestamps are parseable and render in the queue', async ({ page, context, request }) => {
    const cookie = await signIn(context, request);
    const result = await apiGet<{ items: { id: string; createdAt: unknown; updatedAt: unknown }[] }>(
      request, cookie, `${LIVE_API}/api/admin/v1/ingest/work-items?limit=1&sort=addedAt:desc`,
    );
    test.skip(result.items.length === 0, 'a processed fixture is required to prove populated timestamps');
    const item = result.items[0]!;
    assertTimestamp(item.createdAt);
    assertTimestamp(item.updatedAt);
    const events = await apiGet<{ createdAt: unknown }[]>(
      request, cookie, `${LIVE_API}/api/admin/v1/ingest/work-items/${item.id}/events`,
    );
    expect(events.length, 'fixture has audit history').toBeGreaterThan(0);
    for (const event of events) assertTimestamp(event.createdAt);
    await page.addInitScript(() => localStorage.setItem('melodee.locale', 'en-US'));
    await page.goto(`${LIVE_URL}/processing?sort=addedAt:desc`);
    await expect(page.getByRole('main')).toBeVisible();
    // Compare the queue's actual tooltip with a browser-formatted API value.
    // A tuple on the wire or an empty UI cell fails independently of API mocks.
    const rendered = await page.evaluate((timestamp) =>
      new Intl.DateTimeFormat('en-US', { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(timestamp)),
    item.createdAt as string);
    expect(rendered).not.toContain('Invalid Date');
    await expect(page.getByTitle(rendered, { exact: true }).first()).toBeVisible();
  });
});

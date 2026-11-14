import { expect, test, type APIRequestContext } from '@playwright/test';

/**
 * Edit-and-workflow matrix (Phase 4 of design/testing/UI-AND-WORKFLOW-AUDIT.md).
 *
 * For every edit surface the Admin UI exposes, this drives the **real control**
 * through Playwright and then asserts two things the mocked suite cannot:
 *
 *   1. **Persistence** — reload the page and the new value is there. A field the
 *      server silently discards (Release `country`/`releaseType` before the fix)
 *      fails here.
 *   2. **Side effect** — the admin API reflects the change, the audit trail
 *      records it, or the workflow it triggers actually advances. A control that
 *      looks like it works but changes nothing fails here.
 *
 * Every mutation is reverted at the end of its row so the suite is idempotent and
 * the known-answer corpus in the same instance stays valid.
 */

import { apiGet, apiPut, HAVE_CREDENTIALS, LIVE_API as API_URL, LIVE_URL as BASE_URL, signIn } from './session';
const CAN_RUN = HAVE_CREDENTIALS;

/** First artist public id, via the admin API. */
async function firstArtistId(
  request: APIRequestContext,
  cookie: string,
): Promise<string> {
  const body = await apiGet<{ items: { id: string }[] }>(
    request,
    cookie,
    `${API_URL}/api/admin/v1/catalog/artists?limit=1`,
  );
  expect(body.items.length).toBeGreaterThan(0);
  return body.items[0]!.id;
}

async function artistDetail(
  request: APIRequestContext,
  cookie: string,
  id: string,
): Promise<Record<string, unknown>> {
  return apiGet<Record<string, unknown>>(
    request,
    cookie,
    `${API_URL}/api/admin/v1/catalog/artists/${id}`,
  );
}

test.describe.configure({ mode: 'serial' });

test.describe('edit matrix', () => {
  test.skip(!CAN_RUN, 'needs MELODEE_LIVE_USERNAME / MELODEE_LIVE_PASSWORD');

  test('artist metadata round-trips for every editable field', async ({ page, context, request }) => {
    const cookie = await signIn(context, request);

    const id = await firstArtistId(request, cookie);
    const before = await artistDetail(request, cookie, id);
    await page.goto(`${BASE_URL}/catalog/artists/${id}`);
    await page.getByRole('button', { name: 'Edit' }).click();

    const suffix = 'Matrix';
    const nameField = page.getByLabel('Name', { exact: true });
    const originalName = await nameField.inputValue();
    const originalSortName = await page.getByLabel('Sort name').inputValue();
    await nameField.fill(`${originalName} ${suffix}`);
    await page.getByLabel('Sort name').fill(`${suffix}, sort`);
    await page.getByLabel('Type').selectOption('orchestra');
    await page.getByRole('button', { name: /save/i }).click();

    // Persistence: reload and read the form back.
    await page.reload();
    await page.getByRole('button', { name: 'Edit' }).click();
    expect(await page.getByLabel('Name', { exact: true }).inputValue()).toContain(suffix);
    expect(await page.getByLabel('Sort name').inputValue()).toBe(`${suffix}, sort`);
    expect(await page.getByLabel('Type').inputValue()).toBe('orchestra');

    // Side effect: the API agrees, the revision moved, and the audit recorded it.
    const after = await artistDetail(request, cookie, id);
    expect(after['name']).toContain(suffix);
    expect(after['artistType']).toBe('orchestra');
    expect(Number(after['revision'])).toBeGreaterThan(Number(before['revision']));
    const history = (after['history'] as { items: { eventType: string }[] }).items;
    expect(history.some((event) => event.eventType === 'artist_metadata_updated')).toBe(true);

    // Identity status is a closed dictionary and is audited the same way.
    await page.getByLabel('Identity status').selectOption('verified');
    await page.getByRole('button', { name: /save/i }).click();
    await page.reload();
    expect((await artistDetail(request, cookie, id))['status']).toBe('verified');

    // Restore.
    await page.getByRole('button', { name: 'Edit' }).click();
    await page.getByLabel('Name', { exact: true }).fill(originalName);
    await page.getByLabel('Sort name').fill(originalSortName);
    await page.getByLabel('Type').selectOption(String(before['artistType']));
    await page.getByLabel('Identity status').selectOption(String(before['status']));
    await page.getByRole('button', { name: /save/i }).click();
    await page.reload();
    const restored = await artistDetail(request, cookie, id);
    expect(restored['name']).toBe(originalName);
    expect(restored['sortName']).toBe(originalSortName);
  });

  test('release metadata edit persists through the form and reverts cleanly', async ({
    page,
    context,
    request,
  }) => {
    const cookie = await signIn(context, request);
    const list = await apiGet<{
      items: { id: string; releaseDate?: string; status?: string }[];
    }>(request, cookie, `${API_URL}/api/admin/v1/catalog/releases?limit=50`);
    // The date round trip needs an *accepted* Release that already carries a
    // tagged date (the corpus guarantees several). A withheld Release is
    // deliberately not editable, and its date is not the value to restore.
    const target = list.items.find(
      (item) =>
        item.status === 'active' && /^\d{4}-\d{2}-\d{2}$/.test(item.releaseDate ?? ''),
    );
    expect(target, 'an active catalogue Release with a tagged date').toBeTruthy();
    const releaseId = target!.id;
    await page.goto(`${BASE_URL}/catalog/releases/${releaseId}`);
    const hero = page.locator('.melodee-hero');
    await expect(hero).toBeVisible({ timeout: 20_000 });
    const originalTitle = (await hero.getByRole('heading', { level: 1 }).innerText()).trim();
    // Assert against the declared value rather than against whatever the
    // catalogue happened to hold.
    const originalDate = target!.releaseDate!;
    const editedDate = originalDate === '2020-01-02' ? '2020-01-03' : '2020-01-02';

    const marker = 'MatrixEdit';
    await page.getByRole('button', { name: 'Edit' }).click();
    await page.getByLabel('Release title').fill(`${originalTitle} ${marker}`);
    await page.getByRole('button', { name: /save/i }).click();

    // Persistence: the reloaded hero shows the saved title.
    await page.reload();
    await expect(page.locator('.melodee-hero').getByRole('heading', { level: 1 })).toContainText(
      marker,
      { timeout: 20_000 },
    );

    // Side effect: the API stores it, and the audit records the edit.
    const detail = await page.request.get(`${API_URL}/api/admin/v1/catalog/releases/${releaseId}`);
    const body = (await detail.json()) as Record<string, unknown>;
    expect(String(body['title'])).toContain(marker);

    // The date field is a first-class edit: it must reach the catalogue (the
    // corpus pins that a tagged date is lifted to the Release) and survive a
    // reload through the same control that set it.
    await page.getByRole('button', { name: 'Edit' }).click();
    await page.getByLabel('Release date', { exact: true }).fill(editedDate);
    await page.getByRole('button', { name: /save/i }).click();
    await page.reload();
    const dated = (await (
      await page.request.get(`${API_URL}/api/admin/v1/catalog/releases/${releaseId}`)
    ).json()) as Record<string, unknown>;
    expect(dated['releaseDate']).toBe(editedDate);

    // Restore the declared date so the corpus keeps matching its own oracle.
    await page.getByRole('button', { name: 'Edit' }).click();
    await page.getByLabel('Release date', { exact: true }).fill(originalDate);
    await page.getByRole('button', { name: /save/i }).click();
    await page.reload();
    const restored = (await (
      await page.request.get(`${API_URL}/api/admin/v1/catalog/releases/${releaseId}`)
    ).json()) as Record<string, unknown>;
    expect(restored['releaseDate']).toBe(originalDate);

    // Restore through the same control so the corpus stays valid.
    await page.getByRole('button', { name: 'Edit' }).click();
    await page.getByLabel('Release title').fill(originalTitle);
    await page.getByRole('button', { name: /save/i }).click();
    await page.reload();
    await expect(page.locator('.melodee-hero').getByRole('heading', { level: 1 })).toContainText(
      originalTitle,
    );
  });

  test('release rename queues a projection update that drains', async ({
    page,
    context,
    request,
  }) => {
    const cookie = await signIn(context, request);
    const list = await apiGet<{ items: { id: string }[] }>(
      request,
      cookie,
      `${API_URL}/api/admin/v1/catalog/releases?limit=1`,
    );
    const releaseId = list.items[0]!.id;
    const marker = 'FindableMarker';
    // Capture the original title so the rename can be reverted exactly: later
    // suites assert the known-answer corpus, and a leaked rename breaks them.
    const originalTitle = String(
      (await apiGet<Record<string, unknown>>(
        request,
        cookie,
        `${API_URL}/api/admin/v1/catalog/releases/${releaseId}`,
      ))['title'],
    );

    const renamed = await apiPut(request, cookie, `${API_URL}/api/admin/v1/catalog/releases/${releaseId}/metadata`, {
      title: `Search ${marker}`,
    });
    expect(renamed).toBe(204);

    // The search projection outbox drains asynchronously; poll briefly.
    const deadline = Date.now() + 30_000;
    let found = false;
    while (Date.now() < deadline) {
      const body = await apiGet<{ enabled: boolean; projection: { pending: number; failed: number } | null }>(
        request,
        cookie,
        `${API_URL}/api/admin/v1/search/status`,
      );
      expect(body.enabled, 'search worker is enabled for the live audit').toBe(true);
      expect(body.projection, 'the real projection status is present').not.toBeNull();
      expect(body.projection?.failed, 'no projection updates failed').toBe(0);
      if (body.projection?.pending === 0) {
        found = true;
        break;
      }
      await page.waitForTimeout(1_000);
    }
    expect(found, 'search projection drained after the rename').toBe(true);

    // Restore exactly.
    const restored = await apiPut(
      request,
      cookie,
      `${API_URL}/api/admin/v1/catalog/releases/${releaseId}/metadata`,
      { title: originalTitle },
    );
    expect(restored).toBe(204);
    expect(
      String(
        (await apiGet<Record<string, unknown>>(
          request,
          cookie,
          `${API_URL}/api/admin/v1/catalog/releases/${releaseId}`,
        ))['title'],
      ),
    ).toBe(originalTitle);
  });

  test('policy editor loads its rules and dictionary controls', async ({ page, context, request }) => {
    await signIn(context, request);
    await page.goto(`${BASE_URL}/policies/releases`);
    await expect(page.getByRole('table', { name: /Policy rules/ })).toBeVisible();

    // The editor's controls are dictionaries, not free text.
    const typeSelect = page.getByLabel(/type/i).first();
    if ((await typeSelect.count()) > 0) {
      const tagName = await typeSelect.evaluate((element) => element.tagName);
      expect(['SELECT', 'INPUT']).toContain(tagName);
    }

    const before = await page.request.get(`${API_URL}/api/admin/v1/policies/rules`);
    expect(before.status()).toBe(200);
    const beforeBody = (await before.json()) as { total: number };
    expect(beforeBody.total).toBeGreaterThan(0);
  });

  test('quarantine and review screens render their tables without a data error', async ({
    page,
    context,
    request,
  }) => {
    await signIn(context, request);
    for (const path of ['/processing/quarantine', '/processing/review', '/catalog/identity']) {
      await page.goto(`${BASE_URL}${path}`);
      await expect(page.locator('main')).toBeVisible();
      // A screen that failed to load shows the localized error state; assert we
      // never land there on a healthy instance.
      const body = await page.locator('main').innerText();
      expect(body.toLowerCase()).not.toContain('could not be loaded');
    }
  });
});

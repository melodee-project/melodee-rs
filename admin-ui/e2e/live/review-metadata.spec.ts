import { expect, test } from '@playwright/test';

import { HAVE_CREDENTIALS, LIVE_API, LIVE_URL } from './session';

// These are deliberately opt-in synthetic fixtures. Saving a review changes
// a real queue, so never select an arbitrary item on a configured live server.
const editId = process.env.MELODEE_LIVE_EDIT_REVIEW_ID;
const identifyId = process.env.MELODEE_LIVE_IDENTIFY_REVIEW_ID;

test('edit a validation hold and submit its corrected Track number', async ({ page }) => {
  test.skip(!HAVE_CREDENTIALS || !editId, 'requires a disposable normalized review fixture');
  await page.goto(`${LIVE_URL}/processing/review/${editId}`);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Release title', { exact: true }).fill('Review corrected edition');
  await dialog.getByLabel('Track number', { exact: true }).first().fill('3');
  const saved = page.waitForResponse((r) => r.url().endsWith(`/review/${editId}/metadata`));
  await dialog.getByRole('button', { name: 'Save and reprocess' }).click();
  expect((await saved).status()).toBe(204);
  await expect(page.getByText('Release returned to the processing queue.')).toBeVisible();
});

test('identify a validation hold using the selected Melodee Release', async ({ page }) => {
  test.skip(!HAVE_CREDENTIALS || !identifyId, 'requires a disposable review and catalog target');
  await page.goto(`${LIVE_URL}/processing/review/${identifyId}`);
  await page.getByRole('button', { name: 'Identify', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Release title', { exact: true }).fill('Review target edition');
  await dialog.getByLabel('Artist', { exact: true }).fill('Review fixture artist');
  const results = page.waitForResponse((r) => r.url().includes(`/review/${identifyId}/matches?`));
  await dialog.getByRole('button', { name: 'Search', exact: true }).click();
  const { matches } = (await (await results).json()) as {
    matches: { externalId: string; title: string }[];
  };
  const target = matches.find((r) => r.title === 'Review target edition');
  expect(target).toBeDefined();
  await dialog.getByRole('radio', { name: /Review target edition/ }).check();
  await dialog.getByLabel('Track number', { exact: true }).first().fill('2');
  const saved = page.waitForResponse((r) => r.url().endsWith(`/review/${identifyId}/metadata`));
  await dialog.getByRole('button', { name: 'Identify', exact: true }).click();
  expect((await saved).status()).toBe(204);
  await expect(page.getByText('Release returned to the processing queue.')).toBeVisible();
  await expect
    .poll(
      async () => {
        const response = await page.request.get(
          `${LIVE_API}/api/admin/v1/catalog/releases/${target!.externalId}`,
        );
        return (await response.json()).trackCount;
      },
      { timeout: 45_000 },
    )
    .toBe(2);
});

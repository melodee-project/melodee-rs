import { expect, test, type Page } from '@playwright/test';

import { installFixedClock, mockAdminApi, seedPreferences } from './mockApi';

/**
 * Server theme-pack consumption (PRD-THEME-004): the Settings → Theme
 * section lists the packs served by the native endpoints and applies one as
 * the theme axis (independent of light/dark appearance). Theme endpoints
 * are mocked; no live server.
 */

const SERVER_PACKS = [
  {
    id: 'server-default',
    name: 'Server Built-in',
    author: 'Melodee',
    version: '1.0.0',
    description: null,
    isBuiltIn: true,
    previewImage: null,
    hasWarnings: false,
    warningDetails: null,
  },
  {
    id: 'neon',
    name: 'Neon Nights',
    author: 'Example Author',
    version: '2.1.0',
    description: 'High-saturation accent pack',
    isBuiltIn: false,
    previewImage: null,
    hasWarnings: false,
    warningDetails: null,
  },
];

/** Registered after mockAdminApi so these routes win for their paths. */
async function mockThemeApi(page: Page, options: { neonCssStatus?: number } = {}) {
  await page.route('**/api/v1/themes', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(SERVER_PACKS),
    });
  });
  await page.route('**/api/v1/themes/neon/file/theme.css', async (route) => {
    const status = options.neonCssStatus ?? 200;
    await route.fulfill({
      status,
      contentType: 'text/css',
      body:
        status === 200
          ? ':root { --melodee-brand-accent: #ff00ff; --melodee-status-info: #00ffff; }'
          : 'not found',
    });
  });
}

test.beforeEach(async ({ page }) => {
  await mockAdminApi(page);
  await installFixedClock(page);
  await seedPreferences(page);
});

test('lists server theme packs and applies one onto the token layer', async ({ page }) => {
  await mockThemeApi(page);
  await page.goto('system/settings?tab=theme');

  await expect(page.getByRole('radio', { name: /Neon Nights/ })).toBeVisible();
  await expect(page.getByRole('radio', { name: /Melodee default/ })).toBeChecked();

  // Carbon radio inputs are visually hidden; click the label.
  await page.locator('label').filter({ hasText: 'Neon Nights' }).click();

  // The pack attribute switches and its tokens land on the managed layer.
  await expect(page.locator('[data-melodee-theme-pack]')).toHaveAttribute(
    'data-melodee-theme-pack',
    'neon',
  );
  // <style> content is textContent, not rendered text.
  await expect
    .poll(() => page.locator('#melodee-theme-pack-style').evaluate((el) => el.textContent))
    .toContain('--melodee-brand-accent: #ff00ff;');

  // The choice persists with the appearance preference across a reload.
  await expect
    .poll(() => page.evaluate(() => window.localStorage.getItem('melodee.theme-pack')))
    .toBe('neon');
  await page.reload();
  await expect(page.locator('[data-melodee-theme-pack]')).toHaveAttribute(
    'data-melodee-theme-pack',
    'neon',
  );
  await expect(page.getByRole('radio', { name: /Neon Nights/ })).toBeChecked();
});

test('a pack that fails to load falls back to the Melodee default with a notice', async ({
  page,
}) => {
  await mockThemeApi(page, { neonCssStatus: 404 });
  await page.goto('system/settings?tab=theme');
  await expect(page.getByRole('radio', { name: /Neon Nights/ })).toBeVisible();

  await page.locator('label').filter({ hasText: 'Neon Nights' }).click();

  await expect(page.getByText('The theme pack could not be applied')).toBeVisible();
  await expect(page.locator('[data-melodee-theme-pack]')).toHaveAttribute(
    'data-melodee-theme-pack',
    'default',
  );
  await expect(page.getByRole('radio', { name: /Melodee default/ })).toBeChecked();
});

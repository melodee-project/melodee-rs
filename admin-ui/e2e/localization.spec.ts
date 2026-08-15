import { expect, test, type Page } from '@playwright/test';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  CANONICAL_SCREENS,
  gotoScreen,
  installFixedClock,
  mockAdminApi,
  seedPreferences,
} from './mockApi';

/**
 * Localization + RTL verification (W4-3, PRD-I18N-001/005/006, ADR-0137).
 *
 *  - every culture in the 20-culture inventory boots from the persisted
 *    localStorage preference without console errors, renders harvested
 *    translations where coverage exists, and falls back to English for
 *    admin-console vocabulary (PRD-I18N-006);
 *  - a full rendered pass of every canonical screen under ja-JP (100% of the
 *    harvested/mapped set per design/evidence/phase-10/i18n-harvest.md);
 *  - ar-SA AND fa-IR RTL verification on every canonical screen: dir=rtl on
 *    the document root plus a mirrored-layout assertion (the side nav docks
 *    at the inline-start, i.e. the right edge under RTL).
 *
 * Locale catalogs are read from src/i18n/locales so expectations track the
 * harvest instead of duplicating translated strings in the test.
 */

const LOCALES_DIR = join(dirname(fileURLToPath(import.meta.url)), '../src/i18n/locales');
const RTL = new Set(['ar-SA', 'fa-IR']);

interface Culture {
  code: string;
  catalog: Record<string, string>;
}

const CULTURES: Culture[] = readdirSync(LOCALES_DIR)
  .filter((name) => name.endsWith('.json'))
  .sort()
  .map((name) => ({
    code: name.replace(/\.json$/, ''),
    catalog: JSON.parse(readFileSync(join(LOCALES_DIR, name), 'utf8')),
  }));

const EN = CULTURES.find((culture) => culture.code === 'en-US');

/** Collects console errors + page errors for the duration of a test. */
function collectConsoleErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error') {
      errors.push(`${message.text()} @ ${message.location()?.url ?? 'unknown'}`);
    }
  });
  page.on('pageerror', (error) => errors.push(String(error)));
  return errors;
}

// eslint-disable-next-line no-empty-pattern -- Playwright requires the destructured-fixtures first arg.
test.beforeEach(({}, testInfo) => {
  // Culture boot, fallback, and dir derivation are viewport-independent;
  // the desktop project alone carries them to keep the suite fast.
  test.skip(testInfo.project.name !== 'desktop-1600x1200', 'desktop-only localization pass');
});

test('every culture boots without console errors, with harvest + English fallback', async ({
  page,
}, testInfo) => {
  // 20 full page reloads with per-culture assertions exceed the default 30s.
  testInfo.setTimeout(120_000);
  expect(CULTURES).toHaveLength(20);
  expect(EN).toBeDefined();
  const errors = collectConsoleErrors(page);

  const overview = CANONICAL_SCREENS.find((screen) => screen.key === 'overview')!;
  await gotoScreen(page, overview);

  const failures: string[] = [];
  for (const culture of CULTURES) {
    await page.evaluate((code) => {
      window.localStorage.setItem('melodee.locale', code);
    }, culture.code);
    await page.reload();

    // Document root carries the active culture and direction.
    const dir = RTL.has(culture.code) ? 'rtl' : 'ltr';
    try {
      await expect(page.locator('html')).toHaveAttribute('lang', culture.code);
      await expect(page.locator('html')).toHaveAttribute('dir', dir);

      // Harvested where coverage exists: nav.artists is translated in all 19
      // non-English locales (legacy resx mapping).
      const artistsLabel = culture.catalog['nav.artists'] ?? EN!.catalog['nav.artists']!;
      await expect(
        page.getByRole('navigation', { name: 'Main navigation' }).getByText(artistsLabel, { exact: true }),
      ).toBeVisible();
      if (culture.code !== 'en-US' && !artistsLabel.includes('[NEEDS TRANSLATION]')) {
        expect(artistsLabel).not.toBe(EN!.catalog['nav.artists']);
      }

      // Fallback: admin-console vocabulary with no legacy analog stays
      // English in every non-source locale (PRD-I18N-006).
      if (culture.code !== 'en-US') {
        expect(culture.catalog['overview.attention.title']).toBeUndefined();
      }
      await expect(page.getByRole('heading', { name: 'Needs attention' })).toBeVisible();
    } catch (error) {
      failures.push(`${culture.code}: ${String(error).split('\n')[0]}`);
    }
  }

  expect(failures, `cultures failing: ${failures.join('; ')}`).toEqual([]);
  expect(errors, `console errors: ${errors.join('; ')}`).toEqual([]);
});

/** Onboarding ready check that works in any locale (data-driven content). */
async function gotoOnboarding(page: Page, locale: string): Promise<void> {
  await mockAdminApi(page, { onboarding: 'pending' });
  await installFixedClock(page);
  await seedPreferences(page, { locale });
  await page.goto('onboarding');
  // Check names come from the server's preflight payload, not the catalog.
  await expect(page.getByText('Database').first()).toBeVisible();
}

test('canonical screens render under ja-JP (100% harvested) without console errors', async ({
  page,
}, testInfo) => {
  testInfo.setTimeout(120_000);
  const errors = collectConsoleErrors(page);
  for (const screen of CANONICAL_SCREENS) {
    if (screen.key === 'onboarding') {
      // The completion CTA is harvested in ja-JP (セットアップを完了), so the
      // English-based ready helper does not apply.
      await gotoOnboarding(page, 'ja-JP');
      await expect(page.locator('html')).toHaveAttribute('lang', 'ja-JP');
      continue;
    }
    await gotoScreen(page, screen, { locale: 'ja-JP' });
    await expect(page.locator('html')).toHaveAttribute('lang', 'ja-JP');
  }
  // Harvested shell vocabulary renders in Japanese.
  await expect(
    page.getByRole('navigation', { name: 'Main navigation' }).getByText('アーティスト', { exact: true }),
  ).toBeVisible();
  expect(errors, `console errors: ${errors.join('; ')}`).toEqual([]);
});

for (const locale of ['ar-SA', 'fa-IR'] as const) {
  test(`rtl: ${locale} renders every canonical screen mirrored`, async ({ page }, testInfo) => {
    testInfo.setTimeout(120_000);
    const errors = collectConsoleErrors(page);
    const viewportWidth = page.viewportSize()?.width ?? 1600;
    for (const screen of CANONICAL_SCREENS) {
      if (screen.key === 'onboarding') {
        continue; // completion CTA is harvested; covered by the smoke below
      }
      await gotoScreen(page, screen, { locale });
      await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
      await expect(page.locator('html')).toHaveAttribute('lang', locale);

      // Mirrored layout: the side nav docks at the inline-start — the right
      // edge under RTL — and the content column starts at the left edge.
      const navBox = await page.locator('.cds--side-nav').boundingBox();
      const contentBox = await page.locator('.melodee-shell__content').boundingBox();
      expect(navBox, `${screen.key}: nav bounds`).not.toBeNull();
      expect(contentBox, `${screen.key}: content bounds`).not.toBeNull();
      expect(navBox!.x).toBeGreaterThan(viewportWidth / 2);
      expect(contentBox!.x).toBeLessThan(viewportWidth / 2);
    }
    // Onboarding smoke under RTL (harvested CTA, so a bespoke ready check).
    await gotoOnboarding(page, locale);
    await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
    expect(errors, `console errors: ${errors.join('; ')}`).toEqual([]);
  });
}

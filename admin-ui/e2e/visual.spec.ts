import { expect, test } from '@playwright/test';

import { CANONICAL_SCREENS, gotoScreen, type CanonicalScreen } from './mockApi';

/**
 * Visual regression baselines (PRD-UI-REF-002). Real implementation
 * screenshots supersede the generated mockups as the visual reference; these
 * baselines are committed and diffed on every e2e run.
 *
 * Determinism: every screen renders the shared deterministic mock
 * (e2e/mockApi.ts, fixed 2026-08-14 timestamps), Date/timers are frozen via
 * page.clock, artwork falls back to initials (empty image responses), and
 * Playwright disables animations and hides the caret (see
 * playwright.config.ts expect.toHaveScreenshot). Tolerance:
 * maxDiffPixelRatio 0.01 absorbs sub-pixel anti-aliasing jitter only.
 *
 * Variants per the eight canonical screens (PRD §39) at 1600×1200:
 * light (g10), dark (g100), and RTL (ar-SA, light). Plus mobile 375×812
 * light baselines for the shell (overview) and two key screens.
 * Regenerate with: pnpm test:e2e -- --update-snapshots
 */

const DESKTOP_PROJECT = 'desktop-1600x1200';
const MOBILE_PROJECT = 'mobile-375x812';

const CANONICAL_KEYS = [
  'overview',
  'processing-queue',
  'release-review',
  'identity-review',
  'artist-detail',
  'release-detail',
  'policies',
  'system-workers',
] as const;

const VARIANTS = [
  { key: 'light', locale: 'en-US', appearance: 'light' as const },
  { key: 'dark', locale: 'en-US', appearance: 'dark' as const },
  { key: 'rtl', locale: 'ar-SA', appearance: 'light' as const },
];

function screenFor(key: string): CanonicalScreen {
  const screen = CANONICAL_SCREENS.find((entry) => entry.key === key);
  if (!screen) {
    throw new Error(`unknown canonical screen: ${key}`);
  }
  return screen;
}

for (const screenKey of CANONICAL_KEYS) {
  for (const variant of VARIANTS) {
    test(`baseline: ${screenKey} [${variant.key}] @1600x1200`, async ({ page }, testInfo) => {
      test.skip(testInfo.project.name !== DESKTOP_PROJECT, 'desktop-only baseline');
      await gotoScreen(page, screenFor(screenKey), {
        locale: variant.locale,
        appearance: variant.appearance,
      });
      await expect(page).toHaveScreenshot(`${screenKey}-${variant.key}.png`);
    });
  }
}

const MOBILE_KEYS = ['overview', 'processing-queue', 'release-review'] as const;

for (const screenKey of MOBILE_KEYS) {
  test(`baseline: ${screenKey} [mobile-light] @375x812`, async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== MOBILE_PROJECT, 'mobile-only baseline');
    await gotoScreen(page, screenFor(screenKey), { locale: 'en-US', appearance: 'light' });
    await expect(page).toHaveScreenshot(`${screenKey}-mobile-light.png`);
  });
}

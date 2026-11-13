import { expect, test, type Locator, type Page } from '@playwright/test';
import { checkA11y, injectAxe } from 'axe-playwright';

import { CANONICAL_SCREENS, gotoScreen } from './mockApi';

/**
 * Accessibility e2e (UI-SPEC §9, PRD-UI-REF-003). Runs axe-core
 * (WCAG 2.2 A/AA tag set) against every canonical screen at all three
 * canonical viewports, plus a keyboard smoke pass proving Tab reaches each
 * screen's primary action with a visible focus indicator.
 *
 * axe scope: runOnly tags wcag2a/wcag2aa/wcag21a/wcag21aa/wcag22aa — the
 * WCAG 2.2 A+AA rule set per UI-SPEC §9 ("Target: WCAG 2.2 AA"). The
 * best-practice tag is intentionally excluded: it is advisory, not a WCAG
 * conformance requirement, and Carbon upstream carries known best-practice
 * findings we cannot fix from here.
 */

const WCAG_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'];



/**
 * Tabs forward until `target` (or a descendant) holds focus. The bound is
 * generous because the shell chrome (skip link, menu button, nav tree,
 * header actions) legitimately precedes screen content.
 */
async function expectTabReaches(page: Page, target: Locator, maxPresses = 90): Promise<void> {
  for (let press = 0; press < maxPresses; press += 1) {
    if (
      (await target.isVisible()) &&
      (await target.evaluate(
        (el) => el === document.activeElement || el.contains(document.activeElement),
      ))
    ) {
      return;
    }
    await page.keyboard.press('Tab');
  }
  await expect(target).toBeFocused();
}

/** Asserts the focused element renders a visible keyboard focus indicator. */
async function expectVisibleFocus(target: Locator): Promise<void> {
  const indicator = await target.evaluate((el) => {
    const probe = (node: Element) => {
      const style = window.getComputedStyle(node);
      return {
        outline:
          style.outlineStyle !== 'none' &&
          style.outlineWidth !== '0px' &&
          style.outlineColor !== 'transparent',
        shadow: style.boxShadow !== 'none',
      };
    };
    const self = probe(el);
    // Carbon frequently draws focus on a child (button content) or uses an
    // inset box-shadow on the element itself; accept either.
    const child = el.firstElementChild ? probe(el.firstElementChild) : { outline: false, shadow: false };
    return self.outline || self.shadow || child.outline || child.shadow;
  });
  expect(indicator).toBe(true);
}

for (const screen of CANONICAL_SCREENS) {
  test(`axe: ${screen.key} passes WCAG 2.2 A/AA`, async ({ page }) => {
    await gotoScreen(page, screen);
    await injectAxe(page);
    await checkA11y(page, undefined, {
      axeOptions: {
        runOnly: { type: 'tag', values: WCAG_TAGS },
      },
      detailedReport: true,
      detailedReportOptions: { html: false },
    });
  });
}

const KEYBOARD_TARGETS: Record<string, (page: Page) => Promise<Locator> | Locator> = {
  overview: (page) => page.getByRole('link', { name: 'Open the processing queue' }),
  'processing-queue': (page) =>
    page.getByRole('table', { name: 'Processing queue work items' }).locator('tbody tr').first(),
  'release-review': (page) => page.getByRole('button', { name: /^Accept Release/ }),
  'identity-review': (page) => page.getByRole('button', { name: /^Create New Artist/ }),
  // Carbon Tabs use roving tabindex: only the selected tab is Tab-reachable.
  'artist-detail': (page) => page.getByRole('tab', { name: 'Overview', selected: true }),
  'release-detail': (page) => page.getByRole('tab', { name: 'Overview', selected: true }),
  policies: (page) => page.getByRole('row', { name: /POL-LIVE-REVIEW/ }),
  'system-workers': (page) => page.getByRole('button', { name: 'Pause' }).first(),
  'system-search': (page) =>
    page
      .locator('section', { has: page.getByRole('heading', { name: 'Search Index' }) })
      .getByRole('button', { name: 'Rebuild search index' }),
  'system-storage': (page) => page.getByRole('button', { name: 'Refresh' }),
  'system-diagnostics': (page) => page.getByRole('button', { name: 'Refresh' }),
  stats: (page) => page.getByRole('button', { name: 'Refresh' }),
  users: (page) => page.getByRole('button', { name: 'Create user' }),
  settings: (page) => page.getByRole('tab', { name: 'Configuration', selected: true }),
  onboarding: (page) => page.getByRole('button', { name: 'Re-run checks' }),
};

for (const screen of CANONICAL_SCREENS) {
  test(`keyboard: ${screen.key} primary action is reachable with visible focus`, async ({
    page,
  }) => {
    await gotoScreen(page, screen);

    // First Tab lands on the skip-to-content link (Carbon SkipToContent) —
    // the keyboard entry point for the whole shell.
    await page.keyboard.press('Tab');
    await expect(page.getByRole('link', { name: 'Skip to main content' })).toBeFocused();

    // Identity review's decision rail needs a selected observation. Activate
    // the first row from the keyboard so the walk continues from the queue.
    if (screen.key === 'identity-review') {
      const firstRow = page.getByRole('row', { name: /The Nomads Into the Night/ });
      await firstRow.focus();
      await firstRow.press('Enter');
      await expect(page.getByText('Candidate Matches')).toBeVisible();
    }

    const target = await KEYBOARD_TARGETS[screen.key]!(page);
    await expectTabReaches(page, target);
    await expectVisibleFocus(target);
  });
}

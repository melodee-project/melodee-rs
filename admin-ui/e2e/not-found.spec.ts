import { expect, test } from "@playwright/test";

import { installFixedClock, mockAdminApi, seedPreferences } from "./mockApi";

// seedPreferences is used for the appearance-neutral default; the localized
// test overrides the locale with its own init script.

/**
 * Localized themed error page (PRD-WEB-002 `/Error` replacement). A bogus
 * in-app route renders the not-found state inside the app shell — themed,
 * localized, with a working path back to the overview — at all three
 * canonical viewports (this spec runs in every configured project).
 */

test.beforeEach(async ({ page }) => {
  await mockAdminApi(page);
  await installFixedClock(page);
  await seedPreferences(page);
});

test("bogus in-app route renders the localized not-found screen in the shell", async ({
  page,
}) => {
  await page.goto("this-route-does-not-exist");

  // The shell chrome stays mounted: themed page, not a raw browser 404.
  await expect(
    page.getByRole("navigation", { name: "Main navigation" }),
  ).toBeAttached();
  await expect(
    page.getByRole("heading", { name: "Page not found" }).first(),
  ).toBeVisible();
  await expect(
    page.getByText(/does not exist or may have moved/).first(),
  ).toBeVisible();

  // The recovery link navigates back to the overview dashboard.
  await page.getByRole("link", { name: "Back to overview" }).click();
  await expect(
    page.getByRole("heading", { name: "Needs attention" }),
  ).toBeVisible();
});

test("not-found screen renders localized when a non-English culture is seeded", async ({
  page,
}) => {
  // theme/errors keys are new en-US-only strings; other locales fall back to
  // English per ADR-0137 until translated — the page still renders themed.
  // Runs after beforeEach's seed, so this overwrite wins.
  await page.addInitScript(() => {
    window.localStorage.setItem("melodee.locale", "de-DE");
  });
  await page.goto("this-route-does-not-exist");

  await expect(page.locator("html")).toHaveAttribute("lang", "de-DE");
  await expect(
    page.getByRole("heading", { name: "Page not found" }).first(),
  ).toBeVisible();
});

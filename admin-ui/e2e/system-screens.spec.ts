import { expect, test } from "@playwright/test";

import { installFixedClock, mockAdminApi, seedPreferences } from "./mockApi";

/**
 * System → Search / Storage / Diagnostics e2e (placeholder
 * residue closeout). The admin API is mocked with page.route — no live
 * server is required. Runs at all three canonical viewports (UI-SPEC §8)
 * via the playwright projects. Asserts the screens render real server data
 * (never placeholder copy), that query/per-path/log surfaces stay honestly
 * CLI-scoped, and that the removed Matching Rules nav entry is gone.
 */

test.beforeEach(async ({ page }) => {
  await mockAdminApi(page);
  await installFixedClock(page);
  await seedPreferences(page, { locale: "en-US" });
});

test("system/search renders index status, the rebuild control, and the honest query note", async ({
  page,
}) => {
  await page.goto("system/search");

  await expect(
    page.getByRole("heading", { name: "Search Index" }),
  ).toBeVisible();
  await expect(page.getByText("Indexed documents")).toBeVisible();
  await expect(page.getByText("600", { exact: true })).toBeVisible();
  await expect(
    page
      .locator("section", {
        has: page.getByRole("heading", { name: "Search Index" }),
      })
      .getByRole("button", { name: "Rebuild search index" }),
  ).toBeVisible();

  // Honesty: no query box — MQL tooling is CLI/native-API only.
  await expect(
    page.getByText("Query tooling is a CLI and API surface"),
  ).toBeVisible();
  await expect(page.getByRole("searchbox")).toHaveCount(0);
  await expect(page.getByText("Coming soon")).toHaveCount(0);
});

test("system/storage renders subsystem health and the storage-paths preflight detail", async ({
  page,
}) => {
  await page.goto("system/storage");

  await expect(
    page.getByRole("heading", { name: "Storage Subsystem" }),
  ).toBeVisible();
  await expect(page.getByText("storage paths writable")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Storage Paths Preflight" }),
  ).toBeVisible();
  // ADR-0145: each storage role is reported separately.
  await expect(
    page.getByText(
      "inbound: 2 source(s) [/srv/incoming, /srv/drop]; staging: /srv/staging; work: /var/cache/melodee/work; canonical: /srv/melodee",
    ),
  ).toBeVisible();

  // Honesty: per-path capacity/usage is not HTTP-exposed.
  await expect(
    page.getByText("Per-path usage stays a CLI surface"),
  ).toBeVisible();
  await expect(page.getByText("Coming soon")).toHaveCount(0);
});

test("system/diagnostics renders health, leases, the failures link, and the CLI note", async ({
  page,
}) => {
  await page.goto("system/diagnostics");

  await expect(
    page.getByRole("heading", { name: "System Health" }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("table", { name: "Active worker leases" })
      .getByText("worker-3"),
  ).toBeVisible();
  await expect(
    page.getByRole("link", {
      name: "Recent failed work items are listed on System → Workers.",
    }),
  ).toBeVisible();
  await expect(
    page.getByText("Logs and performance remain CLI surfaces"),
  ).toBeVisible();
  await expect(page.getByText("Coming soon")).toHaveCount(0);
});

test("the nav shows Release Rules but no Matching Rules entry", async ({
  page,
}) => {
  await page.goto("policies/releases");
  await expect(page.getByRole("table", { name: "Policy rules" })).toBeVisible();

  // Below lg the nav starts collapsed; expand it before asserting entries.
  const expandButton = page
    .getByRole("button", { name: "Expand navigation" })
    .first();
  if (await expandButton.isVisible()) {
    await expandButton.click();
  }

  const nav = page.getByRole("navigation", { name: "Main navigation" });
  const releaseRules = nav.getByText("Release Rules");
  await expect(releaseRules).toBeVisible();
  // Removed, not placeholder-routed: no matching-rule model exists.
  await expect(nav.getByText("Matching Rules")).toHaveCount(0);
});

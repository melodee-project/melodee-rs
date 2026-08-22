import { expect, test, type Page } from "@playwright/test";

/**
 * Processing Queue e2e . The admin API is mocked with page.route —
 * no live server is required. Covers: queue load with a real envelope at
 * 1600×1200 (and the other canonical viewports), keyboard navigation into
 * the inspector, and the RTL smoke (ar-SA, dir=rtl) for this screen.
 */

const ITEM_ONE = {
  id: "11111111-1111-4111-8111-111111111111",
  apiKey: "33333333-3333-4333-8333-333333333333",
  sourceLibraryId: null,
  targetLibraryId: null,
  sourcePath: "/library/incoming/Business as Usual",
  currentPath: "/library/incoming/Business as Usual",
  targetPath: null,
  sourceFingerprint: null,
  manifestHash: null,
  state: "ready",
  stateReason: null,
  attemptCount: 2,
  nextAttemptAt: null,
  leaseOwner: "worker-7",
  leaseAcquiredAt: "2026-08-14T10:04:00Z",
  leaseExpiresAt: "2026-08-14T10:09:00Z",
  lastErrorCode: null,
  lastErrorMessage: null,
  correlationId: "44444444-4444-4444-8444-444444444444",
  discoveredAt: "2026-08-14T10:00:00Z",
  firstStartedAt: null,
  lastAttemptAt: null,
  updatedAt: "2026-08-14T10:05:00Z",
  createdAt: "2026-08-14T10:00:00Z",
  completedAt: null,
};

const ITEM_TWO = {
  ...ITEM_ONE,
  id: "22222222-2222-4222-8222-222222222222",
  sourcePath: "/library/incoming/Road to Freedom",
  currentPath: "/library/incoming/Road to Freedom",
  state: "failed",
  attemptCount: 5,
  leaseOwner: null,
  leaseAcquiredAt: null,
  leaseExpiresAt: null,
  lastErrorCode: "checksum_mismatch",
  lastErrorMessage: "checksum mismatch",
};

const EVENT = {
  id: 1,
  workItemId: ITEM_ONE.id,
  eventType: "transitioned",
  fromState: "ready",
  toState: "inspecting",
  reasonCode: null,
  workerId: "worker-7",
  correlationId: "55555555-5555-4555-8555-555555555555",
  createdAt: "2026-08-14T10:05:00Z",
  metadata: null,
};

async function mockAdminApi(page: Page) {
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const json = (body: unknown) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (pathname === "/api/admin/v1/system/info") {
      await json({ service: "melodee", version: "test", status: "ok" });
      return;
    }
    if (pathname === "/api/admin/v1/ingest/work-items/counts") {
      await json([
        { state: "ready", count: 1 },
        { state: "failed", count: 1 },
      ]);
      return;
    }
    if (pathname === "/api/admin/v1/ingest/work-items") {
      const state = url.searchParams.get("state");
      const items = [ITEM_ONE, ITEM_TWO].filter(
        (item) => !state || item.state === state,
      );
      await json({ items, total: items.length, limit: 50, offset: 0 });
      return;
    }
    if (pathname === `/api/admin/v1/ingest/work-items/${ITEM_ONE.id}/events`) {
      await json([EVENT]);
      return;
    }
    if (pathname === `/api/admin/v1/ingest/work-items/${ITEM_TWO.id}/events`) {
      await json([]);
      return;
    }
    if (pathname === `/api/admin/v1/ingest/work-items/${ITEM_ONE.id}`) {
      await json(ITEM_ONE);
      return;
    }
    if (pathname === `/api/admin/v1/ingest/work-items/${ITEM_TWO.id}`) {
      await json(ITEM_TWO);
      return;
    }
    await route.fulfill({
      status: 404,
      contentType: "application/json",
      body: JSON.stringify({
        code: "not_found",
        message: "not found",
        correlationId: ITEM_ONE.id,
      }),
    });
  });
}

test.beforeEach(async ({ page }) => {
  await mockAdminApi(page);
  await page.addInitScript(() => {
    window.localStorage.setItem("melodee.locale", "en-US");
  });
});

test("loads the queue with state chips, counts, and server-paged rows", async ({
  page,
}) => {
  // Relative to the /admin/ baseURL so the deep link hits the SPA fallback.
  await page.goto("processing");

  const chips = page.getByRole("group", { name: "Filter by state" });
  await expect(chips.getByRole("tab", { name: "All 2" })).toBeVisible();
  await expect(chips.getByRole("tab", { name: "Ready 1" })).toBeVisible();
  await expect(chips.getByRole("tab", { name: "Failed 1" })).toBeVisible();

  const table = page.getByRole("table", {
    name: "Processing queue work items",
  });
  await expect(table).toBeVisible();
  await expect(table.getByText("Business as Usual")).toBeVisible();
  await expect(table.getByText("Road to Freedom")).toBeVisible();

  // Chip filtering hits the mocked API with the state param.
  await chips.getByRole("tab", { name: "Failed 1" }).click();
  await expect(table.getByText("Road to Freedom")).toBeVisible();
  await expect(table.getByText("Business as Usual")).not.toBeVisible();
  expect(page.url()).toContain("state=failed");
});

test("keyboard: arrows move between rows and Enter opens the inspector", async ({
  page,
}) => {
  // Relative to the /admin/ baseURL so the deep link hits the SPA fallback.
  await page.goto("processing");

  const table = page.getByRole("table", {
    name: "Processing queue work items",
  });
  await expect(table.getByText("Business as Usual")).toBeVisible();

  const rows = table.locator("tbody tr");
  await rows.first().focus();
  await expect(rows.first()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(rows.nth(1)).toBeFocused();
  await page.keyboard.press("Enter");

  const inspector = page.getByRole("complementary", { name: "Inspector" });
  await expect(inspector).toBeVisible();
  await expect(
    inspector.getByText("checksum mismatch", { exact: false }),
  ).toBeVisible();

  // Queue position/filters survive: the table is still rendered with both rows.
  await expect(table.getByText("Business as Usual")).toBeVisible();
  expect(page.url()).toContain(`item=${ITEM_TWO.id}`);
});

test("rtl smoke: ar-SA renders the queue with dir=rtl", async ({ page }) => {
  await page.goto("processing");
  await expect(page.getByText("Business as Usual")).toBeVisible();

  // Switch via the shell language selector (same path as smoke.spec).
  await page.getByRole("button", { name: "Language", exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await expect(page.locator("html")).toHaveAttribute("lang", "ar-SA");
  await expect(page.getByRole("table")).toBeVisible();
  // Data (paths) stays LTR-content; the chrome mirrors.
  await expect(page.getByText("Business as Usual")).toBeVisible();
});

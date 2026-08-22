import { expect, test, type Page } from "@playwright/test";

/**
 * Artist Identity Review e2e . The admin API is mocked with
 * page.route — no live server is required. Covers: the three-pane
 * workspace load (the playwright projects exercise the canonical 1600×1200,
 * 768×1024, and 375×812 viewports per UI-SPEC §8), keyboard navigation
 * (queue → candidates → actions), the RTL (ar-SA, dir=rtl) smoke, and the
 * resolve happy path with a mocked POST body assertion.
 */

const OBSERVATION_ID = "11111111-1111-4111-8111-111111111111";
const OBSERVATION_ID_2 = "22222222-2222-4222-8222-222222222222";
const ARTIST_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

const OBSERVATION = {
  id: OBSERVATION_ID,
  displayValue: "The Nomads",
  rawValue: "The Nomads",
  normalizedValue: "the nomads",
  scope: "release",
  sourceOrdinal: 0,
  sourcePointer: "/library/incoming/Into the Night",
  releaseId: "55555555-5555-4555-8555-555555555555",
  releaseTitle: "Into the Night",
  trackId: null,
  trackTitle: null,
  pendingMatchCount: 2,
  evidenceCount: 5,
  createdAt: "2026-08-14T10:00:00Z",
};

const QUEUE_PAGE = {
  items: [
    OBSERVATION,
    {
      ...OBSERVATION,
      id: OBSERVATION_ID_2,
      displayValue: "Silver Skies",
      normalizedValue: "silver skies",
      releaseTitle: "Fragments",
      pendingMatchCount: 1,
    },
  ],
  total: 2,
  limit: 25,
  offset: 0,
};

const DETAIL = {
  observation: OBSERVATION,
  current: null,
  candidates: [
    {
      artistId: ARTIST_ID,
      artistName: "The Nomads",
      artistStatus: "active",
      latestOutcome: "provisional",
      latestConfidence: 9200,
      latestReason: "Strong alias and release overlap.",
      latestLocked: false,
      latestDecidedAt: "2026-08-14T10:05:00Z",
      decisionCount: 3,
      evidence: [
        {
          kind: "alias",
          polarity: "positive",
          confidence: 9500,
          source: "tags",
          payload: { alias: "Nomads" },
          createdAt: "2026-08-14T10:04:00Z",
        },
        {
          kind: "release",
          polarity: "negative",
          confidence: 3000,
          source: "catalog",
          payload: { release: "Conflicting release" },
          createdAt: "2026-08-14T10:04:00Z",
        },
      ],
    },
  ],
  negativeAssertions: [],
};

const DECISIONS_PAGE = {
  items: [
    {
      eventId: "99999999-9999-4999-8999-999999999999",
      entityType: "observedArtist",
      entityId: OBSERVATION_ID,
      eventType: "match_confirmed",
      actor: "operator-1",
      reason: "Verified against the catalog.",
      payload: {},
      createdAt: "2026-08-14T10:06:00Z",
    },
  ],
  total: 1,
  limit: 20,
  offset: 0,
};

async function mockAdminApi(page: Page) {
  let resolveBody: unknown = null;
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (pathname === "/api/admin/v1/system/info") {
      await json({ service: "melodee", version: "test", status: "ok" });
      return;
    }
    if (pathname === "/api/admin/v1/identity/unresolved") {
      await json(QUEUE_PAGE);
      return;
    }
    if (
      pathname ===
      `/api/admin/v1/identity/observations/${OBSERVATION_ID}/resolve`
    ) {
      resolveBody = route.request().postDataJSON();
      await json({
        observationId: OBSERVATION_ID,
        resolution: "confirm",
        artistId: ARTIST_ID,
        changed: true,
      });
      return;
    }
    if (pathname === `/api/admin/v1/identity/observations/${OBSERVATION_ID}`) {
      await json(DETAIL);
      return;
    }
    if (pathname === "/api/admin/v1/identity/decisions") {
      await json(DECISIONS_PAGE);
      return;
    }
    await json(
      {
        code: "not_found",
        message: "not found",
        correlationId: OBSERVATION_ID,
      },
      404,
    );
  });
  return {
    getResolveBody: () => resolveBody,
  };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem("melodee.locale", "en-US");
  });
});

test("three-pane workspace loads: queue, observation/candidates, actions, feed", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto("catalog/identity");

  // Left: unresolved queue with per-observation pending-match counts.
  const table = page.getByRole("table", {
    name: "Unresolved artist observations",
  });
  await expect(table.getByText("The Nomads")).toBeVisible();
  await expect(table.getByText("2 pending")).toBeVisible();
  await expect(table.getByText("Silver Skies")).toBeVisible();

  // Select the first observation: the URL reflects the selection.
  await table.getByText("The Nomads").click();
  await expect(page).toHaveURL(new RegExp(`observation=${OBSERVATION_ID}`));

  // Center: incoming observation with source context, candidates with
  // confidence and evidence, evidence summary strip.
  await expect(page.getByText("Incoming observation")).toBeVisible();
  await expect(
    page.getByText("/library/incoming/Into the Night"),
  ).toBeVisible();
  await expect(page.getByText("Candidate Matches")).toBeVisible();
  await expect(page.getByText("92%")).toBeVisible();
  await expect(page.getByText("Evidence Summary")).toBeVisible();
  await expect(page.getByText("Alias match")).toBeVisible();

  // Right rail: ordered decision actions, destructive distinct.
  await expect(
    page.getByRole("button", { name: "Confirm Match" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Create New Artist" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Leave Provisional" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Never Match Candidate" }),
  ).toBeVisible();

  // Below the panes: recent identity decisions feed.
  await expect(page.getByText("Recent Identity Decisions")).toBeVisible();
  await expect(
    page.getByText("match_confirmed (observedArtist)"),
  ).toBeVisible();
});

test("keyboard: tab order runs queue → candidates → actions", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto("catalog/identity");
  const table = page.getByRole("table", {
    name: "Unresolved artist observations",
  });
  await expect(table.getByText("The Nomads")).toBeVisible();

  // Focus the first queue row and activate it with Enter.
  const firstRow = page.getByRole("row", { name: /The Nomads Into the Night/ });
  await firstRow.focus();
  await firstRow.press("Enter");
  await expect(page).toHaveURL(new RegExp(`observation=${OBSERVATION_ID}`));
  await expect(page.getByText("Candidate Matches")).toBeVisible();

  // Walk the tab order from the queue row: the candidate radio must receive
  // focus before the decision actions (queue → candidates → actions).
  const candidate = page.getByRole("radio", { name: /The Nomads/ });
  const confirm = page.getByRole("button", { name: "Confirm Match" });
  let candidateIndex = -1;
  let confirmIndex = -1;
  for (
    let step = 0;
    step < 80 && (candidateIndex === -1 || confirmIndex === -1);
    step += 1
  ) {
    await page.keyboard.press("Tab");
    if (
      candidateIndex === -1 &&
      (await candidate.evaluate((el) => el === document.activeElement))
    ) {
      candidateIndex = step;
    }
    if (
      confirmIndex === -1 &&
      (await confirm.evaluate((el) => el === document.activeElement))
    ) {
      confirmIndex = step;
    }
  }
  expect(candidateIndex).toBeGreaterThanOrEqual(0);
  expect(confirmIndex).toBeGreaterThanOrEqual(0);
  expect(candidateIndex).toBeLessThan(confirmIndex);

  // Enter on the focused candidate selects it (radio semantics).
  await candidate.press("Enter");
  await expect(candidate).toHaveAttribute("aria-checked", "true");
});

test("resolve happy path posts resolution, artistId, and audited reason", async ({
  page,
}) => {
  const api = await mockAdminApi(page);
  await page.goto(`catalog/identity?observation=${OBSERVATION_ID}`);
  await expect(page.getByText("Candidate Matches")).toBeVisible();

  await page.getByRole("radio", { name: /The Nomads/ }).click();
  await page.getByRole("button", { name: "Confirm Match" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Reason").fill("Alias and release evidence line up.");
  await dialog.getByRole("button", { name: "Apply decision" }).click();

  await expect(page.getByText("Match confirmed.")).toBeVisible();
  expect(api.getResolveBody()).toEqual({
    resolution: "confirm",
    artistId: ARTIST_ID,
    reason: "Alias and release evidence line up.",
  });
  // Queue refreshed and selection advanced to the next unresolved row.
  await expect(page).toHaveURL(new RegExp(`observation=${OBSERVATION_ID_2}`));
});

test("rtl smoke: ar-SA renders the workspace with dir=rtl", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto(`catalog/identity?observation=${OBSERVATION_ID}`);
  await expect(page.getByText("Candidate Matches")).toBeVisible();

  await page.getByRole("button", { name: "Language", exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await expect(page.locator("html")).toHaveAttribute("lang", "ar-SA");
  // Data stays visible; the chrome mirrors.
  await expect(page.getByText("The Nomads").first()).toBeVisible();
});

import { expect, test, type Page } from "@playwright/test";

/**
 * Release Review e2e . The admin API is mocked with page.route —
 * no live server is required. Covers: review queue load + navigation, the
 * review workspace at the canonical viewports, keyboard activation of the
 * decision actions, the RTL (ar-SA, dir=rtl) smoke, and the decision happy
 * path against the mocked POST.
 */

const WORK_ITEM_ID = "11111111-1111-4111-8111-111111111111";
const RELEASE_ID = "55555555-5555-4555-8555-555555555555";

const WORK_ITEM = {
  id: WORK_ITEM_ID,
  apiKey: "33333333-3333-4333-8333-333333333333",
  sourceLibraryId: null,
  targetLibraryId: null,
  sourcePath: "/library/incoming/Greatest Hits",
  currentPath: "/library/incoming/Greatest Hits",
  targetPath: null,
  sourceFingerprint: null,
  manifestHash: null,
  state: "needs_review",
  stateReason: "policy requires review",
  attemptCount: 1,
  nextAttemptAt: null,
  leaseOwner: null,
  leaseAcquiredAt: null,
  leaseExpiresAt: null,
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

const QUEUE_PAGE = {
  items: [{ ...WORK_ITEM, releaseId: RELEASE_ID }],
  total: 1,
  limit: 50,
  offset: 0,
  countsByState: [
    { state: "needs_review", count: 1 },
    { state: "failed", count: 2 },
  ],
};

const DETAIL = {
  workItem: WORK_ITEM,
  releaseId: RELEASE_ID,
  manifest: {
    manifestHash: "abc123",
    schemaVersion: 1,
    generatedAt: "2026-08-14T10:02:00Z",
    sourcePath: "Greatest Hits",
    releaseTitle: "Greatest Hits",
    releaseArtists: ["Example Artist"],
    tracks: [
      {
        index: 0,
        filePath: "01 Down Under.flac",
        title: "Down Under",
        durationMs: 222000,
        container: "flac",
        codec: "flac",
        mimeType: "audio/flac",
        fingerprintStatus: "captured",
      },
      {
        index: 1,
        filePath: "02 Overkill.flac",
        title: "Overkill",
        durationMs: 223000,
        container: "flac",
        codec: "flac",
        mimeType: "audio/flac",
        fingerprintStatus: "captured",
      },
    ],
    artwork: [],
    sidecars: [],
    findingCount: 0,
  },
  classification: {
    traits: ["greatest_hits"],
    confidence: 9200,
    classifiedAt: "2026-08-14T10:03:00Z",
    classifierVersion: "1.0.0",
    evidence: [
      {
        trait: "greatest_hits",
        source: "tag",
        kind: "title_pattern",
        confidence: 9200,
        detail: 'Title contains "Greatest Hits"',
      },
    ],
  },
  catalogComparison: {
    known: 1,
    new: 1,
    alternate: 0,
    uncertain: 0,
    exactDuplicates: 0,
    knownAudioIdentities: 1,
    alternateRecordings: 0,
    newTracks: 1,
    uncertainTracks: 0,
    totalTracks: 2,
    evaluatedAt: "2026-08-14T10:04:00Z",
    evaluatorVersion: "1.0.0",
    tracks: [
      {
        trackId: "66666666-6666-4666-8666-666666666666",
        category: "known_audio_identity",
        confidence: 9900,
        evidenceKind: "audio_identity_match",
        detail: "Same audio identity as catalog track",
      },
      {
        trackId: "77777777-7777-4777-8777-777777777777",
        category: "new_track",
        confidence: 8000,
        evidenceKind: "no_catalog_match",
        detail: "No catalog match found",
      },
    ],
  },
  decision: {
    id: "88888888-8888-4888-8888-888888888888",
    releaseId: RELEASE_ID,
    ruleSetId: "99999999-9999-4999-8999-999999999999",
    ruleSetVersion: 3,
    disposition: "review",
    completionState: "pending",
    matchedRuleIds: ["greatest_hits_novelty"],
    evidenceSummary: "Release has potentially novel tracks.",
    overrideReason: null,
    actor: { kind: "system" },
    decidedAt: "2026-08-14T10:04:30Z",
    completedAt: null,
  },
  timeline: [
    {
      id: 1,
      workItemId: WORK_ITEM_ID,
      eventType: "transitioned",
      fromState: "validating",
      toState: "needs_review",
      reasonCode: null,
      workerId: "worker-1",
      correlationId: "44444444-4444-4444-8444-444444444444",
      createdAt: "2026-08-14T10:04:00Z",
      metadata: null,
    },
  ],
};

async function mockAdminApi(page: Page) {
  let decisionBody: unknown = null;
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
    if (pathname === "/api/admin/v1/review/queue") {
      await json(QUEUE_PAGE);
      return;
    }
    if (pathname === `/api/admin/v1/review/${WORK_ITEM_ID}/decision`) {
      decisionBody = route.request().postDataJSON();
      await json({
        workItem: WORK_ITEM,
        decision: DETAIL.decision,
        pendingExecution: false,
      });
      return;
    }
    if (pathname.endsWith("/preview")) {
      // No previewable file under the configured roots: the UI must degrade.
      await json(
        {
          code: "preview_unavailable",
          message:
            "no previewable track file is available under the configured roots",
          correlationId: WORK_ITEM_ID,
        },
        404,
      );
      return;
    }
    if (pathname === `/api/admin/v1/review/${WORK_ITEM_ID}`) {
      await json(DETAIL);
      return;
    }
    await json(
      { code: "not_found", message: "not found", correlationId: WORK_ITEM_ID },
      404,
    );
  });
  return {
    getDecisionBody: () => decisionBody,
  };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem("melodee.locale", "en-US");
  });
});

test("review queue loads from the envelope and row activation opens the workspace", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto("processing/review");

  const chips = page.getByRole("group", { name: "Filter by review state" });
  await expect(chips.getByRole("tab", { name: "All 3" })).toBeVisible();
  await expect(
    chips.getByRole("tab", { name: "Needs review 1" }),
  ).toBeVisible();
  await expect(chips.getByRole("tab", { name: "Failed 2" })).toBeVisible();

  const table = page.getByRole("table", { name: "Review queue work items" });
  await expect(table.getByText("Greatest Hits")).toBeVisible();
  await table.getByText("Greatest Hits").click();

  await expect(page).toHaveURL(
    new RegExp(`/processing/review/${WORK_ITEM_ID}`),
  );
  await expect(page.getByText("Example Artist — Greatest Hits")).toBeVisible();
});

test("workspace renders evidence, comparison, decision actions, policy, and timeline", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto(`processing/review/${WORK_ITEM_ID}`);

  await expect(page.getByText("Example Artist — Greatest Hits")).toBeVisible();
  await expect(page.getByText("Classification & Evidence")).toBeVisible();
  await expect(page.getByText('Title contains "Greatest Hits"')).toBeVisible();
  await expect(page.getByText("Catalog Comparison")).toBeVisible();
  await expect(page.getByText("Known Tracks")).toBeVisible();
  await expect(page.getByText("Track Breakdown").first()).toBeVisible();
  await expect(page.getByText("Down Under")).toBeVisible();

  // Inline previews: labeled transports, preload=none (no autoplay). The
  // 404 preview_unavailable path degrades to a localized note when playback
  // is attempted (covered by the component test, which fires the error event).
  const previews = page.locator('audio[preload="none"]');
  await expect(previews).toHaveCount(2);
  await expect(previews.first()).toHaveAttribute(
    "aria-label",
    /Preview track 1/,
  );

  // Ordered decision actions: destructive last.
  await expect(
    page.getByRole("button", { name: "Accept Release" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Quarantine" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Ignore" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Delete", exact: true }),
  ).toBeVisible();

  await expect(page.getByText("Policy Match")).toBeVisible();
  await expect(page.getByText("greatest_hits_novelty")).toBeVisible();
  await expect(page.getByText("Review Timeline")).toBeVisible();
});

test("keyboard: Enter on a decision action opens its confirmation, Escape closes it", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto(`processing/review/${WORK_ITEM_ID}`);
  await expect(
    page.getByRole("button", { name: "Accept Release" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Accept Release" }).press("Enter");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(
    dialog.getByText("Confirm decision: Accept Release"),
  ).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
});

test("decision happy path posts the audited reason", async ({ page }) => {
  const api = await mockAdminApi(page);
  await page.goto(`processing/review/${WORK_ITEM_ID}`);
  await page.getByRole("button", { name: "Accept Release" }).click();

  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Reason").fill("Verified against the catalog.");
  await dialog.getByRole("button", { name: "Apply decision" }).click();

  await expect(page.getByText("Decision recorded.")).toBeVisible();
  expect(api.getDecisionBody()).toEqual({
    action: "accept",
    reason: "Verified against the catalog.",
  });
});

test("rtl smoke: ar-SA renders the workspace with dir=rtl", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto(`processing/review/${WORK_ITEM_ID}`);
  await expect(page.getByText("Example Artist — Greatest Hits")).toBeVisible();

  await page.getByRole("button", { name: "Language", exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await expect(page.locator("html")).toHaveAttribute("lang", "ar-SA");
  // Data stays visible; the chrome mirrors.
  await expect(page.getByText("Example Artist — Greatest Hits")).toBeVisible();
});

import { expect, type Page } from "@playwright/test";

/**
 * Shared deterministic admin-API mock for the Wave-4 quality suites
 * (accessibility, visual regression, localization/RTL). Consolidates the
 * per-spec fixtures into one router so the axe/screenshot/localization
 * passes all render the same canonical screens against the same data.
 *
 * Determinism contract (PRD-UI-REF-002):
 *  - every timestamp is a fixed 2026-08-14 value;
 *  - artwork endpoints answer an undecodable empty 200 so the UI falls back
 *    to deterministic initials avatars (no network-dependent pixels);
 *  - `installFixedClock` freezes Date/timers so relative-time rendering and
 *    health polling never drift between baseline generation and comparison.
 */

export const FIXED_NOW = new Date("2026-08-14T12:30:00Z");

const WORK_ITEM_ID = "11111111-1111-4111-8111-111111111111";
const WORK_ITEM_ID_TWO = "22222222-2222-4222-8222-222222222222";
const RELEASE_ID = "55555555-5555-4555-8555-555555555555";
export const ARTIST_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
export const CATALOG_RELEASE_ID = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const TRACK_ID = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const OBSERVATION_ID = "11111111-1111-4111-8111-111111111111";
const OBSERVATION_ID_2 = "22222222-2222-4222-8222-222222222222";
const USER_ID = "11111111-1111-4111-8111-111111111111";
const GROUP_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

const HEALTH = {
  status: "ok",
  postgresql: { status: "ok", detail: "postgresql answered a bounded probe" },
  tantivy: { status: "ok", detail: "active search generation validated" },
  storage: { status: "ok", detail: "storage paths writable" },
  ffmpeg: { status: "ok", detail: "ffmpeg 8.1" },
  podcastStorage: { status: "disabled", detail: "podcast worker disabled" },
  disk: { status: "ok", detail: "tightest storage root has sufficient free space" },
  workers: { status: "ok", detail: "no stale leases; worker queues are healthy" },
  apiSurfaces: [
    { surface: "native", status: "serving" },
    { surface: "opensubsonic", status: "serving" },
    { surface: "jellyfin", status: "serving" },
    { surface: "admin", status: "serving" },
  ],
};

const DASHBOARD = {
  generatedAt: "2026-08-14T12:00:00Z",
  catalog: {
    artists: 35214,
    releases: 50482,
    tracks: 560291,
    mediaAssets: 600000,
    pendingSearchOutbox: 12,
  },
  workItems: {
    countsByState: [
      { state: "discovered", count: 3 },
      { state: "ready", count: 5 },
      { state: "needs_review", count: 2 },
      { state: "failed", count: 1 },
    ],
    failedLast24h: 4,
    completedLast24h: 128,
  },
  attention: {
    unresolvedIdentities: 23,
    pendingPolicyDecisions: 14,
    failedProcessing: 1,
    quarantined: 8,
  },
  health: HEALTH,
  activity: {
    completedPerHour: [
      { hour: "2026-08-14T10:00:00Z", count: 5 },
      { hour: "2026-08-14T11:00:00Z", count: 9 },
      { hour: "2026-08-14T12:00:00Z", count: 7 },
    ],
  },
};

const ITEM_ONE = {
  id: WORK_ITEM_ID,
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
  id: WORK_ITEM_ID_TWO,
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

const REVIEW_WORK_ITEM = {
  ...ITEM_ONE,
  sourcePath: "/library/incoming/Greatest Hits",
  currentPath: "/library/incoming/Greatest Hits",
  state: "needs_review",
  stateReason: "policy requires review",
  attemptCount: 1,
  leaseOwner: null,
  leaseAcquiredAt: null,
  leaseExpiresAt: null,
};

const REVIEW_DETAIL = {
  workItem: REVIEW_WORK_ITEM,
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

const OBSERVATION = {
  id: OBSERVATION_ID,
  displayValue: "The Nomads",
  rawValue: "The Nomads",
  normalizedValue: "the nomads",
  scope: "release",
  sourceOrdinal: 0,
  sourcePointer: "/library/incoming/Into the Night",
  releaseId: CATALOG_RELEASE_ID,
  releaseTitle: "Into the Night",
  trackId: null,
  trackTitle: null,
  pendingMatchCount: 2,
  evidenceCount: 5,
  createdAt: "2026-08-14T10:00:00Z",
};

const IDENTITY_DETAIL = {
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

const ARTIST_ROW = {
  id: ARTIST_ID,
  name: "The Nomads",
  sortName: "Nomads, The",
  artistType: "group",
  disambiguation: "Australian rock band",
  status: "verified",
  releaseCount: 12,
  trackCount: 128,
  imageId: `artist_${ARTIST_ID}`,
  createdAt: "2026-08-01T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const RELEASE_ROW = {
  id: CATALOG_RELEASE_ID,
  title: "Into the Night",
  displayArtist: "The Nomads",
  releaseDate: "1986-05-12",
  releaseDateText: null,
  status: "active",
  trackCount: 1,
  durationMs: 222000,
  imageId: `album_${CATALOG_RELEASE_ID}`,
  createdAt: "2026-08-01T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const TRACK_ROW = {
  id: TRACK_ID,
  title: "Down Under",
  displayArtist: "The Nomads",
  ordinal: 3,
  discNumber: 1,
  trackNumber: 3,
  durationMs: 222000,
  releaseId: CATALOG_RELEASE_ID,
  releaseTitle: "Into the Night",
  imageId: `song_${TRACK_ID}`,
  createdAt: "2026-08-01T10:00:00Z",
};

const ARTIST_DETAIL = {
  ...ARTIST_ROW,
  beginDate: "1978-01-01",
  endDate: null,
  mergedIntoArtistId: null,
  revision: 3,
  stats: {
    releaseCount: 12,
    billedTrackCount: 128,
    performanceTrackCount: 391,
    allCreditTrackCount: 400,
  },
  aliases: {
    items: [
      {
        alias: "Nomads",
        aliasType: "search",
        locale: null,
        evidenceSource: "tags",
        confidence: 9500,
        locked: false,
        createdAt: "2026-08-01T10:00:00Z",
      },
    ],
    total: 1,
  },
  externalIds: { items: [], total: 0 },
  releases: {
    items: [
      {
        releaseId: CATALOG_RELEASE_ID,
        title: "Into the Night",
        role: "main",
        presentationOrder: 0,
        displayValue: "The Nomads",
        releaseDate: "1986-05-12",
        trackCount: 1,
      },
    ],
    total: 1,
  },
  contributions: { items: [], total: 0 },
  relationships: { items: [], total: 0 },
  memberships: { items: [], total: 0 },
  history: {
    items: [
      {
        eventId: "ffffffff-ffff-4fff-8fff-ffffffffffff",
        eventType: "artist.created",
        actor: "system",
        reason: "catalog import",
        payload: {},
        createdAt: "2026-08-01T10:00:00Z",
      },
    ],
    total: 1,
  },
};

const RELEASE_DETAIL = {
  ...RELEASE_ROW,
  sortTitle: "Into the Night",
  revision: 1,
  mediaBytes: 33554432,
  tracks: {
    items: [
      {
        id: TRACK_ID,
        ordinal: 1,
        title: "Down Under",
        displayArtist: "The Nomads",
        discNumber: 1,
        trackNumber: 1,
        durationMs: 222000,
        sourcePath: "Into the Night/01 Down Under.flac",
        container: "flac",
        codec: "flac",
        byteSize: 33554432,
        mimeType: "audio/flac",
        streamUrl: `/song/stream/${TRACK_ID}/${ARTIST_ID}/mock-signed-token`,
      },
    ],
    total: 1,
  },
  artists: [
    {
      artistId: ARTIST_ID,
      name: "The Nomads",
      status: "verified",
      role: "main",
      presentationOrder: 0,
      displayValue: "The Nomads",
      joinPhrase: "",
    },
  ],
  contributors: { items: [], total: 0 },
  mediaAssets: { items: [], total: 0 },
  classification: null,
  provenance: {
    workItemId: WORK_ITEM_ID,
    manifestId: "77777777-7777-4777-8777-777777777777",
    manifestSchemaVersion: 1,
    semanticManifestHash: "abc123",
    publicationPlanHash: "def456",
    sealedManifestHash: "ghi789",
    resolverVersion: "1.0.0",
    importedAt: "2026-08-01T11:00:00Z",
  },
  history: { items: [], total: 0 },
};

const RULE_SET = {
  id: "01989f00-7000-7000-8000-000500000003",
  name: "phase5-default-rules",
  version: 3,
  createdAt: "2026-08-14T10:00:00Z",
  createdBy: { system: { policyRuleSetVersion: 3 } },
};

const RULES = [
  {
    id: "POL-ADMIN-OVERRIDE",
    ruleType: "administrator_override",
    disposition: "review",
    confidenceThreshold: null,
    predicate: {},
    priority: 0,
    enabled: true,
    matchCount: 4,
  },
  {
    id: "POL-CORRUPT",
    ruleType: "corrupt_media",
    disposition: "quarantine",
    confidenceThreshold: null,
    predicate: {},
    priority: 1,
    enabled: true,
    matchCount: 9,
  },
  {
    id: "POL-GH-REDUNDANT",
    ruleType: "greatest_hits_redundancy",
    disposition: "ignore",
    confidenceThreshold: null,
    predicate: {},
    priority: 4,
    enabled: true,
    matchCount: 132,
  },
  {
    id: "POL-LIVE-REVIEW",
    ruleType: "trait_disposition",
    disposition: "review",
    confidenceThreshold: 8000,
    predicate: { required_traits: ["live"] },
    priority: 5,
    enabled: true,
    matchCount: 12,
  },
];

const POLICY_POOL = {
  name: "policy",
  paused: false,
  pausedBy: null,
  pausedAt: null,
  resumeRequestedAt: null,
  updatedAt: "2026-08-14T12:00:00Z",
  metricsAvailable: true,
  queued: 7,
  running: 2,
  oldestQueuedAgeSeconds: 95,
  completedLastHour: 60,
  throughputPerMinute: 1,
  failedLast24h: 0,
};

const DISCOVERY_POOL = {
  ...POLICY_POOL,
  name: "discovery",
  metricsAvailable: false,
  queued: null,
  running: null,
  oldestQueuedAgeSeconds: null,
  completedLastHour: null,
  throughputPerMinute: null,
  failedLast24h: null,
};

const SEARCH_STATUS = {
  enabled: true,
  projection: {
    pending: 3,
    failed: 0,
    leased: 1,
    oldestAvailableAt: null,
    lastProcessedOutboxId: 42,
    activeGenerationId: "55555555-5555-4555-8555-555555555555",
    latestRebuildStatus: null,
    latestRebuildError: null,
  },
  generation: {
    artists: 100,
    releases: 200,
    tracks: 300,
    total: 600,
    schemaVersion: 1,
  },
  generationErrorCode: null,
  generationError: null,
};

const LEASE = {
  workItemId: "66666666-6666-4666-8666-666666666666",
  apiKey: "77777777-7777-4777-8777-777777777777",
  state: "inspecting",
  leaseOwner: "worker-3",
  leaseAcquiredAt: "2026-08-14T11:55:00Z",
  leaseExpiresAt: "2026-08-14T12:05:00Z",
  lastActivityAt: "2026-08-14T11:56:00Z",
};

const USER = {
  id: USER_ID,
  username: "operator",
  email: "operator@example.com",
  isAdmin: true,
  isEditor: false,
  enabled: true,
  locked: false,
  roles: ["HasStreamRole"],
  createdAt: "2026-08-14T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const GROUP = {
  id: GROUP_ID,
  name: "Family",
  memberCount: 1,
  bindingCount: 0,
  createdAt: "2026-08-14T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const EXPORT_DOC = {
  format: "melodee-config-export/v1",
  warning: "Secrets are REDACTED and must be re-entered after import.",
  exportedAt: "2026-08-14T12:00:00Z",
  settings: {
    "server.port": 8080,
    "smtp.password": "__MELODEE_SECRET_REDACTED__",
  },
};

function onboardingCheck(
  name: string,
  overrides: Record<string, unknown> = {},
) {
  return {
    name,
    required: true,
    passed: true,
    detail: `${name} ok`,
    remediation: null,
    ...overrides,
  };
}

const ONBOARDING_PENDING = {
  completed: false,
  bypassed: false,
  canComplete: false,
  completedBy: null,
  checks: [
    onboardingCheck("database"),
    onboardingCheck("storage_paths"),
    onboardingCheck("media_tools"),
    onboardingCheck("search_index", { required: false }),
    onboardingCheck("admin_user"),
    onboardingCheck("email", { required: false }),
  ],
};

// The server re-evaluates all six preflight checks on every status read, so
// a completed server still reports them — the Storage screen renders the
// storage_paths detail from this payload.
const ONBOARDING_COMPLETED = {
  completed: true,
  bypassed: false,
  canComplete: true,
  completedBy: "admin",
  checks: [
    onboardingCheck("database"),
    onboardingCheck("storage_paths", {
      required: false,
      detail:
        "inbound: 2 source(s) [/srv/incoming, /srv/drop]; staging: /srv/staging; work: /var/cache/melodee/work; canonical: /srv/melodee",
    }),
    onboardingCheck("media_tools"),
    onboardingCheck("search_index", { required: false }),
    onboardingCheck("admin_user"),
    onboardingCheck("email", { required: false }),
  ],
};

export interface MockOptions {
  /** onboarding/status answer: 'completed' (default) or the pending wizard. */
  onboarding?: "completed" | "pending";
}

/**
 * Routes every admin API endpoint the canonical screens consume, answering
 * with the deterministic fixtures above. Unknown endpoints answer the real
 * envelope's 404 error shape so screens render their honest error states
 * instead of hanging.
 */
export async function mockAdminApi(
  page: Page,
  options: MockOptions = {},
): Promise<void> {
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
    if (pathname === "/api/admin/v1/dashboard") {
      await json(DASHBOARD);
      return;
    }
    if (pathname === "/api/admin/v1/system/health") {
      await json(HEALTH);
      return;
    }
    if (pathname === "/api/admin/v1/workers/pools") {
      await json({
        items: [POLICY_POOL, DISCOVERY_POOL],
        total: 2,
        limit: 2,
        offset: 0,
      });
      return;
    }
    if (pathname === "/api/admin/v1/workers/failures") {
      await json({ items: [ITEM_TWO], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === "/api/admin/v1/search/status") {
      await json(SEARCH_STATUS);
      return;
    }
    if (pathname === "/api/admin/v1/ingest/leases") {
      await json([LEASE]);
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
      await json([
        {
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
        },
      ]);
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
    if (pathname === "/api/admin/v1/review/queue") {
      await json({
        items: [{ ...REVIEW_WORK_ITEM, releaseId: RELEASE_ID }],
        total: 1,
        limit: 50,
        offset: 0,
        countsByState: [
          { state: "needs_review", count: 1 },
          { state: "failed", count: 2 },
        ],
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
      await json(REVIEW_DETAIL);
      return;
    }
    if (pathname === "/api/admin/v1/identity/unresolved") {
      await json({
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
      });
      return;
    }
    if (pathname === `/api/admin/v1/identity/observations/${OBSERVATION_ID}`) {
      await json(IDENTITY_DETAIL);
      return;
    }
    if (pathname === "/api/admin/v1/identity/decisions") {
      await json({
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
      });
      return;
    }
    if (pathname === "/api/admin/v1/catalog/artists") {
      await json({ items: [ARTIST_ROW], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === "/api/admin/v1/catalog/releases") {
      await json({ items: [RELEASE_ROW], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === "/api/admin/v1/catalog/tracks") {
      await json({ items: [TRACK_ROW], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === `/api/admin/v1/catalog/artists/${ARTIST_ID}`) {
      await json(ARTIST_DETAIL);
      return;
    }
    if (pathname === `/api/admin/v1/catalog/releases/${CATALOG_RELEASE_ID}`) {
      await json(RELEASE_DETAIL);
      return;
    }
    if (pathname === `/api/admin/v1/catalog/tracks/${TRACK_ID}`) {
      await json({
        ...TRACK_ROW,
        sortTitle: "Down Under",
        audioIdentityId: WORK_ITEM_ID_TWO,
        revision: 1,
        streamUrl: `/song/stream/${TRACK_ID}/${ARTIST_ID}/mock-signed-token`,
        updatedAt: "2026-08-14T10:00:00Z",
        artists: RELEASE_DETAIL.artists,
        contributors: { items: [], total: 0 },
        mediaAssets: [],
      });
      return;
    }
    if (pathname === "/api/admin/v1/policies/rules") {
      await json({
        ruleSet: RULE_SET,
        items: RULES,
        total: RULES.length,
        limit: RULES.length,
        offset: 0,
      });
      return;
    }
    if (pathname === "/api/admin/v1/users") {
      await json({ items: [USER], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === "/api/admin/v1/groups") {
      await json([GROUP]);
      return;
    }
    if (pathname === `/api/admin/v1/groups/${GROUP.id}`) {
      await json({
        ...GROUP,
        members: [{ userId: USER.id, username: "operator" }],
        libraryIds: [],
      });
      return;
    }
    if (pathname === "/api/admin/v1/config/export") {
      await json(EXPORT_DOC);
      return;
    }
    // Settings sub-sections eagerly load their lists; empty inventories are
    // valid server states and keep the console free of 404 noise.
    if (
      pathname === "/api/admin/v1/notifications/templates" ||
      pathname === "/api/admin/v1/notifications/deliveries"
    ) {
      await json([]);
      return;
    }
    if (pathname === "/api/admin/v1/scripts") {
      await json({ scripts: [], wiredEvents: [] });
      return;
    }
    if (pathname === "/api/admin/v1/scripts/runs") {
      await json([]);
      return;
    }
    if (
      pathname === "/api/admin/v1/radio/stations" ||
      pathname === "/api/admin/v1/device-profiles"
    ) {
      await json([]);
      return;
    }
    // Native theme packs (PRD-THEME-004): the Settings → Theme section lists
    // them; a deterministic built-in-only inventory keeps the shared suites
    // free of 404 noise. Theme-pack behavior itself is covered (with a
    // custom pack) by e2e/theme-packs.spec.ts.
    if (pathname === "/api/v1/themes") {
      await json([
        {
          id: "server-default",
          name: "Server Built-in",
          author: "Melodee",
          version: "1.0.0",
          description: null,
          isBuiltIn: true,
          previewImage: null,
          hasWarnings: false,
          warningDetails: null,
        },
      ]);
      return;
    }
    if (pathname === "/api/admin/v1/onboarding/status") {
      await json(
        options.onboarding === "pending"
          ? ONBOARDING_PENDING
          : ONBOARDING_COMPLETED,
      );
      return;
    }
    await json(
      { code: "not_found", message: "not found", correlationId: null },
      404,
    );
  });

  // Absent artwork: the image endpoint answers an undecodable empty 200; the
  // UI falls back to initials, never a broken image (deterministic pixels).
  await page.route("**/images/**", async (route) => {
    await route.fulfill({ status: 200, body: "" });
  });
}

/** Freezes Date/timers before app scripts run so relative times are stable. */
export async function installFixedClock(page: Page): Promise<void> {
  await page.clock.install({ time: FIXED_NOW });
}

/**
 * Seeds locale + appearance before app scripts run. The locale is only
 * written when absent: init scripts re-run on every full navigation, and
 * tests that switch culture mid-flight (via localStorage + reload, or the
 * language selector) must not have their choice clobbered. Every test gets
 * a fresh browser context, so absence correctly means "first navigation".
 */
export async function seedPreferences(
  page: Page,
  preferences: {
    locale?: string;
    appearance?: "light" | "dark" | "system";
  } = {},
): Promise<void> {
  await page.addInitScript((seeded) => {
    if (!window.localStorage.getItem("melodee.locale")) {
      window.localStorage.setItem("melodee.locale", seeded.locale ?? "en-US");
    }
    if (seeded.appearance) {
      window.localStorage.setItem("melodee.appearance", seeded.appearance);
    }
  }, preferences);
}

/**
 * The canonical screens (UI-SPEC §7/§8, PRD §39) plus the WP-19 surfaces
 * (users, settings, onboarding) that complete the admin IA. `path` is
 * relative to the /admin/ baseURL; `ready` waits for the screen's stable,
 * data-driven content (never a spinner) before quality checks run.
 */
export interface CanonicalScreen {
  key: string;
  path: string;
  mockOptions?: MockOptions;
  ready: (page: Page) => Promise<void>;
}

export const CANONICAL_SCREENS: CanonicalScreen[] = [
  {
    key: "overview",
    path: "",
    ready: async (page) => {
      await expect(
        page.getByRole("heading", { name: "Needs attention" }),
      ).toBeVisible();
    },
  },
  {
    key: "processing-queue",
    path: "processing",
    ready: async (page) => {
      await expect(
        page.getByRole("table", { name: "Processing queue work items" }),
      ).toBeVisible();
      await expect(page.getByText("Business as Usual")).toBeVisible();
    },
  },
  {
    key: "release-review",
    path: `processing/review/${WORK_ITEM_ID}`,
    ready: async (page) => {
      await expect(
        page.getByRole("heading", { name: "Greatest Hits" }).first(),
      ).toBeVisible();
    },
  },
  {
    key: "identity-review",
    path: "catalog/identity",
    ready: async (page) => {
      await expect(
        page.getByRole("table", { name: "Unresolved artist observations" }),
      ).toBeVisible();
      await expect(page.getByText("Silver Skies")).toBeVisible();
    },
  },
  {
    key: "artist-detail",
    path: `catalog/artists/${ARTIST_ID}`,
    ready: async (page) => {
      await expect(
        page.getByRole("heading", { name: "The Nomads" }),
      ).toBeVisible();
      await expect(page.getByText("Australian rock band")).toBeVisible();
    },
  },
  {
    key: "release-detail",
    path: `catalog/releases/${CATALOG_RELEASE_ID}`,
    ready: async (page) => {
      await expect(
        page.getByRole("heading", { name: "Into the Night" }),
      ).toBeVisible();
    },
  },
  {
    key: "policies",
    path: "policies/releases",
    ready: async (page) => {
      await expect(
        page.getByRole("table", { name: "Policy rules" }),
      ).toBeVisible();
      await expect(page.getByText("POL-GH-REDUNDANT")).toBeVisible();
    },
  },
  {
    key: "system-workers",
    path: "system/workers",
    ready: async (page) => {
      await expect(
        page.getByText("phase5-default-rules", { exact: false }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("button", { name: "Pause" }).first(),
      ).toBeVisible();
    },
  },
  {
    key: "system-search",
    path: "system/search",
    ready: async (page) => {
      await expect(
        page
          .locator("section", {
            has: page.getByRole("heading", { name: "Search Index" }),
          })
          .getByRole("button", { name: "Rebuild search index" }),
      ).toBeVisible();
      await expect(page.getByText("Indexed documents")).toBeVisible();
    },
  },
  {
    key: "system-storage",
    path: "system/storage",
    ready: async (page) => {
      await expect(
        page.getByRole("heading", { name: "Storage Paths Preflight" }),
      ).toBeVisible();
      await expect(page.getByText("storage paths writable")).toBeVisible();
    },
  },
  {
    key: "system-diagnostics",
    path: "system/diagnostics",
    ready: async (page) => {
      await expect(
        page.getByRole("heading", { name: "System Health" }),
      ).toBeVisible();
      await expect(
        page.getByRole("table", { name: "Active worker leases" }),
      ).toBeVisible();
    },
  },
  {
    key: "users",
    path: "system/users",
    ready: async (page) => {
      await expect(
        page.getByRole("table", { name: "User accounts" }),
      ).toBeVisible();
      await expect(page.getByText("operator@example.com")).toBeVisible();
    },
  },
  {
    key: "settings",
    path: "system/settings",
    ready: async (page) => {
      await expect(page.getByText("server.port")).toBeVisible();
    },
  },
  {
    key: "onboarding",
    path: "onboarding",
    mockOptions: { onboarding: "pending" },
    ready: async (page) => {
      await expect(page.getByText("Database").first()).toBeVisible();
      await expect(
        page.getByRole("button", { name: "Complete setup" }),
      ).toBeVisible();
    },
  },
];

/** Navigates to a canonical screen with mocks + locale seeded and waits for it. */
export async function gotoScreen(
  page: Page,
  screen: CanonicalScreen,
  preferences: {
    locale?: string;
    appearance?: "light" | "dark" | "system";
  } = {},
): Promise<void> {
  await mockAdminApi(page, screen.mockOptions ?? {});
  await installFixedClock(page);
  await seedPreferences(page, preferences);
  await page.goto(screen.path || "/");
  await screen.ready(page);
  const locale = preferences.locale ?? "en-US";
  // Locale fully applied (document root + lazy bundle re-render) before any
  // quality check or screenshot runs.
  await expect(page.locator("html")).toHaveAttribute("lang", locale);
  // The shell health poll flips 'checking' → 'online' once the mocked
  // system/info answers; wait so screenshots never capture the transient.
  // The header indicator hides at the sm breakpoint (UI-SPEC §8), so only
  // wait on it when it participates in the layout.
  const headerHealth = page.locator(".melodee-shell__health");
  if (await headerHealth.isVisible()) {
    await expect(headerHealth).not.toContainText("Checking server status");
  }
  // The nav footer carries the active culture only when the nav is expanded
  // (lg+; the md rail and sm overlay render the toggle button alone).
  if ((page.viewportSize()?.width ?? 0) >= 1056) {
    await expect(page.locator(".melodee-shell__footer")).toContainText(locale);
  }
  // Carbon loads IBM Plex with @font-face declarations. A visible layout can
  // still be using fallback metrics while those faces are in flight, which
  // makes screenshot assertions depend on runner timing. Wait for the
  // browser's FontFaceSet so visual baselines observe the final layout.
  await page.evaluate(async () => {
    await document.fonts.ready;
  });
}

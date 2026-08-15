import { expect, test, type Page } from '@playwright/test';

/**
 * Catalog browsing + detail e2e (WP-15). The admin API is mocked with
 * page.route() — no live server is required. Covers: server-side list
 * envelope + query/sort request mapping, list → detail navigation for
 * Artist/Release/Track (runs at all three canonical viewports via the
 * Playwright projects), keyboard-accessible tabs, the artwork 404/empty
 * fallback, and the ar-SA dir=rtl smoke on the Artist detail.
 */

const ARTIST_ID = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const RELEASE_ID = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const TRACK_ID = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc';

const ARTIST_ROW = {
  id: ARTIST_ID,
  name: 'The Nomads',
  sortName: 'Nomads, The',
  artistType: 'group',
  disambiguation: 'Australian rock band',
  status: 'verified',
  releaseCount: 12,
  trackCount: 128,
  imageId: `artist_${ARTIST_ID}`,
  createdAt: '2026-08-01T10:00:00Z',
  updatedAt: '2026-08-14T10:00:00Z',
};

const RELEASE_ROW = {
  id: RELEASE_ID,
  title: 'Into the Night',
  displayArtist: 'The Nomads',
  releaseDate: '1986-05-12',
  releaseDateText: null,
  status: 'active',
  trackCount: 1,
  durationMs: 222000,
  imageId: `album_${RELEASE_ID}`,
  createdAt: '2026-08-01T10:00:00Z',
  updatedAt: '2026-08-14T10:00:00Z',
};

const TRACK_ROW = {
  id: TRACK_ID,
  title: 'Down Under',
  displayArtist: 'The Nomads',
  ordinal: 3,
  discNumber: 1,
  trackNumber: 3,
  durationMs: 222000,
  releaseId: RELEASE_ID,
  releaseTitle: 'Into the Night',
  imageId: `song_${TRACK_ID}`,
  createdAt: '2026-08-01T10:00:00Z',
};

const ARTIST_DETAIL = {
  id: ARTIST_ID,
  name: 'The Nomads',
  sortName: 'Nomads, The',
  artistType: 'group',
  disambiguation: 'Australian rock band',
  status: 'verified',
  beginDate: '1978-01-01',
  endDate: null,
  mergedIntoArtistId: null,
  revision: 3,
  imageId: `artist_${ARTIST_ID}`,
  createdAt: '2026-08-01T10:00:00Z',
  updatedAt: '2026-08-14T10:00:00Z',
  stats: { releaseCount: 12, billedTrackCount: 128, performanceTrackCount: 391, allCreditTrackCount: 400 },
  aliases: {
    items: [
      {
        alias: 'Nomads',
        aliasType: 'search',
        locale: null,
        evidenceSource: 'tags',
        confidence: 9500,
        locked: false,
        createdAt: '2026-08-01T10:00:00Z',
      },
    ],
    total: 1,
  },
  externalIds: { items: [], total: 0 },
  releases: {
    items: [
      {
        releaseId: RELEASE_ID,
        title: 'Into the Night',
        role: 'main',
        presentationOrder: 0,
        displayValue: 'The Nomads',
        releaseDate: '1986-05-12',
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
        eventId: 'ffffffff-ffff-4fff-8fff-ffffffffffff',
        eventType: 'artist.created',
        actor: 'system',
        reason: 'catalog import',
        payload: {},
        createdAt: '2026-08-01T10:00:00Z',
      },
    ],
    total: 1,
  },
};

const RELEASE_DETAIL = {
  id: RELEASE_ID,
  title: 'Into the Night',
  sortTitle: 'Into the Night',
  displayArtist: 'The Nomads',
  releaseDate: '1986-05-12',
  releaseDateText: null,
  status: 'active',
  revision: 1,
  trackCount: 1,
  durationMs: 222000,
  mediaBytes: 33554432,
  imageId: `album_${RELEASE_ID}`,
  createdAt: '2026-08-01T10:00:00Z',
  updatedAt: '2026-08-14T10:00:00Z',
  tracks: {
    items: [
      {
        id: TRACK_ID,
        ordinal: 1,
        title: 'Down Under',
        displayArtist: 'The Nomads',
        discNumber: 1,
        trackNumber: 1,
        durationMs: 222000,
        sourcePath: 'Into the Night/01 Down Under.flac',
        container: 'flac',
        codec: 'flac',
        byteSize: 33554432,
        mimeType: 'audio/flac',
        streamUrl: `/song/stream/${TRACK_ID}/${ARTIST_ID}/mock-signed-token`,
      },
    ],
    total: 1,
  },
  artists: [
    {
      artistId: ARTIST_ID,
      name: 'The Nomads',
      status: 'verified',
      role: 'main',
      presentationOrder: 0,
      displayValue: 'The Nomads',
      joinPhrase: '',
    },
  ],
  contributors: { items: [], total: 0 },
  mediaAssets: { items: [], total: 0 },
  classification: null,
  provenance: {
    workItemId: '11111111-1111-4111-8111-111111111111',
    manifestId: '77777777-7777-4777-8777-777777777777',
    manifestSchemaVersion: 1,
    semanticManifestHash: 'abc123',
    publicationPlanHash: 'def456',
    sealedManifestHash: 'ghi789',
    resolverVersion: '1.0.0',
    importedAt: '2026-08-01T11:00:00Z',
  },
  history: { items: [], total: 0 },
};

const TRACK_DETAIL = {
  id: TRACK_ID,
  title: 'Down Under',
  sortTitle: 'Down Under',
  displayArtist: 'The Nomads',
  ordinal: 3,
  discNumber: 1,
  trackNumber: 3,
  durationMs: 222000,
  sourcePath: 'Into the Night/03 Down Under.flac',
  audioIdentityId: '22222222-2222-4222-8222-222222222222',
  revision: 1,
  imageId: `song_${TRACK_ID}`,
  streamUrl: `/song/stream/${TRACK_ID}/${ARTIST_ID}/mock-signed-token`,
  createdAt: '2026-08-01T10:00:00Z',
  updatedAt: '2026-08-14T10:00:00Z',
  releaseId: RELEASE_ID,
  releaseTitle: 'Into the Night',
  artists: [
    {
      artistId: ARTIST_ID,
      name: 'The Nomads',
      status: 'verified',
      role: 'main',
      presentationOrder: 0,
      displayValue: 'The Nomads',
      joinPhrase: '',
    },
  ],
  contributors: { items: [], total: 0 },
  mediaAssets: [],
};

async function mockAdminApi(page: Page, options: { artworkUploadFails?: boolean } = {}) {
  const listRequests: string[] = [];
  const artworkRequests: { method: string; body: string }[] = [];
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const method = route.request().method();
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (pathname === '/api/admin/v1/system/info') {
      await json({ service: 'melodee', version: 'test', status: 'ok' });
      return;
    }
    // ADR-0143 artwork override endpoints (raw-body PUT; JSON-reason DELETE).
    if (
      pathname === `/api/admin/v1/catalog/artists/${ARTIST_ID}/image` ||
      pathname === `/api/admin/v1/catalog/releases/${RELEASE_ID}/image`
    ) {
      artworkRequests.push({ method, body: route.request().postData() ?? '' });
      if (method === 'PUT') {
        if (options.artworkUploadFails) {
          await json({ code: 'invalid_image', message: 'rejected', correlationId: null }, 400);
          return;
        }
        await json({
          message: 'Artwork updated',
          imageId: `artist_${ARTIST_ID}`,
          updatedAt: '2026-08-14T12:00:00Z',
        });
        return;
      }
      if (method === 'DELETE') {
        await route.fulfill({ status: 204 });
        return;
      }
    }
    if (pathname === '/api/admin/v1/catalog/artists') {
      listRequests.push(url.search);
      await json({ items: [ARTIST_ROW], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === '/api/admin/v1/catalog/releases') {
      await json({ items: [RELEASE_ROW], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === '/api/admin/v1/catalog/tracks') {
      await json({ items: [TRACK_ROW], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === `/api/admin/v1/catalog/artists/${ARTIST_ID}`) {
      await json(ARTIST_DETAIL);
      return;
    }
    if (pathname === `/api/admin/v1/catalog/releases/${RELEASE_ID}`) {
      await json(RELEASE_DETAIL);
      return;
    }
    if (pathname === `/api/admin/v1/catalog/tracks/${TRACK_ID}`) {
      await json(TRACK_DETAIL);
      return;
    }
    await json({ code: 'not_found', message: 'not found', correlationId: ARTIST_ID }, 404);
  });
  // Absent artwork: the native endpoint answers an undecodable empty 200;
  // the UI must fall back to initials, never a broken image.
  await page.route('**/images/**', async (route) => {
    await route.fulfill({ status: 200, body: '' });
  });
  return { listRequests, artworkRequests };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('artists list loads from the envelope and the search box drives the query filter', async ({
  page,
}) => {
  const api = await mockAdminApi(page);
  await page.goto('catalog/artists');

  await expect(page.getByRole('link', { name: 'The Nomads' })).toBeVisible();
  await expect(page.getByText('Australian rock band')).toBeVisible();

  await page.getByRole('searchbox', { name: 'Search artists' }).fill('nomads');
  await expect
    .poll(() => api.listRequests.some((search) => search.includes('query=nomads')))
    .toBe(true);

  // Whitelisted sort via the column header.
  await page.getByRole('button', { name: 'Sort by Name' }).click();
  await expect
    .poll(() => api.listRequests.some((search) => search.includes('sort=name%3Aasc')))
    .toBe(true);
});

test('list to detail navigation flows for Artist, Release, and Track', async ({ page }) => {
  await mockAdminApi(page);

  await page.goto('catalog/artists');
  await page.getByRole('link', { name: 'The Nomads' }).click();
  await expect(page).toHaveURL(new RegExp(`/catalog/artists/${ARTIST_ID}`));
  await expect(page.getByRole('heading', { name: 'The Nomads' })).toBeVisible();

  // Artwork fell back (empty image body), no broken image remains.
  await expect(page.locator(`img[src="/images/artist_${ARTIST_ID}/small"]`)).toHaveCount(0);

  await page.goto('catalog/releases');
  await page.getByRole('link', { name: 'Into the Night' }).click();
  await expect(page).toHaveURL(new RegExp(`/catalog/releases/${RELEASE_ID}`));
  await expect(page.getByRole('heading', { name: 'Into the Night' })).toBeVisible();

  await page.goto('catalog/tracks');
  await page.getByRole('link', { name: 'Down Under' }).click();
  await expect(page).toHaveURL(new RegExp(`/catalog/tracks/${TRACK_ID}`));
  await expect(page.getByRole('heading', { name: 'Down Under' })).toBeVisible();
  // Track detail links back to its Release.
  await expect(page.getByRole('link', { name: 'Into the Night' }).first()).toBeVisible();
});

test('cataloged tracks render labeled signed audio previews without autoplay', async ({
  page,
}) => {
  await mockAdminApi(page);

  // Track detail: the signed stream URL drives a labeled, non-autoplaying
  // transport (preload="none" means the browser never fetches it unprompted).
  await page.goto(`catalog/tracks/${TRACK_ID}`);
  const trackPreview = page.getByLabel('Audio preview: Down Under');
  await expect(trackPreview).toBeVisible();
  await expect(trackPreview).toHaveAttribute('preload', 'none');
  await expect(trackPreview).toHaveAttribute('src', new RegExp(`^/song/stream/${TRACK_ID}/`));
  await expect(trackPreview).not.toHaveAttribute('autoplay', '');

  // Release detail Tracks tab: one preview per playable track row.
  await page.goto(`catalog/releases/${RELEASE_ID}`);
  await page.getByRole('tab', { name: 'Tracks' }).click();
  const rowPreview = page.getByLabel('Audio preview: Down Under');
  await expect(rowPreview).toBeVisible();
  await expect(rowPreview).toHaveAttribute('preload', 'none');
});

test('artist detail tabs are keyboard accessible', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto(`catalog/artists/${ARTIST_ID}`);
  await expect(page.getByRole('heading', { name: 'The Nomads' })).toBeVisible();

  const overview = page.getByRole('tab', { name: 'Overview' });
  await overview.click();
  await expect(overview).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByText('Nomads', { exact: true })).toBeVisible();

  // Carbon tab lists are arrow-key navigable with roving selection.
  await overview.focus();
  await page.keyboard.press('ArrowRight');
  const releases = page.getByRole('tab', { name: 'Releases' });
  await expect(releases).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(releases).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('link', { name: 'Into the Night' })).toBeVisible();
});

test('release detail shows tracks, an honest empty classification, and provenance', async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto(`catalog/releases/${RELEASE_ID}`);
  await expect(page.getByRole('heading', { name: 'Into the Night' })).toBeVisible();

  await page.getByRole('tab', { name: 'Tracks' }).click();
  await expect(page.getByRole('link', { name: 'Down Under' })).toBeVisible();
  await expect(page.getByText('flac / flac')).toBeVisible();
  await expect(page.getByLabel('Audio preview: Down Under')).toBeVisible();

  await page.getByRole('tab', { name: 'Classification' }).click();
  await expect(page.getByText('This Release has not been classified yet.')).toBeVisible();

  await page.getByRole('tab', { name: 'Provenance' }).click();
  await expect(page.getByText('abc123')).toBeVisible();
  await expect(page.getByText('ghi789')).toBeVisible();
});

// ADR-0143 artwork override management (upload → remove → error path).
const PNG_BYTES = Buffer.from([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
]);

test('artist artwork upload and remove flow through the override endpoints', async ({ page }) => {
  const api = await mockAdminApi(page);
  await page.goto(`catalog/artists/${ARTIST_ID}`);
  await expect(page.getByRole('heading', { name: 'The Nomads' })).toBeVisible();

  await page.getByRole('tab', { name: 'Images' }).click();
  await page.getByLabel('Choose image file').setInputFiles({
    name: 'cover.png',
    mimeType: 'image/png',
    buffer: PNG_BYTES,
  });
  await expect(page.getByText(/Selected: cover.png/)).toBeVisible();
  await page.getByRole('button', { name: 'Upload artwork' }).click();
  await expect
    .poll(() => api.artworkRequests.some((request) => request.method === 'PUT'))
    .toBe(true);
  // The detail refetches after the mutation.
  await expect(page.getByRole('heading', { name: 'The Nomads' })).toBeVisible();

  // Removal goes through the destructive-confirm dialog with a reason.
  await page.getByRole('button', { name: 'Remove custom artwork' }).click();
  await page.getByLabel('Reason').fill('wrong artist photo');
  await page
    .locator('.cds--modal-footer')
    .getByRole('button', { name: 'Remove custom artwork' })
    .click();
  await expect
    .poll(() =>
      api.artworkRequests.some(
        (request) => request.method === 'DELETE' && request.body.includes('wrong artist photo'),
      ),
    )
    .toBe(true);
});

test('release artwork upload error surfaces the localized 400 message', async ({ page }) => {
  await mockAdminApi(page, { artworkUploadFails: true });
  await page.goto(`catalog/releases/${RELEASE_ID}`);
  await expect(page.getByRole('heading', { name: 'Into the Night' })).toBeVisible();

  await page.getByRole('tab', { name: 'Images' }).click();
  await page.getByLabel('Choose image file').setInputFiles({
    name: 'cover.png',
    mimeType: 'image/png',
    buffer: PNG_BYTES,
  });
  await page.getByRole('button', { name: 'Upload artwork' }).click();
  await expect(page.getByText(/The server rejected the image/)).toBeVisible();
});

test('release artwork client-side validation rejects a non-image file', async ({ page }) => {
  const api = await mockAdminApi(page);
  await page.goto(`catalog/releases/${RELEASE_ID}`);
  await page.getByRole('tab', { name: 'Images' }).click();
  await page.getByLabel('Choose image file').setInputFiles({
    name: 'notes.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from('not an image'),
  });
  await expect(page.getByText('Only JPEG or PNG images can be uploaded.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Upload artwork' })).toBeDisabled();
  expect(api.artworkRequests).toHaveLength(0);
});

test('rtl smoke: ar-SA renders the artist detail with dir=rtl', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto(`catalog/artists/${ARTIST_ID}`);
  await expect(page.getByRole('heading', { name: 'The Nomads' })).toBeVisible();

  await page.getByRole('button', { name: 'Language', exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
  // Data stays visible; untranslated catalog keys fall back to English.
  await expect(page.getByRole('heading', { name: 'The Nomads' })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Overview' })).toBeVisible();
});

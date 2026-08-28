import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { ReleaseDetail } from '../../api/types';
import { ReleaseDetailScreen } from './ReleaseDetailScreen';

vi.mock('../../api/admin', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../api/admin')>();
  return {
    ...original,
    getCatalogRelease: vi.fn(),
  };
});

const { getCatalogRelease } = await import('../../api/admin');
const mockGetRelease = vi.mocked(getCatalogRelease);

const RELEASE_ID = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const ARTIST_ID = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const TRACK_ID = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc';
const WORK_ITEM_ID = '11111111-1111-4111-8111-111111111111';

function makeDetail(overrides: Partial<ReleaseDetail> = {}): ReleaseDetail {
  return {
    id: RELEASE_ID,
    title: 'Into the Night',
    sortTitle: 'Into the Night',
    displayArtist: 'The Nomads',
    releaseDate: '1986-05-12',
    releaseDateText: null,
    status: 'active',
    workflowState: 'indexed',
    policyDisposition: 'accept',
    policyEvidenceSummary: 'No configured policy rule matched; accepting Release',
    revision: 1,
    trackCount: 1,
    durationMs: 222_000,
    mediaBytes: 33_554_432,
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
          durationMs: 222_000,
          sourcePath: 'Into the Night/01 Down Under.flac',
          container: 'flac',
          codec: 'flac',
          byteSize: 33_554_432,
          mimeType: 'audio/flac',
          streamUrl: `/song/stream/${TRACK_ID}/dddddddd-dddd-4ddd-8ddd-dddddddddddd/signed-token`,
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
    mediaAssets: {
      items: [
        {
          id: '99999999-9999-4999-8999-999999999999',
          canonicalRelativePath: 'Into the Night/01 Down Under.flac',
          byteSize: 33_554_432,
          mimeType: 'audio/flac',
          container: 'flac',
          codec: 'flac',
          sampleRateHz: 44_100,
          channels: 2,
          bitrateBps: null,
          durationMs: 222_000,
        },
      ],
      total: 1,
    },
    classification: null,
    provenance: {
      workItemId: WORK_ITEM_ID,
      manifestId: '77777777-7777-4777-8777-777777777777',
      manifestSchemaVersion: 1,
      semanticManifestHash: 'abc123',
      publicationPlanHash: 'def456',
      sealedManifestHash: 'ghi789',
      resolverVersion: '1.0.0',
      importedAt: '2026-08-01T11:00:00Z',
    },
    history: {
      items: [
        {
          eventId: 'ffffffff-ffff-4fff-8fff-ffffffffffff',
          eventType: 'release.cataloged',
          actor: 'system',
          reason: 'catalog import',
          payload: {},
          createdAt: '2026-08-01T10:00:00Z',
        },
      ],
      total: 1,
    },
    ...overrides,
  };
}

function renderScreen(detail: ReleaseDetail) {
  mockGetRelease.mockResolvedValue(detail);
  return render(
    <MemoryRouter initialEntries={[`/catalog/releases/${RELEASE_ID}`]}>
      <Routes>
        <Route path="/catalog/releases/:id" element={<ReleaseDetailScreen />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('ReleaseDetailScreen', () => {
  it('renders the header stats strip, tracks table, artists, files, and provenance', async () => {
    renderScreen(makeDetail());
    expect(await screen.findByRole('heading', { name: 'Into the Night' })).toBeInTheDocument();
    expect(mockGetRelease).toHaveBeenCalledWith(RELEASE_ID, expect.any(AbortSignal));
    expect(screen.getAllByText('The Nomads').length).toBeGreaterThan(0);
    expect(screen.getByText('Active')).toBeInTheDocument();

    // Tracks tab: ordinal "1.1", track link, format, size, signed preview.
    fireEvent.click(screen.getByRole('tab', { name: 'Tracks' }));
    expect(screen.getByText('1.1')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Down Under' })).toHaveAttribute(
      'href',
      `/catalog/tracks/${TRACK_ID}`,
    );
    expect(screen.getAllByText('flac / flac').length).toBeGreaterThan(0);
    const preview = screen.getByLabelText('Audio preview: Down Under');
    expect(preview.tagName).toBe('AUDIO');
    expect(preview).toHaveAttribute(
      'src',
      `/song/stream/${TRACK_ID}/dddddddd-dddd-4ddd-8ddd-dddddddddddd/signed-token`,
    );
    expect(preview).toHaveAttribute('preload', 'none');

    // Artists tab: billed artist link with role.
    fireEvent.click(screen.getByRole('tab', { name: 'Artists' }));
    expect(screen.getByRole('link', { name: 'The Nomads' })).toHaveAttribute(
      'href',
      `/catalog/artists/${ARTIST_ID}`,
    );
    expect(screen.getByText('main')).toBeInTheDocument();

    // Files tab: media asset row.
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }));
    expect(screen.getByText('Into the Night/01 Down Under.flac')).toBeInTheDocument();
    expect(screen.getByText('audio/flac')).toBeInTheDocument();

    // Provenance tab: work item + manifest hashes + imported timestamp.
    fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
    expect(screen.getByText(WORK_ITEM_ID)).toBeInTheDocument();
    expect(screen.getByText('abc123')).toBeInTheDocument();
    expect(screen.getByText('ghi789')).toBeInTheDocument();
    expect(screen.getByText('1.0.0')).toBeInTheDocument();

    // History tab.
    fireEvent.click(screen.getByRole('tab', { name: 'History' }));
    expect(screen.getByText('release.cataloged')).toBeInTheDocument();
  });

  it('renders no preview affordance when a track carries a null streamUrl', async () => {
    const detail = makeDetail();
    detail.tracks.items = detail.tracks.items.map((track) => ({ ...track, streamUrl: null }));
    renderScreen(detail);
    await screen.findByRole('heading', { name: 'Into the Night' });
    fireEvent.click(screen.getByRole('tab', { name: 'Tracks' }));
    expect(screen.getByRole('link', { name: 'Down Under' })).toBeInTheDocument();
    expect(screen.queryByLabelText('Audio preview: Down Under')).not.toBeInTheDocument();
  });

  it('explains a withheld policy review instead of presenting it as Deprecated', async () => {
    renderScreen(
      makeDetail({
        status: 'deprecated',
        workflowState: 'needs_review',
        policyDisposition: 'review',
        policyEvidenceSummary: 'Release classification is Unknown',
      }),
    );
    expect(await screen.findByRole('heading', { name: 'Into the Night' })).toBeInTheDocument();
    expect(screen.getAllByText('Needs review').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Release classification is Unknown').length).toBeGreaterThan(0);
    expect(screen.queryByText('Deprecated')).not.toBeInTheDocument();
  });

  it('shows an honest empty Classification tab when no classification exists', async () => {
    renderScreen(makeDetail({ classification: null }));
    await screen.findByRole('heading', { name: 'Into the Night' });
    fireEvent.click(screen.getByRole('tab', { name: 'Classification' }));
    expect(screen.getByText('This Release has not been classified yet.')).toBeInTheDocument();
  });

  it('renders classification traits when the Phase 5 policy has run', async () => {
    renderScreen(
      makeDetail({
        classification: {
          traits: ['greatest_hits'],
          confidence: 9200,
          classifiedAt: '2026-08-03T10:00:00Z',
          classifierVersion: '1.0.0',
        },
      }),
    );
    await screen.findByRole('heading', { name: 'Into the Night' });
    fireEvent.click(screen.getByRole('tab', { name: 'Classification' }));
    expect(screen.getByText('greatest_hits')).toBeInTheDocument();
    expect(screen.getByText('92%')).toBeInTheDocument();
  });

  it('shows an honest empty Provenance tab for pre-manifest Releases', async () => {
    renderScreen(makeDetail({ provenance: null }));
    await screen.findByRole('heading', { name: 'Into the Night' });
    fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
    expect(
      screen.getByText('No import provenance is recorded for this Release.'),
    ).toBeInTheDocument();
  });
});

import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { TrackDetail } from '../../api/types';
import { TrackDetailScreen } from './TrackDetailScreen';

vi.mock('../../api/admin', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../api/admin')>();
  return {
    ...original,
    getCatalogTrack: vi.fn(),
  };
});

const { getCatalogTrack } = await import('../../api/admin');
const mockGetTrack = vi.mocked(getCatalogTrack);

const TRACK_ID = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc';
const RELEASE_ID = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const ARTIST_ID = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';

function makeDetail(overrides: Partial<TrackDetail> = {}): TrackDetail {
  return {
    id: TRACK_ID,
    title: 'Down Under',
    sortTitle: 'Down Under',
    displayArtist: 'The Nomads',
    ordinal: 3,
    discNumber: 1,
    trackNumber: 3,
    durationMs: 222_000,
    sourcePath: 'Into the Night/03 Down Under.flac',
    audioIdentityId: '22222222-2222-4222-8222-222222222222',
    revision: 1,
    imageId: `song_${TRACK_ID}`,
    streamUrl: `/song/stream/${TRACK_ID}/dddddddd-dddd-4ddd-8ddd-dddddddddddd/signed-token`,
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
    contributors: {
      items: [
        {
          artistId: ARTIST_ID,
          artistName: 'The Nomads',
          role: 'producer',
          roleDetail: null,
          isPerformance: false,
          evidenceSource: 'tags',
          displayValue: null,
          presentationOrder: 0,
          trackId: TRACK_ID,
          trackTitle: 'Down Under',
        },
      ],
      total: 1,
    },
    mediaAssets: [
      {
        purpose: 'audio',
        id: '99999999-9999-4999-8999-999999999999',
        canonicalRelativePath: 'Into the Night/03 Down Under.flac',
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
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('TrackDetailScreen', () => {
  it('renders facts, the release linkage, artists, contributors, and media assets', async () => {
    mockGetTrack.mockResolvedValue(makeDetail());
    render(
      <MemoryRouter initialEntries={[`/catalog/tracks/${TRACK_ID}`]}>
        <Routes>
          <Route path="/catalog/tracks/:id" element={<TrackDetailScreen />} />
        </Routes>
      </MemoryRouter>,
    );
    expect(await screen.findByRole('heading', { name: 'Down Under' })).toBeInTheDocument();
    expect(mockGetTrack).toHaveBeenCalledWith(TRACK_ID);

    // Facts: ordinal renders as disc.track, release links to the catalog route.
    expect(screen.getByText('1.3')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Into the Night' })).toHaveAttribute(
      'href',
      `/catalog/releases/${RELEASE_ID}`,
    );
    expect(
      screen.getAllByText('Into the Night/03 Down Under.flac').length,
    ).toBeGreaterThan(0);

    // Billed artist and contributor link to the Artist detail route.
    expect(screen.getAllByRole('link', { name: 'The Nomads' })[0]).toHaveAttribute(
      'href',
      `/catalog/artists/${ARTIST_ID}`,
    );
    expect(screen.getByText('producer')).toBeInTheDocument();

    // Media asset with purpose.
    expect(screen.getByText('audio')).toBeInTheDocument();
    expect(screen.getByText('flac / flac')).toBeInTheDocument();

    // Signed audio preview: labeled transport, never autoplays.
    const preview = screen.getByLabelText('Audio preview: Down Under');
    expect(preview.tagName).toBe('AUDIO');
    expect(preview).toHaveAttribute(
      'src',
      `/song/stream/${TRACK_ID}/dddddddd-dddd-4ddd-8ddd-dddddddddddd/signed-token`,
    );
    expect(preview).toHaveAttribute('preload', 'none');
  });

  it('renders no preview affordance when streamUrl is null', async () => {
    mockGetTrack.mockResolvedValue(makeDetail({ streamUrl: null }));
    render(
      <MemoryRouter initialEntries={[`/catalog/tracks/${TRACK_ID}`]}>
        <Routes>
          <Route path="/catalog/tracks/:id" element={<TrackDetailScreen />} />
        </Routes>
      </MemoryRouter>,
    );
    await screen.findByRole('heading', { name: 'Down Under' });
    expect(screen.queryByLabelText('Audio preview: Down Under')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Audio preview')).not.toBeInTheDocument();
  });

  it('renders honest empty states when a track has no contributors or files', async () => {
    mockGetTrack.mockResolvedValue(
      makeDetail({ contributors: { items: [], total: 0 }, mediaAssets: [] }),
    );
    render(
      <MemoryRouter initialEntries={[`/catalog/tracks/${TRACK_ID}`]}>
        <Routes>
          <Route path="/catalog/tracks/:id" element={<TrackDetailScreen />} />
        </Routes>
      </MemoryRouter>,
    );
    await screen.findByRole('heading', { name: 'Down Under' });
    expect(screen.getByText('No contributors are recorded for this Track.')).toBeInTheDocument();
    expect(screen.getByText('No media files are linked to this Track.')).toBeInTheDocument();
  });
});

import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { ArtistDetail } from '../../api/types';
import { ArtistDetailScreen } from './ArtistDetailScreen';

vi.mock('../../api/admin', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../api/admin')>();
  return {
    ...original,
    getCatalogArtist: vi.fn(),
  };
});

const { getCatalogArtist } = await import('../../api/admin');
const mockGetArtist = vi.mocked(getCatalogArtist);

const ARTIST_ID = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const TARGET_ID = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd';
const RELEASE_ID = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const TRACK_ID = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc';
const MEMBER_ID = 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee';

function makeDetail(overrides: Partial<ArtistDetail> = {}): ArtistDetail {
  return {
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
    externalIds: {
      items: [
        {
          provider: 'musicbrainz',
          externalId: 'mb-123',
          confidence: 9000,
          evidenceSource: 'lookup',
          verifiedAt: '2026-08-02T10:00:00Z',
          locked: false,
        },
      ],
      total: 1,
    },
    releases: {
      items: [
        {
          releaseId: RELEASE_ID,
          title: 'Into the Night',
          role: 'main',
          presentationOrder: 0,
          displayValue: 'The Nomads',
          releaseDate: '1986-05-12',
          trackCount: 10,
        },
      ],
      total: 1,
    },
    contributions: {
      items: [
        {
          role: 'performer',
          roleDetail: 'vocals',
          isPerformance: true,
          evidenceSource: 'tags',
          releaseId: RELEASE_ID,
          releaseTitle: 'Into the Night',
          trackId: TRACK_ID,
          trackTitle: 'Down Under',
        },
        {
          role: 'producer',
          roleDetail: null,
          isPerformance: false,
          evidenceSource: 'tags',
          releaseId: RELEASE_ID,
          releaseTitle: 'Into the Night',
          trackId: null,
          trackTitle: null,
        },
      ],
      total: 2,
    },
    relationships: {
      items: [
        {
          sourceArtistId: MEMBER_ID,
          sourceName: 'Colin Example',
          targetArtistId: ARTIST_ID,
          targetName: 'The Nomads',
          relationshipType: 'member_of',
          role: 'vocals',
          startDate: '1978-01-01',
          endDate: '2012-12-31',
          confidence: 9800,
          evidenceSource: 'tags',
          locked: false,
        },
      ],
      total: 1,
    },
    memberships: {
      items: [
        {
          memberArtistId: MEMBER_ID,
          memberName: 'Colin Example',
          groupArtistId: ARTIST_ID,
          groupName: 'The Nomads',
          role: 'vocals',
          startDate: '1978-01-01',
          endDate: null,
          confidence: 9900,
          evidenceSource: 'tags',
          locked: false,
        },
      ],
      total: 1,
    },
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
    ...overrides,
  };
}

function renderScreen(detail: ArtistDetail) {
  mockGetArtist.mockResolvedValue(detail);
  return render(
    <MemoryRouter initialEntries={[`/catalog/artists/${ARTIST_ID}`]}>
      <Routes>
        <Route path="/catalog/artists/:id" element={<ArtistDetailScreen />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('ArtistDetailScreen', () => {
  it('renders header facts, stat cards, and section tabs from the detail payload', async () => {
    renderScreen(makeDetail());
    expect(await screen.findByRole('heading', { name: 'The Nomads' })).toBeInTheDocument();
    expect(mockGetArtist).toHaveBeenCalledWith(ARTIST_ID, expect.any(AbortSignal));
    expect(screen.getAllByText('Australian rock band').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Group').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Verified').length).toBeGreaterThan(0);

    // Stat cards (releaseCount=12, billedTrackCount=128, performanceTrackCount=391, aliases=1).
    expect(screen.getByText('12')).toBeInTheDocument();
    expect(screen.getByText('128')).toBeInTheDocument();
    expect(screen.getByText('391')).toBeInTheDocument();

    // External id chip in the header.
    expect(screen.getAllByText('musicbrainz').length).toBeGreaterThan(0);

    // Overview tab: typed aliases.
    fireEvent.click(screen.getByRole('tab', { name: 'Overview' }));
    expect(screen.getByText('Nomads')).toBeInTheDocument();
    expect(screen.getByText('Search alias')).toBeInTheDocument();

    // Releases tab links into the Release detail route.
    fireEvent.click(screen.getByRole('tab', { name: 'Releases' }));
    expect(screen.getByRole('link', { name: 'Into the Night' })).toHaveAttribute(
      'href',
      `/catalog/releases/${RELEASE_ID}`,
    );

    // Appearances vs Contributions split on the isPerformance flag.
    fireEvent.click(screen.getByRole('tab', { name: 'Appearances' }));
    expect(screen.getByRole('link', { name: 'Down Under' })).toHaveAttribute(
      'href',
      `/catalog/tracks/${TRACK_ID}`,
    );
    fireEvent.click(screen.getByRole('tab', { name: 'Contributions' }));
    expect(screen.getByText('producer')).toBeInTheDocument();
    expect(screen.getByText('Release-level credit')).toBeInTheDocument();

    // Relationships: the "other" side is the member (source), not this artist.
    fireEvent.click(screen.getByRole('tab', { name: 'Relationships' }));
    expect(screen.getAllByRole('link', { name: 'Colin Example' })[0]).toHaveAttribute(
      'href',
      `/catalog/artists/${MEMBER_ID}`,
    );
    expect(screen.getByText('Member of this group')).toBeInTheDocument();

    // History tab: audit timeline.
    fireEvent.click(screen.getByRole('tab', { name: 'History' }));
    expect(screen.getByText('artist.created')).toBeInTheDocument();
  });

  it('falls back to EntityAvatar initials when the artwork image fails to load', async () => {
    renderScreen(makeDetail());
    await screen.findByRole('heading', { name: 'The Nomads' });
    // The header artwork is the small image (the Images tab uses medium).
    const image = document.querySelector(`img[src="/images/artist_${ARTIST_ID}/small"]`);
    expect(image).not.toBeNull();
    fireEvent.error(image as HTMLElement);
    expect(
      document.querySelector(`img[src="/images/artist_${ARTIST_ID}/small"]`),
    ).toBeNull();
  });

  it('handles a merged artist: identity tab links to the merge target', async () => {
    renderScreen(
      makeDetail({
        status: 'merged',
        mergedIntoArtistId: TARGET_ID,
        externalIds: { items: [], total: 0 },
      }),
    );
    await screen.findByRole('heading', { name: 'The Nomads' });
    fireEvent.click(screen.getByRole('tab', { name: 'Identity' }));
    expect(screen.getByText('This Artist was merged into')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: TARGET_ID })).toHaveAttribute(
      'href',
      `/catalog/artists/${TARGET_ID}`,
    );
    expect(screen.getByText('No external identities are recorded for this Artist.')).toBeInTheDocument();
  });
});

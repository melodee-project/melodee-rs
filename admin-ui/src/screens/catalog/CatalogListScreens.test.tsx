import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type {
  ArtistMergePreview,
  CatalogArtistListItem,
  CatalogReleaseListItem,
  CatalogTrackListItem,
  Page,
} from '../../api/types';
import { ArtistsScreen } from './ArtistsScreen';
import { ReleasesScreen } from './ReleasesScreen';
import { TracksScreen } from './TracksScreen';

vi.mock('../../api/admin', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../api/admin')>();
  return {
    ...original,
    listCatalogArtists: vi.fn(),
    listCatalogReleases: vi.fn(),
    listCatalogTracks: vi.fn(),
    previewArtistMerge: vi.fn(),
    batchMergeArtists: vi.fn(),
  };
});

const {
  batchMergeArtists,
  listCatalogArtists,
  listCatalogReleases,
  listCatalogTracks,
  previewArtistMerge,
} = await import('../../api/admin');
const mockBatchMergeArtists = vi.mocked(batchMergeArtists);
const mockListArtists = vi.mocked(listCatalogArtists);
const mockListReleases = vi.mocked(listCatalogReleases);
const mockListTracks = vi.mocked(listCatalogTracks);
const mockPreviewArtistMerge = vi.mocked(previewArtistMerge);

const ARTIST: CatalogArtistListItem = {
  id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  name: 'The Nomads',
  sortName: 'Nomads, The',
  artistType: 'group',
  disambiguation: 'Australian rock band',
  status: 'verified',
  releaseCount: 12,
  trackCount: 128,
  imageId: 'artist_aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  createdAt: '2026-08-01T10:00:00Z',
  updatedAt: '2026-08-14T10:00:00Z',
};

const DUPLICATE_ARTIST: CatalogArtistListItem = {
  ...ARTIST,
  id: 'dddddddd-dddd-4ddd-8ddd-dddddddddddd',
  disambiguation: null,
  releaseCount: 2,
  trackCount: 18,
  imageId: 'artist_dddddddd-dddd-4ddd-8ddd-dddddddddddd',
};

const OTHER_ARTIST: CatalogArtistListItem = {
  ...ARTIST,
  id: 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee',
  name: 'Unrelated Artist',
  sortName: 'Unrelated Artist',
  disambiguation: null,
  releaseCount: 1,
  trackCount: 5,
  imageId: 'artist_eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee',
};

const MERGE_PREVIEW: ArtistMergePreview = {
  target: {
    id: ARTIST.id,
    name: ARTIST.name,
    sortName: ARTIST.sortName,
    artistType: ARTIST.artistType,
    disambiguation: ARTIST.disambiguation,
    beginDate: null,
    endDate: null,
    status: ARTIST.status,
  },
  sources: [
    {
      id: DUPLICATE_ARTIST.id,
      name: DUPLICATE_ARTIST.name,
      sortName: DUPLICATE_ARTIST.sortName,
      artistType: DUPLICATE_ARTIST.artistType,
      disambiguation: DUPLICATE_ARTIST.disambiguation,
      beginDate: null,
      endDate: null,
      status: DUPLICATE_ARTIST.status,
    },
  ],
  impact: {
    releases: 2,
    tracks: 18,
    releaseCredits: 2,
    trackCredits: 18,
    contributions: 3,
    aliases: 1,
    externalIdentities: 1,
    relationships: 0,
    memberships: 0,
    observations: 4,
    userStates: 2,
    userStateConflicts: 1,
    chartLinks: 0,
    operatorImages: 1,
    providerImages: 1,
    providerImageConflicts: 1,
    providerCandidates: 1,
    providerTasks: 1,
  },
  metadataConflicts: ['disambiguation'],
};

const RELEASE: CatalogReleaseListItem = {
  id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  title: 'Into the Night',
  displayArtist: 'The Nomads',
  releaseDate: '1986-05-12',
  releaseDateText: null,
  status: 'active',
  workflowState: 'indexed',
  policyDisposition: 'accept',
  policyEvidenceSummary: 'No configured policy rule matched; accepting Release',
  trackCount: 10,
  durationMs: 2_280_000,
  imageId: 'album_bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  createdAt: '2026-08-01T10:00:00Z',
  updatedAt: '2026-08-14T10:00:00Z',
};

const TRACK: CatalogTrackListItem = {
  id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc',
  title: 'Down Under',
  displayArtist: 'The Nomads',
  ordinal: 3,
  discNumber: 1,
  trackNumber: 3,
  durationMs: 222_000,
  releaseId: RELEASE.id,
  releaseTitle: RELEASE.title,
  imageId: 'song_cccccccc-cccc-4ccc-8ccc-cccccccccccc',
  createdAt: '2026-08-01T10:00:00Z',
};

function page<T>(items: T[], total = items.length): Page<T> {
  return { items, total, limit: 50, offset: 0 };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockListArtists.mockResolvedValue(page([ARTIST], 137));
  mockListReleases.mockResolvedValue(page([RELEASE], 137));
  mockListTracks.mockResolvedValue(page([TRACK], 137));
  mockPreviewArtistMerge.mockResolvedValue(MERGE_PREVIEW);
  mockBatchMergeArtists.mockResolvedValue({
    mergeIds: ['ffffffff-ffff-4fff-8fff-ffffffffffff'],
    targetArtistId: ARTIST.id,
    sourceArtistIds: [DUPLICATE_ARTIST.id],
  });
});

describe('ArtistsScreen', () => {
  it('aborts the HTTP request when the screen unmounts', async () => {
    let requestSignal: AbortSignal | undefined;
    let completeRequest: ((result: Page<CatalogArtistListItem>) => void) | undefined;
    mockListArtists.mockImplementation(
      (_params, signal) =>
        new Promise((resolve) => {
          requestSignal = signal;
          completeRequest = resolve;
        }),
    );
    const view = render(
      <MemoryRouter initialEntries={['/catalog/artists']}>
        <ArtistsScreen />
      </MemoryRouter>,
    );
    await waitFor(() => expect(requestSignal).toBeDefined());
    view.unmount();
    expect(requestSignal?.aborted).toBe(true);
    completeRequest?.(page([]));
  });

  it('renders the server envelope and maps query/sort/page URL state to request params', async () => {
    render(
      <MemoryRouter initialEntries={['/catalog/artists?sort=name:desc&page=2&pageSize=25']}>
        <ArtistsScreen />
      </MemoryRouter>,
    );
    expect(await screen.findByRole('link', { name: 'The Nomads' })).toHaveAttribute(
      'href',
      `/catalog/artists/${ARTIST.id}`,
    );
    expect(mockListArtists).toHaveBeenCalledWith(
      {
        query: undefined,
        limit: 25,
        offset: 25,
        sort: 'name:desc',
      },
      expect.any(AbortSignal),
    );
    // Total-count pagination: 137 server-side rows over one rendered row.
    expect(screen.getByText(/137/)).toBeInTheDocument();
    expect(screen.getByText('Group')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'The Nomads' })).toBeInTheDocument();
  });

  it('binds the search box to the query filter and header clicks to whitelisted sort', async () => {
    render(
      <MemoryRouter initialEntries={['/catalog/artists']}>
        <ArtistsScreen />
      </MemoryRouter>,
    );
    await screen.findByRole('link', { name: 'The Nomads' });
    expect(mockListArtists).toHaveBeenLastCalledWith(
      {
        query: undefined,
        limit: 50,
        offset: 0,
        sort: undefined,
      },
      expect.any(AbortSignal),
    );

    fireEvent.change(screen.getByRole('searchbox', { name: 'Search artists' }), {
      target: { value: 'nomads' },
    });
    await waitFor(
      () => {
        expect(mockListArtists).toHaveBeenLastCalledWith(
          expect.objectContaining({ query: 'nomads', offset: 0 }),
          expect.any(AbortSignal),
        );
      },
      { timeout: 2000 },
    );

    fireEvent.click(screen.getByRole('button', { name: 'Sort by Name' }));
    await waitFor(() => {
      expect(mockListArtists).toHaveBeenLastCalledWith(
        expect.objectContaining({ sort: 'name:asc' }),
        expect.any(AbortSignal),
      );
    });
  });

  it('previews and confirms a page-scoped multi-Artist merge', async () => {
    mockListArtists.mockResolvedValue(
      page([ARTIST, DUPLICATE_ARTIST, OTHER_ARTIST], 3),
    );
    render(
      <MemoryRouter initialEntries={['/catalog/artists']}>
        <ArtistsScreen />
      </MemoryRouter>,
    );
    await screen.findByRole('link', { name: 'Unrelated Artist' });

    const mergeButton = screen.getByRole('button', { name: 'Merge 0 selected Artists' });
    expect(mergeButton).toBeDisabled();
    const rowSelections = screen.getAllByRole('checkbox', { name: 'Select row' });
    fireEvent.click(rowSelections[0]);
    fireEvent.click(rowSelections[1]);
    expect(screen.getByRole('button', { name: 'Merge 2 selected Artists' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Merge 2 selected Artists' }));

    expect(await screen.findByRole('heading', { name: 'Merge Artists' })).toBeInTheDocument();
    await waitFor(() => {
      expect(mockPreviewArtistMerge).toHaveBeenCalledWith({
        sourceArtistIds: [DUPLICATE_ARTIST.id],
        targetArtistId: ARTIST.id,
      });
    });
    expect(await screen.findByText('Merge impact')).toBeInTheDocument();
    const confirmButton = screen.getByRole('button', { name: 'Merge Artists' });
    expect(confirmButton).toBeDisabled();

    fireEvent.change(screen.getByLabelText('Reason'), {
      target: { value: 'Duplicate import identity' },
    });
    fireEvent.change(screen.getByLabelText('Type MERGE to confirm.'), {
      target: { value: 'MERGE' },
    });
    expect(confirmButton).toBeEnabled();
    fireEvent.click(confirmButton);

    await waitFor(() => {
      expect(mockBatchMergeArtists).toHaveBeenCalledWith({
        sourceArtistIds: [DUPLICATE_ARTIST.id],
        targetArtistId: ARTIST.id,
        reason: 'Duplicate import identity',
      });
    });
    expect(
      await screen.findByText('The selected Artists were merged successfully.'),
    ).toBeInTheDocument();
    expect(mockListArtists).toHaveBeenCalledTimes(2);
  });
});

describe('ReleasesScreen', () => {
  it('renders the envelope and maps URL state to request params', async () => {
    render(
      <MemoryRouter
        initialEntries={['/catalog/releases?query=night&sort=releaseDate:asc&page=3&pageSize=25']}
      >
        <ReleasesScreen />
      </MemoryRouter>,
    );
    expect(await screen.findByRole('link', { name: 'Into the Night' })).toHaveAttribute(
      'href',
      `/catalog/releases/${RELEASE.id}`,
    );
    expect(mockListReleases).toHaveBeenCalledWith(
      {
        query: 'night',
        limit: 25,
        offset: 50,
        sort: 'releaseDate:asc',
      },
      expect.any(AbortSignal),
    );
    expect(screen.getByText('The Nomads')).toBeInTheDocument();
  });

  it('honors the sort whitelist by ignoring unknown sort keys', async () => {
    render(
      <MemoryRouter initialEntries={['/catalog/releases?sort=haxx:asc']}>
        <ReleasesScreen />
      </MemoryRouter>,
    );
    await screen.findByRole('link', { name: 'Into the Night' });
    expect(mockListReleases).toHaveBeenCalledWith(
      expect.objectContaining({ sort: undefined }),
      expect.any(AbortSignal),
    );
  });

  it('labels a withheld policy review by its actionable state, not Deprecated', async () => {
    mockListReleases.mockResolvedValue(
      page([
        {
          ...RELEASE,
          status: 'deprecated',
          workflowState: 'needs_review',
          policyDisposition: 'review',
          policyEvidenceSummary: 'Release classification is Unknown',
        },
      ]),
    );
    render(
      <MemoryRouter initialEntries={['/catalog/releases']}>
        <ReleasesScreen />
      </MemoryRouter>,
    );
    expect(await screen.findByText('Needs review')).toBeInTheDocument();
    expect(screen.queryByText('Deprecated')).not.toBeInTheDocument();
    expect(screen.getByTitle('Release classification is Unknown')).toBeInTheDocument();
  });
});

describe('TracksScreen', () => {
  it('renders the envelope with release linkage and maps URL state to request params', async () => {
    render(
      <MemoryRouter initialEntries={['/catalog/tracks?sort=durationMs:desc&page=2&pageSize=25']}>
        <TracksScreen />
      </MemoryRouter>,
    );
    expect(await screen.findByRole('link', { name: 'Down Under' })).toHaveAttribute(
      'href',
      `/catalog/tracks/${TRACK.id}`,
    );
    expect(screen.getByRole('link', { name: 'Into the Night' })).toHaveAttribute(
      'href',
      `/catalog/releases/${RELEASE.id}`,
    );
    expect(mockListTracks).toHaveBeenCalledWith(
      {
        query: undefined,
        limit: 25,
        offset: 25,
        sort: 'durationMs:desc',
      },
      expect.any(AbortSignal),
    );
  });

  it('shows the localized empty state when the server page is empty', async () => {
    mockListTracks.mockResolvedValue(page([], 0));
    render(
      <MemoryRouter initialEntries={['/catalog/tracks?query=zzz']}>
        <TracksScreen />
      </MemoryRouter>,
    );
    expect(await screen.findByText('No tracks')).toBeInTheDocument();
  });
});

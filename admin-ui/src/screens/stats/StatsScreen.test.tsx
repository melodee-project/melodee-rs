import { render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Stats } from '../../api/types';
import { StatsScreen } from './StatsScreen';

vi.mock('../../api/admin', () => ({
  getStats: vi.fn(),
}));

const { getStats } = await import('../../api/admin');
const mockGetStats = vi.mocked(getStats);

function makeStats(overrides: Partial<Stats> = {}): Stats {
  return {
    generatedAt: '2026-08-14T12:00:00Z',
    processing: { waitingForPublication: 84, awaitingReview: 7, publishedLastHour: 43, publishedLastDay: 258, importedLastHour: 40, importedLastDay: 256 },
    library: {
      artists: 417,
      releases: 128,
      tracks: 1_632,
      mediaAssets: 1_632,
      totalBytes: 18_432_000_000,
      totalDurationMs: 468_000_000,
      averageTrackDurationMs: 286_765,
      losslessTracks: 96,
      formats: [
        { container: 'mp3', codec: 'mp3', tracks: 1_536, bytes: 17_400_000_000, lossless: false },
        { container: 'flac', codec: 'flac', tracks: 96, bytes: 1_032_000_000, lossless: true },
      ],
      decades: [
        { decade: 2020, releases: 98 },
        { decade: 1990, releases: 9 },
      ],
      largestReleases: [
        {
          id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc',
          title: 'Business as Usual',
          artist: 'Men at Work',
          tracks: 10,
          bytes: 1_310_000_000,
        },
      ],
    },
    listening: {
      totalPlays: 12_345,
      totalPlaytimeSeconds: 2_592_000,
      uniqueTracks: 1_204,
      uniqueListeners: 6,
      playsPerDay: [
        { day: '2026-08-03', plays: 612, seconds: 128_520 },
        { day: '2026-08-04', plays: 704, seconds: 147_840 },
        { day: '2026-08-05', plays: 655, seconds: 137_550 },
      ],
      topArtists: [{ name: 'Men at Work', plays: 1_204, seconds: 252_840 }],
      topTracks: [
        { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', title: 'Down Under', artist: 'Men at Work', plays: 312 },
      ],
    },
    database: {
      totalBytes: 214_748_364,
      tables: [{ name: 'tracks', bytes: 3_145_728 }],
      workItems: 321,
      playEvents: 12_345,
      pendingProjection: 0,
      assetFileBytes: 2097152,
      assetFileCount: 3,
      poolCapacity: 100,
      poolInUse: 26,
    },
    ...overrides,
  };
}

function renderScreen() {
  return render(
    <MemoryRouter>
      <StatsScreen />
    </MemoryRouter>,
  );
}

describe('StatsScreen', () => {
  beforeEach(() => {
    mockGetStats.mockReset();
  });

  it('renders the library, listening, and database panels from the API payload', async () => {
    mockGetStats.mockResolvedValue(makeStats());
    renderScreen();

    expect(await screen.findByRole('heading', { name: 'Media library' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Playback activity' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Database' })).toBeInTheDocument();
    expect(screen.getByText('3 unique files on persistent storage')).toBeInTheDocument();

    expect(screen.getByRole('heading', { name: 'Release processing' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Waiting for publication.*84/ })).toHaveAttribute('href', '/processing/queue?state=staged');
    expect(screen.getByText('258 in the last 24 hours')).toBeInTheDocument();

    // Headline numbers come straight from the server aggregates (locale-grouped).
    expect(screen.getAllByText('1,632').length).toBeGreaterThan(0);
    expect(screen.getAllByText('12,345').length).toBeGreaterThan(0);
    // Lossless share is derived from counts, not hard-coded: 96 of 1,632 tracks.
    expect(screen.getByText('6%')).toBeInTheDocument();
  });

  it('exposes every chart series as an accessible table instead of relying on color', async () => {
    mockGetStats.mockResolvedValue(makeStats());
    renderScreen();

    const plays = await screen.findByRole('table', { name: 'Plays per day' });
    expect(within(plays).getByText('2026-08-04')).toBeInTheDocument();
    expect(within(plays).getByText('704')).toBeInTheDocument();

    const decades = screen.getByRole('table', { name: 'Releases by decade' });
    expect(within(decades).getByText('2020s')).toBeInTheDocument();
    expect(within(decades).getByText('98')).toBeInTheDocument();
  });

  it('shows honest empty states instead of invented numbers', async () => {
    mockGetStats.mockResolvedValue(
      makeStats({
        library: {
          ...makeStats().library,
          tracks: 0,
          releases: 0,
          totalBytes: 0,
          totalDurationMs: 0,
          losslessTracks: 0,
          formats: [],
          decades: [],
          largestReleases: [],
        },
        listening: {
          ...makeStats().listening,
          totalPlays: 0,
          playsPerDay: [],
          topArtists: [],
          topTracks: [],
        },
      }),
    );
    renderScreen();

    expect(await screen.findByText('No published media yet.')).toBeInTheDocument();
    expect(screen.getByText('No release years recorded yet.')).toBeInTheDocument();
    expect(screen.getAllByText('No plays recorded yet.').length).toBeGreaterThan(0);
  });

  it('reports a load failure with a retry action', async () => {
    mockGetStats.mockRejectedValue(new Error('stats unavailable'));
    renderScreen();

    expect(
      await screen.findByText('Statistics could not be loaded'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
  });
});

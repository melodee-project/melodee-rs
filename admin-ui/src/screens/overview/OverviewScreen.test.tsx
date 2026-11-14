import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Dashboard, Page, WorkItem } from '../../api/types';
import { stageProgress } from '../processing/workItem';
import { OverviewScreen } from './OverviewScreen';
import { countStates, INBOUND_STATES, layoutActivitySeries, sortedBuckets } from './overview';

vi.mock('../../api/admin', () => ({
  getDashboard: vi.fn(),
  getOnboardingStatus: vi.fn(),
  listWorkItems: vi.fn(),
}));

const { getDashboard, getOnboardingStatus, listWorkItems } = await import('../../api/admin');
const mockGetDashboard = vi.mocked(getDashboard);
const mockGetOnboardingStatus = vi.mocked(getOnboardingStatus);
const mockListWorkItems = vi.mocked(listWorkItems);

/** Two queue rows: one bound to a catalog Release, one not yet imported. */
function makeQueueItem(overrides: Partial<WorkItem> = {}): WorkItem {
  return {
    id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
    apiKey: '33333333-3333-4333-8333-333333333333',
    sourceLibraryId: null,
    targetLibraryId: null,
    sourcePath: '/library/incoming/Business as Usual',
    currentPath: '/library/incoming/Business as Usual',
    targetPath: null,
    sourceFingerprint: null,
    manifestHash: null,
    state: 'publishing',
    stateReason: null,
    attemptCount: 1,
    nextAttemptAt: null,
    leaseOwner: null,
    leaseAcquiredAt: null,
    leaseExpiresAt: null,
    lastErrorCode: null,
    lastErrorMessage: null,
    correlationId: '44444444-4444-4444-8444-444444444444',
    discoveredAt: '2026-08-14T10:00:00Z',
    firstStartedAt: null,
    lastAttemptAt: null,
    updatedAt: '2026-08-14T10:05:00Z',
    createdAt: '2026-08-14T10:00:00Z',
    completedAt: null,
    releaseId: '55555555-5555-4555-8555-555555555555',
    releaseTitle: 'Business as Usual',
    artistName: 'Men at Work',
    mediaBytes: 33600000,
    ...overrides,
  };
}

function makeQueuePage(items: WorkItem[]): Page<WorkItem> {
  return { items, total: items.length, limit: 5, offset: 0 };
}

/** Default: onboarding already completed — no banner. */
const ONBOARDING_COMPLETE = {
  completed: true,
  bypassed: false,
  canComplete: true,
  completedBy: 'admin',
  checks: [],
};

function makeDashboard(overrides: Partial<Dashboard> = {}): Dashboard {
  return {
    generatedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
    catalog: {
      artists: 35214,
      releases: 50482,
      tracks: 560291,
      mediaAssets: 600000,
      pendingSearchOutbox: 12,
    },
    workItems: {
      countsByState: [
        { state: 'discovered', count: 3 },
        { state: 'ready', count: 5 },
        { state: 'needs_review', count: 2 },
        { state: 'failed', count: 1 },
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
    health: {
      status: 'ok',
      postgresql: { status: 'ok', detail: 'postgresql answered a bounded probe' },
      postgresqlPool: {
        maxConnections: 32,
        openConnections: 2,
        idleConnections: 1,
        inUseConnections: 1,
        utilizationPercent: 3.125,
      },
      tantivy: { status: 'ok', detail: 'active search generation validated' },
      storage: { status: 'ok', detail: 'storage paths writable' },
      ffmpeg: { status: 'ok', detail: 'ffmpeg 8.1' },
      podcastStorage: { status: 'disabled', detail: 'podcast worker disabled' },
      disk: { status: 'ok', detail: 'tightest of 2 root(s): storage has 512000 MiB free (50%)' },
      workers: { status: 'ok', detail: 'no stale leases; oldest ready item 0s; 0 ready / 0 claimed' },
      apiSurfaces: [
        { surface: 'native', status: 'serving' },
        { surface: 'opensubsonic', status: 'serving' },
        { surface: 'jellyfin', status: 'serving' },
        { surface: 'admin', status: 'serving' },
      ],
      resourceBounds: [],
    },
    deltas: { inboundToday: 12, newForReview24h: 5 },
    activity: {
      completedPerHour: [
        { hour: '2026-08-14T11:00:00Z', count: 9 },
        { hour: '2026-08-14T10:00:00Z', count: 5 },
      ],
      indexedPerHour: [
        { hour: '2026-08-14T11:00:00Z', count: 8 },
        { hour: '2026-08-14T10:00:00Z', count: 4 },
      ],
    },
    ...overrides,
  };
}

function renderScreen() {
  return render(
    <MemoryRouter initialEntries={['/']}>
      <OverviewScreen />
    </MemoryRouter>,
  );
}

describe('overview helpers', () => {
  it('groups work-item counts by state set', () => {
    const counts = [
      { state: 'discovered' as const, count: 3 },
      { state: 'settling' as const, count: 2 },
      { state: 'ready' as const, count: 7 },
    ];
    expect(countStates(counts, INBOUND_STATES)).toBe(5);
    expect(countStates(counts, ['ready'])).toBe(7);
  });

  it('sorts activity buckets ascending by hour', () => {
    const sorted = sortedBuckets([
      { hour: '2026-08-14T11:00:00Z', count: 9 },
      { hour: '2026-08-14T10:00:00Z', count: 5 },
    ]);
    expect(sorted[0]?.hour).toBe('2026-08-14T10:00:00Z');
  });

  it('lays out two series on one shared scale over the union of hours', () => {
    const layout = layoutActivitySeries(
      [
        { hour: '2026-08-14T10:00:00Z', count: 5 },
        { hour: '2026-08-14T11:00:00Z', count: 10 },
      ],
      [{ hour: '2026-08-14T11:00:00Z', count: 4 }],
      720,
      100,
    );
    expect(layout.hours).toEqual(['2026-08-14T10:00:00Z', '2026-08-14T11:00:00Z']);
    expect(layout.max).toBe(10);
    // Releases series: two points, the peak sitting on the baseline-to-top scale.
    expect(layout.releases.points).toHaveLength(2);
    expect(layout.releases.path.startsWith('M ')).toBe(true);
    expect(layout.releases.peak).toBe(10);
    expect(layout.releases.points[1]?.y).toBe(0);
    expect(layout.releases.points[0]?.y).toBeGreaterThan(0);
    // Tracks series shares the axis: 4 of 10 renders below the releases peak.
    expect(layout.tracks.points).toHaveLength(1);
    expect(layout.tracks.points[0]?.y).toBe(60);

    const empty = layoutActivitySeries([], [], 720, 100);
    expect(empty.hours).toEqual([]);
    expect(empty.releases.path).toBe('');
  });

  it('derives stage progress from the documented lifecycle mapping', () => {
    // Monotonic through the happy path, terminal at both ends.
    expect(stageProgress('discovered')).toBeLessThan(stageProgress('normalizing'));
    expect(stageProgress('normalizing')).toBeLessThan(stageProgress('publishing'));
    expect(stageProgress('indexed')).toBe(100);
    expect(stageProgress('failed')).toBe(100);
    expect(stageProgress('ignored')).toBe(0);
  });
});

describe('OverviewScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetDashboard.mockResolvedValue(makeDashboard());
    mockGetOnboardingStatus.mockResolvedValue(ONBOARDING_COMPLETE);
    mockListWorkItems.mockResolvedValue(makeQueuePage([makeQueueItem()]));
  });

  it.each([0, 1])('links the review KPI to the matching state when the count is %s', async (count) => {
    const dashboard = makeDashboard();
    dashboard.workItems.countsByState = [{ state: 'needs_review', count }];
    mockGetDashboard.mockResolvedValue(dashboard);
    renderScreen();
    const card = await screen.findByRole('link', { name: /Needs review/ });
    expect(card).toHaveAttribute('href', '/processing/review?state=needs_review');
    expect(within(card).getByText(String(count))).toBeInTheDocument();
  });

  it('renders KPI cards, attention rows, and subsystem health from the DTO', async () => {
    renderScreen();
    expect(await screen.findByText('35,214')).toBeInTheDocument();
    expect(screen.getByText('50,482')).toBeInTheDocument();
    expect(screen.getByText('560,291')).toBeInTheDocument();
    // Failures are actionable attention, not a seventh KPI that forces the
    // dashboard's six-card metric row to wrap.
    expect(screen.queryByText('Failed (24h)')).not.toBeInTheDocument();

    const attention = screen.getByRole('heading', { name: 'Needs attention' }).closest('section');
    expect(attention).not.toBeNull();
    expect(within(attention as HTMLElement).getByText('Unresolved artist identities')).toBeInTheDocument();
    expect(within(attention as HTMLElement).getByText('23')).toBeInTheDocument();
    expect(within(attention as HTMLElement).getByText('Failed processing')).toBeInTheDocument();
    expect(
      within(attention as HTMLElement).getByRole('link', { name: /Quarantined/ }),
    ).toHaveAttribute('href', '/processing/review?state=quarantined');

    const health = screen.getByRole('heading', { name: 'System health' }).closest('section');
    expect(health).not.toBeNull();
    expect(within(health as HTMLElement).getByText('PostgreSQL')).toBeInTheDocument();
    expect(within(health as HTMLElement).getByText('Tantivy search index')).toBeInTheDocument();
    expect(within(health as HTMLElement).getAllByText('Healthy').length).toBeGreaterThan(0);
    expect(within(health as HTMLElement).getByText('OpenSubsonic')).toBeInTheDocument();
  });

  it('renders the activity chart with a screen-reader table alternative', async () => {
    renderScreen();
    expect(
      await screen.findByRole('img', {
        name: 'Line chart of Releases processed and Tracks indexed per hour',
      }),
    ).toBeInTheDocument();
    // Both series are named in the visible legend, and the same two series
    // drive the screen-reader data table (column headers + every hour row).
    const seriesNames = screen.getAllByText('Releases processed');
    expect(seriesNames.length).toBe(2);
    expect(seriesNames.some((element) => element.tagName === 'TH')).toBe(true);
    // The indexed column header is deliberately not the legend's wording:
    // the legend names the series, the column names the measure.
    expect(screen.getByText('Tracks indexed')).toBeInTheDocument();
    const dataTable = screen.getByRole('table', { name: 'Processing activity per hour' });
    expect(within(dataTable).getByText('Releases processed')).toBeInTheDocument();
    expect(within(dataTable).getByText('Indexed')).toBeInTheDocument();
    expect(within(dataTable).getAllByRole('row')).toHaveLength(3);
    // … and available as a data table for assistive tech.
    expect(screen.getByText('Processing activity per hour')).toBeInTheDocument();
  });

  it('renders the activity empty state when no completions are reported', async () => {
    mockGetDashboard.mockResolvedValue(
      makeDashboard({ activity: { completedPerHour: [], indexedPerHour: [] } }),
    );
    renderScreen();
    expect(
      await screen.findByText('No processing completions in the last 24 hours.'),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole('img', {
        name: 'Line chart of Releases processed and Tracks indexed per hour',
      }),
    ).not.toBeInTheDocument();
  });

  it('renders the KPI secondary lines from the dashboard deltas', async () => {
    renderScreen();
    expect(await screen.findByText('12 today')).toBeInTheDocument();
    expect(screen.getByText('5 new in 24h')).toBeInTheDocument();
    // Active-now is derived from the state counts (3 discovered + 5 ready…).
    expect(screen.getByText(/active now/)).toBeInTheDocument();
  });

  it('renders the Release-level queue preview with bound and unbound rows', async () => {
    mockListWorkItems.mockResolvedValue(
      makeQueuePage([
        makeQueueItem(),
        makeQueueItem({
          id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
          releaseId: null,
          releaseTitle: undefined,
          artistName: undefined,
          mediaBytes: undefined,
          currentPath: '/library/incoming/Road to Freedom',
          state: 'ready',
        }),
      ]),
    );
    renderScreen();
    const table = await screen.findByRole('table', { name: 'Current processing queue' });
    // Bound row links into the catalog and shows its artist.
    expect(within(table).getByRole('link', { name: 'Business as Usual' })).toHaveAttribute(
      'href',
      '/catalog/releases/55555555-5555-4555-8555-555555555555',
    );
    expect(within(table).getByText('Men at Work')).toBeInTheDocument();
    // Unbound row falls back to the path-derived display name and says so.
    expect(within(table).getByText('Road to Freedom')).toBeInTheDocument();
    expect(within(table).getAllByText('Not yet imported')).toHaveLength(1);
    // … and the artist cell is an honest em dash for the unbound row.
    expect(within(table).getByLabelText('Not yet imported', { selector: 'span' })).toBeInTheDocument();
    // Progress is a labeled stage, never a bare bar.
    expect(
      within(table).getByRole('progressbar', { name: 'Business as Usual processing stage' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open the processing queue' })).toHaveAttribute(
      'href',
      '/processing',
    );
  });

  it('keeps the queue card usable when the preview request fails', async () => {
    mockListWorkItems.mockRejectedValue(new Error('queue unavailable'));
    renderScreen();
    expect(await screen.findByText('35,214')).toBeInTheDocument();
    expect(screen.getByText('No work items are in the queue.')).toBeInTheDocument();
  });

  it('shows the attention empty state when every count is zero', async () => {
    mockGetDashboard.mockResolvedValue(
      makeDashboard({
        attention: {
          unresolvedIdentities: 0,
          pendingPolicyDecisions: 0,
          failedProcessing: 0,
          quarantined: 0,
        },
      }),
    );
    renderScreen();
    expect(
      await screen.findByText('Nothing needs attention right now.'),
    ).toBeInTheDocument();
  });

  it('surfaces degraded storage as an attention row', async () => {
    const dashboard = makeDashboard({
      attention: {
        unresolvedIdentities: 0,
        pendingPolicyDecisions: 0,
        failedProcessing: 0,
        quarantined: 0,
      },
    });
    dashboard.health.storage = { status: 'degraded', detail: 'inbound path not writable' };
    mockGetDashboard.mockResolvedValue(dashboard);
    renderScreen();
    expect(await screen.findByText('Storage check degraded')).toBeInTheDocument();
  });

  it('renders an error notification with retry when the dashboard fails', async () => {
    mockGetDashboard.mockRejectedValue(new Error('boom'));
    renderScreen();
    expect(
      await screen.findByText('The dashboard could not be loaded'),
    ).toBeInTheDocument();
    mockGetDashboard.mockResolvedValue(makeDashboard());
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('35,214')).toBeInTheDocument();
  });

  it('shows the setup banner when onboarding is incomplete and not bypassed', async () => {
    mockGetOnboardingStatus.mockResolvedValue({
      ...ONBOARDING_COMPLETE,
      completed: false,
      canComplete: false,
    });
    renderScreen();
    expect(await screen.findByText('Setup is not complete')).toBeInTheDocument();
  });

  it('shows no setup banner when onboarding was bypassed or the status endpoint fails', async () => {
    mockGetOnboardingStatus.mockResolvedValue({
      ...ONBOARDING_COMPLETE,
      completed: false,
      bypassed: true,
    });
    const { unmount } = renderScreen();
    expect(await screen.findByText('35,214')).toBeInTheDocument();
    expect(screen.queryByText('Setup is not complete')).not.toBeInTheDocument();

    unmount();
    mockGetOnboardingStatus.mockRejectedValue(new Error('status surface unavailable'));
    renderScreen();
    expect(await screen.findAllByText('35,214')).not.toHaveLength(0);
    expect(screen.queryByText('Setup is not complete')).not.toBeInTheDocument();
  });
});

import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Dashboard } from '../../api/types';
import { OverviewScreen } from './OverviewScreen';
import { countStates, INBOUND_STATES, layoutBars, sortedBuckets } from './overview';

vi.mock('../../api/admin', () => ({
  getDashboard: vi.fn(),
  getOnboardingStatus: vi.fn(),
}));

const { getDashboard, getOnboardingStatus } = await import('../../api/admin');
const mockGetDashboard = vi.mocked(getDashboard);
const mockGetOnboardingStatus = vi.mocked(getOnboardingStatus);

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
      tantivy: { status: 'ok', detail: 'active search generation validated' },
      storage: { status: 'ok', detail: 'storage paths writable' },
      ffmpeg: { status: 'ok', detail: 'ffmpeg 8.1' },
      apiSurfaces: [
        { surface: 'native', status: 'serving' },
        { surface: 'opensubsonic', status: 'serving' },
        { surface: 'jellyfin', status: 'serving' },
        { surface: 'admin', status: 'serving' },
      ],
    },
    activity: {
      completedPerHour: [
        { hour: '2026-08-14T11:00:00Z', count: 9 },
        { hour: '2026-08-14T10:00:00Z', count: 5 },
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

  it('lays out bars scaled to the maximum bucket', () => {
    const bars = layoutBars(
      [
        { hour: '2026-08-14T10:00:00Z', count: 5 },
        { hour: '2026-08-14T11:00:00Z', count: 10 },
      ],
      720,
      160,
    );
    expect(bars).toHaveLength(2);
    expect(bars[1]?.height).toBe(160);
    expect(bars[0]?.height).toBeLessThan(160);
    expect(layoutBars([], 720, 160)).toEqual([]);
  });
});

describe('OverviewScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetDashboard.mockResolvedValue(makeDashboard());
    mockGetOnboardingStatus.mockResolvedValue(ONBOARDING_COMPLETE);
  });

  it('renders KPI cards, attention rows, and subsystem health from the DTO', async () => {
    renderScreen();
    expect(await screen.findByText('35,214')).toBeInTheDocument();
    expect(screen.getByText('50,482')).toBeInTheDocument();
    expect(screen.getByText('560,291')).toBeInTheDocument();
    // Work-item KPIs: inbound = discovered 3, failed 24h = 4.
    expect(screen.getByText('Failed (24h)')).toBeInTheDocument();
    expect(screen.getByText('4')).toBeInTheDocument();

    const attention = screen.getByRole('heading', { name: 'Needs attention' }).closest('section');
    expect(attention).not.toBeNull();
    expect(within(attention as HTMLElement).getByText('Unresolved artist identities')).toBeInTheDocument();
    expect(within(attention as HTMLElement).getByText('23')).toBeInTheDocument();
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
      await screen.findByRole('img', { name: 'Bar chart of completed work items per hour' }),
    ).toBeInTheDocument();
    // The same series is available as a data table for assistive tech.
    expect(screen.getByText('Completed work items per hour')).toBeInTheDocument();
  });

  it('renders the activity empty state when no completions are reported', async () => {
    mockGetDashboard.mockResolvedValue(
      makeDashboard({ activity: { completedPerHour: [] } }),
    );
    renderScreen();
    expect(
      await screen.findByText('No processing completions in the last 24 hours.'),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole('img', { name: 'Bar chart of completed work items per hour' }),
    ).not.toBeInTheDocument();
  });

  it('renders the mini queue table with links into the processing queue', async () => {
    renderScreen();
    const table = await screen.findByRole('table', { name: 'Work items by state' });
    expect(within(table).getByText('needs_review')).toBeInTheDocument();
    expect(within(table).getByText('2')).toBeInTheDocument();
    expect(
      screen.getByRole('link', { name: 'Open the processing queue' }),
    ).toHaveAttribute('href', '/processing');
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

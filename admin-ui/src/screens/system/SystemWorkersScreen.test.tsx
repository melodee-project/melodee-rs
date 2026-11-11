import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '../../api/client';
import type {
  Dashboard,
  Lease,
  SearchStatus,
  SystemHealth,
  WorkItem,
  WorkerPool,
} from '../../api/types';
import { SystemWorkersScreen } from './SystemWorkersScreen';

vi.mock('../../api/admin', () => ({
  getDashboard: vi.fn(),
  getDiagnosticsBundle: vi.fn(),
  getSystemHealth: vi.fn(),
  listJobHistory: vi.fn(),
  listLeases: vi.fn(),
  listWorkerFailures: vi.fn(),
  listWorkerPools: vi.fn(),
  pauseWorkerPool: vi.fn(),
  requestSearchRebuild: vi.fn(),
  resumeWorkerPool: vi.fn(),
  getSearchStatus: vi.fn(),
}));

const admin = await import('../../api/admin');
const mockGetDashboard = vi.mocked(admin.getDashboard);
const mockGetDiagnosticsBundle = vi.mocked(admin.getDiagnosticsBundle);
const mockGetSystemHealth = vi.mocked(admin.getSystemHealth);
const mockListJobHistory = vi.mocked(admin.listJobHistory);
const mockListLeases = vi.mocked(admin.listLeases);
const mockListWorkerFailures = vi.mocked(admin.listWorkerFailures);
const mockListWorkerPools = vi.mocked(admin.listWorkerPools);
const mockPauseWorkerPool = vi.mocked(admin.pauseWorkerPool);
const mockRequestSearchRebuild = vi.mocked(admin.requestSearchRebuild);
const mockResumeWorkerPool = vi.mocked(admin.resumeWorkerPool);
const mockGetSearchStatus = vi.mocked(admin.getSearchStatus);

function makePool(overrides: Partial<WorkerPool> = {}): WorkerPool {
  return {
    name: 'policy',
    paused: false,
    pausedBy: null,
    pausedAt: null,
    resumeRequestedAt: null,
    updatedAt: '2026-08-14T12:00:00Z',
    metricsAvailable: true,
    queued: 7,
    running: 2,
    oldestQueuedAgeSeconds: 95,
    completedLastHour: 60,
    throughputPerMinute: 1,
    failedLast24h: 0,
    ...overrides,
  };
}

function makeFailure(overrides: Partial<WorkItem> = {}): WorkItem {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    apiKey: '33333333-3333-4333-8333-333333333333',
    sourceLibraryId: null,
    targetLibraryId: null,
    sourcePath: '/library/incoming/Business as Usual',
    currentPath: '/library/incoming/Business as Usual',
    targetPath: null,
    sourceFingerprint: null,
    manifestHash: null,
    state: 'failed',
    stateReason: null,
    attemptCount: 5,
    nextAttemptAt: null,
    leaseOwner: null,
    leaseAcquiredAt: null,
    leaseExpiresAt: null,
    lastErrorCode: 'checksum_mismatch',
    lastErrorMessage: 'checksum mismatch',
    correlationId: '44444444-4444-4444-8444-444444444444',
    discoveredAt: '2026-08-14T10:00:00Z',
    firstStartedAt: null,
    lastAttemptAt: null,
    updatedAt: '2026-08-14T10:05:00Z',
    createdAt: '2026-08-14T10:00:00Z',
    completedAt: null,
    ...overrides,
  };
}

const HEALTH: SystemHealth = {
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
    { surface: 'admin', status: 'serving' },
  ],
  resourceBounds: [],
};

const SEARCH_ENABLED: SearchStatus = {
  enabled: true,
  projection: {
    pending: 3,
    failed: 0,
    leased: 1,
    oldestAvailableAt: null,
    lastProcessedOutboxId: 42,
    activeGenerationId: '55555555-5555-4555-8555-555555555555',
    latestRebuildStatus: null,
    latestRebuildError: null,
  },
  generation: { artists: 100, releases: 200, tracks: 300, total: 600, schemaVersion: 1 },
  generationErrorCode: null,
  generationError: null,
};

const LEASE: Lease = {
  workItemId: '66666666-6666-4666-8666-666666666666',
  apiKey: '77777777-7777-4777-8777-777777777777',
  state: 'inspecting',
  leaseOwner: 'worker-3',
  leaseAcquiredAt: '2026-08-14T11:55:00Z',
  leaseExpiresAt: '2026-08-14T12:05:00Z',
  lastActivityAt: '2026-08-14T11:56:00Z',
};

function makeDashboard(): Dashboard {
  return {
    generatedAt: '2026-08-14T12:00:00Z',
    catalog: { artists: 1, releases: 2, tracks: 3, mediaAssets: 4, pendingSearchOutbox: 0 },
    workItems: {
      countsByState: [{ state: 'failed', count: 1 }],
      failedLast24h: 3,
      completedLast24h: 4231,
    },
    attention: {
      unresolvedIdentities: 0,
      pendingPolicyDecisions: 0,
      failedProcessing: 1,
      quarantined: 0,
    },
    health: HEALTH,
    activity: { completedPerHour: [] },
  };
}

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="search-params">{location.search}</output>;
}

function renderScreen(initialEntry = '/system/workers') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <SystemWorkersScreen />
      <LocationProbe />
    </MemoryRouter>,
  );
}

/** Carbon keeps closed modals mounted; pick the dialog containing a heading. */
async function findDialogWithText(text: string): Promise<HTMLElement> {
  const dialogs = await screen.findAllByRole('dialog');
  const match = dialogs.find((dialog) => within(dialog).queryByText(text) !== null);
  if (!match) {
    throw new Error(`no dialog contains "${text}"`);
  }
  return match;
}

/** The panel trigger, not the (mounted) modal's primary button. */
function panelButton(name: string): HTMLElement {
  const matches = screen
    .getAllByRole('button', { name })
    .filter((element) => element.closest('[role="dialog"]') === null);
  if (matches.length !== 1) {
    throw new Error(`expected exactly one panel button "${name}", found ${matches.length}`);
  }
  const [button] = matches;
  return button as HTMLElement;
}

describe('SystemWorkersScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetDiagnosticsBundle.mockResolvedValue({
      generatedAt: '2026-08-14T12:00:00Z',
      uptimeSeconds: 1_054_800,
      server: { name: 'melodee', version: 'test', buildProfile: 'test', pid: 1 },
    });
    mockListWorkerPools.mockResolvedValue({
      items: [
        makePool(),
        makePool({
          name: 'discovery',
          metricsAvailable: false,
          queued: null,
          running: null,
          oldestQueuedAgeSeconds: null,
          completedLastHour: null,
          throughputPerMinute: null,
          failedLast24h: null,
        }),
      ],
      total: 2,
      limit: 2,
      offset: 0,
    });
    mockGetDashboard.mockResolvedValue(makeDashboard());
    mockGetSystemHealth.mockResolvedValue(HEALTH);
    mockListJobHistory.mockResolvedValue({ items: [], total: 0, limit: 50, offset: 0 });
    mockListWorkerFailures.mockResolvedValue({
      items: [makeFailure()],
      total: 1,
      limit: 50,
      offset: 0,
    });
    mockGetSearchStatus.mockResolvedValue(SEARCH_ENABLED);
    mockListLeases.mockResolvedValue([LEASE]);
  });

  it('renders KPI cards and the pools table with honest absent metrics', async () => {
    renderScreen();
    const poolsTable = await screen.findByRole('table', {
      name: 'Worker pools and queue metrics',
    });
    expect(within(poolsTable).getByText('policy')).toBeInTheDocument();
    expect(within(poolsTable).getByText('discovery')).toBeInTheDocument();
    // The discovery pool has no durable queue table: honest absence, not zeros.
    const discoveryRow = within(poolsTable).getByText('discovery').closest('tr');
    expect(discoveryRow).not.toBeNull();
    expect(
      within(discoveryRow as HTMLElement).getAllByText('Not reported').length,
    ).toBeGreaterThan(0);
    // KPI cards: pools total and dashboard 24h windows.
    expect(screen.getByText('4,231')).toBeInTheDocument();
    expect(screen.getByText('Completed (24h)')).toBeInTheDocument();
  });

  it('pauses a pool after the confirmation modal and updates the row', async () => {
    mockPauseWorkerPool.mockResolvedValue(
      makePool({ paused: true, pausedBy: 'admin-1', pausedAt: '2026-08-14T12:01:00Z' }),
    );
    renderScreen();
    const poolsTable = await screen.findByRole('table', {
      name: 'Worker pools and queue metrics',
    });
    const policyRow = within(poolsTable).getByText('policy').closest('tr') as HTMLElement;
    fireEvent.click(within(policyRow).getByRole('button', { name: 'Pause' }));

    const dialog = await findDialogWithText('Pause pool policy');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Pause' }));

    await waitFor(() => expect(mockPauseWorkerPool).toHaveBeenCalledWith('policy'));
    expect(await within(policyRow).findByText('Paused')).toBeInTheDocument();
    expect(within(policyRow).getByText('Paused by admin-1')).toBeInTheDocument();
  });

  it('resumes a paused pool without a confirmation step', async () => {
    mockListWorkerPools.mockResolvedValue({
      items: [makePool({ paused: true, pausedBy: 'admin-1' })],
      total: 1,
      limit: 1,
      offset: 0,
    });
    mockResumeWorkerPool.mockResolvedValue(makePool({ paused: false }));
    renderScreen();
    const poolsTable = await screen.findByRole('table', {
      name: 'Worker pools and queue metrics',
    });
    fireEvent.click(within(poolsTable).getByRole('button', { name: 'Resume' }));
    await waitFor(() => expect(mockResumeWorkerPool).toHaveBeenCalledWith('policy'));
    expect(await within(poolsTable).findByText('Active')).toBeInTheDocument();
  });

  it('renders recent failures and navigates to the processing inspector', async () => {
    renderScreen();
    const failuresTable = await screen.findByRole('table', {
      name: 'Recent failed work items',
    });
    expect(within(failuresTable).getByText('checksum mismatch')).toBeInTheDocument();
    fireEvent.click(within(failuresTable).getByText('/library/incoming/Business as Usual'));
    await waitFor(() =>
      expect(screen.getByTestId('search-params')).toHaveTextContent(
        'state=failed&item=11111111-1111-4111-8111-111111111111',
      ),
    );
  });

  it('renders search status and surfaces a 409 rebuild conflict', async () => {
    mockRequestSearchRebuild.mockRejectedValue(
      new ApiError(409, 'rebuild_in_progress', 'an unfinished search rebuild already exists', null),
    );
    renderScreen();
    expect(await screen.findByText('Indexed documents')).toBeInTheDocument();
    expect(screen.getByText('600')).toBeInTheDocument();

    fireEvent.click(panelButton('Rebuild search index'));
    const dialog = await findDialogWithText('Rebuild the search index');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Rebuild search index' }));

    await waitFor(() => expect(mockRequestSearchRebuild).toHaveBeenCalled());
    expect(
      await screen.findByText('A rebuild is already in progress.'),
    ).toBeInTheDocument();
  });

  it('acknowledges a rebuild request with the job id and no fake progress', async () => {
    mockRequestSearchRebuild.mockResolvedValue({
      jobId: '88888888-8888-4888-8888-888888888888',
      status: 'pending',
      requestedBy: 'admin-1',
    });
    renderScreen();
    await screen.findByText('Indexed documents');
    fireEvent.click(panelButton('Rebuild search index'));
    const dialog = await findDialogWithText('Rebuild the search index');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Rebuild search index' }));
    expect(
      await screen.findByText(/Rebuild requested \(job 88888888-8888-4888-8888-888888888888\)/),
    ).toBeInTheDocument();
  });

  it('disables the rebuild trigger honestly when the search worker is off', async () => {
    mockGetSearchStatus.mockResolvedValue({
      enabled: false,
      projection: null,
      generation: null,
      generationErrorCode: null,
      generationError: null,
    });
    renderScreen();
    expect(await screen.findByText('Search worker disabled')).toBeInTheDocument();
    expect(panelButton('Rebuild search index')).toBeDisabled();
  });

  it('renders active lease evidence', async () => {
    renderScreen();
    const leasesTable = await screen.findByRole('table', { name: 'Active worker leases' });
    expect(within(leasesTable).getByText('worker-3')).toBeInTheDocument();
    expect(
      within(leasesTable).getByRole('link', { name: LEASE.workItemId }),
    ).toHaveAttribute('href', `/processing?item=${LEASE.workItemId}`);
  });

  it('states that logs and performance remain CLI surfaces', async () => {
    renderScreen();
    expect(
      await screen.findByText('Logs and performance remain CLI surfaces'),
    ).toBeInTheDocument();
  });

  it('renders the subsystem health panel from /system/health', async () => {
    renderScreen();
    const health = (
      await screen.findByRole('heading', { name: 'System Health' })
    ).closest('section');
    expect(health).not.toBeNull();
    expect(within(health as HTMLElement).getByText('FFmpeg')).toBeInTheDocument();
    expect(within(health as HTMLElement).getByText('Melodee API v1')).toBeInTheDocument();
  });
});

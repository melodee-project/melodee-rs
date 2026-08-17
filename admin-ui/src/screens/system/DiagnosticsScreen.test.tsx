import { render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '../../api/client';
import type { Lease, SystemHealth } from '../../api/types';
import { DiagnosticsScreen } from './DiagnosticsScreen';

vi.mock('../../api/admin', () => ({
  getSystemHealth: vi.fn(),
  listLeases: vi.fn(),
}));

const admin = await import('../../api/admin');
const mockGetSystemHealth = vi.mocked(admin.getSystemHealth);
const mockListLeases = vi.mocked(admin.listLeases);

const HEALTH: SystemHealth = {
  status: 'ok',
  postgresql: { status: 'ok', detail: 'postgresql answered a bounded probe' },
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

const LEASE: Lease = {
  workItemId: '66666666-6666-4666-8666-666666666666',
  apiKey: '77777777-7777-4777-8777-777777777777',
  state: 'inspecting',
  leaseOwner: 'worker-3',
  leaseAcquiredAt: '2026-08-14T11:55:00Z',
  leaseExpiresAt: '2026-08-14T12:05:00Z',
  lastActivityAt: '2026-08-14T11:56:00Z',
};

function renderScreen() {
  return render(
    <MemoryRouter initialEntries={['/system/diagnostics']}>
      <DiagnosticsScreen />
    </MemoryRouter>,
  );
}

describe('DiagnosticsScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetSystemHealth.mockResolvedValue(HEALTH);
    mockListLeases.mockResolvedValue([LEASE]);
  });

  it('renders the subsystem health panel from /system/health', async () => {
    renderScreen();
    const health = (
      await screen.findByRole('heading', { name: 'System Health' })
    ).closest('section');
    expect(health).not.toBeNull();
    expect(within(health as HTMLElement).getByText('PostgreSQL')).toBeInTheDocument();
    expect(within(health as HTMLElement).getByText('FFmpeg')).toBeInTheDocument();
    expect(within(health as HTMLElement).getByText('Melodee API v1')).toBeInTheDocument();
  });

  it('renders the shared active-leases table', async () => {
    renderScreen();
    const leasesTable = await screen.findByRole('table', { name: 'Active worker leases' });
    expect(within(leasesTable).getByText('worker-3')).toBeInTheDocument();
    expect(
      within(leasesTable).getByRole('link', { name: LEASE.workItemId }),
    ).toHaveAttribute('href', `/processing?item=${LEASE.workItemId}`);
  });

  it('renders the leases empty state honestly when none are active', async () => {
    mockListLeases.mockResolvedValue([]);
    renderScreen();
    expect(await screen.findByText('No active leases.')).toBeInTheDocument();
  });

  it('links to the Workers screen for recent failures and keeps the CLI note', async () => {
    renderScreen();
    expect(
      await screen.findByRole('link', {
        name: 'Recent failed work items are listed on System → Workers.',
      }),
    ).toHaveAttribute('href', '/system/workers');
    expect(
      screen.getByText('Logs and performance remain CLI surfaces'),
    ).toBeInTheDocument();
  });

  it('renders a retryable error state when the health fetch fails', async () => {
    mockGetSystemHealth.mockRejectedValue(new ApiError(500, 'server_error', 'boom', null));
    renderScreen();
    expect(
      await screen.findByText('System information could not be loaded'),
    ).toBeInTheDocument();
    expect(screen.getByText('boom')).toBeInTheDocument();
  });
});

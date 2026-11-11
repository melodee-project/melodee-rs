import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { WorkItem, WorkItemEvent } from '../../api/types';
import { HistoryScreen } from './HistoryScreen';
import { QuarantineScreen } from './QuarantineScreen';

vi.mock('../../api/admin', () => ({
  listWorkItems: vi.fn(),
  getWorkItem: vi.fn(),
  getWorkItemEvents: vi.fn(),
}));

const { listWorkItems, getWorkItem, getWorkItemEvents } = await import('../../api/admin');
const mockListWorkItems = vi.mocked(listWorkItems);
const mockGetWorkItem = vi.mocked(getWorkItem);
const mockGetWorkItemEvents = vi.mocked(getWorkItemEvents);

const ID_ONE = '11111111-1111-4111-8111-111111111111';

function makeWorkItem(overrides: Partial<WorkItem> = {}): WorkItem {
  return {
    id: ID_ONE,
    apiKey: '33333333-3333-4333-8333-333333333333',
    sourceLibraryId: null,
    targetLibraryId: null,
    sourcePath: '/library/incoming/Business as Usual',
    currentPath: '/library/incoming/Business as Usual',
    targetPath: null,
    sourceFingerprint: null,
    manifestHash: null,
    state: 'quarantined',
    stateReason: 'operator decision',
    attemptCount: 2,
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
    completedAt: '2026-08-14T10:06:00Z',
    ...overrides,
  };
}

function makeEvent(overrides: Partial<WorkItemEvent> = {}): WorkItemEvent {
  return {
    id: 1,
    workItemId: ID_ONE,
    eventType: 'transitioned',
    fromState: 'needs_review',
    toState: 'quarantined',
    reasonCode: 'operator_decision',
    workerId: null,
    correlationId: '55555555-5555-4555-8555-555555555555',
    createdAt: '2026-08-14T10:05:00Z',
    metadata: null,
    ...overrides,
  };
}

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="search-params">{location.search}</output>;
}

function renderScreen(component: React.ReactElement, initialEntry: string) {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      {component}
      <LocationProbe />
    </MemoryRouter>,
  );
}

describe('QuarantineScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListWorkItems.mockResolvedValue({
      items: [makeWorkItem()],
      total: 1,
      limit: 50,
      offset: 0,
    });
    mockGetWorkItem.mockResolvedValue(makeWorkItem());
    mockGetWorkItemEvents.mockResolvedValue([makeEvent()]);
  });

  it('pins the listing to the quarantined state server-side', async () => {
    renderScreen(<QuarantineScreen />, '/processing/quarantine');
    expect(await screen.findByText('Business as Usual')).toBeInTheDocument();
    await waitFor(() =>
      expect(mockListWorkItems).toHaveBeenLastCalledWith(
        expect.objectContaining({ state: 'quarantined', offset: 0, limit: 50 }),
      ),
    );
  });

  it('links each row to the review workspace (state-appropriate)', async () => {
    renderScreen(<QuarantineScreen />, '/processing/quarantine');
    const link = await screen.findByRole('link', { name: 'Open review' });
    expect(link).toHaveAttribute('href', `/processing/review/${ID_ONE}`);
  });

  it('opens the work-item inspector with the event history via URL state', async () => {
    renderScreen(<QuarantineScreen />, '/processing/quarantine');
    fireEvent.click(await screen.findByText('Business as Usual'));
    expect(await screen.findByTestId('search-params')).toHaveTextContent(`item=${ID_ONE}`);
    expect(await screen.findByText(/needs_review → quarantined/)).toBeInTheDocument();
  });

  it('shows the empty state when nothing is quarantined', async () => {
    mockListWorkItems.mockResolvedValue({ items: [], total: 0, limit: 50, offset: 0 });
    renderScreen(<QuarantineScreen />, '/processing/quarantine');
    expect(await screen.findByText('Nothing quarantined')).toBeInTheDocument();
  });
});

describe('HistoryScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListWorkItems.mockResolvedValue({
      items: [makeWorkItem({ state: 'cataloged' })],
      total: 1,
      limit: 50,
      offset: 0,
    });
    mockGetWorkItem.mockResolvedValue(makeWorkItem({ state: 'cataloged' }));
    mockGetWorkItemEvents.mockResolvedValue([makeEvent({ toState: 'cataloged' })]);
  });

  it('defaults to the cataloged terminal state and switches server-side', async () => {
    renderScreen(<HistoryScreen />, '/processing/history');
    expect(await screen.findByText('Business as Usual')).toBeInTheDocument();
    await waitFor(() =>
      expect(mockListWorkItems).toHaveBeenLastCalledWith(
        expect.objectContaining({ state: 'cataloged' }),
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Ignored' }));
    await waitFor(() =>
      expect(mockListWorkItems).toHaveBeenLastCalledWith(
        expect.objectContaining({ state: 'ignored', offset: 0 }),
      ),
    );
    expect(await screen.findByTestId('search-params')).toHaveTextContent('state=ignored');
  });

  it('offers no review-workspace link for terminal states', async () => {
    renderScreen(<HistoryScreen />, '/processing/history');
    expect(await screen.findByText('Business as Usual')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Open review' })).not.toBeInTheDocument();
  });

  it('falls back to the default state for an unknown state param', async () => {
    renderScreen(<HistoryScreen />, '/processing/history?state=ready');
    await waitFor(() =>
      expect(mockListWorkItems).toHaveBeenLastCalledWith(
        expect.objectContaining({ state: 'cataloged' }),
      ),
    );
  });
});

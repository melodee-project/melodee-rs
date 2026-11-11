import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { WorkItem, WorkItemEvent } from '../../api/types';
import { ProcessingQueueScreen } from './ProcessingQueueScreen';

vi.mock('../../api/admin', () => ({
  listWorkItems: vi.fn(),
  getWorkItemCounts: vi.fn(),
  getWorkItem: vi.fn(),
  getWorkItemEvents: vi.fn(),
}));

const { listWorkItems, getWorkItemCounts, getWorkItem, getWorkItemEvents } = await import(
  '../../api/admin'
);
const mockListWorkItems = vi.mocked(listWorkItems);
const mockGetWorkItemCounts = vi.mocked(getWorkItemCounts);
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
    state: 'ready',
    stateReason: null,
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
    completedAt: null,
    ...overrides,
  };
}

function makeEvent(overrides: Partial<WorkItemEvent> = {}): WorkItemEvent {
  return {
    id: 1,
    workItemId: ID_ONE,
    eventType: 'transitioned',
    fromState: 'ready',
    toState: 'inspecting',
    reasonCode: null,
    workerId: 'worker-1',
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

function renderScreen(initialEntry = '/processing') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <ProcessingQueueScreen />
      <LocationProbe />
    </MemoryRouter>,
  );
}

describe('ProcessingQueueScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetWorkItemCounts.mockResolvedValue([
      { state: 'ready', count: 1 },
      { state: 'failed', count: 3 },
    ]);
    mockListWorkItems.mockResolvedValue({
      items: [makeWorkItem()],
      total: 4,
      limit: 50,
      offset: 0,
    });
    mockGetWorkItem.mockResolvedValue(
      makeWorkItem({ leaseOwner: 'worker-1', lastErrorMessage: 'checksum mismatch' }),
    );
    mockGetWorkItemEvents.mockResolvedValue([makeEvent()]);
  });

  it('renders rows from the pagination envelope and live count pills', async () => {
    renderScreen();
    expect(await screen.findByText('Business as Usual')).toBeInTheDocument();
    // Pills are driven by /counts: All total plus per-state counts, exposed as
    // toggle buttons (ADR-0184) rather than Carbon ContentSwitcher tabs.
    // The pill name is the localized label; the count is a sibling span.
    const pills = await screen.findAllByRole('button', { pressed: true });
    expect(pills[0]).toHaveTextContent('All');
    const group = screen.getByRole('group', { name: 'Filter by state' });
    expect(within(group).getByRole('button', { name: /All/ })).toHaveTextContent('4');
    expect(within(group).getByRole('button', { name: /Ready/ })).toHaveTextContent('1');
    expect(within(group).getByRole('button', { name: /Failed/ })).toHaveTextContent('3');
  });

  it('applies the pill state filter to the API call and the URL', async () => {
    renderScreen();
    const group = await screen.findByRole('group', { name: 'Filter by state' });
    fireEvent.click(within(group).getByRole('button', { name: /Failed/ }));
    await waitFor(() =>
      expect(mockListWorkItems).toHaveBeenLastCalledWith(
        expect.objectContaining({ state: 'failed', offset: 0 }),
      ),
    );
    expect(screen.getByTestId('search-params')).toHaveTextContent('state=failed');
  });

  it('emits only whitelisted sort params', async () => {
    renderScreen();
    await screen.findByText('Business as Usual');
    fireEvent.click(screen.getByRole('button', { name: 'Sort by Attempts' }));
    await waitFor(() =>
      expect(mockListWorkItems).toHaveBeenLastCalledWith(
        expect.objectContaining({ sort: 'attemptCount:asc' }),
      ),
    );
    expect(screen.getByTestId('search-params')).toHaveTextContent('sort=attemptCount%3Aasc');
  });

  it('ignores a non-whitelisted sort key in the URL', async () => {
    renderScreen('/processing?sort=password:asc');
    await waitFor(() => expect(mockListWorkItems).toHaveBeenCalled());
    expect(mockListWorkItems).toHaveBeenLastCalledWith(
      expect.objectContaining({ sort: undefined }),
    );
  });

  it('opens the inspector with detail and event history on row activation', async () => {
    renderScreen();
    fireEvent.click(await screen.findByText('Business as Usual'));
    await waitFor(() => expect(mockGetWorkItem).toHaveBeenCalledWith(ID_ONE));
    expect(mockGetWorkItemEvents).toHaveBeenCalledWith(ID_ONE);
    expect((await screen.findAllByText('worker-1')).length).toBeGreaterThan(0);
    expect(screen.getByText('checksum mismatch', { exact: false })).toBeInTheDocument();
    expect(screen.getByText('transitioned: ready → inspecting')).toBeInTheDocument();
    // Selection is reflected in the URL; the queue position is untouched.
    expect(screen.getByTestId('search-params')).toHaveTextContent(`item=${ID_ONE}`);
    // The queue row stays mounted (position preserved) alongside the inspector.
    const table = screen.getByRole('table', { name: 'Processing queue work items' });
    expect(within(table).getByText('Business as Usual')).toBeInTheDocument();
  });

  it('opens the inspector from the keyboard (Enter on a focused row)', async () => {
    renderScreen();
    const row = (await screen.findByText('Business as Usual')).closest('tr');
    expect(row).not.toBeNull();
    row?.focus();
    fireEvent.keyDown(row as HTMLElement, { key: 'Enter' });
    await waitFor(() => expect(mockGetWorkItem).toHaveBeenCalledWith(ID_ONE));
  });

  it('updates the URL and offset when paging', async () => {
    mockListWorkItems.mockResolvedValue({ items: [makeWorkItem()], total: 137, limit: 50, offset: 0 });
    renderScreen();
    await screen.findByText('Business as Usual');
    fireEvent.click(screen.getByRole('button', { name: /next page/i }));
    await waitFor(() =>
      expect(mockListWorkItems).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 50 })),
    );
    expect(screen.getByTestId('search-params')).toHaveTextContent('page=2');
  });

  it('shows the bulk-action bar with a disabled action when rows are selected', async () => {
    renderScreen();
    await screen.findByText('Business as Usual');
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select all rows' }));
    expect(await screen.findByText('1 selected')).toBeInTheDocument();
    const bulkButton = screen.getByRole('button', { name: 'Bulk actions' });
    expect(bulkButton).toBeDisabled();
  });

  it('renders an error notification with retry when the listing fails', async () => {
    mockListWorkItems.mockRejectedValue(new Error('boom'));
    renderScreen();
    expect(
      await screen.findByText('The processing queue could not be loaded'),
    ).toBeInTheDocument();
    mockListWorkItems.mockResolvedValue({ items: [makeWorkItem()], total: 1, limit: 50, offset: 0 });
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('Business as Usual')).toBeInTheDocument();
  });

  it('renders the empty state when the envelope has no items', async () => {
    mockListWorkItems.mockResolvedValue({ items: [], total: 0, limit: 50, offset: 0 });
    renderScreen();
    expect(await screen.findByText('No work items')).toBeInTheDocument();
  });
});

import { fireEvent, render, within, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { ReviewQueueItem, ReviewQueuePage } from '../../api/types';
import { ReviewQueueScreen } from './ReviewQueueScreen';

vi.mock('../../api/admin', () => ({
  listReviewQueue: vi.fn(),
}));

const { listReviewQueue } = await import('../../api/admin');
const mockListReviewQueue = vi.mocked(listReviewQueue);

const ID_ONE = '11111111-1111-4111-8111-111111111111';
const ID_TWO = '22222222-2222-4222-8222-222222222222';

function makeItem(overrides: Partial<ReviewQueueItem> = {}): ReviewQueueItem {
  return {
    id: ID_ONE,
    apiKey: '33333333-3333-4333-8333-333333333333',
    sourceLibraryId: null,
    targetLibraryId: null,
    sourcePath: '/library/incoming/Greatest Hits',
    currentPath: '/library/incoming/Greatest Hits',
    targetPath: null,
    sourceFingerprint: null,
    manifestHash: null,
    state: 'needs_review',
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
    ...overrides,
  };
}

function makePage(overrides: Partial<ReviewQueuePage> = {}): ReviewQueuePage {
  return {
    items: [makeItem()],
    total: 3,
    limit: 50,
    offset: 0,
    countsByState: [
      { state: 'needs_review', count: 2 },
      { state: 'failed', count: 1 },
    ],
    ...overrides,
  };
}

function LocationProbe() {
  const location = useLocation();
  return (
    <output data-testid="location">
      {location.pathname}
      {location.search}
    </output>
  );
}

function renderScreen(initialEntry = '/processing/review') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <ReviewQueueScreen />
      <LocationProbe />
    </MemoryRouter>,
  );
}

describe('ReviewQueueScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListReviewQueue.mockResolvedValue(makePage());
  });

  it('renders rows and state chips from the listing envelope', async () => {
    renderScreen();
    expect(await screen.findByText('Greatest Hits')).toBeInTheDocument();
    // Chips are driven by the envelope's countsByState.
    // Filter pills (ADR-0184): toggle buttons with live counts.
    const group = await screen.findByRole('group', { name: 'Filter by review state' });
    expect(within(group).getByRole('button', { name: /All/ })).toHaveTextContent('3');
    expect(within(group).getByRole('button', { name: /Needs review/ })).toHaveTextContent('2');
    expect(within(group).getByRole('button', { name: /Failed/ })).toHaveTextContent('1');
    // Quarantined has no count in the envelope, so no chip is rendered.
    expect(screen.queryByRole('tab', { name: /Quarantined/ })).not.toBeInTheDocument();
  });

  it('applies the chip state filter to the API call and the URL', async () => {
    renderScreen();
    fireEvent.click(await screen.findByRole('button', { name: /Failed/ }));
    await waitFor(() =>
      expect(mockListReviewQueue).toHaveBeenLastCalledWith(
        expect.objectContaining({ state: 'failed', offset: 0 }),
      ),
    );
    expect(screen.getByTestId('location')).toHaveTextContent('state=failed');
  });

  it('emits only whitelisted sort params', async () => {
    renderScreen();
    await screen.findByText('Greatest Hits');
    fireEvent.click(screen.getByRole('button', { name: 'Sort by Updated' }));
    await waitFor(() =>
      expect(mockListReviewQueue).toHaveBeenLastCalledWith(
        expect.objectContaining({ sort: 'updatedAt:asc' }),
      ),
    );
  });

  it('ignores a non-reviewable state in the URL', async () => {
    renderScreen('/processing/review?state=deleted');
    await waitFor(() => expect(mockListReviewQueue).toHaveBeenCalled());
    expect(mockListReviewQueue).toHaveBeenLastCalledWith(
      expect.objectContaining({ state: undefined }),
    );
  });

  it('navigates to the review workspace on row activation', async () => {
    renderScreen();
    fireEvent.click(await screen.findByText('Greatest Hits'));
    expect(screen.getByTestId('location')).toHaveTextContent(`/processing/review/${ID_ONE}`);
  });

  it('opens the workspace from the keyboard (Enter on a focused row)', async () => {
    mockListReviewQueue.mockResolvedValue(
      makePage({
        items: [
          makeItem(),
          makeItem({
            id: ID_TWO,
            sourcePath: '/library/incoming/Other',
            currentPath: '/library/incoming/Other',
          }),
        ],
      }),
    );
    renderScreen();
    const row = (await screen.findByText('Greatest Hits')).closest('tr');
    expect(row).not.toBeNull();
    row?.focus();
    fireEvent.keyDown(row as HTMLElement, { key: 'ArrowDown' });
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: 'Enter' });
    expect(screen.getByTestId('location')).toHaveTextContent(`/processing/review/${ID_TWO}`);
  });

  it('renders an error notification with retry when the listing fails', async () => {
    mockListReviewQueue.mockRejectedValue(new Error('boom'));
    renderScreen();
    expect(
      await screen.findByText('The review queue could not be loaded'),
    ).toBeInTheDocument();
    mockListReviewQueue.mockResolvedValue(makePage());
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('Greatest Hits')).toBeInTheDocument();
  });

  it('renders the empty state when the envelope has no items', async () => {
    mockListReviewQueue.mockResolvedValue(
      makePage({ items: [], total: 0, countsByState: [] }),
    );
    renderScreen();
    expect(await screen.findByText('Nothing to review')).toBeInTheDocument();
  });
});

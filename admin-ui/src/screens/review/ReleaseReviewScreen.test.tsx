import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '../../api/client';
import type { ReviewDetail } from '../../api/types';
import { ReleaseReviewScreen } from './ReleaseReviewScreen';

vi.mock('../../api/admin', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../api/admin')>();
  return {
    ...original,
    getReviewDetail: vi.fn(),
    postReviewDecision: vi.fn(),
    postReviewReprocess: vi.fn(),
  };
});

const { getReviewDetail, postReviewDecision, postReviewReprocess } = await import(
  '../../api/admin'
);
const mockGetReviewDetail = vi.mocked(getReviewDetail);
const mockPostReviewDecision = vi.mocked(postReviewDecision);
const mockPostReviewReprocess = vi.mocked(postReviewReprocess);

const WORK_ITEM_ID = '11111111-1111-4111-8111-111111111111';
const RELEASE_ID = '55555555-5555-4555-8555-555555555555';

function makeDetail(overrides: Partial<ReviewDetail> = {}): ReviewDetail {
  return {
    workItem: {
      id: WORK_ITEM_ID,
      apiKey: '33333333-3333-4333-8333-333333333333',
      sourceLibraryId: null,
      targetLibraryId: null,
      sourcePath: '/library/incoming/Greatest Hits',
      currentPath: '/library/incoming/Greatest Hits',
      targetPath: null,
      sourceFingerprint: null,
      manifestHash: null,
      state: 'needs_review',
      stateReason: 'policy requires review',
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
    },
    releaseId: RELEASE_ID,
    manifest: {
      manifestHash: 'abc123',
      schemaVersion: 1,
      generatedAt: '2026-08-14T10:02:00Z',
      sourcePath: 'Greatest Hits',
      releaseTitle: 'Greatest Hits',
      releaseArtists: ['Example Artist'],
      tracks: [
        {
          index: 0,
          filePath: '01 Down Under.flac',
          title: 'Down Under',
          durationMs: 222_000,
          container: 'flac',
          codec: 'flac',
          mimeType: 'audio/flac',
          fingerprintStatus: 'captured',
        },
        {
          index: 1,
          filePath: '02 Overkill.flac',
          title: 'Overkill',
          durationMs: 223_000,
          container: 'flac',
          codec: 'flac',
          mimeType: 'audio/flac',
          fingerprintStatus: 'captured',
        },
      ],
      artwork: [
        {
          relativePath: 'cover.jpg',
          source: 'file',
          width: 500,
          height: 500,
          sizeBytes: 120_000,
          mimeType: 'image/jpeg',
          isEmbedded: false,
        },
      ],
      sidecars: [],
      findingCount: 0,
    },
    classification: {
      traits: ['greatest_hits'],
      confidence: 9200,
      classifiedAt: '2026-08-14T10:03:00Z',
      classifierVersion: '1.0.0',
      evidence: [
        {
          trait: 'greatest_hits',
          source: 'tag',
          kind: 'title_pattern',
          confidence: 9200,
          detail: 'Title contains "Greatest Hits"',
        },
      ],
    },
    catalogComparison: {
      known: 1,
      new: 1,
      alternate: 0,
      uncertain: 0,
      exactDuplicates: 0,
      knownAudioIdentities: 1,
      alternateRecordings: 0,
      newTracks: 1,
      uncertainTracks: 0,
      totalTracks: 2,
      evaluatedAt: '2026-08-14T10:04:00Z',
      evaluatorVersion: '1.0.0',
      tracks: [
        {
          trackId: '66666666-6666-4666-8666-666666666666',
          category: 'known_audio_identity',
          confidence: 9900,
          evidenceKind: 'audio_identity_match',
          detail: 'Same audio identity as catalog track',
        },
        {
          trackId: '77777777-7777-4777-8777-777777777777',
          category: 'new_track',
          confidence: 8000,
          evidenceKind: 'no_catalog_match',
          detail: 'No catalog match found',
        },
      ],
    },
    decision: {
      id: '88888888-8888-4888-8888-888888888888',
      releaseId: RELEASE_ID,
      ruleSetId: '99999999-9999-4999-8999-999999999999',
      ruleSetVersion: 3,
      disposition: 'review',
      completionState: 'pending',
      matchedRuleIds: ['greatest_hits_novelty'],
      evidenceSummary: 'Release has potentially novel tracks.',
      overrideReason: null,
      actor: { kind: 'system' },
      decidedAt: '2026-08-14T10:04:30Z',
      completedAt: null,
    },
    timeline: [
      {
        id: 1,
        workItemId: WORK_ITEM_ID,
        eventType: 'transitioned',
        fromState: 'validating',
        toState: 'needs_review',
        reasonCode: null,
        workerId: 'worker-1',
        correlationId: '44444444-4444-4444-8444-444444444444',
        createdAt: '2026-08-14T10:04:00Z',
        metadata: null,
      },
    ],
    ...overrides,
  };
}

function renderScreen() {
  return render(
    <MemoryRouter initialEntries={[`/processing/review/${WORK_ITEM_ID}`]}>
      <Routes>
        <Route path="/processing/review/:workItemId" element={<ReleaseReviewScreen />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('ReleaseReviewScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetReviewDetail.mockResolvedValue(makeDetail());
    mockPostReviewDecision.mockResolvedValue({
      workItem: makeDetail().workItem,
      decision: makeDetail().decision,
      pendingExecution: false,
    });
    mockPostReviewReprocess.mockResolvedValue(makeDetail().workItem);
  });

  it('renders the header, evidence, comparison, decision panel, policy, and timeline', async () => {
    renderScreen();
    // Hero card: display title from the manifest plus the state chip. The
    // breadcrumb repeats the title, so assert on the level-1 heading.
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Example Artist — Greatest Hits' }),
    ).toBeInTheDocument();
    expect(screen.getByText('needs_review')).toBeInTheDocument();
    // Classification & Evidence with confidence.
    expect(screen.getByText('Classification & Evidence')).toBeInTheDocument();
    expect(screen.getByText('Title contains "Greatest Hits"')).toBeInTheDocument();
    expect(screen.getByText('92%')).toBeInTheDocument();
    // Catalog comparison aggregates.
    expect(screen.getByText('Catalog Comparison')).toBeInTheDocument();
    expect(screen.getByText('Known Tracks')).toBeInTheDocument();
    expect(screen.getByText('New Tracks')).toBeInTheDocument();
    expect(screen.getByText('Uncertain')).toBeInTheDocument();
    // Track breakdown with novelty result pills and inline previews.
    expect(screen.getByText('Track Breakdown')).toBeInTheDocument();
    expect(screen.getByText('Down Under')).toBeInTheDocument();
    expect(screen.getByText('Overkill')).toBeInTheDocument();
    expect(screen.getByText('Known')).toBeInTheDocument();
    expect(screen.getByText('New')).toBeInTheDocument();
    const previews = screen.getAllByLabelText(/Preview track/);
    expect(previews).toHaveLength(2);
    // Decision panel: ordered actions, destructive last and distinct.
    const accept = screen.getByRole('button', { name: /^Accept Release/ });
    const quarantine = screen.getByRole('button', { name: /^Quarantine/ });
    const ignore = screen.getByRole('button', { name: /^Ignore/ });
    const deleteButton = screen.getByRole('button', { name: /^Delete/ });
    expect(accept).toBeInTheDocument();
    expect(deleteButton).toBeInTheDocument();
    const order = [accept, quarantine, ignore, deleteButton];
    for (let index = 1; index < order.length; index += 1) {
      const position = order[index - 1]?.compareDocumentPosition(order[index] as Node);
      expect(position && position & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    // Policy match with matched rule, recommendation, and reason.
    expect(screen.getByText('Policy Match')).toBeInTheDocument();
    expect(screen.getByText('greatest_hits_novelty')).toBeInTheDocument();
    expect(screen.getByText('Release has potentially novel tracks.')).toBeInTheDocument();
    // Review timeline.
    expect(screen.getByText('Review Timeline')).toBeInTheDocument();
    expect(screen.getByText('transitioned: validating → needs_review')).toBeInTheDocument();
  });

  it('posts accept with the audited reason and no override', async () => {
    renderScreen();
    fireEvent.click(await screen.findByRole('button', { name: /^Accept Release/ }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.change(screen.getByLabelText('Reason'), {
      target: { value: 'Verified against the catalog.' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Apply decision' }));
    await waitFor(() =>
      expect(mockPostReviewDecision).toHaveBeenCalledWith(WORK_ITEM_ID, {
        action: 'accept',
        reason: 'Verified against the catalog.',
      }),
    );
    expect(await screen.findByText('Decision recorded.')).toBeInTheDocument();
    expect(dialog).not.toBeInTheDocument();
  });

  it('requires a reason before a decision can be applied', async () => {
    renderScreen();
    fireEvent.click(await screen.findByRole('button', { name: /^Quarantine/ }));
    await screen.findByRole('dialog');
    expect(screen.getByRole('button', { name: 'Apply decision' })).toBeDisabled();
    expect(mockPostReviewDecision).not.toHaveBeenCalled();
  });

  it('delete shows impact and requires the typed confirmation before posting override', async () => {
    renderScreen();
    fireEvent.click(await screen.findByRole('button', { name: /^Delete/ }));
    const dialog = await screen.findByRole('dialog');
    // Impact from the manifest: 1 Release, 2 Tracks, 3 files (tracks + artwork).
    expect(within(dialog).getByText('Affected Releases')).toBeInTheDocument();
    expect(within(dialog).getByText('Affected Tracks')).toBeInTheDocument();
    expect(within(dialog).getByText('Affected files')).toBeInTheDocument();
    expect(within(dialog).getByText('greatest_hits_novelty')).toBeInTheDocument();
    expect(
      within(dialog).getByText(/quarantine or ignore keeps the content recoverable/),
    ).toBeInTheDocument();
    const confirmButton = screen.getByRole('button', { name: 'Delete Release' });
    expect(confirmButton).toBeDisabled();
    expect(screen.getByLabelText('Reason (optional)')).toHaveValue('');
    fireEvent.change(screen.getByLabelText(/Type DELETE to confirm/), {
      target: { value: 'DELETE' },
    });
    expect(confirmButton).toBeEnabled();
    fireEvent.click(confirmButton);
    await waitFor(() =>
      expect(mockPostReviewDecision).toHaveBeenCalledWith(WORK_ITEM_ID, {
        action: 'delete',
        reason: '',
        override: true,
      }),
    );
    expect(await screen.findByText('Release and managed media deleted.')).toBeInTheDocument();
  });

  it('surfaces a 409 illegal transition as an inline notification', async () => {
    mockPostReviewDecision.mockRejectedValue(
      new ApiError(409, 'invalid_state', "work item is 'cataloged' and not awaiting review", 'corr-1'),
    );
    renderScreen();
    fireEvent.click(await screen.findByRole('button', { name: /^Ignore/ }));
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'No action.' } });
    fireEvent.click(screen.getByRole('button', { name: 'Apply decision' }));
    expect(
      await screen.findByText(/could not be applied: work item is 'cataloged'/),
    ).toBeInTheDocument();
  });

  it('surfaces override_required with a localized note', async () => {
    mockPostReviewDecision.mockRejectedValue(
      new ApiError(400, 'override_required', 'delete requires explicit override', 'corr-2'),
    );
    renderScreen();
    fireEvent.click(await screen.findByRole('button', { name: /^Accept Release/ }));
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'Trying.' } });
    fireEvent.click(screen.getByRole('button', { name: 'Apply decision' }));
    expect(
      await screen.findByText(/Delete requires the typed confirmation/),
    ).toBeInTheDocument();
  });

  it('shows a fallback note when the preview stream is unavailable (404)', async () => {
    renderScreen();
    const preview = (await screen.findAllByLabelText(/Preview track/))[0];
    expect(preview).toBeDefined();
    fireEvent.error(preview as Element);
    expect(
      await screen.findByText('Audio preview is not available for this track.'),
    ).toBeInTheDocument();
  });

  it('offers reprocess instead of decisions for a failed work item', async () => {
    const failed = makeDetail();
    failed.workItem = { ...failed.workItem, state: 'failed' };
    mockGetReviewDetail.mockResolvedValue(failed);
    renderScreen();
    fireEvent.click(await screen.findByRole('button', { name: 'Reprocess' }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Reprocess' }));
    await waitFor(() =>
      expect(mockPostReviewReprocess).toHaveBeenCalledWith(WORK_ITEM_ID, undefined),
    );
    expect(
      await screen.findByText('The work item was returned to the processing queue.'),
    ).toBeInTheDocument();
  });

  it('renders an error notification with retry when the detail load fails', async () => {
    mockGetReviewDetail.mockRejectedValue(new ApiError(404, 'not_found', 'work item not found', null));
    renderScreen();
    expect(
      await screen.findByText('The review detail could not be loaded'),
    ).toBeInTheDocument();
    mockGetReviewDetail.mockResolvedValue(makeDetail());
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Example Artist — Greatest Hits' }),
    ).toBeInTheDocument();
  });
});

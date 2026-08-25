import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { MetadataTransformPanel } from './MetadataTransformPanel';

vi.mock('../../api/admin', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../api/admin')>();
  return {
    ...original,
    applyReleaseMetadataTransform: vi.fn(),
    listReleaseMetadataAudits: vi.fn(),
    previewReleaseMetadataTransform: vi.fn(),
    revertReleaseMetadataTransform: vi.fn(),
  };
});

const admin = await import('../../api/admin');
const mockApply = vi.mocked(admin.applyReleaseMetadataTransform);
const mockList = vi.mocked(admin.listReleaseMetadataAudits);
const mockPreview = vi.mocked(admin.previewReleaseMetadataTransform);

describe('MetadataTransformPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockList.mockResolvedValue([]);
    mockApply.mockResolvedValue({
      auditId: '11111111-1111-4111-8111-111111111111',
      status: 'success',
      previewHash: 'preview',
      affectedFiles: 1,
      deltas: [],
      recovery: {},
    });
  });

  it('previews changes and requires confirmation before applying', async () => {
    mockPreview.mockResolvedValue({
      previewHash: 'preview',
      operation: 'remove_featuring_artists',
      affectedFiles: 1,
      deltas: [{ file: 'track.mp3', field: 'title', old: 'Title feat. Guest', new: 'Title' }],
      validationErrors: [],
    });
    render(<MetadataTransformPanel releaseId="release-id" refresh={vi.fn()} />);

    fireEvent.click(screen.getByRole('button', { name: 'Preview changes' }));
    await screen.findByText('1 file(s) will be changed.');
    expect(mockPreview).toHaveBeenCalledWith('release-id', 'remove_featuring_artists', {});
    expect(mockApply).not.toHaveBeenCalled();

    const applyTrigger = screen
      .getAllByRole('button', { name: 'Apply changes' })
      .find((button) => !button.closest('[role="dialog"]'));
    fireEvent.click(applyTrigger as HTMLElement);
    const dialog = await screen.findByRole('dialog', {
      name: 'Apply metadata transformation?',
    });
    expect(mockApply).not.toHaveBeenCalled();
    const submit = Array.from(dialog.querySelectorAll('button.cds--btn--primary'))
      .find((button) => !button.classList.contains('cds--modal-close'));
    fireEvent.click(submit as HTMLElement);
    await waitFor(() => expect(mockApply).toHaveBeenCalledWith(
      'release-id',
      'remove_featuring_artists',
      'preview',
      {},
    ));
  });
});

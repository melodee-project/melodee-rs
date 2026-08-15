import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { DestructiveActionDialog } from './DestructiveActionDialog';

function renderDialog(extra: Partial<Parameters<typeof DestructiveActionDialog>[0]> = {}) {
  const onConfirm = vi.fn();
  const onClose = vi.fn();
  render(
    <DestructiveActionDialog
      open
      title="Delete Release"
      actionLabel="Delete permanently"
      affectedReleases={3}
      affectedTracks={41}
      affectedFiles={44}
      storageImpactBytes={1_500_000_000}
      matchedRule="duplicate-release"
      onClose={onClose}
      onConfirm={onConfirm}
      {...extra}
    />,
  );
  return { onConfirm, onClose };
}

describe('DestructiveActionDialog', () => {
  it('shows affected counts, storage impact, matched rule, and the alternative note', () => {
    renderDialog();
    expect(screen.getByText('Affected Releases')).toBeInTheDocument();
    expect(screen.getByText('Affected Tracks')).toBeInTheDocument();
    expect(screen.getByText('Matched rule')).toBeInTheDocument();
    expect(screen.getByText('duplicate-release')).toBeInTheDocument();
    expect(screen.getByText(/quarantine or ignore/)).toBeInTheDocument();
  });

  it('requires a reason before the danger action enables', () => {
    const { onConfirm } = renderDialog();
    const dangerButton = screen.getByRole('button', { name: 'Delete permanently' });
    expect(dangerButton).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'duplicate of R2' } });
    expect(dangerButton).toBeEnabled();
    fireEvent.click(dangerButton);
    expect(onConfirm).toHaveBeenCalledWith('duplicate of R2');
  });

  it('delete-class actions require the typed confirmation phrase', () => {
    const { onConfirm } = renderDialog({ deleteClass: true });
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'cleanup' } });
    const dangerButton = screen.getByRole('button', { name: 'Delete permanently' });
    expect(dangerButton).toBeDisabled();
    const confirmInput = screen.getByLabelText('Type DELETE to confirm.');
    fireEvent.change(confirmInput, { target: { value: 'DELET' } });
    expect(dangerButton).toBeDisabled();
    fireEvent.change(confirmInput, { target: { value: 'DELETE' } });
    expect(dangerButton).toBeEnabled();
    fireEvent.click(dangerButton);
    expect(onConfirm).toHaveBeenCalledWith('cleanup');
  });
});

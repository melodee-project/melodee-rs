import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { DecisionPanel, type DecisionChoice } from './DecisionPanel';

const CHOICES: DecisionChoice[] = [
  { id: 'delete', label: 'Delete files', kind: 'destructive' },
  { id: 'accept', label: 'Accept match', kind: 'recommended' },
  { id: 'quarantine', label: 'Quarantine', kind: 'safe' },
];

describe('DecisionPanel', () => {
  it('orders choices recommended first and destructive last', () => {
    render(<DecisionPanel choices={CHOICES} onSelect={() => undefined} />);
    const buttons = screen.getAllByRole('button').map((button) => button.textContent);
    const accept = buttons.findIndex((text) => text?.includes('Accept match'));
    const quarantine = buttons.findIndex((text) => text?.includes('Quarantine'));
    const remove = buttons.findIndex((text) => text?.includes('Delete files'));
    expect(accept).toBeGreaterThanOrEqual(0);
    expect(accept).toBeLessThan(quarantine);
    expect(quarantine).toBeLessThan(remove);
  });

  it('never renders destructive choices visually equal to safe ones', () => {
    render(<DecisionPanel choices={CHOICES} onSelect={() => undefined} />);
    const destructive = screen.getByRole('button', { name: /^Delete files/ });
    const recommended = screen.getByRole('button', { name: /^Accept match/ });
    expect(destructive.className).toContain('melodee-action--danger');
    expect(recommended.className).not.toContain('melodee-action--danger');
    expect(recommended.className).toContain('melodee-action--primary');
  });

  it('reports selection through onSelect', () => {
    const onSelect = vi.fn();
    render(<DecisionPanel choices={CHOICES} onSelect={onSelect} />);
    fireEvent.click(screen.getByRole('button', { name: /^Quarantine/ }));
    expect(onSelect).toHaveBeenCalledWith('quarantine');
  });
});

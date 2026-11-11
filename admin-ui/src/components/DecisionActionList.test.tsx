import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { DecisionActionList } from './DecisionActionList';

describe('DecisionActionList', () => {
  it('renders ordered actions with reasons, chips, and a destructive variant', () => {
    const onSelect = vi.fn();
    const { container } = render(
      <DecisionActionList
        onSelect={onSelect}
        actions={[
          {
            id: 'accept',
            label: 'Accept Release',
            kind: 'primary',
            reason: 'Add to the library and process',
            chip: { kind: 'success', label: 'Recommended' },
          },
          { id: 'quarantine', label: 'Quarantine', reason: 'Hold out of the catalog' },
          { id: 'delete', label: 'Delete', kind: 'danger', reason: 'Remove source files' },
        ]}
      />,
    );
    expect(screen.getByText('Add to the library and process')).toBeInTheDocument();
    expect(screen.getByText('Recommended')).toBeInTheDocument();
    // Accessible name stays exactly the operator label; reason/chip become the
    // accessible description.
    // Accessible name carries the visible text (label, chip, reason) so a
    // screen-reader user hears what a sighted operator sees (WCAG 2.5.3).
    expect(
      screen.getByRole('button', {
        name: /^Accept Release.*Add to the library and process$/,
      }),
    ).toBeInTheDocument();
    expect(container.querySelector('.melodee-action--primary')).not.toBeNull();
    expect(container.querySelector('.melodee-action--danger')).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /^Delete/ }));
    expect(onSelect).toHaveBeenCalledWith('delete');
  });

  it('disables an action without hiding its reason', () => {
    render(
      <DecisionActionList
        onSelect={() => undefined}
        actions={[{ id: 'x', label: 'Accept', disabled: true, reason: 'Awaiting import' }]}
      />,
    );
    expect(screen.getByRole('button', { name: /^Accept/ })).toBeDisabled();
    expect(screen.getByText('Awaiting import')).toBeInTheDocument();
  });
});

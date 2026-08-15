import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';

import { ErrorState } from './ErrorState';

/**
 * The localized error states (PRD-WEB-002 `/Error` replacement): not-found,
 * forbidden, and unexpected are distinct; not-found/unexpected link back to
 * the overview, forbidden does not (every route is guarded — it would loop).
 */

function renderState(kind: 'notFound' | 'forbidden' | 'unexpected') {
  return render(
    <MemoryRouter>
      <ErrorState kind={kind} />
    </MemoryRouter>,
  );
}

describe('ErrorState', () => {
  it('renders the localized not-found state with a link back to the overview', () => {
    renderState('notFound');

    expect(screen.getAllByText('Page not found').length).toBeGreaterThan(0);
    expect(screen.getAllByText(/does not exist or may have moved/).length).toBeGreaterThan(0);
    const back = screen.getByRole('link', { name: 'Back to overview' });
    expect(back).toHaveAttribute('href', '/');
  });

  it('renders access denied distinctly and without a misleading back link', () => {
    renderState('forbidden');

    expect(
      screen.getAllByText(/does not have administrator rights/).length,
    ).toBeGreaterThan(0);
    expect(screen.queryByText('Page not found')).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Back to overview' })).not.toBeInTheDocument();
  });

  it('renders the unexpected-error state with a recovery link', () => {
    renderState('unexpected');

    expect(screen.getAllByText('Something went wrong').length).toBeGreaterThan(0);
    expect(screen.getByRole('link', { name: 'Back to overview' })).toBeInTheDocument();
  });
});

import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { RouteErrorBoundary } from './RouteErrorBoundary';

/**
 * The render-error boundary degrades a throwing screen to the themed
 * unexpected-error state and recovers when navigation changes resetKey.
 */

function Thrower(): never {
  throw new Error('boom');
}

describe('RouteErrorBoundary', () => {
  beforeEach(() => {
    // Expected render errors would otherwise fail the test run's console.
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
  });

  it('renders children when nothing throws', () => {
    render(
      <MemoryRouter>
        <RouteErrorBoundary resetKey="/">
          <p>healthy screen</p>
        </RouteErrorBoundary>
      </MemoryRouter>,
    );
    expect(screen.getByText('healthy screen')).toBeInTheDocument();
  });

  it('renders the localized unexpected-error state when a child throws', () => {
    render(
      <MemoryRouter>
        <RouteErrorBoundary resetKey="/">
          <Thrower />
        </RouteErrorBoundary>
      </MemoryRouter>,
    );
    expect(screen.getAllByText('Something went wrong').length).toBeGreaterThan(0);
    expect(screen.getByRole('link', { name: 'Back to overview' })).toBeInTheDocument();
  });

  it('recovers when resetKey changes (navigation)', () => {
    const { rerender } = render(
      <MemoryRouter>
        <RouteErrorBoundary resetKey="/broken">
          <Thrower />
        </RouteErrorBoundary>
      </MemoryRouter>,
    );
    expect(screen.getAllByText('Something went wrong').length).toBeGreaterThan(0);

    rerender(
      <MemoryRouter>
        <RouteErrorBoundary resetKey="/">
          <p>healthy screen</p>
        </RouteErrorBoundary>
      </MemoryRouter>,
    );
    expect(screen.getByText('healthy screen')).toBeInTheDocument();
  });
});

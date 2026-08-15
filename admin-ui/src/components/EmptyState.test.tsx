import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { EmptyState } from './EmptyState';

describe('EmptyState', () => {
  it('renders title, description, and action in a status region', () => {
    render(
      <EmptyState
        title="Nothing here"
        description="Try changing filters"
        action={<button>Clear</button>}
      />,
    );
    const region = screen.getByRole('status');
    expect(region).toHaveTextContent('Nothing here');
    expect(region).toHaveTextContent('Try changing filters');
    expect(screen.getByRole('button', { name: 'Clear' })).toBeInTheDocument();
  });

  it('renders the error tone through the status semantics', () => {
    render(<EmptyState title="Access denied" tone="error" />);
    expect(screen.getByTitle('Failed, rejected, or danger')).toBeInTheDocument();
  });
});

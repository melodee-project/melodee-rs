import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { PageHeader } from './PageHeader';

describe('PageHeader', () => {
  it('renders title as a level-1 heading with subtitle and actions', () => {
    render(
      <PageHeader title="Processing" subtitle="Ingestion pipeline" actions={<button>Refresh</button>} />,
    );
    expect(screen.getByRole('heading', { level: 1, name: 'Processing' })).toBeInTheDocument();
    expect(screen.getByText('Ingestion pipeline')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeInTheDocument();
  });

  it('omits subtitle when not provided', () => {
    render(<PageHeader title="Overview" />);
    expect(screen.getByRole('heading', { name: 'Overview' })).toBeInTheDocument();
    expect(screen.queryByText('Ingestion pipeline')).not.toBeInTheDocument();
  });

  it('renders an optional breadcrumb line', () => {
    render(<PageHeader title="Release Review" breadcrumb={<a href="/review">Review Queue</a>} />);
    expect(screen.getByRole('link', { name: 'Review Queue' })).toBeInTheDocument();
  });
});

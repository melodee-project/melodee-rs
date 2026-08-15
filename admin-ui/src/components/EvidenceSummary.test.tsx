import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { EvidenceSummary } from './EvidenceSummary';

describe('EvidenceSummary', () => {
  it('renders the evidence section with all items', () => {
    render(
      <EvidenceSummary
        items={[
          { source: 'tags', observation: 'A' },
          { source: 'path', observation: 'B' },
        ]}
      />,
    );
    expect(screen.getByRole('heading', { name: 'Evidence' })).toBeInTheDocument();
    expect(screen.getByText('A')).toBeInTheDocument();
    expect(screen.getByText('B')).toBeInTheDocument();
  });

  it('renders an empty state when there is no evidence', () => {
    render(<EvidenceSummary items={[]} />);
    expect(screen.getByText('No data available')).toBeInTheDocument();
  });
});

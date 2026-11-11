import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { EvidenceTileGrid } from './EvidenceTileGrid';

describe('EvidenceTileGrid', () => {
  it('renders one labeled tile per signal with its toned value', () => {
    const { container } = render(
      <EvidenceTileGrid
        tiles={[
          { label: 'Alias match', value: 2 },
          { label: 'Year match', value: 1, tone: 'success' },
          { label: 'Negative evidence', value: 1, tone: 'error' },
        ]}
      />,
    );
    expect(screen.getByText('Alias match')).toBeInTheDocument();
    expect(screen.getByText('Negative evidence')).toBeInTheDocument();
    expect(container.querySelectorAll('.melodee-tile')).toHaveLength(3);
    expect(container.querySelector('.melodee-tile--error')).not.toBeNull();
    expect(container.querySelector('.melodee-tile--success')).not.toBeNull();
  });
});

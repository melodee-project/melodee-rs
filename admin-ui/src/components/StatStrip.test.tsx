import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';

import { StatStrip } from './StatStrip';

describe('StatStrip', () => {
  it('renders each stat value and label', () => {
    render(
      <MemoryRouter>
        <StatStrip
          items={[
            { label: 'Releases', value: 12 },
            { label: 'Tracks', value: 128 },
          ]}
        />
      </MemoryRouter>,
    );
    expect(screen.getByText('12')).toBeInTheDocument();
    expect(screen.getByText('Releases')).toBeInTheDocument();
    expect(screen.getByText('128')).toBeInTheDocument();
  });

  it('links a stat when a destination is given', () => {
    render(
      <MemoryRouter>
        <StatStrip items={[{ label: 'Aliases', value: 3, to: '/catalog/artists/1?tab=identity' }]} />
      </MemoryRouter>,
    );
    expect(screen.getByRole('link', { name: /Aliases/ })).toHaveAttribute(
      'href',
      '/catalog/artists/1?tab=identity',
    );
  });
});

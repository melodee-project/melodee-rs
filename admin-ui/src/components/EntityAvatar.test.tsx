import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { EntityAvatar } from './EntityAvatar';

describe('EntityAvatar', () => {
  it('renders artwork with meaningful alt text', () => {
    render(<EntityAvatar name="Radiohead" imageUrl="/images/artist_1/80" />);
    expect(screen.getByRole('img', { name: 'Radiohead' })).toBeInTheDocument();
  });

  it('marks artwork decorative when asked', () => {
    const { container } = render(
      <EntityAvatar name="Radiohead" imageUrl="/images/artist_1/80" decorative />,
    );
    const img = container.querySelector('img');
    expect(img).toHaveAttribute('alt', '');
  });

  it('renders an accessible initial fallback without artwork', () => {
    render(<EntityAvatar name="Björk" />);
    expect(screen.getByRole('img', { name: 'Björk' })).toHaveTextContent('B');
  });
});

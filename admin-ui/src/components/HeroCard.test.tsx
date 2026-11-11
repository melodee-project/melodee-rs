import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { HeroCard } from './HeroCard';

describe('HeroCard', () => {
  it('renders media, title, subtitle, chips, facts, and actions', () => {
    render(
      <HeroCard
        media={<span data-testid="media">art</span>}
        title="Business as Usual"
        subtitle="Men at Work · 1981"
        chips={<span>Cataloged</span>}
        facts={[{ label: 'Tracks', value: 10 }, { label: 'Duration', value: '38:42' }]}
        actions={<button>Play sample</button>}
      />,
    );
    expect(screen.getByRole('heading', { level: 1, name: 'Business as Usual' })).toBeInTheDocument();
    expect(screen.getByText('Men at Work · 1981')).toBeInTheDocument();
    expect(screen.getByText('Cataloged')).toBeInTheDocument();
    expect(screen.getByText('Tracks')).toBeInTheDocument();
    expect(screen.getByText('38:42')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Play sample' })).toBeInTheDocument();
    expect(screen.getByTestId('media')).toBeInTheDocument();
  });

  it('omits optional regions when not provided', () => {
    const { container } = render(<HeroCard title="The Nomads" />);
    expect(container.querySelector('.melodee-hero__chips')).toBeNull();
    expect(container.querySelector('.melodee-hero__facts')).toBeNull();
    expect(container.querySelector('.melodee-hero__actions')).toBeNull();
  });
});

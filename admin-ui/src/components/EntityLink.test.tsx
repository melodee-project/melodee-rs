import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';

import { ArtistLink, ReleaseLink, TrackLink } from './EntityLink';

describe('entity links', () => {
  it('ArtistLink points at the catalog artist route', () => {
    render(
      <MemoryRouter>
        <ArtistLink id="abc 123" name="Radiohead" />
      </MemoryRouter>,
    );
    const link = screen.getByRole('link', { name: 'Radiohead' });
    expect(link).toHaveAttribute('href', '/catalog/artists/abc%20123');
  });

  it('ReleaseLink and TrackLink use Release/Track routes, never album/song', () => {
    render(
      <MemoryRouter>
        <ReleaseLink id="r1" name="OK Computer" />
        <TrackLink id="t1" name="Airbag" />
      </MemoryRouter>,
    );
    expect(screen.getByRole('link', { name: 'OK Computer' })).toHaveAttribute(
      'href',
      '/catalog/releases/r1',
    );
    expect(screen.getByRole('link', { name: 'Airbag' })).toHaveAttribute(
      'href',
      '/catalog/tracks/t1',
    );
  });
});

import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { AudioPreview } from './AudioPreview';

describe('AudioPreview', () => {
  it('renders a labeled native transport without autoplay', () => {
    const { container } = render(<AudioPreview src="/audio/track.mp3" />);
    const audio = container.querySelector('audio');
    expect(audio).not.toBeNull();
    expect(audio).toHaveAttribute('controls');
    expect(audio).toHaveAttribute('preload', 'none');
    expect(audio).not.toHaveAttribute('autoplay');
    expect(audio).toHaveAttribute('aria-label', 'Audio preview');
  });

  it('accepts an explicit label', () => {
    const { container } = render(<AudioPreview src="/x.mp3" label="Preview: Airbag" />);
    expect(container.querySelector('audio')).toHaveAttribute('aria-label', 'Preview: Airbag');
  });
});

import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { USER_AVATAR_CHANGED_EVENT } from '../api/client';
import { SessionAvatar } from './SessionAvatar';

describe('SessionAvatar', () => {
  it('requests the signed-in account picture even with no stored username', () => {
    render(<SessionAvatar username={null} />);
    const image = screen.getByAltText('');
    expect(image.getAttribute('src')).toMatch(/^\/api\/admin\/v1\/session\/avatar\?v=\d+$/);
    expect(image.closest('[aria-hidden]')).toHaveAttribute('aria-hidden', 'true');
  });

  it('falls back on image failure and retries after an avatar change', () => {
    render(<SessionAvatar username="steven" />);
    const image = screen.getByAltText('');
    const firstSrc = image.getAttribute('src');
    fireEvent.error(image);
    expect(screen.getByText('S')).toBeVisible();
    fireEvent(window, new Event(USER_AVATAR_CHANGED_EVENT));
    expect(screen.getByAltText('').getAttribute('src')).not.toBe(firstSrc);
  });
});

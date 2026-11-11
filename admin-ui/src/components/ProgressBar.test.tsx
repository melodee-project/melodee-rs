import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { ProgressBar } from './ProgressBar';

describe('ProgressBar', () => {
  it('exposes a labeled progressbar with numeric value and stage text', () => {
    render(<ProgressBar value={72} label="Business as Usual" stage="Normalizing" />);
    const bar = screen.getByRole('progressbar', { name: 'Business as Usual' });
    expect(bar).toHaveAttribute('aria-valuenow', '72');
    expect(bar).toHaveAttribute('aria-valuemin', '0');
    expect(bar).toHaveAttribute('aria-valuemax', '100');
    expect(bar.getAttribute('aria-valuetext')).toContain('Normalizing');
    expect(bar.getAttribute('aria-valuetext')).toContain('72');
  });

  it('clamps out-of-range values and renders the bar width', () => {
    const { container } = render(<ProgressBar value={140} label="Overflow" />);
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100');
    const fill = container.querySelector<HTMLElement>('.melodee-progress__bar');
    expect(fill?.style.inlineSize).toBe('100%');

    const { container: negative } = render(<ProgressBar value={-5} label="Underflow" />);
    expect(negative.querySelector<HTMLElement>('.melodee-progress__bar')?.style.inlineSize).toBe(
      '0%',
    );
  });
});

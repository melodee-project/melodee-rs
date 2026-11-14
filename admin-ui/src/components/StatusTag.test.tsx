import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { StatusChip, StatusTag, type StatusKind } from './StatusTag';

const KINDS: StatusKind[] = ['success', 'info', 'warning', 'error', 'pending', 'neutral'];

describe('StatusChip', () => {
  it.each(KINDS)('renders %s with a label, icon, and description (never color-only)', (kind) => {
    const { container } = render(<StatusChip kind={kind} />);
    const chip = container.querySelector(`.melodee-chip--${kind}`);
    expect(chip).not.toBeNull();
    // Icon present (svg), label text present, description available both as
    // tooltip and as assistive text.
    expect(container.querySelector('svg')).not.toBeNull();
    expect(chip?.textContent?.trim().length ?? 0).toBeGreaterThan(0);
    expect(container.querySelector('[title]')?.getAttribute('title')).toBeTruthy();
    expect(container.querySelector('.cds--visually-hidden')?.textContent?.trim()).toBeTruthy();
    expect(chip?.getAttribute('aria-label')).toBeTruthy();
  });

  it('uses the localized default labels', () => {
    render(<StatusChip kind="error" />);
    expect(screen.getByText('Failed')).toBeInTheDocument();
  });

  it('accepts label/description overrides', () => {
    render(<StatusChip kind="success" label="Cataloged" description="Release is in the catalog" />);
    expect(screen.getByText('Cataloged')).toBeInTheDocument();
    expect(screen.getByTitle('Release is in the catalog')).toBeInTheDocument();
  });

  it('exposes StatusTag as the historical alias of the same chip', () => {
    expect(StatusTag).toBe(StatusChip);
  });
});

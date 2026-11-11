import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { FilterPills } from './FilterPills';

const ITEMS = [
  { key: '', label: 'All', count: '1,248' },
  { key: 'processing', label: 'Processing', count: '7' },
  { key: 'review', label: 'Review', count: '23' },
];

describe('FilterPills', () => {
  it('renders a labeled group with pressed state reflecting the value', () => {
    render(
      <FilterPills items={ITEMS} value="review" onChange={() => undefined} ariaLabel="Filter by state" />,
    );
    expect(screen.getByRole('group', { name: 'Filter by state' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Review/ })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: /All/ })).toHaveAttribute('aria-pressed', 'false');
  });

  it('reports the selected key, and null for the all pill', () => {
    const onChange = vi.fn();
    render(<FilterPills items={ITEMS} value={null} onChange={onChange} ariaLabel="Filter" />);
    fireEvent.click(screen.getByRole('button', { name: /Processing/ }));
    expect(onChange).toHaveBeenCalledWith('processing');
    fireEvent.click(screen.getByRole('button', { name: /All/ }));
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it('moves focus with arrow keys, Home, and End', () => {
    render(<FilterPills items={ITEMS} value={null} onChange={() => undefined} ariaLabel="Filter" />);
    const all = screen.getByRole('button', { name: /All/ });
    const processing = screen.getByRole('button', { name: /Processing/ });
    const review = screen.getByRole('button', { name: /Review/ });

    all.focus();
    fireEvent.keyDown(all, { key: 'ArrowRight' });
    expect(processing).toHaveFocus();
    fireEvent.keyDown(processing, { key: 'End' });
    expect(review).toHaveFocus();
    fireEvent.keyDown(review, { key: 'ArrowRight' });
    expect(all).toHaveFocus();
    fireEvent.keyDown(all, { key: 'ArrowLeft' });
    expect(review).toHaveFocus();
    fireEvent.keyDown(review, { key: 'Home' });
    expect(all).toHaveFocus();
  });
});

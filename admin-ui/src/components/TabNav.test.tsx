import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { TabNav } from './TabNav';

const TABS = [
  { key: 'overview', label: 'Overview' },
  { key: 'tracks', label: 'Tracks' },
  { key: 'files', label: 'Files' },
];

describe('TabNav', () => {
  it('renders a tablist with the active tab selected and only it tabbable', () => {
    render(<TabNav tabs={TABS} value="tracks" onChange={() => undefined} ariaLabel="Release sections" />);
    expect(screen.getByRole('tablist', { name: 'Release sections' })).toBeInTheDocument();
    const active = screen.getByRole('tab', { name: 'Tracks' });
    expect(active).toHaveAttribute('aria-selected', 'true');
    expect(active).toHaveAttribute('tabindex', '0');
    expect(screen.getByRole('tab', { name: 'Overview' })).toHaveAttribute('tabindex', '-1');
  });

  it('reports selection and supports arrow/Home/End focus', () => {
    const onChange = vi.fn();
    render(<TabNav tabs={TABS} value="overview" onChange={onChange} ariaLabel="Sections" />);
    const overview = screen.getByRole('tab', { name: 'Overview' });
    const tracks = screen.getByRole('tab', { name: 'Tracks' });
    const files = screen.getByRole('tab', { name: 'Files' });

    fireEvent.click(tracks);
    expect(onChange).toHaveBeenCalledWith('tracks');

    overview.focus();
    fireEvent.keyDown(overview, { key: 'ArrowRight' });
    expect(tracks).toHaveFocus();
    fireEvent.keyDown(tracks, { key: 'End' });
    expect(files).toHaveFocus();
    fireEvent.keyDown(files, { key: 'ArrowRight' });
    expect(overview).toHaveFocus();
    fireEvent.keyDown(overview, { key: 'ArrowLeft' });
    expect(files).toHaveFocus();
    fireEvent.keyDown(files, { key: 'Home' });
    expect(overview).toHaveFocus();
  });
});

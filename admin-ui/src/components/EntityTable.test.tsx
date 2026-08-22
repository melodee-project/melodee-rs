import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { describe, expect, it } from 'vitest';

import { EntityTable, type EntityColumn } from './EntityTable';

const COLUMNS: EntityColumn[] = [
  { key: 'name', header: 'Name', sortable: true },
  { key: 'state', header: 'State', sortable: true },
  { key: 'notes', header: 'Notes' },
];

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="search-params">{location.search}</output>;
}

function renderTable(initialEntry = '/processing') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <EntityTable
        caption="Work items"
        columns={COLUMNS}
        rows={[{ id: '1', cells: { name: 'Release A', state: 'ready', notes: '—' } }]}
        totalRows={137}
      />
      <LocationProbe />
    </MemoryRouter>,
  );
}

describe('EntityTable', () => {
  it('renders rows with an accessible caption', () => {
    renderTable();
    expect(screen.getByRole('table', { name: 'Work items' })).toBeInTheDocument();
    expect(screen.getByText('Release A')).toBeInTheDocument();
  });

  it('reflects sort changes in the URL (shareable state)', () => {
    renderTable();
    fireEvent.click(screen.getByRole('button', { name: 'Sort by Name' }));
    expect(screen.getByTestId('search-params')).toHaveTextContent('sort=name%3Aasc');
  });

  it('renders total-count pagination, not row-count pagination', () => {
    renderTable();
    // 137 server-side rows over one rendered row proves no client-side
    // slicing assumption leaks into the pager.
    expect(screen.getByText(/137/)).toBeInTheDocument();
  });

  it('shows the empty state when the server page is empty', () => {
    render(
      <MemoryRouter>
        <EntityTable caption="Work items" columns={COLUMNS} rows={[]} totalRows={0} />
      </MemoryRouter>,
    );
    expect(screen.getByText('No records to display')).toBeInTheDocument();
  });
});

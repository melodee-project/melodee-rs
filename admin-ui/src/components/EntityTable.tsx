import {
  Checkbox,
  Pagination,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
  TableToolbar,
  TableToolbarContent,
} from '@carbon/react';
import type { KeyboardEvent, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router-dom';

import { EmptyState } from './EmptyState';

/**
 * Catalog-scale table (UI-SPEC §6, PRD-UI-010): server-side sort and page
 * only — the parent fetches rows for the current page/sort; meaningful state
 * is reflected in the URL query so views are shareable (UI-SPEC §7).
 *
 * Sorting is driven by header clicks that update `sort=key:asc|desc` in the
 * URL; paging via Carbon Pagination wired to `page`/`pageSize` params.
 */
export interface EntityColumn {
  /** Column key; also the sort whitelist key when `sortable`. */
  key: string;
  header: string;
  sortable?: boolean;
}

export interface EntityRow {
  id: string;
  cells: Record<string, ReactNode>;
}

/** Page-scoped bulk selection state owned by the parent screen. */
export interface EntityTableSelection {
  selectedIds: readonly string[];
  onChange: (ids: string[]) => void;
}

export interface EntityTableProps {
  caption: string;
  columns: EntityColumn[];
  rows: EntityRow[];
  /** Total rows server-side for the current filter (pagination honesty). */
  totalRows: number;
  toolbar?: ReactNode;
  compact?: boolean;
  emptyTitle?: string;
  emptyDescription?: string;
  /**
   * Single-select row activation (UI-SPEC §7): rows become focusable, Enter
   * activates, ArrowUp/ArrowDown move focus between rows, click activates.
   */
  activeRowId?: string | null;
  onRowActivate?: (id: string) => void;
  /** Bulk selection checkboxes; the parent swaps the toolbar when non-empty. */
  selection?: EntityTableSelection;
}

function parseSort(raw: string | null, columns: EntityColumn[]): { key: string; direction: 'asc' | 'desc' } | null {
  if (!raw) {
    return null;
  }
  const [key, direction] = raw.split(':');
  if (
    !key ||
    (direction !== 'asc' && direction !== 'desc') ||
    !columns.some((column) => column.key === key && column.sortable)
  ) {
    return null;
  }
  return { key, direction };
}

export function EntityTable({
  caption,
  columns,
  rows,
  totalRows,
  toolbar,
  compact = false,
  emptyTitle,
  emptyDescription,
  activeRowId,
  onRowActivate,
  selection,
}: EntityTableProps) {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();

  const page = Math.max(1, Number.parseInt(searchParams.get('page') ?? '1', 10) || 1);
  const pageSize = Math.max(1, Number.parseInt(searchParams.get('pageSize') ?? '50', 10) || 50);
  const sort = parseSort(searchParams.get('sort'), columns);

  const updateParams = (updates: Record<string, string | null>) => {
    const next = new URLSearchParams(searchParams);
    for (const [key, value] of Object.entries(updates)) {
      if (value === null) {
        next.delete(key);
      } else {
        next.set(key, value);
      }
    }
    setSearchParams(next, { replace: true });
  };

  const onSort = (key: string) => {
    const direction = sort?.key === key && sort.direction === 'asc' ? 'desc' : 'asc';
    updateParams({ sort: `${key}:${direction}`, page: '1' });
  };

  const selected = new Set(selection?.selectedIds ?? []);
  const pageIds = rows.map((row) => row.id);
  const allSelected = pageIds.length > 0 && pageIds.every((id) => selected.has(id));
  const someSelected = pageIds.some((id) => selected.has(id));

  const toggleAll = () => {
    if (!selection) {
      return;
    }
    const next = new Set(selection.selectedIds);
    if (allSelected) {
      for (const id of pageIds) {
        next.delete(id);
      }
    } else {
      for (const id of pageIds) {
        next.add(id);
      }
    }
    selection.onChange([...next]);
  };

  const toggleRow = (id: string, checked: boolean) => {
    if (!selection) {
      return;
    }
    const next = new Set(selection.selectedIds);
    if (checked) {
      next.add(id);
    } else {
      next.delete(id);
    }
    selection.onChange([...next]);
  };

  const onRowKeyDown = (event: KeyboardEvent<HTMLTableRowElement>, id: string) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      onRowActivate?.(id);
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const sibling =
        event.key === 'ArrowDown'
          ? event.currentTarget.nextElementSibling
          : event.currentTarget.previousElementSibling;
      if (sibling instanceof HTMLElement) {
        sibling.focus();
      }
    }
  };

  const sortedAnnouncement = sort
    ? `${columns.find((column) => column.key === sort.key)?.header ?? sort.key} ${
        sort.direction === 'asc' ? t('table.sortedAscending') : t('table.sortedDescending')
      }`
    : t('table.notSorted');

  return (
    // The visible card heading is the surrounding Surface's title, so the
    // container carries the caption as its accessible name only.
    <TableContainer className="melodee-table-card" aria-label={caption}>
      {toolbar ? (
        <TableToolbar aria-label={caption}>
          <TableToolbarContent>{toolbar}</TableToolbarContent>
        </TableToolbar>
      ) : null}
      {/* Sorted-column announcement (UI-SPEC §9). */}
      <span role="status" className="cds--visually-hidden">
        {sortedAnnouncement}
      </span>
      {/*
        UI-SPEC §4: queue/triage tables switch between "compact" and Carbon's
        default density. Carbon's `sm` and `md` sizes render identically for a
        plain table (both use 0.4375/0.375rem cell padding), so `sm` made the
        density control a no-op; `xs` is Carbon's dense table and matches the
        2rem `--melodee-density-compact` token.
      */}
      <Table size={compact ? 'xs' : 'md'} className="melodee-table-skin" aria-label={caption}>
        <TableHead>
          <TableRow>
            {selection ? (
              <TableHeader>
                <Checkbox
                  id={`${caption}-select-all`}
                  labelText={t('table.selectAllLabel')}
                  hideLabel
                  checked={allSelected}
                  indeterminate={someSelected && !allSelected}
                  onChange={(_event, { checked }) => {
                    void checked;
                    toggleAll();
                  }}
                />
              </TableHeader>
            ) : null}
            {columns.map((column) => {
              const isSorted = sort?.key === column.key;
              return (
                <TableHeader
                  key={column.key}
                  isSortable={column.sortable ?? false}
                  isSortHeader={isSorted}
                  sortDirection={
                    isSorted ? (sort.direction === 'asc' ? 'ASC' : 'DESC') : 'NONE'
                  }
                  onClick={column.sortable ? () => onSort(column.key) : undefined}
                  aria-label={
                    column.sortable ? t('table.sortBy', { column: column.header }) : undefined
                  }
                >
                  {column.header}
                </TableHeader>
              );
            })}
          </TableRow>
        </TableHead>
        <TableBody>
          {rows.map((row) => {
            const activatable = Boolean(onRowActivate);
            return (
              <TableRow
                key={row.id}
                aria-selected={activatable ? activeRowId === row.id : undefined}
                tabIndex={activatable ? 0 : undefined}
                className={
                  activatable
                    ? `melodee-table__row--activatable${
                        activeRowId === row.id ? ' melodee-table__row--active' : ''
                      }`
                    : undefined
                }
                onClick={activatable ? () => onRowActivate?.(row.id) : undefined}
                onKeyDown={activatable ? (event) => onRowKeyDown(event, row.id) : undefined}
              >
                {selection ? (
                  <TableCell onClick={(event) => event.stopPropagation()}>
                    <Checkbox
                      id={`${caption}-select-${row.id}`}
                      labelText={t('table.selectionLabel')}
                      hideLabel
                      checked={selected.has(row.id)}
                      onChange={(_event, { checked }) => toggleRow(row.id, checked)}
                    />
                  </TableCell>
                ) : null}
                {columns.map((column) => (
                  <TableCell key={column.key}>{row.cells[column.key] ?? null}</TableCell>
                ))}
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      {rows.length === 0 ? (
        <EmptyState
          title={emptyTitle ?? t('empty.noRecords')}
          description={emptyDescription ?? t('empty.noResults')}
          compact
        />
      ) : null}
      <Pagination
        page={page}
        pageSize={pageSize}
        totalItems={totalRows}
        pageSizes={[25, 50, 100]}
        itemsPerPageText={t('common.itemsPerPage')}
        onChange={({ page: nextPage, pageSize: nextPageSize }) => {
          updateParams({
            page: String(nextPage),
            pageSize: String(nextPageSize),
          });
        }}
      />
    </TableContainer>
  );
}

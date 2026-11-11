import { Renew } from '@carbon/icons-react';
import { ActionableNotification, Button, Loading, Search } from '@carbon/react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router-dom';

import { getWorkItemCounts, listWorkItems } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { IngestionState, Page, SortParam, WorkItem, WorkItemStateCount } from '../../api/types';
import { EntityTable, type EntityColumn, type EntityRow } from '../../components/EntityTable';
import { FilterPills } from '../../components/FilterPills';
import { PageHeader } from '../../components/PageHeader';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { ProgressBar } from '../../components/ProgressBar';
import { Surface } from '../../components/Surface';
import { AbsentCell, StackedCell } from '../../components/cells';
import { formatBytes, formatDateTime, formatNumber, formatRelativeTime } from '../../i18n/format';
import { WorkItemInspector } from './WorkItemInspector';
import { stageProgress, workItemDisplayName } from './workItem';

/**
 * Processing Queue (PRD-UI-006, ADR-0184): dense, server-paged work-item table
 * with live state-count filter pills, Release-level rows (title, artist,
 * stage progress, size), a single-select inspector card, and an explicit
 * (disabled) bulk-action bar. Filter/search/sort/page/selection state is
 * reflected in the URL so views are shareable (UI-SPEC §7).
 *
 * Honest omissions: the reference's `Priority` column has no counterpart in the
 * domain, so `Attempts` occupies that position; `Progress` is the documented
 * stage mapping, never an elapsed-time estimate.
 */

/** Sort whitelist shared with the backend (WorkItemSort::parse). */
const SORT_KEYS = new Set(['addedAt', 'updatedAt', 'state', 'attemptCount']);

/** Canonical state order for the filter pills (melodee-domain IngestionState). */
const STATE_ORDER: IngestionState[] = [
  'discovered',
  'settling',
  'ready',
  'inspecting',
  'normalizing',
  'validating',
  'needs_review',
  'staged',
  'publishing',
  'published',
  'cataloging',
  'cataloged',
  'indexing',
  'indexed',
  'failed',
  'quarantined',
  'ignored',
  'deleted',
];

const STATE_SET = new Set<string>(STATE_ORDER);
const COUNTS_REFRESH_MS = 30_000;
const SEARCH_DEBOUNCE_MS = 300;

function parseState(raw: string | null): IngestionState | undefined {
  return raw && STATE_SET.has(raw) ? (raw as IngestionState) : undefined;
}

function parseSort(raw: string | null): SortParam | undefined {
  if (!raw) {
    return undefined;
  }
  const [key, direction] = raw.split(':');
  if (!key || (direction !== 'asc' && direction !== 'desc') || !SORT_KEYS.has(key)) {
    return undefined;
  }
  return `${key}:${direction}` as SortParam;
}

function parsePage(raw: string | null): number {
  return Math.max(1, Number.parseInt(raw ?? '1', 10) || 1);
}

export function ProcessingQueueScreen() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();

  const stateFilter = parseState(searchParams.get('state'));
  const sort = parseSort(searchParams.get('sort'));
  const page = parsePage(searchParams.get('page'));
  const pageSize = Math.max(1, Number.parseInt(searchParams.get('pageSize') ?? '50', 10) || 50);
  const inspectorItemId = searchParams.get('item');
  const query = searchParams.get('query') ?? '';
  const density = searchParams.get('density') === 'default' ? 'default' : 'compact';

  // Search input is local while typing so the table does not refetch on every
  // keystroke; commits to the URL after a short debounce. External URL changes
  // (back/forward, a shared link) re-seed the input during render — the
  // sanctioned alternative to a synchronizing effect.
  const [searchText, setSearchText] = useState(query);
  const [prevQuery, setPrevQuery] = useState(query);
  if (prevQuery !== query) {
    setPrevQuery(query);
    setSearchText(query);
  }
  useEffect(() => {
    if (searchText === query) {
      return;
    }
    const timer = window.setTimeout(() => {
      const next = new URLSearchParams(searchParams);
      if (searchText.trim()) {
        next.set('query', searchText.trim());
      } else {
        next.delete('query');
      }
      next.set('page', '1');
      setSearchParams(next, { replace: true });
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [searchText, query, searchParams, setSearchParams]);

  const [pageData, setPageData] = useState<Page<WorkItem> | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [counts, setCounts] = useState<WorkItemStateCount[] | null>(null);
  const [bulkSelected, setBulkSelected] = useState<string[]>([]);

  const updateParams = useCallback(
    (updates: Record<string, string | null>) => {
      const next = new URLSearchParams(searchParams);
      for (const [key, value] of Object.entries(updates)) {
        if (value === null) {
          next.delete(key);
        } else {
          next.set(key, value);
        }
      }
      setSearchParams(next, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  // Server-side page fetch. Rows stay on screen while a refetch is in flight
  // so paging/sorting does not flash an empty table.
  const fetchPage = useCallback(
    (signal: { cancelled: boolean }) => {
      return listWorkItems({
        state: stateFilter,
        query: query || undefined,
        limit: pageSize,
        offset: (page - 1) * pageSize,
        sort,
      })
        .then((result) => {
          if (!signal.cancelled) {
            setPageData(result);
            setListError(null);
            setLoading(false);
          }
        })
        .catch((cause: unknown) => {
          if (!signal.cancelled) {
            setListError(
              cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null),
            );
            setLoading(false);
          }
        });
    },
    [stateFilter, sort, page, pageSize, query],
  );

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchPage(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchPage]);

  // The displayed page changed: bulk selection refers to rows that may no
  // longer be visible, so it resets (state adjusted during render, per the
  // React docs pattern). Single-selection (the inspector) is preserved in
  // the URL.
  const queryKey = `${stateFilter ?? ''}|${sort ?? ''}|${page}|${pageSize}|${query}`;
  const [prevQueryKey, setPrevQueryKey] = useState(queryKey);
  if (prevQueryKey !== queryKey) {
    setPrevQueryKey(queryKey);
    setBulkSelected([]);
  }

  // Live state counts, refreshed on an interval with cleanup; refreshing
  // counts never touches table or selection state.
  useEffect(() => {
    let cancelled = false;
    const poll = () => {
      getWorkItemCounts()
        .then((result) => {
          if (!cancelled) {
            setCounts(result);
          }
        })
        .catch(() => {
          // Count failure is non-fatal: pills keep their last values.
        });
    };
    poll();
    const timer = window.setInterval(poll, COUNTS_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: 'attemptCount', header: t('queue.column.attempts'), sortable: true },
      { key: 'release', header: t('queue.column.release') },
      { key: 'artist', header: t('overview.queue.column.artist') },
      { key: 'state', header: t('queue.column.state'), sortable: true },
      { key: 'progress', header: t('queue.column.progress') },
      { key: 'addedAt', header: t('queue.column.added'), sortable: true },
      { key: 'size', header: t('queue.column.size') },
    ],
    [t],
  );

  const rows: EntityRow[] = useMemo(
    () =>
      (pageData?.items ?? []).map((item) => {
        const title = item.releaseTitle ?? workItemDisplayName(item);
        return {
          id: item.id,
          cells: {
            attemptCount: formatNumber(item.attemptCount),
            release: (
              <StackedCell
                primary={title}
                secondary={item.releaseTitle ? item.currentPath : t('queue.unbound')}
              />
            ),
            artist: item.artistName ?? <AbsentCell label={t('queue.unbound')} />,
            state: <ProcessingStateTag state={item.state} />,
            progress: (
              <ProgressBar
                value={stageProgress(item.state)}
                label={t('queue.progress.label', { title })}
                stage={t(`queue.state.${item.state}`, { defaultValue: item.state })}
              />
            ),
            addedAt: (
              <span title={formatDateTime(item.createdAt)}>
                {formatRelativeTime(item.createdAt)}
              </span>
            ),
            size: item.mediaBytes ? formatBytes(item.mediaBytes) : <AbsentCell label={t('queue.column.size')} />,
          },
        };
      }),
    [pageData, t],
  );

  // Pills: All plus every state the counts endpoint reports, in canonical
  // order. The counts endpoint omits empty states by design.
  const pills = useMemo(() => {
    const total = (counts ?? []).reduce((sum, entry) => sum + entry.count, 0);
    const byState = new Map((counts ?? []).map((entry) => [entry.state, entry.count]));
    return [
      { key: '', label: t('common.all'), count: formatNumber(total) },
      ...STATE_ORDER.filter((state) => byState.has(state)).map((state) => ({
        key: state,
        label: t(`queue.state.${state}`),
        count: formatNumber(byState.get(state) ?? 0),
      })),
    ];
  }, [counts, t]);

  const toolbar =
    bulkSelected.length > 0 ? (
      <div className="melodee-queue-bulk-bar">
        <span className="melodee-queue-bulk-bar__count">
          {t('queue.bulk.selected', { count: bulkSelected.length })}
        </span>
        <Button kind="primary" size="sm" disabled title={t('queue.bulk.unavailable')}>
          {t('queue.bulk.actions')}
        </Button>
      </div>
    ) : (
      <div className="melodee-queue-toolbar">
        <div className="melodee-queue-toolbar__search">
          <Search
            size="sm"
            labelText={t('queue.searchLabel')}
            placeholder={t('queue.searchPlaceholder')}
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
            closeButtonLabelText={t('actions.clear')}
          />
        </div>
        <Button
          kind="ghost"
          size="sm"
          onClick={() =>
            updateParams({ density: density === 'compact' ? 'default' : 'compact' })
          }
          title={t('queue.density')}
        >
          {t('queue.density')}
        </Button>
        <span className="melodee-queue-toolbar__spacer" />
        <Button
          kind="ghost"
          size="sm"
          renderIcon={Renew}
          aria-label={t('queue.refresh')}
          onClick={() => {
            setLoading(pageData === null);
            void fetchPage({ cancelled: false });
          }}
        >
          {t('queue.refresh')}
        </Button>
        <Button kind="primary" size="sm" disabled title={t('queue.bulk.unavailable')}>
          {t('queue.bulk.actions')}
        </Button>
      </div>
    );

  return (
    <div className="melodee-queue-layout">
      <div className="melodee-queue-main">
        <PageHeader title={t('queue.title')} subtitle={t('queue.subtitle')} />
        {listError ? (
          <ActionableNotification
            inline
            kind="error"
            lowContrast
            title={t('queue.loadErrorTitle')}
            subtitle={listError.message}
            hideCloseButton
            actionButtonLabel={t('actions.retry')}
            onActionButtonClick={() => void fetchPage({ cancelled: false })}
          />
        ) : null}
        <div className="melodee-queue-chips">
          <FilterPills
            items={pills}
            value={stateFilter ?? ''}
            onChange={(key) => updateParams({ state: key, page: '1' })}
            ariaLabel={t('queue.filterByState')}
          />
        </div>
        {loading && pageData === null ? (
          <Loading description={t('common.loading')} withOverlay={false} />
        ) : (
          <Surface variant="flush" className="melodee-table-card">
            <EntityTable
              caption={t('queue.tableCaption')}
              columns={columns}
              rows={rows}
              totalRows={pageData?.total ?? 0}
              toolbar={toolbar}
              compact={density === 'compact'}
              emptyTitle={t('queue.emptyTitle')}
              emptyDescription={t('queue.emptyDescription')}
              activeRowId={inspectorItemId}
              onRowActivate={(id) => updateParams({ item: id })}
              selection={{ selectedIds: bulkSelected, onChange: setBulkSelected }}
            />
          </Surface>
        )}
      </div>
      {inspectorItemId ? (
        <WorkItemInspector
          key={inspectorItemId}
          workItemId={inspectorItemId}
          onClose={() => updateParams({ item: null })}
        />
      ) : null}
    </div>
  );
}

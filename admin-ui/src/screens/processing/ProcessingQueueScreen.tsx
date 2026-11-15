import { Renew } from '@carbon/icons-react';
import {
  ActionableNotification,
  Button,
  InlineNotification,
  Loading,
  Modal,
  Search,
} from '@carbon/react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router-dom';

import { getWorkItemCounts, listWorkItems, purgeWorkItems } from '../../api/admin';
import { ApiError } from '../../api/client';
import type {
  IngestionPhase,
  IngestionState,
  Page,
  PurgeWorkItemsResult,
  SortParam,
  WorkItem,
  WorkItemStateCount,
} from '../../api/types';
import { EntityTable, type EntityColumn, type EntityRow } from '../../components/EntityTable';
import { FilterPills } from '../../components/FilterPills';
import { PageHeader } from '../../components/PageHeader';
import { ProgressBar } from '../../components/ProgressBar';
import { StatusChip } from '../../components/StatusTag';
import { Surface } from '../../components/Surface';
import { AbsentCell, StackedCell } from '../../components/cells';
import { formatBytes, formatDateTime, formatNumber, formatRelativeTime } from '../../i18n/format';
import { WorkItemInspector } from './WorkItemInspector';
import { countByPhase, PHASE_ORDER, phaseForState, phaseStatusKind } from './phase';
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
const PHASE_SET = new Set<string>(PHASE_ORDER);

function parsePhase(raw: string | null): IngestionPhase | undefined {
  return raw && PHASE_SET.has(raw) ? (raw as IngestionPhase) : undefined;
}
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
  const phaseFilter = parsePhase(searchParams.get('phase'));
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
  const [bulkOpen, setBulkOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [purgeError, setPurgeError] = useState<string | null>(null);
  const [notice, setNotice] = useState<{
    kind: 'success' | 'error' | 'warning';
    text: string;
  } | null>(null);

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
        // Phase is the coarse filter (a state set); `state` remains for exact
        // deep links such as the dashboard attention rows.
        phase: phaseFilter,
        state: phaseFilter ? undefined : stateFilter,
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
    [stateFilter, phaseFilter, sort, page, pageSize, query],
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
  const queryKey = `${phaseFilter ?? ''}|${stateFilter ?? ''}|${sort ?? ''}|${page}|${pageSize}|${query}`;
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
            state: (
              <>
                {/* The semantic label an operator triages on. "Active" means a
                    client can already see and play it. */}
                <StatusChip
                  kind={phaseStatusKind(item.phase ?? phaseForState(item.state))}
                  label={t(`phase.${item.phase ?? phaseForState(item.state)}`)}
                />
                <span className="melodee-cell-stack__secondary">{item.state}</span>
              </>
            ),
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

  // Pills are phases (ADR-0185): the counts endpoint reports per state, so the
  // phases are aggregated client-side and empty phases are omitted.
  const pills = useMemo(() => {
    const total = (counts ?? []).reduce((sum, entry) => sum + entry.count, 0);
    return [
      { key: '', label: t('common.all'), count: formatNumber(total) },
      ...countByPhase(counts ?? []).map((entry) => ({
        key: entry.phase,
        label: t(`phase.${entry.phase}`),
        count: formatNumber(entry.count),
      })),
    ];
  }, [counts, t]);

  // A phase pill is active for an explicit phase filter, and for a legacy
  // `state=` deep link whose state belongs to that phase.
  const activePill = phaseFilter ?? (stateFilter ? phaseForState(stateFilter) : '');

  const toolbar =
    bulkSelected.length > 0 ? (
      <div className="melodee-queue-bulk-bar">
        <span className="melodee-queue-bulk-bar__count">
          {t('queue.bulk.selected', { count: bulkSelected.length })}
        </span>
        <Button kind="primary" size="sm" onClick={() => setBulkOpen(true)}>
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

  const selectedItems = (pageData?.items ?? []).filter((item) =>
    bulkSelected.includes(item.id),
  );
  const allSelectedFailed =
    selectedItems.length > 0 && selectedItems.every((item) => item.state === 'failed');
  const canConfirmPurge = !deleting;

  const closeDelete = () => {
    setDeleteOpen(false);
    setPurgeError(null);
  };

  const submitPurge = async () => {
    if (!canConfirmPurge) return;
    setDeleting(true);
    setPurgeError(null);
    try {
      const result: PurgeWorkItemsResult = await purgeWorkItems(bulkSelected);
      const deleted = result.deleted.length;
      const failed = result.failed.length;
      setBulkSelected([]);
      setBulkOpen(false);
      closeDelete();
      setNotice(
        failed === 0
          ? { kind: 'success', text: t('queue.bulk.success', { count: deleted }) }
          : {
              kind: 'warning',
              text: t('queue.bulk.partial', { deleted, failed }),
            },
      );
      void fetchPage({ cancelled: false });
      void getWorkItemCounts()
        .then(setCounts)
        .catch(() => undefined);
    } catch (cause) {
      setPurgeError(
        cause instanceof ApiError ? cause.message : t('queue.bulk.deleteError'),
      );
    } finally {
      setDeleting(false);
    }
  };

  return (
    <>
    <div className="melodee-queue-layout">
      <div className="melodee-queue-main">
        <PageHeader title={t('queue.title')} subtitle={t('queue.subtitle')} />
        {notice ? (
          <InlineNotification
            kind={notice.kind}
            lowContrast
            title={notice.text}
            onCloseButtonClick={() => setNotice(null)}
          />
        ) : null}
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
            value={activePill}
            onChange={(key) => updateParams({ phase: key, state: null, page: '1' })}
            ariaLabel={t('queue.filterByPhase')}
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
    <Modal
      open={bulkOpen}
      modalHeading={t('queue.bulk.title')}
      primaryButtonText={t('queue.bulk.deleteSelected')}
      secondaryButtonText={t('actions.cancel')}
      danger
      primaryButtonDisabled={!allSelectedFailed}
      onRequestClose={() => setBulkOpen(false)}
      onRequestSubmit={() => {
        if (!allSelectedFailed) return;
        setBulkOpen(false);
        setPurgeError(null);
        setDeleteOpen(true);
      }}
    >
      <p>{t('queue.bulk.selected', { count: bulkSelected.length })}</p>
      <p>
        {allSelectedFailed
          ? t('queue.bulk.deleteHelp')
          : t('queue.bulk.onlyFailed')}
      </p>
    </Modal>
    <Modal
      open={deleteOpen}
      danger
      modalHeading={t('queue.bulk.confirmTitle')}
      primaryButtonText={deleting ? t('common.deleting') : t('common.yes')}
      secondaryButtonText={t('common.no')}
      primaryButtonDisabled={!canConfirmPurge}
      onRequestClose={closeDelete}
      onRequestSubmit={() => void submitPurge()}
    >
      <p>{t('queue.bulk.confirmBody', { count: bulkSelected.length })}</p>
      {purgeError ? (
        <InlineNotification
          kind="error"
          lowContrast
          title={purgeError}
          hideCloseButton
        />
      ) : null}
    </Modal>
    </>
  );
}

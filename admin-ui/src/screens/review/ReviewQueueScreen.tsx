import { Renew } from '@carbon/icons-react';
import { ActionableNotification, Button, Loading } from '@carbon/react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router-dom';

import { listReviewQueue } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { IngestionState, ReviewQueuePage, SortParam } from '../../api/types';
import { EntityTable, type EntityColumn, type EntityRow } from '../../components/EntityTable';
import { FilterPills } from '../../components/FilterPills';
import { PageHeader } from '../../components/PageHeader';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { formatDateTime, formatNumber, formatRelativeTime } from '../../i18n/format';
import { queueRowDisplayName, REVIEW_STATES } from './review';

/**
 * Review Queue (PRD-UI-007): the review-able slice of the work-item
 * pipeline (needs_review / failed / quarantined) as a dense, server-paged
 * table with state filter chips fed by the listing envelope's own counts.
 * Filter/sort/page state lives in the URL so views are shareable
 * (UI-SPEC §7). Row activation opens the Release Review workspace; there is
 * no bulk mutation over review items.
 */

/** Sort whitelist shared with the backend (WorkItemSort::parse). */
const SORT_KEYS = new Set(['addedAt', 'updatedAt', 'state', 'attemptCount']);

const STATE_SET = new Set<string>(REVIEW_STATES);

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

export function ReviewQueueScreen() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  const stateFilter = parseState(searchParams.get('state'));
  const sort = parseSort(searchParams.get('sort'));
  const page = parsePage(searchParams.get('page'));
  const pageSize = Math.max(1, Number.parseInt(searchParams.get('pageSize') ?? '50', 10) || 50);

  const [pageData, setPageData] = useState<ReviewQueuePage | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);

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

  // Server-side page fetch. Rows stay on screen while a refetch is in
  // flight so paging/sorting does not flash an empty table.
  const fetchPage = useCallback(
    (signal: { cancelled: boolean }) => {
      return listReviewQueue({
        state: stateFilter,
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
    [stateFilter, sort, page, pageSize],
  );

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchPage(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchPage]);

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: 'release', header: t('queue.column.release') },
      { key: 'state', header: t('queue.column.state'), sortable: true },
      { key: 'attemptCount', header: t('queue.column.attempts'), sortable: true },
      { key: 'addedAt', header: t('queue.column.added'), sortable: true },
      { key: 'updatedAt', header: t('queue.column.updated'), sortable: true },
      { key: 'lastError', header: t('queue.column.lastError') },
    ],
    [t],
  );

  const rows: EntityRow[] = useMemo(
    () =>
      (pageData?.items ?? []).map((item) => ({
        id: item.id,
        cells: {
          release: (
            <>
              <span className="melodee-work-item__title">{queueRowDisplayName(item)}</span>
              <span className="melodee-work-item__id">{item.id}</span>
            </>
          ),
          state: <ProcessingStateTag state={item.state} />,
          attemptCount: formatNumber(item.attemptCount),
          addedAt: (
            <span title={formatDateTime(item.createdAt)}>{formatRelativeTime(item.createdAt)}</span>
          ),
          updatedAt: (
            <span title={formatDateTime(item.updatedAt)}>{formatRelativeTime(item.updatedAt)}</span>
          ),
          lastError: item.lastErrorMessage ?? t('common.none'),
        },
      })),
    [pageData, t],
  );

  // Chips: All plus each review-able state the envelope reports, in
  // canonical order. countsByState omits empty states by design.
  const chips = useMemo(() => {
    const counts = pageData?.countsByState ?? [];
    const byState = new Map(counts.map((entry) => [entry.state, entry.count]));
    return {
      total: counts.reduce((sum, entry) => sum + entry.count, 0),
      entries: REVIEW_STATES.filter((state) => byState.has(state)).map((state) => ({
        state,
        count: byState.get(state) ?? 0,
      })),
    };
  }, [pageData]);

  const toolbar = (
    <Button
      kind="ghost"
      size="sm"
      renderIcon={Renew}
      aria-label={t('review.queue.refresh')}
      onClick={() => {
        setLoading(pageData === null);
        void fetchPage({ cancelled: false });
      }}
    >
      {t('review.queue.refresh')}
    </Button>
  );

  return (
    <div className="melodee-queue-main">
      <PageHeader title={t('review.queue.title')} subtitle={t('review.queue.subtitle')} />
      {listError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('review.queue.loadErrorTitle')}
          subtitle={listError.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => void fetchPage({ cancelled: false })}
        />
      ) : null}
      <div className="melodee-queue-chips">
        <FilterPills
          ariaLabel={t('review.queue.filterByState')}
          value={stateFilter ?? ''}
          onChange={(key) => updateParams({ state: key, page: '1' })}
          items={[
            { key: '', label: t('common.all'), count: formatNumber(chips.total) },
            ...chips.entries.map((entry) => ({
              key: entry.state,
              label: t(`queue.state.${entry.state}`),
              count: formatNumber(entry.count),
            })),
          ]}
        />
      </div>
      {loading && pageData === null ? (
        <Loading description={t('common.loading')} withOverlay={false} />
      ) : (
        <EntityTable
          caption={t('review.queue.tableCaption')}
          columns={columns}
          rows={rows}
          totalRows={pageData?.total ?? 0}
          toolbar={toolbar}
          compact
          emptyTitle={t('review.queue.emptyTitle')}
          emptyDescription={t('review.queue.emptyDescription')}
          onRowActivate={(id) =>
            void navigate(`/processing/review/${encodeURIComponent(id)}`)
          }
        />
      )}
    </div>
  );
}

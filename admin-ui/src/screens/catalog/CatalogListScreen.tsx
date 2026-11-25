import { ActionableNotification, Loading, Search } from '@carbon/react';
import type { ReactNode, SyntheticEvent } from 'react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router-dom';

import type { CatalogListParams } from '../../api/admin';
import type { ApiError } from '../../api/client';
import type { Page } from '../../api/types';
import {
  EntityTable,
  type EntityColumn,
  type EntityRow,
  type EntityTableSelection,
} from '../../components/EntityTable';
import { PageHeader } from '../../components/PageHeader';
import {
  parseCatalogSort,
  parsePageParam,
  parsePageSizeParam,
  toApiError,
} from './catalog';

/**
 * Server-side catalog browsing table (PRD-UI-005/010, UI-SPEC §6/§7):
 * the search box binds to the backend `query` filter, sorting is restricted
 * to the backend whitelist, and pagination is envelope-driven. All state is
 * reflected in the URL (query/sort/page/pageSize) so views are shareable.
 */
export interface CatalogListScreenProps<T extends { id: string }> {
  titleKey: string;
  subtitleKey: string;
  captionKey: string;
  searchLabelKey: string;
  emptyTitleKey: string;
  emptyDescriptionKey: string;
  /** Backend sort whitelist (mirrors admin_catalog.rs). */
  sortKeys: readonly string[];
  fetchPage: (params: CatalogListParams, signal?: AbortSignal) => Promise<Page<T>>;
  columns: EntityColumn[];
  toRow: (item: T) => EntityRow;
  detailPath: (item: T) => string;
  selection?: EntityTableSelection;
  toolbarActions?: ReactNode;
  notice?: ReactNode;
  refreshToken?: number;
  onItemsChange?: (items: T[]) => void;
}

/** Debounce so typing does not issue one catalog query per keystroke. */
const SEARCH_DEBOUNCE_MS = 300;

/**
 * Interactive cell content inside an activatable EntityTable row: clicks and
 * keys on a link must not also trigger row activation (double navigation).
 */
export function CellInteractive({ children }: { children: ReactNode }) {
  const stop = (event: SyntheticEvent) => event.stopPropagation();
  return (
    <span onClick={stop} onKeyDown={stop}>
      {children}
    </span>
  );
}

export function CatalogListScreen<T extends { id: string }>({
  titleKey,
  subtitleKey,
  captionKey,
  searchLabelKey,
  emptyTitleKey,
  emptyDescriptionKey,
  sortKeys,
  fetchPage,
  columns,
  toRow,
  detailPath,
  selection,
  toolbarActions,
  notice,
  refreshToken = 0,
  onItemsChange,
}: CatalogListScreenProps<T>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  const sortWhitelist = new Set(sortKeys);
  const sort = parseCatalogSort(searchParams.get('sort'), sortWhitelist);
  const page = parsePageParam(searchParams.get('page'));
  const pageSize = parsePageSizeParam(searchParams.get('pageSize'));
  const queryParam = searchParams.get('query') ?? '';

  const [pageData, setPageData] = useState<Page<T> | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const request = useRef<AbortController | null>(null);

  // Search text is local while typing; the debounced effect publishes it to
  // the URL (resetting to page 1), which drives the server fetch.
  const [searchText, setSearchText] = useState(queryParam);
  const [prevQueryParam, setPrevQueryParam] = useState(queryParam);
  if (prevQueryParam !== queryParam) {
    setPrevQueryParam(queryParam);
    setSearchText(queryParam);
  }

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

  useEffect(() => {
    if (searchText === queryParam) {
      return;
    }
    const timer = window.setTimeout(() => {
      updateParams({ query: searchText || null, page: '1' });
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [searchText, queryParam, updateParams]);

  // Server-side page fetch. Rows stay on screen while a refetch is in
  // flight so paging/sorting does not flash an empty table.
  const fetchCurrentPage = useCallback(
    () => {
      request.current?.abort();
      const controller = new AbortController();
      request.current = controller;
      return fetchPage(
        {
          query: queryParam || undefined,
          limit: pageSize,
          offset: (page - 1) * pageSize,
          sort,
        },
        controller.signal,
      )
        .then((result) => {
          if (!controller.signal.aborted) {
            setPageData(result);
            onItemsChange?.(result.items);
            setListError(null);
            setLoading(false);
          }
        })
        .catch((cause: unknown) => {
          if (!controller.signal.aborted) {
            setListError(toApiError(cause));
            setLoading(false);
          }
        });
    },
    [fetchPage, queryParam, page, pageSize, sort, onItemsChange],
  );

  useEffect(() => {
    void fetchCurrentPage();
    return () => request.current?.abort();
  }, [fetchCurrentPage, refreshToken]);

  const rows: EntityRow[] = (pageData?.items ?? []).map(toRow);

  const toolbar = (
    <>
      <Search
        labelText={t(searchLabelKey)}
        placeholder={t(searchLabelKey)}
        size="md"
        value={searchText}
        onChange={(event) => setSearchText(event.target.value)}
        onClear={() => setSearchText('')}
      />
      {toolbarActions}
    </>
  );

  return (
    <div className="melodee-queue-main melodee-catalog-list">
      <PageHeader title={t(titleKey)} subtitle={t(subtitleKey)} />
      {notice}
      {listError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('catalog.list.loadErrorTitle')}
          subtitle={listError.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => void fetchCurrentPage()}
        />
      ) : null}
      {loading && pageData === null ? (
        <Loading description={t('common.loading')} withOverlay={false} />
      ) : (
        <EntityTable
          caption={t(captionKey)}
          columns={columns}
          rows={rows}
          totalRows={pageData?.total ?? 0}
          toolbar={toolbar}
          selection={selection}
          emptyTitle={t(emptyTitleKey)}
          emptyDescription={t(emptyDescriptionKey)}
          onRowActivate={(id) => {
            const item = (pageData?.items ?? []).find((candidate) => candidate.id === id);
            if (item) {
              void navigate(detailPath(item));
            }
          }}
        />
      )}
    </div>
  );
}

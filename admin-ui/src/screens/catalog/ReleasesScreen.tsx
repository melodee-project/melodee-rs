import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { listCatalogReleases } from '../../api/admin';
import type { CatalogReleaseListItem } from '../../api/types';
import { ReleaseLink } from '../../components/EntityLink';
import type { EntityColumn } from '../../components/EntityTable';
import { StatusTag } from '../../components/StatusTag';
import { formatDate, formatDateTime, formatDuration, formatNumber, formatRelativeTime } from '../../i18n/format';
import { catalogStatusKind } from './catalog';
import { CatalogListScreen, CellInteractive } from './CatalogListScreen';

/**
 * Catalog Releases browsing (WP-15, PRD-UI-005/010): server-side paged table
 * over GET /api/admin/v1/catalog/releases with the backend sort whitelist
 * (title/releaseDate/createdAt/updatedAt) and substring `query` filter.
 */
const SORT_KEYS = ['title', 'releaseDate', 'createdAt', 'updatedAt'];

export function ReleasesScreen() {
  const { t } = useTranslation();

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: 'title', header: t('catalog.column.title'), sortable: true },
      { key: 'displayArtist', header: t('catalog.column.artist') },
      { key: 'releaseDate', header: t('catalog.column.releaseDate'), sortable: true },
      { key: 'status', header: t('catalog.column.status') },
      { key: 'trackCount', header: t('catalog.column.tracks') },
      { key: 'durationMs', header: t('catalog.column.duration') },
      { key: 'updatedAt', header: t('catalog.column.updated'), sortable: true },
    ],
    [t],
  );

  const toRow = useCallback(
    (item: CatalogReleaseListItem) => ({
      id: item.id,
      cells: {
        title: (
          <CellInteractive>
            <ReleaseLink id={item.id} name={item.title} />
          </CellInteractive>
        ),
        displayArtist: item.displayArtist ?? t('common.none'),
        releaseDate: item.releaseDate
          ? formatDate(item.releaseDate)
          : (item.releaseDateText ?? t('common.none')),
        status: (
          <StatusTag
            kind={catalogStatusKind(item.status)}
            label={t(`catalog.status.${item.status}`, { defaultValue: item.status })}
          />
        ),
        trackCount: formatNumber(item.trackCount),
        durationMs: formatDuration(item.durationMs),
        updatedAt: (
          <span title={formatDateTime(item.updatedAt)}>{formatRelativeTime(item.updatedAt)}</span>
        ),
      },
    }),
    [t],
  );

  return (
    <CatalogListScreen
      titleKey="catalog.releases.title"
      subtitleKey="catalog.releases.subtitle"
      captionKey="catalog.releases.tableCaption"
      searchLabelKey="catalog.releases.searchLabel"
      emptyTitleKey="catalog.releases.emptyTitle"
      emptyDescriptionKey="catalog.releases.emptyDescription"
      sortKeys={SORT_KEYS}
      fetchPage={listCatalogReleases}
      columns={columns}
      toRow={toRow}
      detailPath={(item) => `/catalog/releases/${encodeURIComponent(item.id)}`}
    />
  );
}

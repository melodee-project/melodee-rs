import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { listCatalogTracks } from '../../api/admin';
import type { CatalogTrackListItem } from '../../api/types';
import { ReleaseLink, TrackLink } from '../../components/EntityLink';
import type { EntityColumn } from '../../components/EntityTable';
import { formatDateTime, formatDuration, formatNumber, formatRelativeTime } from '../../i18n/format';
import { trackOrdinalLabel } from './catalog';
import { CatalogListScreen, CellInteractive } from './CatalogListScreen';

/**
 * Catalog Tracks browsing (PRD-UI-005/010): server-side paged table
 * over GET /api/admin/v1/catalog/tracks with the backend sort whitelist
 * (title/durationMs/createdAt) and substring `query` filter. No audio
 * preview here: admin catalog DTOs carry no signed stream URLs (preview
 * exists in Release Review for pre-publication audio).
 */
const SORT_KEYS = ['title', 'durationMs', 'createdAt'];

export function TracksScreen() {
  const { t } = useTranslation();

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: 'title', header: t('catalog.column.title'), sortable: true },
      { key: 'displayArtist', header: t('catalog.column.artist') },
      { key: 'release', header: t('catalog.column.release') },
      { key: 'ordinal', header: t('catalog.column.ordinal') },
      { key: 'durationMs', header: t('catalog.column.duration'), sortable: true },
      { key: 'createdAt', header: t('catalog.column.created'), sortable: true },
    ],
    [t],
  );

  const toRow = useCallback(
    (item: CatalogTrackListItem) => ({
      id: item.id,
      cells: {
        title: (
          <CellInteractive>
            <TrackLink id={item.id} name={item.title} />
          </CellInteractive>
        ),
        displayArtist: item.displayArtist ?? t('common.none'),
        release: (
          <CellInteractive>
            <ReleaseLink id={item.releaseId} name={item.releaseTitle} />
          </CellInteractive>
        ),
        ordinal: trackOrdinalLabel(item.ordinal, item.discNumber, item.trackNumber, formatNumber),
        durationMs: item.durationMs != null ? formatDuration(item.durationMs) : t('common.none'),
        createdAt: (
          <span title={formatDateTime(item.createdAt)}>{formatRelativeTime(item.createdAt)}</span>
        ),
      },
    }),
    [t],
  );

  return (
    <CatalogListScreen
      titleKey="catalog.tracks.title"
      subtitleKey="catalog.tracks.subtitle"
      captionKey="catalog.tracks.tableCaption"
      searchLabelKey="catalog.tracks.searchLabel"
      emptyTitleKey="catalog.tracks.emptyTitle"
      emptyDescriptionKey="catalog.tracks.emptyDescription"
      sortKeys={SORT_KEYS}
      fetchPage={listCatalogTracks}
      columns={columns}
      toRow={toRow}
      detailPath={(item) => `/catalog/tracks/${encodeURIComponent(item.id)}`}
    />
  );
}

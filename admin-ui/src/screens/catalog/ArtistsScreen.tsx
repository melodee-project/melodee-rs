import { useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";

import { listCatalogArtists } from "../../api/admin";
import type { CatalogArtistListItem } from "../../api/types";
import { ArtistLink } from "../../components/EntityLink";
import type { EntityColumn } from "../../components/EntityTable";
import { StatusTag } from "../../components/StatusTag";
import {
  formatDateTime,
  formatNumber,
  formatRelativeTime,
} from "../../i18n/format";
import { catalogStatusKind } from "./catalog";
import { CatalogListScreen, CellInteractive } from "./CatalogListScreen";

/**
 * Catalog Artists browsing (PRD-UI-005/010): server-side paged table
 * over GET /api/admin/v1/catalog/artists with the backend sort whitelist
 * (name/createdAt/updatedAt) and substring `query` filter.
 */
const SORT_KEYS = ["name", "createdAt", "updatedAt"];

export function ArtistsScreen() {
  const { t } = useTranslation();

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: "name", header: t("catalog.column.name"), sortable: true },
      { key: "artistType", header: t("catalog.column.type") },
      { key: "status", header: t("catalog.column.status") },
      { key: "releaseCount", header: t("catalog.column.releases") },
      { key: "trackCount", header: t("catalog.column.tracks") },
      { key: "updatedAt", header: t("catalog.column.updated"), sortable: true },
    ],
    [t],
  );

  const toRow = useCallback(
    (item: CatalogArtistListItem) => ({
      id: item.id,
      cells: {
        name: (
          <CellInteractive>
            <ArtistLink id={item.id} name={item.name} />
            {item.disambiguation ? (
              <span className="melodee-work-item__id">
                {item.disambiguation}
              </span>
            ) : null}
          </CellInteractive>
        ),
        artistType: t(`catalog.artistType.${item.artistType}`, {
          defaultValue: item.artistType,
        }),
        status: (
          <StatusTag
            kind={catalogStatusKind(item.status)}
            label={t(`catalog.status.${item.status}`, {
              defaultValue: item.status,
            })}
          />
        ),
        releaseCount: formatNumber(item.releaseCount),
        trackCount: formatNumber(item.trackCount),
        updatedAt: (
          <span title={formatDateTime(item.updatedAt)}>
            {formatRelativeTime(item.updatedAt)}
          </span>
        ),
      },
    }),
    [t],
  );

  return (
    <CatalogListScreen
      titleKey="catalog.artists.title"
      subtitleKey="catalog.artists.subtitle"
      captionKey="catalog.artists.tableCaption"
      searchLabelKey="catalog.artists.searchLabel"
      emptyTitleKey="catalog.artists.emptyTitle"
      emptyDescriptionKey="catalog.artists.emptyDescription"
      sortKeys={SORT_KEYS}
      fetchPage={listCatalogArtists}
      columns={columns}
      toRow={toRow}
      detailPath={(item) => `/catalog/artists/${encodeURIComponent(item.id)}`}
    />
  );
}

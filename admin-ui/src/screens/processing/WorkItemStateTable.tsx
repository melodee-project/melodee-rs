import { Renew } from "@carbon/icons-react";
import {
  ActionableNotification,
  Button,
  ContentSwitcher,
  Link,
  Loading,
  Switch,
} from "@carbon/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link as RouterLink, useSearchParams } from "react-router-dom";

import { listWorkItems } from "../../api/admin";
import { ApiError } from "../../api/client";
import type {
  IngestionState,
  Page,
  SortParam,
  WorkItem,
} from "../../api/types";
import {
  EntityTable,
  type EntityColumn,
  type EntityRow,
} from "../../components/EntityTable";
import { ProcessingStateTag } from "../../components/ProcessingStateTag";
import {
  formatDateTime,
  formatNumber,
  formatRelativeTime,
} from "../../i18n/format";
import { WorkItemInspector } from "./WorkItemInspector";
import { workItemDisplayName } from "./workItem";

/**
 * Server-paged work-item table scoped to one or more terminal states
 * (Quarantine, History; PRD-INGEST-010, SPEC §20). Reuses the  * inspector for the append-only event history; states where an operator
 * decision is still possible additionally link to the review workspace
 * (`state-appropriate`: the review queue accepts needs_review | failed |
 * quarantined server-side). Filter/sort/page/selection state lives in the
 * URL so views are shareable (UI-SPEC §7).
 */

/** Sort whitelist shared with the backend (WorkItemSort::parse). */
const SORT_KEYS = new Set(["addedAt", "updatedAt", "state", "attemptCount"]);

function parseSort(raw: string | null): SortParam | undefined {
  if (!raw) {
    return undefined;
  }
  const [key, direction] = raw.split(":");
  if (
    !key ||
    (direction !== "asc" && direction !== "desc") ||
    !SORT_KEYS.has(key)
  ) {
    return undefined;
  }
  return `${key}:${direction}` as SortParam;
}

function parsePage(raw: string | null): number {
  return Math.max(1, Number.parseInt(raw ?? "1", 10) || 1);
}

export interface WorkItemStateTableProps {
  /** Allowed states; a single state pins the table, several render a switcher. */
  states: readonly IngestionState[];
  /** States for which a link to the review workspace is state-appropriate. */
  reviewLinkStates?: readonly IngestionState[];
  tableCaption: string;
  emptyTitle: string;
  emptyDescription: string;
}

export function WorkItemStateTable({
  states,
  reviewLinkStates = [],
  tableCaption,
  emptyTitle,
  emptyDescription,
}: WorkItemStateTableProps) {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();

  const requestedState = searchParams.get("state");
  const stateFilter: IngestionState =
    states.length === 1
      ? (states[0] as IngestionState)
      : (states.find((state) => state === requestedState) ??
        (states[0] as IngestionState));
  const sort = parseSort(searchParams.get("sort"));
  const page = parsePage(searchParams.get("page"));
  const pageSize = Math.max(
    1,
    Number.parseInt(searchParams.get("pageSize") ?? "50", 10) || 50,
  );
  const inspectorItemId = searchParams.get("item");

  const [pageData, setPageData] = useState<Page<WorkItem> | null>(null);
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

  const fetchPage = useCallback(
    (signal: { cancelled: boolean }) => {
      return listWorkItems({
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
              cause instanceof ApiError
                ? cause
                : new ApiError(0, "unknown", String(cause), null),
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
      { key: "release", header: t("queue.column.release") },
      { key: "state", header: t("queue.column.state"), sortable: true },
      {
        key: "attemptCount",
        header: t("queue.column.attempts"),
        sortable: true,
      },
      { key: "addedAt", header: t("queue.column.added"), sortable: true },
      { key: "updatedAt", header: t("queue.column.updated"), sortable: true },
      { key: "lastError", header: t("queue.column.lastError") },
      ...(reviewLinkStates.includes(stateFilter)
        ? [{ key: "actions", header: t("quarantine.column.actions") }]
        : []),
    ],
    [t, reviewLinkStates, stateFilter],
  );

  const rows: EntityRow[] = useMemo(
    () =>
      (pageData?.items ?? []).map((item) => ({
        id: item.id,
        cells: {
          release: (
            <>
              <span className="melodee-work-item__title">
                {workItemDisplayName(item)}
              </span>
              <span className="melodee-work-item__id">{item.id}</span>
            </>
          ),
          state: <ProcessingStateTag state={item.state} />,
          attemptCount: formatNumber(item.attemptCount),
          addedAt: (
            <span title={formatDateTime(item.createdAt)}>
              {formatRelativeTime(item.createdAt)}
            </span>
          ),
          updatedAt: (
            <span title={formatDateTime(item.updatedAt)}>
              {formatRelativeTime(item.updatedAt)}
            </span>
          ),
          lastError: item.lastErrorMessage ?? t("common.none"),
          ...(reviewLinkStates.includes(stateFilter)
            ? {
                actions: (
                  <Link
                    as={RouterLink}
                    to={`/processing/review/${encodeURIComponent(item.id)}`}
                    onClick={(event: React.MouseEvent) =>
                      event.stopPropagation()
                    }
                  >
                    {t("quarantine.openReview")}
                  </Link>
                ),
              }
            : {}),
        },
      })),
    [pageData, t, reviewLinkStates, stateFilter],
  );

  const toolbar = (
    <Button
      kind="ghost"
      size="sm"
      renderIcon={Renew}
      aria-label={t("queue.refresh")}
      onClick={() => {
        setLoading(pageData === null);
        void fetchPage({ cancelled: false });
      }}
    >
      {t("queue.refresh")}
    </Button>
  );

  return (
    <div className="melodee-queue-layout">
      <div className="melodee-queue-main">
        {listError ? (
          <ActionableNotification
            inline
            kind="error"
            lowContrast
            title={t("queue.loadErrorTitle")}
            subtitle={listError.message}
            hideCloseButton
            actionButtonLabel={t("actions.retry")}
            onActionButtonClick={() => void fetchPage({ cancelled: false })}
          />
        ) : null}
        {states.length > 1 ? (
          <div
            className="melodee-queue-chips"
            role="group"
            aria-label={t("queue.filterByState")}
          >
            <ContentSwitcher
              size="sm"
              selectedIndex={Math.max(
                0,
                states.findIndex((state) => state === stateFilter),
              )}
              onChange={(data) => {
                const name = typeof data.name === "string" ? data.name : "";
                updateParams({ state: name || null, page: "1" });
              }}
            >
              {states.map((state) => (
                <Switch
                  key={state}
                  name={state}
                  text={t(`queue.state.${state}`)}
                />
              ))}
            </ContentSwitcher>
          </div>
        ) : null}
        {loading && pageData === null ? (
          <Loading description={t("common.loading")} withOverlay={false} />
        ) : (
          <EntityTable
            caption={tableCaption}
            columns={columns}
            rows={rows}
            totalRows={pageData?.total ?? 0}
            toolbar={toolbar}
            compact
            emptyTitle={emptyTitle}
            emptyDescription={emptyDescription}
            activeRowId={inspectorItemId}
            onRowActivate={(id) => updateParams({ item: id })}
          />
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

import { Renew } from "@carbon/icons-react";
import {
  ActionableNotification,
  Button,
  ContentSwitcher,
  Loading,
  Switch,
} from "@carbon/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";

import { getWorkItemCounts, listWorkItems } from "../../api/admin";
import { ApiError } from "../../api/client";
import type {
  IngestionState,
  Page,
  SortParam,
  WorkItem,
  WorkItemStateCount,
} from "../../api/types";
import {
  EntityTable,
  type EntityColumn,
  type EntityRow,
} from "../../components/EntityTable";
import { PageHeader } from "../../components/PageHeader";
import { ProcessingStateTag } from "../../components/ProcessingStateTag";
import {
  formatNumber,
  formatRelativeTime,
  formatDateTime,
} from "../../i18n/format";
import { WorkItemInspector } from "./WorkItemInspector";
import { workItemDisplayName } from "./workItem";

/**
 * Processing Queue (PRD-UI-006, ): dense, server-paged work-item table
 * with live state-count filter chips, single-select inspector, and an
 * explicit (disabled) bulk-action bar. Filter/sort/page/selection state is
 * reflected in the URL so views are shareable (UI-SPEC §7).
 */

/** Sort whitelist shared with the backend (WorkItemSort::parse). */
const SORT_KEYS = new Set(["addedAt", "updatedAt", "state", "attemptCount"]);

/** Canonical state order for the filter chips (melodee-domain IngestionState). */
const STATE_ORDER: IngestionState[] = [
  "discovered",
  "settling",
  "ready",
  "inspecting",
  "normalizing",
  "validating",
  "needs_review",
  "staged",
  "publishing",
  "published",
  "cataloging",
  "cataloged",
  "indexing",
  "indexed",
  "failed",
  "quarantined",
  "ignored",
  "deleted",
];

const STATE_SET = new Set<string>(STATE_ORDER);
const COUNTS_REFRESH_MS = 30_000;

function parseState(raw: string | null): IngestionState | undefined {
  return raw && STATE_SET.has(raw) ? (raw as IngestionState) : undefined;
}

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

export function ProcessingQueueScreen() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();

  const stateFilter = parseState(searchParams.get("state"));
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

  // The displayed page changed: bulk selection refers to rows that may no
  // longer be visible, so it resets (state adjusted during render, per the
  // React docs pattern). Single-selection (the inspector) is preserved in
  // the URL.
  const queryKey = `${stateFilter ?? ""}|${sort ?? ""}|${page}|${pageSize}`;
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
          // Count failure is non-fatal: chips keep their last values.
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
        },
      })),
    [pageData, t],
  );

  // Chips: All plus every state the counts endpoint reports, in canonical
  // order. The counts endpoint omits empty states by design.
  const chips = useMemo(() => {
    const total = (counts ?? []).reduce((sum, entry) => sum + entry.count, 0);
    const byState = new Map(
      (counts ?? []).map((entry) => [entry.state, entry.count]),
    );
    return {
      total,
      entries: STATE_ORDER.filter((state) => byState.has(state)).map(
        (state) => ({
          state,
          count: byState.get(state) ?? 0,
        }),
      ),
    };
  }, [counts]);

  const selectedChipIndex = stateFilter
    ? chips.entries.findIndex((entry) => entry.state === stateFilter) + 1
    : 0;

  const toolbar =
    bulkSelected.length > 0 ? (
      <div className="melodee-queue-bulk-bar">
        <span className="melodee-queue-bulk-bar__count">
          {t("queue.bulk.selected", { count: bulkSelected.length })}
        </span>
        <Button
          kind="ghost"
          size="sm"
          disabled
          title={t("queue.bulk.unavailable")}
        >
          {t("queue.bulk.actions")}
        </Button>
      </div>
    ) : (
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
        <PageHeader title={t("queue.title")} subtitle={t("queue.subtitle")} />
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
        <div
          className="melodee-queue-chips"
          role="group"
          aria-label={t("queue.filterByState")}
        >
          <ContentSwitcher
            size="sm"
            selectedIndex={selectedChipIndex === -1 ? 0 : selectedChipIndex}
            onChange={(data) => {
              const name = typeof data.name === "string" ? data.name : "";
              updateParams({ state: name || null, page: "1" });
            }}
          >
            {[
              <Switch
                key="__all__"
                name=""
                text={`${t("common.all")} ${formatNumber(chips.total)}`}
              />,
              ...chips.entries.map((entry) => (
                <Switch
                  key={entry.state}
                  name={entry.state}
                  text={`${t(`queue.state.${entry.state}`)} ${formatNumber(entry.count)}`}
                />
              )),
            ]}
          </ContentSwitcher>
        </div>
        {loading && pageData === null ? (
          <Loading description={t("common.loading")} withOverlay={false} />
        ) : (
          <EntityTable
            caption={t("queue.tableCaption")}
            columns={columns}
            rows={rows}
            totalRows={pageData?.total ?? 0}
            toolbar={toolbar}
            compact
            emptyTitle={t("queue.emptyTitle")}
            emptyDescription={t("queue.emptyDescription")}
            activeRowId={inspectorItemId}
            onRowActivate={(id) => updateParams({ item: id })}
            selection={{ selectedIds: bulkSelected, onChange: setBulkSelected }}
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

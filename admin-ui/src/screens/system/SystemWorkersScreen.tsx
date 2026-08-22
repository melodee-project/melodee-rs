import { Renew } from "@carbon/icons-react";
import {
  ActionableNotification,
  Button,
  Heading,
  InlineNotification,
  Link,
  Loading,
  Modal,
} from "@carbon/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";

import {
  getDashboard,
  getSystemHealth,
  listWorkerFailures,
  listWorkerPools,
  pauseWorkerPool,
  resumeWorkerPool,
} from "../../api/admin";
import { ApiError } from "../../api/client";
import type {
  Page,
  SortParam,
  SystemHealth,
  WorkItem,
  WorkerPool,
} from "../../api/types";
import { EmptyState } from "../../components/EmptyState";
import {
  EntityTable,
  type EntityColumn,
  type EntityRow,
} from "../../components/EntityTable";
import { MetricCard } from "../../components/MetricCard";
import { PageHeader } from "../../components/PageHeader";
import { StatusTag } from "../../components/StatusTag";
import { SystemHealthPanel } from "../../components/SystemHealthPanel";
import {
  formatDateTime,
  formatDuration,
  formatNumber,
  formatRelativeTime,
} from "../../i18n/format";
import { LeasesPanel } from "./LeasesPanel";
import { SearchPanel } from "./SearchPanel";

/**
 * System / Workers / Diagnostics (PRD-WORK-002, SPEC §22):
 * durable worker-pool table with lease-coordinated pause/resume, aggregated
 * subsystem health, recent failed work items, search index status with a
 * rebuild trigger, and active lease evidence. The search and lease panels
 * are the shared SearchPanel/LeasesPanel also used by the System → Search
 * and System → Diagnostics screens — the logic exists exactly once.
 *
 * Honesty rules honored throughout: pools without a durable queue table
 * report `metricsAvailable: false` and render "not reported" instead of
 * invented zeros; the rebuild button never fakes progress (the search worker
 * claims the durable job in the background); structured logs and performance
 * metrics are not HTTP-exposed and are stated as CLI surfaces rather than
 * linked to nonexistent UI.
 */

/** Sort whitelist shared with the backend (WorkItemSort::parse). */
const FAILURE_SORT_KEYS = new Set([
  "addedAt",
  "updatedAt",
  "state",
  "attemptCount",
]);

interface Notice {
  kind: "success" | "error" | "info" | "warning";
  text: string;
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError
    ? cause
    : new ApiError(0, "unknown", String(cause), null);
}

function parsePage(raw: string | null): number {
  return Math.max(1, Number.parseInt(raw ?? "1", 10) || 1);
}

function parseSort(raw: string | null): SortParam | undefined {
  if (!raw) {
    return undefined;
  }
  const [key, direction] = raw.split(":");
  if (
    !key ||
    (direction !== "asc" && direction !== "desc") ||
    !FAILURE_SORT_KEYS.has(key)
  ) {
    return undefined;
  }
  return `${key}:${direction}` as SortParam;
}

/** Metric cell: honest absence when the pool has no durable queue table. */
function PoolMetric({ value }: { value: number | null }) {
  const { t } = useTranslation();
  if (value === null) {
    return (
      <span
        className="melodee-metric-absent"
        title={t("system.pools.metricsUnavailableHint")}
      >
        {t("system.pools.metricsUnavailable")}
      </span>
    );
  }
  return <>{formatNumber(value)}</>;
}

export function SystemWorkersScreen() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  const failuresPage = parsePage(searchParams.get("page"));
  const failuresPageSize = Math.max(
    1,
    Number.parseInt(searchParams.get("pageSize") ?? "50", 10) || 50,
  );
  const failuresSort = parseSort(searchParams.get("sort"));

  const [pools, setPools] = useState<Page<WorkerPool> | null>(null);
  const [poolsError, setPoolsError] = useState<ApiError | null>(null);
  const [totals, setTotals] = useState<{
    completed24h: number;
    failed24h: number;
  } | null>(null);
  const [health, setHealth] = useState<SystemHealth | null>(null);
  const [healthError, setHealthError] = useState<ApiError | null>(null);
  const [failures, setFailures] = useState<Page<WorkItem> | null>(null);
  const [failuresError, setFailuresError] = useState<ApiError | null>(null);
  const [pauseTarget, setPauseTarget] = useState<WorkerPool | null>(null);
  const [poolBusy, setPoolBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  // Bumped by the refresh action so the shared SearchPanel/LeasesPanel
  // re-run their fetches without this screen owning them.
  const [panelRefreshKey, setPanelRefreshKey] = useState(0);

  const fetchPools = useCallback((signal: { cancelled: boolean }) => {
    return Promise.all([listWorkerPools(), getDashboard()])
      .then(([poolPage, dashboard]) => {
        if (!signal.cancelled) {
          setPools(poolPage);
          setTotals({
            completed24h: dashboard.workItems.completedLast24h,
            failed24h: dashboard.workItems.failedLast24h,
          });
          setPoolsError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setPoolsError(toApiError(cause));
        }
      });
  }, []);

  const fetchHealth = useCallback((signal: { cancelled: boolean }) => {
    return getSystemHealth()
      .then((result) => {
        if (!signal.cancelled) {
          setHealth(result);
          setHealthError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setHealthError(toApiError(cause));
        }
      });
  }, []);

  const fetchFailures = useCallback(
    (signal: { cancelled: boolean }) => {
      return listWorkerFailures({
        limit: failuresPageSize,
        offset: (failuresPage - 1) * failuresPageSize,
        sort: failuresSort,
      })
        .then((result) => {
          if (!signal.cancelled) {
            setFailures(result);
            setFailuresError(null);
          }
        })
        .catch((cause: unknown) => {
          if (!signal.cancelled) {
            setFailuresError(toApiError(cause));
          }
        });
    },
    [failuresPage, failuresPageSize, failuresSort],
  );

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchPools(signal);
    void fetchHealth(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchPools, fetchHealth]);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchFailures(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchFailures]);

  /** Applies the pool DTO returned by pause/resume to the table in place. */
  const applyPoolUpdate = (updated: WorkerPool) => {
    setPools((current) =>
      current
        ? {
            ...current,
            items: current.items.map((item) =>
              item.name === updated.name ? updated : item,
            ),
          }
        : current,
    );
  };

  const changePoolState = (pool: WorkerPool, paused: boolean) => {
    setPoolBusy(pool.name);
    const request = paused
      ? pauseWorkerPool(pool.name)
      : resumeWorkerPool(pool.name);
    request
      .then((updated) => {
        applyPoolUpdate(updated);
        setNotice(null);
      })
      .catch((cause: unknown) => {
        const error = toApiError(cause);
        setNotice({
          kind: "error",
          text: t(
            paused ? "system.pools.pauseError" : "system.pools.resumeError",
            {
              message: error.message,
            },
          ),
        });
      })
      .finally(() => {
        setPoolBusy(null);
        setPauseTarget(null);
      });
  };

  const poolColumns: EntityColumn[] = useMemo(
    () => [
      { key: "pool", header: t("system.pools.column.pool") },
      { key: "status", header: t("system.pools.column.status") },
      { key: "running", header: t("system.pools.column.running") },
      { key: "queued", header: t("system.pools.column.queued") },
      { key: "throughput", header: t("system.pools.column.throughput") },
      {
        key: "completedLastHour",
        header: t("system.pools.column.completedLastHour"),
      },
      { key: "failed24h", header: t("system.pools.column.failed24h") },
      { key: "oldestAge", header: t("system.pools.column.oldestAge") },
      { key: "actions", header: t("system.pools.column.actions") },
    ],
    [t],
  );

  const poolRows: EntityRow[] = useMemo(
    () =>
      (pools?.items ?? []).map((pool) => ({
        id: pool.name,
        cells: {
          pool: pool.name,
          status: (
            <>
              {pool.paused ? (
                <StatusTag kind="neutral" label={t("status.paused")} />
              ) : (
                <StatusTag kind="info" label={t("status.info")} />
              )}
              {pool.paused && pool.pausedBy ? (
                <span className="melodee-pool-paused-by">
                  {t("system.pools.pausedBy", { actor: pool.pausedBy })}
                </span>
              ) : null}
              {!pool.paused && pool.resumeRequestedAt ? (
                <span className="melodee-pool-paused-by">
                  {t("system.pools.resumeRequested")}
                </span>
              ) : null}
            </>
          ),
          running: <PoolMetric value={pool.running} />,
          queued: <PoolMetric value={pool.queued} />,
          throughput:
            pool.throughputPerMinute === null ? (
              <PoolMetric value={null} />
            ) : (
              formatNumber(Math.round(pool.throughputPerMinute * 10) / 10)
            ),
          completedLastHour: <PoolMetric value={pool.completedLastHour} />,
          failed24h: <PoolMetric value={pool.failedLast24h} />,
          oldestAge:
            pool.oldestQueuedAgeSeconds === null ? (
              <PoolMetric value={null} />
            ) : (
              formatDuration(pool.oldestQueuedAgeSeconds * 1000)
            ),
          actions: pool.paused ? (
            <Button
              kind="ghost"
              size="sm"
              disabled={poolBusy === pool.name}
              onClick={() => changePoolState(pool, false)}
            >
              {t("system.pools.resume")}
            </Button>
          ) : (
            <Button
              kind="ghost"
              size="sm"
              disabled={poolBusy === pool.name}
              onClick={() => setPauseTarget(pool)}
            >
              {t("system.pools.pause")}
            </Button>
          ),
        },
      })),
    // changePoolState is stable enough for this screen's render cycle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [pools, poolBusy, t],
  );

  const failureColumns: EntityColumn[] = useMemo(
    () => [
      { key: "workItem", header: t("system.failures.column.workItem") },
      { key: "error", header: t("system.failures.column.error") },
      {
        key: "updatedAt",
        header: t("system.failures.column.time"),
        sortable: true,
      },
    ],
    [t],
  );

  const failureRows: EntityRow[] = useMemo(
    () =>
      (failures?.items ?? []).map((item) => ({
        id: item.id,
        cells: {
          workItem: (
            <>
              <span className="melodee-work-item__title">
                {item.sourcePath}
              </span>
              <span className="melodee-work-item__id">{item.id}</span>
            </>
          ),
          error: item.lastErrorMessage ?? t("common.none"),
          updatedAt: (
            <span title={formatDateTime(item.updatedAt)}>
              {formatRelativeTime(item.updatedAt)}
            </span>
          ),
        },
      })),
    [failures, t],
  );

  const queuedTotal = (pools?.items ?? []).reduce(
    (sum, pool) => sum + (pool.queued ?? 0),
    0,
  );

  return (
    <>
      <PageHeader
        title={t("system.workersTitle")}
        subtitle={t("system.subtitle")}
        actions={
          <Button
            kind="ghost"
            size="sm"
            renderIcon={Renew}
            onClick={() => {
              void fetchPools({ cancelled: false });
              void fetchHealth({ cancelled: false });
              void fetchFailures({ cancelled: false });
              setPanelRefreshKey((key) => key + 1);
            }}
          >
            {t("actions.refresh")}
          </Button>
        }
      />
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          hideCloseButton={false}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      <div className="melodee-metric-grid">
        <MetricCard
          label={t("system.kpi.workers")}
          value={pools ? formatNumber(pools.total) : "—"}
        />
        <MetricCard
          label={t("system.kpi.queued")}
          value={pools ? formatNumber(queuedTotal) : "—"}
        />
        <MetricCard
          label={t("system.kpi.completed24h")}
          value={totals ? formatNumber(totals.completed24h) : "—"}
        />
        <MetricCard
          label={t("system.kpi.failed24h")}
          value={totals ? formatNumber(totals.failed24h) : "—"}
          status={totals && totals.failed24h > 0 ? "error" : undefined}
        />
      </div>
      <div className="melodee-system-columns">
        <div className="melodee-system-main">
          {poolsError ? (
            <ActionableNotification
              inline
              kind="error"
              lowContrast
              title={t("system.loadErrorTitle")}
              subtitle={poolsError.message}
              hideCloseButton
              actionButtonLabel={t("actions.retry")}
              onActionButtonClick={() => void fetchPools({ cancelled: false })}
            />
          ) : null}
          {pools === null && !poolsError ? (
            <Loading description={t("common.loading")} withOverlay={false} />
          ) : (
            <EntityTable
              caption={t("system.pools.caption")}
              columns={poolColumns}
              rows={poolRows}
              totalRows={pools?.total ?? 0}
              compact
              emptyTitle={t("system.pools.emptyTitle")}
              emptyDescription={t("system.pools.emptyDescription")}
            />
          )}

          {failuresError ? (
            <ActionableNotification
              inline
              kind="error"
              lowContrast
              title={t("system.failures.loadErrorTitle")}
              subtitle={failuresError.message}
              hideCloseButton
              actionButtonLabel={t("actions.retry")}
              onActionButtonClick={() =>
                void fetchFailures({ cancelled: false })
              }
            />
          ) : null}
          <EntityTable
            caption={t("system.failures.caption")}
            columns={failureColumns}
            rows={failureRows}
            totalRows={failures?.total ?? 0}
            compact
            emptyTitle={t("system.failures.emptyTitle")}
            emptyDescription={t("system.failures.emptyDescription")}
            onRowActivate={(id) =>
              void navigate(
                `/processing?state=failed&item=${encodeURIComponent(id)}`,
              )
            }
          />

          <LeasesPanel refreshKey={panelRefreshKey} />
        </div>

        <div className="melodee-system-rail">
          {healthError ? (
            <ActionableNotification
              inline
              kind="error"
              lowContrast
              title={t("system.loadErrorTitle")}
              subtitle={healthError.message}
              hideCloseButton
              actionButtonLabel={t("actions.retry")}
              onActionButtonClick={() => void fetchHealth({ cancelled: false })}
            />
          ) : null}
          {health ? (
            <SystemHealthPanel
              health={health}
              title={t("system.health.title")}
              headingId="system-health-title"
            />
          ) : null}

          <SearchPanel refreshKey={panelRefreshKey} />

          <section
            className="melodee-panel"
            aria-labelledby="system-diagnostics-title"
          >
            <Heading
              id="system-diagnostics-title"
              className="melodee-panel__title"
            >
              {t("system.diagnostics.title")}
            </Heading>
            <p className="melodee-panel__subtitle">
              <Link href="#system-health-title">
                {t("system.diagnostics.runCheck")}
              </Link>
            </p>
            <p className="melodee-panel__subtitle">
              {t("system.diagnostics.runCheckDescription")}
            </p>
            <EmptyState
              compact
              title={t("system.diagnostics.cliNoteTitle")}
              description={t("system.diagnostics.cliNote")}
            />
          </section>
        </div>
      </div>

      <Modal
        open={pauseTarget !== null}
        modalHeading={t("system.pools.pauseConfirmTitle", {
          pool: pauseTarget?.name ?? "",
        })}
        primaryButtonText={t("system.pools.pause")}
        secondaryButtonText={t("actions.cancel")}
        onRequestClose={() => setPauseTarget(null)}
        onRequestSubmit={() => {
          if (pauseTarget) {
            changePoolState(pauseTarget, true);
          }
        }}
      >
        <p>{t("system.pools.pauseConfirmBody")}</p>
      </Modal>
    </>
  );
}

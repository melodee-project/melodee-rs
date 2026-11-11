import { ArrowRight, Renew, WarningFilled } from '@carbon/icons-react';
import { ActionableNotification, Button, Loading } from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink, useNavigate } from 'react-router-dom';

import { getDashboard, getOnboardingStatus, listWorkItems } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { Dashboard, WorkItem } from '../../api/types';
import { MetricCard } from '../../components/MetricCard';
import { PageHeader } from '../../components/PageHeader';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { ProgressBar } from '../../components/ProgressBar';
import { StackedCell } from '../../components/cells';
import { Surface } from '../../components/Surface';
import { SystemHealthPanel } from '../../components/SystemHealthPanel';
import { formatNumber, formatRelativeTime } from '../../i18n/format';
import { stageProgress, workItemDisplayName } from '../processing/workItem';
import { ActivityChart } from './ActivityChart';
import { ACTIVE_STATES, countStates, INBOUND_STATES, sortedBuckets } from './overview';

/**
 * Overview dashboard (PRD-UI-001, SPEC §22, ADR-0184): answers "what needs
 * attention?" first, then KPI counts with their recent-change lines, processing
 * activity, system health, and the Release-level current queue.
 *
 * Everything renders from the aggregated `GET /api/admin/v1/dashboard` payload
 * plus the first page of the work-item queue (enriched with the bound Release);
 * the panel refreshes every 60s with cancellation cleanup, and failures surface
 * a retryable error without discarding the last good data.
 */

const REFRESH_MS = 60_000;
const QUEUE_PREVIEW_LIMIT = 5;

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

export function OverviewScreen() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [dashboard, setDashboard] = useState<Dashboard | null>(null);
  const [queue, setQueue] = useState<WorkItem[]>([]);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  // First-run setup pointer: shown only when the status endpoint answers
  // with an incomplete, non-bypassed onboarding; an endpoint failure shows
  // no banner (no global redirect trap that would break when the surface
  // is unavailable).
  const [onboardingPending, setOnboardingPending] = useState(false);

  const fetchDashboard = useCallback((signal: { cancelled: boolean }) => {
    return Promise.all([
      getDashboard(),
      // The preview is decoration-grade: a failure must never fail the screen,
      // so it falls back to the state summary below.
      listWorkItems({ limit: QUEUE_PREVIEW_LIMIT, sort: 'updatedAt:desc' }).catch(() => null),
    ])
      .then(([result, page]) => {
        if (!signal.cancelled) {
          setDashboard(result);
          setQueue(page?.items ?? []);
          setError(null);
          setLoading(false);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setError(toApiError(cause));
          setLoading(false);
        }
      });
  }, []);

  // Initial load plus the 60s auto-refresh; the last good payload stays on
  // screen while a refresh is in flight or fails.
  useEffect(() => {
    const signal = { cancelled: false };
    void fetchDashboard(signal);
    const timer = window.setInterval(() => void fetchDashboard(signal), REFRESH_MS);
    return () => {
      signal.cancelled = true;
      window.clearInterval(timer);
    };
  }, [fetchDashboard]);

  useEffect(() => {
    let cancelled = false;
    getOnboardingStatus()
      .then((status) => {
        // Strict shape gate: only an explicit incomplete/non-bypassed status
        // raises the banner — a malformed payload must not.
        if (!cancelled) {
          setOnboardingPending(status.completed === false && status.bypassed === false);
        }
      })
      .catch(() => {
        // No banner when the status surface is unavailable.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (loading && dashboard === null) {
    return (
      <>
        <PageHeader title={t('overview.title')} subtitle={t('overview.subtitle')} />
        <Loading description={t('common.loading')} withOverlay={false} />
      </>
    );
  }

  const countsByState = dashboard?.workItems.countsByState ?? [];
  const completedBuckets = sortedBuckets(dashboard?.activity.completedPerHour ?? []);
  const indexedBuckets = sortedBuckets(dashboard?.activity.indexedPerHour ?? []);
  const hasActivity = completedBuckets.length > 0 || indexedBuckets.length > 0;

  const attention = dashboard?.attention;
  const attentionRows = attention
    ? [
        {
          key: 'unresolvedIdentities',
          count: attention.unresolvedIdentities,
          to: '/catalog/identity',
        },
        {
          key: 'pendingPolicyDecisions',
          count: attention.pendingPolicyDecisions,
          to: '/processing/review?state=needs_review',
        },
        {
          key: 'failedProcessing',
          count: attention.failedProcessing,
          to: '/processing?state=failed',
        },
        {
          key: 'quarantined',
          count: attention.quarantined,
          to: '/processing/review?state=quarantined',
        },
      ]
    : [];
  const storageDegraded = dashboard?.health.storage.status === 'degraded';
  const attentionEmpty = attentionRows.every((row) => row.count === 0) && !storageDegraded;

  const activeCount = countStates(countsByState, ACTIVE_STATES);
  const pendingCount = countsByState.reduce((sum, entry) => sum + entry.count, 0);

  return (
    <>
      <PageHeader
        title={t('overview.title')}
        subtitle={t('overview.subtitle')}
        actions={
          <div className="melodee-overview-actions">
            {dashboard ? (
              <span className="melodee-overview-generated">
                {t('overview.generatedAt', {
                  time: formatRelativeTime(dashboard.generatedAt),
                })}
              </span>
            ) : null}
            <Button
              kind="ghost"
              size="sm"
              renderIcon={Renew}
              onClick={() => {
                setLoading(dashboard === null);
                void fetchDashboard({ cancelled: false });
              }}
            >
              {t('actions.refresh')}
            </Button>
          </div>
        }
      />
      {onboardingPending ? (
        <ActionableNotification
          inline
          kind="info"
          lowContrast
          title={t('onboarding.bannerTitle')}
          subtitle={t('onboarding.bannerBody')}
          hideCloseButton
          actionButtonLabel={t('onboarding.bannerAction')}
          onActionButtonClick={() => void navigate('/onboarding')}
        />
      ) : null}
      {error ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('overview.loadErrorTitle')}
          subtitle={error.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => {
            setLoading(dashboard === null);
            void fetchDashboard({ cancelled: false });
          }}
        />
      ) : null}
      {dashboard ? (
        <>
          <div className="melodee-metric-grid">
            <MetricCard
              label={t('overview.kpi.inbound')}
              value={formatNumber(countStates(countsByState, INBOUND_STATES))}
              delta={
                dashboard.deltas
                  ? {
                      text: t('overview.kpi.inboundDelta', {
                        count: formatNumber(dashboard.deltas.inboundToday),
                      }),
                    }
                  : undefined
              }
            />
            <MetricCard
              label={t('overview.kpi.processing')}
              value={formatNumber(activeCount)}
              delta={{
                text: t('overview.kpi.processingDelta', { count: formatNumber(activeCount) }),
              }}
            />
            <MetricCard
              label={t('overview.kpi.review')}
              value={formatNumber(countStates(countsByState, ['needs_review']))}
              delta={
                dashboard.deltas
                  ? {
                      text: t('overview.kpi.reviewDelta', {
                        count: formatNumber(dashboard.deltas.newForReview24h),
                      }),
                      tone: 'warning',
                    }
                  : undefined
              }
            />
            <MetricCard
              label={t('overview.kpi.artists')}
              value={formatNumber(dashboard.catalog.artists)}
              hint={t('overview.kpi.artistsHint')}
            />
            <MetricCard
              label={t('overview.kpi.releases')}
              value={formatNumber(dashboard.catalog.releases)}
              hint={t('overview.kpi.releasesHint')}
            />
            <MetricCard
              label={t('overview.kpi.tracks')}
              value={formatNumber(dashboard.catalog.tracks)}
              hint={t('overview.kpi.tracksHint')}
            />
          </div>
          <div className="melodee-overview-columns">
            <Surface title={t('overview.activity.title')} subtitle={t('overview.activity.subtitle')}>
              {hasActivity ? (
                <ActivityChart releases={completedBuckets} tracks={indexedBuckets} />
              ) : (
                <p className="melodee-empty__description">{t('overview.activity.empty')}</p>
              )}
            </Surface>
            <Surface title={t('overview.attention.title')}>
              {attentionEmpty ? (
                <p className="melodee-empty__description">{t('overview.attention.empty')}</p>
              ) : (
                <ul className="melodee-attention-list">
                  {attentionRows
                    .filter((row) => row.count > 0)
                    .map((row) => (
                      <li key={row.key} className="melodee-attention-row">
                        <span className="melodee-attention-row__icon" aria-hidden>
                          <WarningFilled size={12} />
                        </span>
                        <RouterLink className="melodee-attention-row__link" to={row.to}>
                          {t(`overview.attention.${row.key}`)}
                        </RouterLink>
                        <span className="melodee-attention-row__count">
                          {formatNumber(row.count)}
                        </span>
                      </li>
                    ))}
                  {storageDegraded ? (
                    <li className="melodee-attention-row">
                      <span className="melodee-attention-row__icon" aria-hidden>
                        <WarningFilled size={12} />
                      </span>
                      <RouterLink className="melodee-attention-row__link" to="/system/workers">
                        {t('overview.attention.storageDegraded')}
                      </RouterLink>
                    </li>
                  ) : null}
                </ul>
              )}
            </Surface>
          </div>
          <div className="melodee-overview-columns melodee-overview-columns--secondary">
            <SystemHealthPanel
              health={dashboard.health}
              title={t('overview.health.title')}
              headingId="overview-health-title"
            />
            <Surface
              title={t('overview.queue.title')}
              subtitle={t('overview.queue.summary', {
                active: formatNumber(activeCount),
                pending: formatNumber(pendingCount),
              })}
              footer={
                <Button kind="ghost" size="sm" as={RouterLink} to="/processing" renderIcon={ArrowRight}>
                  {t('overview.queue.open')}
                </Button>
              }
            >
              {queue.length === 0 ? (
                <p className="melodee-empty__description">{t('overview.queue.empty')}</p>
              ) : (
                <div className="melodee-table-scroll">
                <table
                  className="cds--data-table cds--data-table--sm melodee-mini-table"
                  aria-label={t('overview.queue.caption')}
                >
                  <thead>
                    <tr>
                      <th scope="col">{t('overview.queue.column.release')}</th>
                      <th scope="col">{t('overview.queue.column.artist')}</th>
                      <th scope="col">{t('overview.queue.column.state')}</th>
                      <th scope="col">{t('overview.queue.column.progress')}</th>
                      <th scope="col">{t('overview.queue.column.added')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {queue.map((item) => {
                      const title = item.releaseTitle ?? workItemDisplayName(item);
                      return (
                        <tr key={item.id}>
                          <td>
                            <StackedCell
                              primary={
                                item.releaseId ? (
                                  <RouterLink to={`/catalog/releases/${item.releaseId}`}>
                                    {title}
                                  </RouterLink>
                                ) : (
                                  title
                                )
                              }
                              secondary={
                                item.releaseTitle ? undefined : t('queue.unbound')
                              }
                            />
                          </td>
                          <td>
                            {item.artistName ?? (
                              <span className="melodee-absent" aria-label={t('queue.unbound')}>
                                —
                              </span>
                            )}
                          </td>
                          <td>
                            <ProcessingStateTag state={item.state} />
                          </td>
                          <td>
                            <ProgressBar
                              value={stageProgress(item.state)}
                              label={t('queue.progress.label', { title })}
                              stage={t(`queue.state.${item.state}`, { defaultValue: item.state })}
                            />
                          </td>
                          <td>
                            <span title={item.updatedAt}>
                              {formatRelativeTime(item.createdAt)}
                            </span>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
                </div>
              )}
            </Surface>
          </div>
        </>
      ) : null}
    </>
  );
}

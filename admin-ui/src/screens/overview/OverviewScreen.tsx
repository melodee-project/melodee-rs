import { ArrowRight, Renew } from '@carbon/icons-react';
import { ActionableNotification, Button, Heading, Link, Loading } from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink, useNavigate } from 'react-router-dom';

import { getDashboard, getOnboardingStatus } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { Dashboard } from '../../api/types';
import { EmptyState } from '../../components/EmptyState';
import { MetricCard } from '../../components/MetricCard';
import { PageHeader } from '../../components/PageHeader';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { SystemHealthPanel } from '../../components/SystemHealthPanel';
import { formatDateTime, formatNumber, formatRelativeTime } from '../../i18n/format';
import { ActivityChart } from './ActivityChart';
import { ACTIVE_STATES, countStates, INBOUND_STATES, sortedBuckets } from './overview';

/**
 * Overview dashboard (PRD-UI-001, SPEC §22): answers "what needs
 * attention?" first, then KPI counts, processing activity, system health,
 * and the current queue summary. Everything renders from the aggregated
 * GET /api/admin/v1/dashboard payload; the panel refreshes every 60s with
 * cancellation cleanup, and failures surface a retryable error without
 * discarding the last good data.
 */

const REFRESH_MS = 60_000;

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

export function OverviewScreen() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [dashboard, setDashboard] = useState<Dashboard | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  // First-run setup pointer: shown only when the status endpoint answers
  // with an incomplete, non-bypassed onboarding; an endpoint failure shows
  // no banner (no global redirect trap that would break when the surface
  // is unavailable).
  const [onboardingPending, setOnboardingPending] = useState(false);

  const fetchDashboard = useCallback((signal: { cancelled: boolean }) => {
    return getDashboard()
      .then((result) => {
        if (!signal.cancelled) {
          setDashboard(result);
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
  const buckets = sortedBuckets(dashboard?.activity.completedPerHour ?? []);

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

  return (
    <>
      <PageHeader
        title={t('overview.title')}
        subtitle={t('overview.subtitle')}
        actions={
          <div className="melodee-overview-actions">
            {dashboard ? (
              <span
                className="melodee-overview-generated"
                title={formatDateTime(dashboard.generatedAt)}
              >
                {t('overview.generatedAt', { time: formatRelativeTime(dashboard.generatedAt) })}
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
            <MetricCard label={t('overview.kpi.inbound')} value={formatNumber(countStates(countsByState, INBOUND_STATES))} />
            <MetricCard label={t('overview.kpi.processing')} value={formatNumber(countStates(countsByState, ACTIVE_STATES))} />
            <MetricCard
              label={t('overview.kpi.review')}
              value={formatNumber(countStates(countsByState, ['needs_review']))}
            />
            <MetricCard label={t('overview.kpi.artists')} value={formatNumber(dashboard.catalog.artists)} />
            <MetricCard label={t('overview.kpi.releases')} value={formatNumber(dashboard.catalog.releases)} />
            <MetricCard label={t('overview.kpi.tracks')} value={formatNumber(dashboard.catalog.tracks)} />
          </div>
          <div className="melodee-overview-columns">
            <section className="melodee-panel" aria-labelledby="overview-activity-title">
              <Heading id="overview-activity-title" className="melodee-panel__title">
                {t('overview.activity.title')}
              </Heading>
              <p className="melodee-panel__subtitle">{t('overview.activity.subtitle')}</p>
              {buckets.length === 0 ? (
                <EmptyState compact title={t('overview.activity.empty')} />
              ) : (
                <ActivityChart buckets={buckets} />
              )}
            </section>
            <section className="melodee-panel" aria-labelledby="overview-attention-title">
              <Heading id="overview-attention-title" className="melodee-panel__title">
                {t('overview.attention.title')}
              </Heading>
              {attentionEmpty ? (
                <EmptyState compact tone="success" title={t('overview.attention.empty')} />
              ) : (
                <ul className="melodee-attention-list">
                  {attentionRows
                    .filter((row) => row.count > 0)
                    .map((row) => (
                      <li key={row.key} className="melodee-attention-row">
                        <Link as={RouterLink} to={row.to} className="melodee-attention-row__link">
                          {t(`overview.attention.${row.key}`)}
                          <ArrowRight aria-hidden />
                        </Link>
                        <span className="melodee-attention-row__count">
                          {formatNumber(row.count)}
                        </span>
                      </li>
                    ))}
                  {storageDegraded ? (
                    <li className="melodee-attention-row">
                      <Link
                        as={RouterLink}
                        to="/system/workers"
                        className="melodee-attention-row__link"
                      >
                        {t('overview.attention.storageDegraded')}
                        <ArrowRight aria-hidden />
                      </Link>
                    </li>
                  ) : null}
                </ul>
              )}
            </section>
          </div>
          <div className="melodee-overview-columns melodee-overview-columns--secondary">
            <SystemHealthPanel
              health={dashboard.health}
              title={t('overview.health.title')}
              headingId="overview-health-title"
            />
            <section className="melodee-panel" aria-labelledby="overview-queue-title">
              <Heading id="overview-queue-title" className="melodee-panel__title">
                {t('overview.queue.title')}
              </Heading>
              {countsByState.length === 0 ? (
                <EmptyState compact title={t('overview.queue.empty')} />
              ) : (
                <table
                  className="cds--data-table cds--data-table--sm melodee-mini-table"
                  aria-label={t('overview.queue.caption')}
                >
                  <thead>
                    <tr>
                      <th scope="col">{t('overview.queue.column.state')}</th>
                      <th scope="col">{t('overview.queue.column.count')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {countsByState.map((entry) => (
                      <tr key={entry.state}>
                        <td>
                          <Link as={RouterLink} to={`/processing?state=${entry.state}`}>
                            <ProcessingStateTag state={entry.state} />
                          </Link>
                        </td>
                        <td>{formatNumber(entry.count)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              <div className="melodee-panel__footer">
                <Button
                  kind="ghost"
                  size="sm"
                  as={RouterLink}
                  to="/processing"
                  renderIcon={ArrowRight}
                >
                  {t('overview.queue.open')}
                </Button>
              </div>
            </section>
          </div>
        </>
      ) : null}
    </>
  );
}

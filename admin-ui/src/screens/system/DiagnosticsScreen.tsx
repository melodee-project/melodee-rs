import { Renew } from '@carbon/icons-react';
import { ActionableNotification, Button, Heading, Link } from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink } from 'react-router-dom';

import { getSystemHealth } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { SystemHealth } from '../../api/types';
import { EmptyState } from '../../components/EmptyState';
import { PageHeader } from '../../components/PageHeader';
import { SystemHealthPanel } from '../../components/SystemHealthPanel';
import { LeasesPanel } from './LeasesPanel';

/**
 * System → Diagnostics (SPEC §22): the aggregated subsystem health panel
 * (the doctor surface — the same subsystem checks as onboarding/readyz),
 * active lease evidence, and a drill-down link to the recent-failures table
 * on System → Workers.
 *
 * Honesty rule: structured logs and performance metrics are not
 * HTTP-exposed; they are stated as CLI surfaces rather than linked to
 * nonexistent UI.
 */

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

export function DiagnosticsScreen() {
  const { t } = useTranslation();
  const [health, setHealth] = useState<SystemHealth | null>(null);
  const [healthError, setHealthError] = useState<ApiError | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

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

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchHealth(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchHealth, refreshKey]);

  return (
    <>
      <PageHeader
        title={t('system.diagnosticsTitle')}
        subtitle={t('system.diagnostics.screenSubtitle')}
        actions={
          <Button
            kind="ghost"
            size="sm"
            renderIcon={Renew}
            onClick={() => setRefreshKey((key) => key + 1)}
          >
            {t('actions.refresh')}
          </Button>
        }
      />
      {healthError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('system.loadErrorTitle')}
          subtitle={healthError.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => void fetchHealth({ cancelled: false })}
        />
      ) : null}
      {health ? (
        <SystemHealthPanel
          health={health}
          title={t('system.health.title')}
          headingId="system-health-title"
        />
      ) : null}
      <LeasesPanel refreshKey={refreshKey} />
      <section className="melodee-panel" aria-labelledby="system-diagnostics-title">
        <Heading id="system-diagnostics-title" className="melodee-panel__title">
          {t('system.diagnostics.title')}
        </Heading>
        <p className="melodee-panel__subtitle">
          <Link as={RouterLink} to="/system/workers">
            {t('system.diagnostics.failuresLink')}
          </Link>
        </p>
        <EmptyState
          compact
          title={t('system.diagnostics.cliNoteTitle')}
          description={t('system.diagnostics.cliNote')}
        />
      </section>
    </>
  );
}

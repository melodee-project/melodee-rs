import { Renew } from '@carbon/icons-react';
import { ActionableNotification, Button, Heading, InlineNotification } from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { getOnboardingStatus, getSystemHealth } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { OnboardingCheck, OnboardingStatus, SystemHealth } from '../../api/types';
import { EmptyState } from '../../components/EmptyState';
import { HealthIndicator, type HealthState } from '../../components/HealthIndicator';
import { PageHeader } from '../../components/PageHeader';
import { StatusTag } from '../../components/StatusTag';

/**
 * System → Storage: real storage state only. Two server surfaces exist:
 * the storage subsystem verdict of GET /system/health and the
 * `storage_paths` preflight check (inbound sources + storage root, with
 * remediation) of GET /onboarding/status. No per-path capacity/usage is
 * exposed over HTTP, so none is rendered — the CLI surfaces are stated
 * honestly instead of inventing fields.
 */

const STATE_BY_STATUS = { ok: 'online', degraded: 'degraded', disabled: 'disabled' } as const;

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

function storagePathsCheck(status: OnboardingStatus | null): OnboardingCheck | null {
  return status?.checks.find((check) => check.name === 'storage_paths') ?? null;
}

export function StorageScreen() {
  const { t } = useTranslation();
  const [health, setHealth] = useState<SystemHealth | null>(null);
  const [healthError, setHealthError] = useState<ApiError | null>(null);
  const [onboarding, setOnboarding] = useState<OnboardingStatus | null>(null);
  const [onboardingError, setOnboardingError] = useState<ApiError | null>(null);
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

  const fetchOnboarding = useCallback((signal: { cancelled: boolean }) => {
    return getOnboardingStatus()
      .then((result) => {
        if (!signal.cancelled) {
          setOnboarding(result);
          setOnboardingError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setOnboardingError(toApiError(cause));
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchHealth(signal);
    void fetchOnboarding(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchHealth, fetchOnboarding, refreshKey]);

  const pathsCheck = storagePathsCheck(onboarding);

  return (
    <>
      <PageHeader
        title={t('system.storageTitle')}
        subtitle={t('system.storage.screenSubtitle')}
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
      <section className="melodee-panel" aria-labelledby="system-storage-health-title">
        <Heading id="system-storage-health-title" className="melodee-panel__title">
          {t('system.storage.healthTitle')}
        </Heading>
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
          <ul className="melodee-health-list">
            <li className="melodee-health-row">
              <span className="melodee-health-row__name">{t('system.health.storage')}</span>
              <span className="melodee-health-row__status">
                <HealthIndicator
                  state={STATE_BY_STATUS[health.storage.status] as HealthState}
                  label={t(`system.health.value.${health.storage.status}`)}
                />
                <span className="melodee-health-row__detail">{health.storage.detail}</span>
              </span>
            </li>
          </ul>
        ) : null}
      </section>
      <section className="melodee-panel" aria-labelledby="system-storage-paths-title">
        <Heading id="system-storage-paths-title" className="melodee-panel__title">
          {t('system.storage.pathsTitle')}
        </Heading>
        {onboardingError ? (
          <ActionableNotification
            inline
            kind="error"
            lowContrast
            title={t('system.loadErrorTitle')}
            subtitle={onboardingError.message}
            hideCloseButton
            actionButtonLabel={t('actions.retry')}
            onActionButtonClick={() => void fetchOnboarding({ cancelled: false })}
          />
        ) : null}
        {onboarding && !pathsCheck ? (
          <EmptyState compact title={t('system.storage.checkMissing')} />
        ) : null}
        {pathsCheck ? (
          <>
            <p className="melodee-panel__subtitle">
              <StatusTag
                kind={pathsCheck.passed ? 'success' : 'error'}
                label={t(pathsCheck.passed ? 'system.storage.checkPassed' : 'system.storage.checkFailed')}
              />{' '}
              {pathsCheck.detail}
            </p>
            {pathsCheck.remediation ? (
              <InlineNotification
                kind="warning"
                lowContrast
                title={t('system.storage.remediationTitle')}
                subtitle={pathsCheck.remediation}
                hideCloseButton
              />
            ) : null}
          </>
        ) : null}
      </section>
      <section className="melodee-panel" aria-labelledby="system-storage-cli-title">
        <Heading id="system-storage-cli-title" className="melodee-panel__title">
          {t('system.storage.perPathNoteTitle')}
        </Heading>
        <p className="melodee-panel__subtitle">{t('system.storage.perPathNote')}</p>
      </section>
    </>
  );
}

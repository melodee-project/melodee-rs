import { Renew } from '@carbon/icons-react';
import {
  ActionableNotification,
  Button,
  InlineNotification,
  Loading,
  ProgressIndicator,
  ProgressStep,
  StructuredListBody,
  StructuredListCell,
  StructuredListRow,
  StructuredListWrapper,
} from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Navigate } from 'react-router-dom';

import { completeOnboarding, getOnboardingStatus } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { OnboardingStatus } from '../../api/types';
import { PageHeader } from '../../components/PageHeader';
import { StatusTag } from '../../components/StatusTag';

/**
 * First-run onboarding wizard (WP-19, PRD-SETUP-001/002): the six preflight
 * checks with pass/fail, operator-safe detail, and remediation text; re-run
 * is a status refetch (the checks evaluate live server-side on every read).
 * Completion is fail-closed — the button enables only when `canComplete`
 * and the server re-evaluates on POST, so a 409 onboarding_checks_failed
 * refetches status and re-renders the failing checks rather than trusting
 * client state. Completed or migration-bypassed setups redirect to /.
 */

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

export function OnboardingWizard() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<OnboardingStatus | null>(null);
  const [loadError, setLoadError] = useState<ApiError | null>(null);
  const [completeError, setCompleteError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const fetchStatus = useCallback((signal: { cancelled: boolean }) => {
    return getOnboardingStatus()
      .then((result) => {
        if (!signal.cancelled) {
          setStatus(result);
          setLoadError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setLoadError(toApiError(cause));
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchStatus(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchStatus]);

  const complete = () => {
    if (!status?.canComplete || busy) {
      return;
    }
    setBusy(true);
    setCompleteError(null);
    completeOnboarding()
      .then((result) => setStatus(result))
      .catch((cause: unknown) => {
        const error = toApiError(cause);
        // Fail-closed: the server re-evaluated and still found failing
        // checks; refresh so the per-check display reflects live state.
        if (error.status === 409 && error.code === 'onboarding_checks_failed') {
          setCompleteError(t('onboarding.completeConflict'));
          void fetchStatus({ cancelled: false });
        } else {
          setCompleteError(t('onboarding.completeError', { message: error.message }));
        }
      })
      .finally(() => setBusy(false));
  };

  if (status && (status.completed || status.bypassed)) {
    return <Navigate to="/" replace />;
  }

  const firstFailingIndex = status
    ? status.checks.findIndex((check) => check.required && !check.passed)
    : 0;

  return (
    <>
      <PageHeader title={t('onboarding.title')} subtitle={t('onboarding.subtitle')} />
      {loadError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('onboarding.loadErrorTitle')}
          subtitle={loadError.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => void fetchStatus({ cancelled: false })}
        />
      ) : null}
      {completeError ? (
        <InlineNotification
          kind="error"
          lowContrast
          title={completeError}
          onCloseButtonClick={() => setCompleteError(null)}
        />
      ) : null}
      {status === null && !loadError ? (
        <Loading description={t('common.loading')} withOverlay={false} />
      ) : null}
      {status ? (
        <>
          {/* The horizontal stepper sizes each step at ~112px; with six checks
              it is wider than the sm viewport and must scroll inside its own
              container rather than expand the page (UI-SPEC §8). */}
          <div className="melodee-onboarding-steps-scroll">
            <ProgressIndicator
              currentIndex={firstFailingIndex === -1 ? status.checks.length : firstFailingIndex}
              aria-label={t('onboarding.stepsLabel')}
            >
            {status.checks.map((check) => (
              <ProgressStep
                key={check.name}
                label={t(`onboarding.check.${check.name}`, { defaultValue: check.name })}
                secondaryLabel={
                  check.passed ? t('onboarding.checkPassed') : t('onboarding.checkFailed')
                }
                complete={check.passed}
                current={
                  !check.passed &&
                  status.checks.findIndex((candidate) => candidate.required && !candidate.passed) ===
                    status.checks.indexOf(check)
                }
              />
            ))}
            </ProgressIndicator>
          </div>
          {/* Carbon's structured list has a 500px min-width; at sm it must
              scroll inside its own container rather than overflow the page
              (UI-SPEC §8, mirrors the review table scroll pattern). */}
          <div className="melodee-onboarding-checks-scroll">
            <StructuredListWrapper aria-label={t('onboarding.checksCaption')}>
            <StructuredListBody>
              {status.checks.map((check) => (
                <StructuredListRow key={check.name}>
                  <StructuredListCell>
                    <strong>
                      {t(`onboarding.check.${check.name}`, { defaultValue: check.name })}
                    </strong>
                    {!check.required ? (
                      <span className="melodee-panel__subtitle"> {t('onboarding.checkOptional')}</span>
                    ) : null}
                  </StructuredListCell>
                  <StructuredListCell>
                    {check.passed ? (
                      <StatusTag kind="success" label={t('onboarding.checkPassed')} />
                    ) : check.required ? (
                      <StatusTag kind="error" label={t('onboarding.checkFailed')} />
                    ) : (
                      <StatusTag kind="warning" label={t('onboarding.checkFailed')} />
                    )}
                  </StructuredListCell>
                  <StructuredListCell>
                    <p>{check.detail}</p>
                    {check.remediation ? (
                      <p className="melodee-panel__subtitle">
                        {t('onboarding.remediationLabel')}: {check.remediation}
                      </p>
                    ) : null}
                  </StructuredListCell>
                </StructuredListRow>
              ))}
            </StructuredListBody>
            </StructuredListWrapper>
          </div>
          <div className="melodee-panel__footer" style={{ marginBlockStart: '1rem' }}>
            <Button
              kind="secondary"
              size="sm"
              renderIcon={Renew}
              onClick={() => void fetchStatus({ cancelled: false })}
            >
              {t('onboarding.rerun')}
            </Button>
            <Button
              kind="primary"
              size="sm"
              disabled={!status.canComplete || busy}
              title={!status.canComplete ? t('onboarding.completeBlocked') : undefined}
              onClick={complete}
            >
              {t('onboarding.completeSetup')}
            </Button>
          </div>
          {!status.canComplete ? (
            <p className="melodee-panel__subtitle">{t('onboarding.completeBlocked')}</p>
          ) : null}
        </>
      ) : null}
    </>
  );
}

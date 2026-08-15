import { useTranslation } from 'react-i18next';

import { StatusTag, type StatusKind } from './StatusTag';

export type HealthState = 'online' | 'offline' | 'checking' | 'unknown' | 'degraded' | 'disabled';

const HEALTH_KINDS: Record<HealthState, StatusKind> = {
  online: 'success',
  offline: 'error',
  checking: 'info',
  unknown: 'neutral',
  degraded: 'warning',
  disabled: 'neutral',
};

/**
 * Health status indicator (never color-only, UI-SPEC §5). Used for the
 * header server status and for per-subsystem health rows; `label` overrides
 * the default localized state label when the caller has more precise
 * vocabulary (e.g. subsystem verdicts).
 */
export function HealthIndicator({ state, label }: { state: HealthState; label?: string }) {
  const { t } = useTranslation();
  return <StatusTag kind={HEALTH_KINDS[state]} label={label ?? t(`health.${state}`)} />;
}

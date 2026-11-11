import { useTranslation } from 'react-i18next';

import { StatusChip, type StatusKind } from './StatusTag';

export type HealthState = 'online' | 'offline' | 'checking' | 'unknown' | 'degraded' | 'disabled';

const HEALTH_KINDS: Record<HealthState, StatusKind> = {
  online: 'success',
  offline: 'error',
  checking: 'info',
  unknown: 'neutral',
  degraded: 'warning',
  disabled: 'neutral',
};

/** Tone used by dots and verdict text for a health state. */
export type HealthTone = 'success' | 'warning' | 'error' | 'info' | 'neutral';

const HEALTH_TONE: Record<HealthState, HealthTone> = {
  online: 'success',
  offline: 'error',
  checking: 'info',
  unknown: 'neutral',
  degraded: 'warning',
  disabled: 'neutral',
};

/**
 * Health status indicator (never color-only, UI-SPEC §5). Used for the header
 * server status and for per-subsystem health rows; `label` overrides the
 * default localized state label when the caller has more precise vocabulary
 * (e.g. subsystem verdicts).
 */
export function HealthIndicator({ state, label }: { state: HealthState; label?: string }) {
  const { t } = useTranslation();
  return <StatusChip kind={HEALTH_KINDS[state]} label={label ?? t(`health.${state}`)} />;
}

/**
 * Compact dot + label pair (UI-SPEC §6.1, ADR-0184). The reference renders
 * subsystem and server health as a colored dot followed by text, which reads
 * faster in dense rails than a pill. The label is always present, so the dot is
 * decorative — `aria-hidden` — and never carries the meaning alone.
 */
export function HealthDot({
  state,
  label,
  tone,
  className,
}: {
  state: HealthState;
  label: string;
  /** Override the dot tone when the caller distinguishes more verdicts. */
  tone?: HealthTone;
  className?: string;
}) {
  const resolvedTone = tone ?? HEALTH_TONE[state];
  return (
    <span className={className}>
      <span
        aria-hidden
        className={`melodee-health-row__dot${
          resolvedTone === 'neutral' ? '' : ` melodee-health-row__dot--${resolvedTone}`
        }`}
      />
      {label}
    </span>
  );
}

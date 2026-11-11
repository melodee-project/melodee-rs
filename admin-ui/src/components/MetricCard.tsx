import type { ReactNode } from 'react';

import { StatusChip, type StatusKind } from './StatusTag';

/**
 * KPI tile for overview/health surfaces (UI-SPEC §6.1, ADR-0184). Mirrors the
 * reference: label, large numeral, then a secondary line that is either a
 * delta, a hint, or a status chip. Value text is passed pre-formatted (use the
 * i18n Intl helpers) so locale formatting stays centralized.
 */
export type MetricDeltaTone = 'default' | 'warning' | 'error' | 'muted';

export function MetricCard({
  label,
  value,
  delta,
  hint,
  status,
  description,
  children,
}: {
  label: string;
  value: ReactNode;
  /** Secondary line under the numeral, e.g. "+12 today" or "8 active now". */
  delta?: { text: string; tone?: MetricDeltaTone };
  /** Static explanatory line, used when there is no delta. */
  hint?: string;
  status?: StatusKind;
  description?: string;
  children?: ReactNode;
}) {
  const deltaTone = delta?.tone ?? 'default';
  return (
    <div
      className="melodee-metric-card"
      aria-label={description ? `${label}: ${description}` : label}
    >
      <p className="melodee-metric-card__label">{label}</p>
      <p className="melodee-metric-card__value">{value}</p>
      {delta ? (
        <p
          className={`melodee-metric-card__delta${
            deltaTone === 'default' ? '' : ` melodee-metric-card__delta--${deltaTone}`
          }`}
        >
          {delta.text}
        </p>
      ) : null}
      {hint && !delta ? <p className="melodee-metric-card__hint">{hint}</p> : null}
      {status ? <StatusChip kind={status} /> : null}
      {children}
    </div>
  );
}

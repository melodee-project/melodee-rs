import { Tile } from '@carbon/react';
import type { ReactNode } from 'react';

import { StatusTag, type StatusKind } from './StatusTag';

/**
 * KPI tile for overview/health surfaces. Value text is passed pre-formatted
 * (use the i18n Intl helpers) so locale formatting stays centralized.
 */
export function MetricCard({
  label,
  value,
  status,
  description,
  children,
}: {
  label: string;
  value: ReactNode;
  status?: StatusKind;
  description?: string;
  children?: ReactNode;
}) {
  return (
    <Tile className="melodee-metric-card" aria-label={description ? `${label}: ${description}` : label}>
      <p className="melodee-metric-card__label">{label}</p>
      <p className="melodee-metric-card__value">{value}</p>
      {status ? <StatusTag kind={status} /> : null}
      {children}
    </Tile>
  );
}

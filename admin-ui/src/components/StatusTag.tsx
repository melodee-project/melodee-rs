import {
  CheckmarkFilled,
  ErrorFilled,
  InProgress,
  PendingFilled,
  Subtract,
  WarningFilled,
} from '@carbon/icons-react';
import { Tag } from '@carbon/react';
import { useTranslation } from 'react-i18next';

/**
 * The six-way status semantic (UI-SPEC §5). Status is never color-only:
 * every render carries an icon, a localized label, and a description exposed
 * via the accessible name/tooltip.
 */
export type StatusKind = 'success' | 'info' | 'warning' | 'error' | 'provisional' | 'neutral';

type CarbonTagType = 'green' | 'blue' | 'red' | 'gray';

const CARBON_TAG_TYPES: Partial<Record<StatusKind, CarbonTagType>> = {
  success: 'green',
  info: 'blue',
  error: 'red',
  neutral: 'gray',
};

const ICONS: Record<StatusKind, typeof CheckmarkFilled> = {
  success: CheckmarkFilled,
  info: InProgress,
  warning: WarningFilled,
  error: ErrorFilled,
  provisional: PendingFilled,
  neutral: Subtract,
};

export interface StatusTagProps {
  kind: StatusKind;
  /** Override the default localized label for this kind. */
  label?: string;
  /** Override the default localized description for this kind. */
  description?: string;
}

export function StatusTag({ kind, label, description }: StatusTagProps) {
  const { t } = useTranslation();
  const resolvedLabel = label ?? t(`status.${kind}`);
  const resolvedDescription = description ?? t(`status.${kind}Description`);
  const Icon = ICONS[kind];
  const carbonType = CARBON_TAG_TYPES[kind];
  const customClass =
    kind === 'warning'
      ? 'melodee-status-tag--warning'
      : kind === 'provisional'
        ? 'melodee-status-tag--provisional'
        : undefined;
  return (
    // The wrapper carries the tooltip; the visually-hidden description keeps
    // the semantic meaning available to assistive tech without relying on
    // hover (UI-SPEC §9: no hover-only information).
    <span className="melodee-status-tag" title={resolvedDescription}>
      <Tag
        type={carbonType ?? 'outline'}
        className={customClass}
        aria-label={`${resolvedLabel}. ${resolvedDescription}`}
      >
        <Icon aria-hidden size={12} /> {resolvedLabel}
        <span className="cds--visually-hidden"> {resolvedDescription}</span>
      </Tag>
    </span>
  );
}

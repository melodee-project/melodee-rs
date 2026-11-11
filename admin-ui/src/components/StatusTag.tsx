import {
  CheckmarkFilled,
  ErrorFilled,
  InProgress,
  PendingFilled,
  Subtract,
  WarningFilled,
} from '@carbon/icons-react';
import type { ComponentType } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * The six-way status semantic (UI-SPEC §5) rendered as a Melodee status chip
 * (UI-SPEC §6.1, ADR-0184). Status is never color-only: every render carries an
 * icon, a localized label, and a description exposed via the accessible name
 * and tooltip.
 *
 * The chip is Melodee-owned rather than a Carbon `Tag` because the reference
 * treatment is a flat tinted pill with a tinted ink (no ring, no border), which
 * Carbon tags cannot express without forking their internals.
 */
export type StatusKind = 'success' | 'info' | 'warning' | 'error' | 'provisional' | 'neutral';

const ICONS: Record<
  StatusKind,
  ComponentType<{ size?: number; className?: string; 'aria-hidden'?: boolean }>
> = {
  success: CheckmarkFilled,
  info: InProgress,
  warning: WarningFilled,
  error: ErrorFilled,
  provisional: PendingFilled,
  neutral: Subtract,
};

export interface StatusChipProps {
  kind: StatusKind;
  /** Override the default localized label for this kind. */
  label?: string;
  /** Override the default localized description for this kind. */
  description?: string;
  /** Hide the icon where a denser surface needs it (never hides the text). */
  hideIcon?: boolean;
}

export function StatusChip({ kind, label, description, hideIcon = false }: StatusChipProps) {
  const { t } = useTranslation();
  const resolvedLabel = label ?? t(`status.${kind}`);
  const resolvedDescription = description ?? t(`status.${kind}Description`);
  const Icon = ICONS[kind];
  return (
    // The wrapper carries the tooltip; the visually-hidden description keeps
    // the semantic meaning available to assistive tech without relying on
    // hover (UI-SPEC §9: no hover-only information).
    <span
      className={`melodee-chip melodee-chip--${kind}`}
      title={resolvedDescription}
      aria-label={`${resolvedLabel}. ${resolvedDescription}`}
    >
      {hideIcon ? null : <Icon className="melodee-chip__icon" size={12} aria-hidden />}
      <span>{resolvedLabel}</span>
      <span className="cds--visually-hidden"> {resolvedDescription}</span>
    </span>
  );
}

/** Historical name for `StatusChip`; both exports render the same chip. */
export const StatusTag = StatusChip;

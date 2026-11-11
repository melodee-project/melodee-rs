import type { ReactNode } from 'react';

import { StatusChip } from './StatusTag';

/**
 * Shared empty/placeholder state (UI-SPEC §6.1). Placeholder screens, empty
 * tables, denied access, and 404s all render through this so the vocabulary
 * stays consistent and localized.
 */
export function EmptyState({
  title,
  description,
  action,
  tone,
  compact = false,
}: {
  title: string;
  description?: string;
  action?: ReactNode;
  tone?: 'default' | 'error' | 'success';
  compact?: boolean;
}) {
  return (
    <div
      role="status"
      className={`melodee-empty${compact ? ' melodee-empty--compact' : ''}`}
      style={{ paddingBlock: compact ? 'var(--melodee-space-4)' : 'var(--melodee-space-8)' }}
    >
      {tone === 'success' ? (
        <StatusChip kind="success" label={title} />
      ) : tone === 'error' ? (
        <StatusChip kind="error" label={title} />
      ) : (
        <p className="melodee-empty__title">{title}</p>
      )}
      {description ? <p className="melodee-empty__description">{description}</p> : null}
      {action}
    </div>
  );
}

import { Heading } from '@carbon/react';
import type { ReactNode } from 'react';

import { StatusTag } from './StatusTag';

/**
 * Shared empty/placeholder state. Placeholder screens, empty tables, denied
 * access, and 404s all render through this so the vocabulary stays
 * consistent and localized.
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
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        textAlign: 'center',
        gap: '0.5rem',
        paddingBlockStart: compact ? '1rem' : '4rem',
        paddingBlockEnd: compact ? '1rem' : '4rem',
      }}
    >
      {tone === 'success' ? (
        <StatusTag kind="success" label={title} />
      ) : tone === 'error' ? (
        <StatusTag kind="error" label={title} />
      ) : (
        <Heading style={{ fontSize: '1.25rem' }}>{title}</Heading>
      )}
      {description ? (
        <p style={{ color: 'var(--cds-text-secondary)', maxInlineSize: '32rem' }}>{description}</p>
      ) : null}
      {action}
    </div>
  );
}

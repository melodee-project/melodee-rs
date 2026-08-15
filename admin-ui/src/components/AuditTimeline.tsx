import { Heading, StructuredListBody, StructuredListCell, StructuredListRow, StructuredListWrapper } from '@carbon/react';
import { useTranslation } from 'react-i18next';

import { formatDateTime, formatRelativeTime } from '../i18n/format';
import { EmptyState } from './EmptyState';

export interface AuditEvent {
  id: string;
  occurredAt: string;
  action: string;
  actor?: string | null;
  reason?: string | null;
}

/**
 * Append-only audit/history rendering ("what happened and why"). When
 * `relative` is set the timestamp renders as locale relative time with the
 * absolute date-time available via the accessible tooltip.
 */
export function AuditTimeline({
  events,
  relative = false,
}: {
  events: AuditEvent[];
  relative?: boolean;
}) {
  const { t } = useTranslation();
  if (events.length === 0) {
    return <EmptyState title={t('audit.empty')} compact />;
  }
  return (
    <section aria-label={t('audit.title')}>
      <StructuredListWrapper>
        <StructuredListBody>
          {events.map((event) => (
            <StructuredListRow key={event.id}>
              <StructuredListCell>
                {relative ? (
                  <span title={formatDateTime(event.occurredAt)}>
                    {formatRelativeTime(event.occurredAt)}
                  </span>
                ) : (
                  formatDateTime(event.occurredAt)
                )}
              </StructuredListCell>
              <StructuredListCell>
                <Heading style={{ fontSize: '0.875rem' }}>{event.action}</Heading>
                {event.actor ? (
                  <span style={{ color: 'var(--cds-text-secondary)' }}>{event.actor}</span>
                ) : null}
              </StructuredListCell>
              <StructuredListCell>{event.reason ?? t('common.none')}</StructuredListCell>
            </StructuredListRow>
          ))}
        </StructuredListBody>
      </StructuredListWrapper>
    </section>
  );
}

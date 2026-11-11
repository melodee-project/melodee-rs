import { useTranslation } from 'react-i18next';

import { formatDateTime, formatRelativeTime } from '../i18n/format';
import { EmptyState } from './EmptyState';
import { TimelineList, type TimelineTone } from './TimelineList';

export interface AuditEvent {
  id: string;
  occurredAt: string;
  action: string;
  actor?: string | null;
  reason?: string | null;
}

/** Maps an audit action name onto a timeline marker tone. */
function toneForAction(action: string): TimelineTone {
  const normalized = action.toLowerCase();
  if (normalized.includes('fail') || normalized.includes('reject') || normalized.includes('delete')) {
    return 'error';
  }
  if (normalized.includes('review') || normalized.includes('warn') || normalized.includes('quarantine')) {
    return 'warning';
  }
  if (normalized.includes('catalog') || normalized.includes('publish') || normalized.includes('confirm')) {
    return 'success';
  }
  return 'info';
}

/**
 * Append-only audit/history rendering ("what happened and why") on the shared
 * timeline primitive (UI-SPEC §6.1). When `relative` is set the timestamp
 * renders as locale relative time with the absolute date-time available via the
 * accessible tooltip.
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
      <TimelineList
        events={events.map((event) => ({
          id: event.id,
          title: event.action,
          detail: (
            <>
              {event.actor ? <span>{event.actor}</span> : null}
              {event.actor ? <span aria-hidden> · </span> : null}
              <span>{event.reason ?? t('common.none')}</span>
            </>
          ),
          time: relative ? (
            <span title={formatDateTime(event.occurredAt)}>
              {formatRelativeTime(event.occurredAt)}
            </span>
          ) : (
            formatDateTime(event.occurredAt)
          ),
          tone: toneForAction(event.action),
        }))}
      />
    </section>
  );
}

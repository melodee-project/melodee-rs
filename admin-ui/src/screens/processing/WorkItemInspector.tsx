import {
  Button,
  Heading,
  InlineNotification,
  SkeletonText,
  StructuredListBody,
  StructuredListCell,
  StructuredListRow,
  StructuredListWrapper,
} from '@carbon/react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { getWorkItem, getWorkItemEvents } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { WorkItem, WorkItemEvent } from '../../api/types';
import { AuditTimeline, type AuditEvent } from '../../components/AuditTimeline';
import { InspectorPanel } from '../../components/InspectorPanel';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { formatDateTime } from '../../i18n/format';
import { workItemDisplayName } from './workItem';

/**
 * Right-rail work-item inspector (UI-SPEC §7): facts, a derived next-action
 * explanation, and the append-only event history. No mutation actions exist
 * over the admin API yet (decision/reprocess endpoints ship with WP-12), so
 * the action renders disabled with an explanation rather than faking a call.
 * The parent remounts this component per work item (`key={workItemId}`) so
 * switching selection starts from a clean loading state.
 */

interface InspectorData {
  item: WorkItem;
  events: WorkItemEvent[];
}

function toAuditEvent(event: WorkItemEvent): AuditEvent {
  const transition =
    event.fromState && event.toState ? `${event.fromState} → ${event.toState}` : null;
  return {
    id: String(event.id),
    occurredAt: event.createdAt,
    action: transition ? `${event.eventType}: ${transition}` : event.eventType,
    actor: event.workerId,
    reason: event.reasonCode,
  };
}

export function WorkItemInspector({
  workItemId,
  onClose,
}: {
  workItemId: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [data, setData] = useState<InspectorData | null>(null);
  const [error, setError] = useState<ApiError | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([getWorkItem(workItemId), getWorkItemEvents(workItemId)])
      .then(([item, events]) => {
        if (!cancelled) {
          setData({ item, events });
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setError(cause instanceof ApiError ? cause : new ApiError(0, 'unknown', '', null));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [workItemId]);

  const title = data ? workItemDisplayName(data.item) : t('queue.itemFallback');

  const facts: { label: string; value: string }[] = data
    ? [
        { label: t('queue.fact.reason'), value: data.item.stateReason },
        { label: t('queue.fact.attempts'), value: String(data.item.attemptCount) },
        { label: t('queue.fact.worker'), value: data.item.leaseOwner },
        { label: t('queue.fact.source'), value: data.item.sourcePath },
        { label: t('queue.fact.currentPath'), value: data.item.currentPath },
        { label: t('queue.fact.targetPath'), value: data.item.targetPath },
        { label: t('queue.fact.fingerprint'), value: data.item.sourceFingerprint },
        {
          label: t('queue.fact.lastError'),
          value: data.item.lastErrorMessage
            ? data.item.lastErrorCode
              ? `${data.item.lastErrorCode}: ${data.item.lastErrorMessage}`
              : data.item.lastErrorMessage
            : null,
        },
        { label: t('queue.fact.discovered'), value: formatDateTime(data.item.discoveredAt) },
        { label: t('queue.fact.added'), value: formatDateTime(data.item.createdAt) },
        { label: t('queue.fact.updated'), value: formatDateTime(data.item.updatedAt) },
        {
          label: t('queue.fact.lastAttempt'),
          value: data.item.lastAttemptAt ? formatDateTime(data.item.lastAttemptAt) : null,
        },
        {
          label: t('queue.fact.nextAttempt'),
          value: data.item.nextAttemptAt ? formatDateTime(data.item.nextAttemptAt) : null,
        },
        {
          label: t('queue.fact.completed'),
          value: data.item.completedAt ? formatDateTime(data.item.completedAt) : null,
        },
      ].flatMap(({ label, value }) => (value ? [{ label, value }] : []))
    : [];

  return (
    <InspectorPanel title={title} open onClose={onClose}>
      {error ? (
        <InlineNotification
          kind="error"
          lowContrast
          title={t('queue.inspector.loadError')}
          subtitle={error.message}
          hideCloseButton
        />
      ) : null}
      {!data && !error ? <SkeletonText paragraph lineCount={6} /> : null}
      {data ? (
        <>
          <StructuredListWrapper aria-label={t('queue.inspector.facts')}>
            <StructuredListBody>
              <StructuredListRow>
                <StructuredListCell>{t('queue.fact.state')}</StructuredListCell>
                <StructuredListCell>
                  <ProcessingStateTag state={data.item.state} />
                </StructuredListCell>
              </StructuredListRow>
              {facts.map((fact) => (
                <StructuredListRow key={fact.label}>
                  <StructuredListCell>{fact.label}</StructuredListCell>
                  <StructuredListCell>{fact.value}</StructuredListCell>
                </StructuredListRow>
              ))}
            </StructuredListBody>
          </StructuredListWrapper>

          <section aria-labelledby="queue-next-action">
            <Heading id="queue-next-action" style={{ fontSize: '1rem', marginBlockStart: '1rem' }}>
              {t('queue.nextAction.title')}
            </Heading>
            <p style={{ color: 'var(--cds-text-secondary)' }}>
              {t(`queue.nextAction.${data.item.state}`, {
                defaultValue: t('queue.nextAction.unknown'),
              })}
            </p>
            <Button kind="tertiary" size="sm" disabled title={t('queue.actions.unavailable')}>
              {t('queue.actions.reprocess')}
            </Button>
          </section>

          <div style={{ marginBlockStart: '1rem' }}>
            <AuditTimeline events={data.events.map(toAuditEvent)} />
          </div>
        </>
      ) : null}
    </InspectorPanel>
  );
}

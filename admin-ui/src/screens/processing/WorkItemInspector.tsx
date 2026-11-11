import { ArrowRight } from '@carbon/icons-react';
import { Button, InlineNotification, SkeletonText } from '@carbon/react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink } from 'react-router-dom';

import { getWorkItem, getWorkItemEvents } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { WorkItem, WorkItemEvent } from '../../api/types';
import { AuditTimeline, type AuditEvent } from '../../components/AuditTimeline';
import { EntityAvatar } from '../../components/EntityAvatar';
import { InspectorPanel } from '../../components/InspectorPanel';
import { KeyValueList } from '../../components/KeyValueList';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { ProgressBar } from '../../components/ProgressBar';
import { Surface } from '../../components/Surface';
import { formatBytes, formatDateTime } from '../../i18n/format';
import { phaseForState } from './phase';
import { stageProgress, workItemDisplayName } from './workItem';

/**
 * Right-rail work-item inspector (UI-SPEC §7, ADR-0184): the reference's
 * "Release Inspector" card — summary block, state/progress facts, the derived
 * next action, and the append-only event history.
 *
 * No mutation actions exist over the admin API yet (decision/reprocess
 * endpoints ship with the review wave), so actions render disabled with an
 * explanation rather than faking a call. The parent remounts this component per
 * work item (`key={workItemId}`) so switching selection starts clean.
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

  const title = data
    ? (data.item.releaseTitle ?? workItemDisplayName(data.item))
    : t('queue.itemFallback');
  const stageLabel = data
    ? t(`queue.state.${data.item.state}`, { defaultValue: data.item.state })
    : '';

  const facts = data
    ? [
        { term: t('queue.fact.state'), value: <ProcessingStateTag state={data.item.state} /> },
        {
          term: t('phase.label'),
          value: t(`phase.${data.item.phase ?? phaseForState(data.item.state)}`),
        },
        {
          term: t('queue.column.progress'),
          value: (
            <ProgressBar
              value={stageProgress(data.item.state)}
              label={t('queue.progress.label', { title })}
              stage={stageLabel}
            />
          ),
        },
        { term: t('overview.queue.column.artist'), value: data.item.artistName ?? t('queue.unbound') },
        { term: t('queue.column.size'), value: data.item.mediaBytes ? formatBytes(data.item.mediaBytes) : '—' },
        { term: t('queue.fact.attempts'), value: String(data.item.attemptCount) },
        { term: t('queue.fact.state'), value: data.item.stateReason ?? '—' },
        { term: t('queue.fact.worker'), value: data.item.leaseOwner ?? '—' },
        { term: t('queue.fact.source'), value: data.item.sourcePath },
        { term: t('queue.fact.currentPath'), value: data.item.currentPath },
        { term: t('queue.fact.targetPath'), value: data.item.targetPath ?? '—' },
        { term: t('queue.fact.fingerprint'), value: data.item.sourceFingerprint ?? '—' },
        {
          term: t('queue.fact.lastError'),
          value: data.item.lastErrorMessage
            ? data.item.lastErrorCode
              ? `${data.item.lastErrorCode}: ${data.item.lastErrorMessage}`
              : data.item.lastErrorMessage
            : '—',
        },
        { term: t('queue.fact.discovered'), value: formatDateTime(data.item.discoveredAt) },
        { term: t('queue.fact.added'), value: formatDateTime(data.item.createdAt) },
        { term: t('queue.fact.updated'), value: formatDateTime(data.item.updatedAt) },
        {
          term: t('queue.fact.completed'),
          value: data.item.completedAt ? formatDateTime(data.item.completedAt) : '—',
        },
      ]
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
          <div className="melodee-review-header">
            <EntityAvatar name={data.item.releaseTitle ?? title} size={64} decorative />
            <div className="melodee-review-header__title">
              <p className="melodee-hero__title" style={{ fontSize: '1.25rem' }}>
                {title}
              </p>
              <p className="melodee-hero__subtitle">
                {data.item.artistName ?? t('queue.unbound')}
              </p>
              <div className="melodee-hero__chips">
                <ProcessingStateTag state={data.item.state} />
              </div>
            </div>
          </div>
          <div style={{ marginBlockStart: 'var(--melodee-space-4)' }}>
            <KeyValueList items={facts} ariaLabel={t('queue.inspector.summary')} />
          </div>
          <Surface
            title={t('queue.nextAction.title')}
            headingLevel={3}
            variant="inset"
            className="melodee-inspector__next-action"
          >
            <p className="melodee-surface__subtitle">
              {t(`queue.nextAction.${data.item.state}`, {
                defaultValue: t('queue.nextAction.unknown'),
              })}
            </p>
            <div className="melodee-inspector__actions">
              {data.item.state === 'needs_review' ? (
                <Button kind="primary" size="sm" as={RouterLink} to={`/processing/review/${data.item.id}`}>
                  {t('queue.openReview')}
                </Button>
              ) : (
                <Button kind="tertiary" size="sm" disabled title={t('queue.actions.unavailable')}>
                  {t('queue.actions.reprocess')}
                </Button>
              )}
              <Button
                kind="ghost"
                size="sm"
                as={RouterLink}
                to="/processing/review?state=quarantined"
                renderIcon={ArrowRight}
              >
                {t('queue.quarantineAction')}
              </Button>
            </div>
          </Surface>
          <div style={{ marginBlockStart: 'var(--melodee-space-4)' }}>
            <AuditTimeline events={data.events.map(toAuditEvent)} />
          </div>
        </>
      ) : null}
    </InspectorPanel>
  );
}

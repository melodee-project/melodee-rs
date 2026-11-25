import { useTranslation } from 'react-i18next';

import { StatusTag, type StatusKind } from './StatusTag';

/**
 * Maps ingestion/processing work-item states onto the six-way status
 * semantics (UI-SPEC §5). Invariants honored: classification ≠ failure,
 * quarantine is distinct from failed processing and ignored content.
 */
const STATE_KINDS: Record<string, StatusKind> = {
  // Release lifecycle: Discovered → Processing → Review → Cataloged
  discovered: 'neutral',
  new: 'info',
  ready: 'info',
  processing: 'info',
  indexing: 'info',
  active: 'info',
  in_progress: 'info',
  running: 'info',
  review: 'warning',
  needs_review: 'warning',
  settling: 'info',
  inspecting: 'info',
  normalizing: 'info',
  validating: 'info',
  publishing: 'info',
  published: 'success',
  cataloging: 'info',
  indexed: 'success',
  deleted: 'neutral',
  waiting: 'warning',
  pending: 'warning',
  uncertain: 'warning',
  quarantined: 'warning',
  cataloged: 'success',
  complete: 'success',
  completed: 'success',
  accepted: 'success',
  healthy: 'success',
  failed: 'error',
  rejected: 'error',
  error: 'error',
  // Legacy rows recorded the state string "provisional"; keep rendering them.
  provisional: 'pending',
  identity: 'pending',
  identity_review: 'pending',
  disabled: 'neutral',
  ignored: 'neutral',
  paused: 'neutral',
};

export function ProcessingStateTag({ state }: { state: string }) {
  const { t } = useTranslation();
  const normalized = state.trim().toLowerCase().replace(/[\s-]+/g, '_');
  if (normalized === 'staged') {
    return (
      <StatusTag
        kind="pending"
        label={t('queue.state.staged')}
        description={t('queue.waitingForPublisherDescription')}
      />
    );
  }
  const kind = STATE_KINDS[normalized] ?? 'neutral';
  // The state string is server data; it renders as the label while the
  // localized description anchors the semantic meaning.
  return <StatusTag kind={kind} label={state} description={t(`status.${kind}Description`)} />;
}

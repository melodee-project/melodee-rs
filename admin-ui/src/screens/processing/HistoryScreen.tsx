import { useTranslation } from 'react-i18next';

import type { IngestionState } from '../../api/types';
import { PageHeader } from '../../components/PageHeader';
import { WorkItemStateTable } from './WorkItemStateTable';

/**
 * Terminal-state set for the History view: completed processing
 * (cataloged / indexed) and final dispositions (ignored / deleted). The
 * work-items listing filters to exactly one state server-side, so the view
 * switches between these rather than fabricating a client-side union.
 */
const TERMINAL_STATES: readonly IngestionState[] = ['cataloged', 'indexed', 'ignored', 'deleted'];

/**
 * History (PRD-INGEST-010, SPEC §20): completed processing and disposition
 * history over the same durable work-item store; the inspector shows the
 * append-only event timeline. No review-workspace link — terminal states
 * accept no further operator decision.
 */
export function HistoryScreen() {
  const { t } = useTranslation();
  return (
    <>
      <PageHeader title={t('queue.historyTitle')} subtitle={t('queue.historySubtitle')} />
      <WorkItemStateTable
        states={TERMINAL_STATES}
        tableCaption={t('history.tableCaption')}
        emptyTitle={t('history.emptyTitle')}
        emptyDescription={t('history.emptyDescription')}
      />
    </>
  );
}

import { useTranslation } from 'react-i18next';

import { PageHeader } from '../../components/PageHeader';
import { WorkItemStateTable } from './WorkItemStateTable';

/**
 * Quarantine (PRD-INGEST-010, UI-SPEC §5): work items held apart from the
 * catalog. Quarantine is distinct from failed processing and ignored
 * content; each row links to the review workspace where the quarantine
 * decision is taken (the review queue accepts the quarantined state).
 */
export function QuarantineScreen() {
  const { t } = useTranslation();
  return (
    <>
      <PageHeader title={t('queue.quarantineTitle')} subtitle={t('queue.quarantineSubtitle')} />
      <WorkItemStateTable
        states={['quarantined']}
        reviewLinkStates={['quarantined']}
        tableCaption={t('quarantine.tableCaption')}
        emptyTitle={t('quarantine.emptyTitle')}
        emptyDescription={t('quarantine.emptyDescription')}
      />
    </>
  );
}

import { Heading } from '@carbon/react';
import { useTranslation } from 'react-i18next';

import { EvidenceItem, type Evidence } from './EvidenceItem';
import { EmptyState } from './EmptyState';

/** Grouped evidence block for review workspaces (evidence first, UI-SPEC §7). */
export function EvidenceSummary({ items, title }: { items: Evidence[]; title?: string }) {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="evidence-summary-heading">
      <Heading id="evidence-summary-heading" style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
        {title ?? t('evidence.summaryTitle')}
      </Heading>
      {items.length === 0 ? (
        <EmptyState title={t('empty.noData')} compact />
      ) : (
        items.map((item, index) => (
          <EvidenceItem key={`${item.source}-${index}`} evidence={item} />
        ))
      )}
    </section>
  );
}

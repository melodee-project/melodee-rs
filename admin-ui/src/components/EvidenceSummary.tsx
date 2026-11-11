import { useTranslation } from 'react-i18next';

import { EvidenceItem, type Evidence } from './EvidenceItem';
import { EvidenceTileGrid, type EvidenceTile } from './EvidenceTileGrid';
import { EmptyState } from './EmptyState';

/**
 * Grouped evidence block for review workspaces (evidence first, UI-SPEC §7).
 * When `tiles` is supplied, the reference's compact evidence counters render
 * above the itemized list so the operator reads the shape of the evidence
 * before its details.
 */
export function EvidenceSummary({
  items,
  title,
  tiles,
}: {
  items: Evidence[];
  title?: string;
  tiles?: EvidenceTile[];
}) {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="evidence-summary-heading">
      <h2 id="evidence-summary-heading" className="melodee-surface__title">
        {title ?? t('evidence.summaryTitle')}
      </h2>
      {tiles && tiles.length > 0 ? (
        <div style={{ marginBlock: 'var(--melodee-space-3)' }}>
          <EvidenceTileGrid tiles={tiles} />
        </div>
      ) : null}
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

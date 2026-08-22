import { useTranslation } from 'react-i18next';

import { formatNumber } from '../i18n/format';

/**
 * One evidence row in a review workspace: the original observation always
 * shown alongside the resolved value (UI-SPEC §7 review pattern).
 */
export interface Evidence {
  /** Where this evidence came from (tag, filename, provider, …). */
  source: string;
  observation: string;
  resolved?: string | null;
  /** 0..1 confidence when the resolver provides one. */
  confidence?: number | null;
}

export function EvidenceItem({ evidence }: { evidence: Evidence }) {
  const { t } = useTranslation();
  return (
    <dl className="melodee-evidence-item" style={{ marginBlockEnd: '0.75rem' }}>
      <dt className="melodee-evidence-item__label">{evidence.source}</dt>
      <dd style={{ marginInlineStart: 0 }}>
        <span aria-label={t('evidence.observation')}>{evidence.observation}</span>
        {evidence.resolved != null && evidence.resolved !== evidence.observation ? (
          <>
            {' → '}
            <strong aria-label={t('evidence.resolved')}>{evidence.resolved}</strong>
          </>
        ) : null}
        {evidence.confidence != null ? (
          <span className="melodee-evidence-item__label">
            {' '}
            ({t('evidence.confidence')} {formatNumber(Math.round(evidence.confidence * 100))}%)
          </span>
        ) : null}
      </dd>
    </dl>
  );
}

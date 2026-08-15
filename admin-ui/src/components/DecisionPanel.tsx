import { Button, Heading } from '@carbon/react';
import { useTranslation } from 'react-i18next';

import { StatusTag } from './StatusTag';

/**
 * Ordered decision choices (UI-SPEC §6): the recommended action is visually
 * primary; destructive choices render last and are never visually equal to
 * safe ones (PRD-UI-008).
 */
export interface DecisionChoice {
  id: string;
  label: string;
  description?: string;
  kind: 'recommended' | 'safe' | 'destructive';
}

export function DecisionPanel({
  choices,
  selected,
  onSelect,
}: {
  choices: DecisionChoice[];
  selected?: string | null;
  onSelect: (id: string) => void;
}) {
  const { t } = useTranslation();
  const recommended = choices.filter((choice) => choice.kind === 'recommended');
  const safe = choices.filter((choice) => choice.kind === 'safe');
  const destructive = choices.filter((choice) => choice.kind === 'destructive');
  const ordered = [...recommended, ...safe, ...destructive];

  return (
    <section aria-labelledby="decision-panel-heading">
      <Heading id="decision-panel-heading" style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
        {t('decision.chooseAction')}
      </Heading>
      <div role="radiogroup" aria-labelledby="decision-panel-heading">
        {ordered.map((choice) => (
          <div key={choice.id} className="melodee-decision-panel__choice">
            <Button
              kind={choice.kind === 'destructive' ? 'danger--tertiary' : choice.kind === 'recommended' ? 'primary' : 'tertiary'}
              aria-pressed={selected === choice.id}
              onClick={() => onSelect(choice.id)}
            >
              {choice.label}
            </Button>
            {choice.kind === 'recommended' ? (
              <span className="melodee-decision-panel__badge">
                <StatusTag kind="success" label={t('decision.recommended')} />
              </span>
            ) : null}
            {choice.kind === 'destructive' ? (
              <span className="melodee-decision-panel__badge">
                <StatusTag kind="error" label={t('decision.destructiveChoice')} />
              </span>
            ) : null}
            {choice.description ? (
              <p style={{ color: 'var(--cds-text-secondary)', marginBlockStart: '0.25rem' }}>
                {choice.description}
              </p>
            ) : null}
          </div>
        ))}
      </div>
    </section>
  );
}

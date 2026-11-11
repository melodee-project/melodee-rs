import { useTranslation } from 'react-i18next';

import { DecisionActionList } from './DecisionActionList';

/**
 * Ordered decision choices (UI-SPEC §6): the recommended action is visually
 * primary; destructive choices render last and are never visually equal to
 * safe ones (PRD-UI-008). Thin semantic wrapper over `DecisionActionList` so
 * review screens keep their existing contract.
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
    <div role="radiogroup" aria-label={t('decision.chooseAction')}>
      <DecisionActionList
        onSelect={onSelect}
        actions={ordered.map((choice) => ({
          id: choice.id,
          label: choice.label,
          reason: choice.description,
          kind:
            choice.kind === 'destructive'
              ? 'danger'
              : choice.kind === 'recommended'
                ? 'primary'
                : 'secondary',
          chip:
            choice.kind === 'recommended'
              ? { kind: 'success' as const, label: t('decision.recommended') }
              : choice.kind === 'destructive'
                ? { kind: 'error' as const, label: t('decision.destructiveChoice') }
                : undefined,
          disabled: selected != null && selected === choice.id ? false : undefined,
        }))}
      />
    </div>
  );
}

import type { ReactNode } from 'react';

import { StatusChip, type StatusKind } from './StatusTag';

/**
 * Ordered decision actions (UI-SPEC §6.1, §7; PRD-UI-008). Mirrors the
 * reference rail: the recommended action is the single primary button, safe
 * alternatives are outlined, and destructive actions are tinted with their own
 * tint so they can never be mistaken for a safe choice.
 *
 * Accessibility contract: the button's accessible name is its full visible
 * text — label, chip, and reason — so a screen-reader user hears everything the
 * sighted operator sees and WCAG 2.5.3 (label in name) holds by construction.
 * Each action carries its own reason; a caller that needs typed confirmation
 * (delete-class) opens `DestructiveActionDialog` from `onSelect`.
 */
export type DecisionActionKind = 'primary' | 'secondary' | 'danger';

export interface DecisionAction {
  id: string;
  label: string;
  reason?: ReactNode;
  kind?: DecisionActionKind;
  /** Renders a chip beside the label (e.g. recommended / destructive). */
  chip?: { kind: StatusKind; label: string };
  disabled?: boolean;
}

export function DecisionActionList({
  actions,
  onSelect,
}: {
  actions: DecisionAction[];
  onSelect: (id: string) => void;
}) {
  return (
    <div className="melodee-actions">
      {actions.map((action) => (
        <button
          key={action.id}
          type="button"
          className={`melodee-action melodee-action--${action.kind ?? 'secondary'}`}
          disabled={action.disabled}
          onClick={() => onSelect(action.id)}
        >
          <span className="melodee-action__label">
            {action.label}
            {action.chip ? (
              <span className="melodee-action__badge">
                <StatusChip kind={action.chip.kind} label={action.chip.label} hideIcon />
              </span>
            ) : null}
          </span>
          {action.reason ? (
            <span className="melodee-action__reason">{action.reason}</span>
          ) : null}
        </button>
      ))}
    </div>
  );
}

import type { ReactNode } from 'react';

/**
 * Table cell content presets (UI-SPEC §6.1, ADR-0184). The reference renders a
 * primary name with a small secondary line beneath it (title + file path,
 * observation + release); these helpers keep that pattern out of screens.
 */
export function StackedCell({ primary, secondary }: { primary: ReactNode; secondary?: ReactNode }) {
  return (
    <>
      <span className="melodee-cell-stack__primary">{primary}</span>
      {secondary ? <span className="melodee-cell-stack__secondary">{secondary}</span> : null}
    </>
  );
}

/** Muted em dash for genuinely absent values (never a zero or empty string). */
export function AbsentCell({ label }: { label: string }) {
  return (
    <span className="melodee-absent" aria-label={label}>
      —
    </span>
  );
}

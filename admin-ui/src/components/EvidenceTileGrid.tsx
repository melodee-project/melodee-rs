import type { ReactNode } from 'react';

/**
 * Compact evidence counters (UI-SPEC §6.1, ADR-0184): the reference's row of
 * small tiles summarizing how many signals support a match or classification.
 * Tone is additive — the label and number always carry the meaning.
 */
export type EvidenceTileTone = 'default' | 'success' | 'warning' | 'error' | 'info';

export interface EvidenceTile {
  label: string;
  value: ReactNode;
  tone?: EvidenceTileTone;
}

export function EvidenceTileGrid({ tiles }: { tiles: EvidenceTile[] }) {
  return (
    <div className="melodee-tiles">
      {tiles.map((tile) => (
        <div
          key={tile.label}
          className={`melodee-tile${
            tile.tone && tile.tone !== 'default' ? ` melodee-tile--${tile.tone}` : ''
          }`}
        >
          <p className="melodee-tile__label">{tile.label}</p>
          <p className="melodee-tile__value">{tile.value}</p>
        </div>
      ))}
    </div>
  );
}

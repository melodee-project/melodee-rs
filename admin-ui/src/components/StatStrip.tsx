import type { ReactNode } from 'react';
import { Link as RouterLink } from 'react-router-dom';

/**
 * Numeral-focused stat row for detail screens (UI-SPEC §6.1, ADR-0184).
 * Mirrors the reference's five-cell summary under an Artist/Release hero.
 */
export interface StatStripItem {
  label: string;
  value: ReactNode;
  /** Optional in-app destination; the whole cell becomes a link. */
  to?: string;
}

export function StatStrip({ items, ariaLabel }: { items: StatStripItem[]; ariaLabel?: string }) {
  return (
    <div className="melodee-stat-strip" role="group" aria-label={ariaLabel}>
      {items.map((item) => {
        const body = (
          <>
            <p className="melodee-stat__value">{item.value}</p>
            <p className="melodee-stat__label">{item.label}</p>
          </>
        );
        return item.to ? (
          <RouterLink key={item.label} className="melodee-stat" to={item.to}>
            {body}
          </RouterLink>
        ) : (
          <div key={item.label} className="melodee-stat">
            {body}
          </div>
        );
      })}
    </div>
  );
}

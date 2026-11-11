import type { ReactNode } from 'react';

/**
 * Term/value pairs (UI-SPEC §6.1, ADR-0184). Replaces the per-screen fact
 * grids: values align to the inline end, optional detail renders beneath in
 * secondary text. Renders a real `<dl>` so the pairing is programmatic.
 */
export interface KeyValueItem {
  term: string;
  value: ReactNode;
  detail?: ReactNode;
}

export function KeyValueList({ items, ariaLabel }: { items: KeyValueItem[]; ariaLabel?: string }) {
  return (
    <dl className="melodee-kv" aria-label={ariaLabel}>
      {items.map((item) => (
        <div className="melodee-kv__row" key={item.term}>
          <dt className="melodee-kv__term">{item.term}</dt>
          <dd className="melodee-kv__value">
            {item.value}
            {item.detail ? <span className="melodee-kv__detail">{item.detail}</span> : null}
          </dd>
        </div>
      ))}
    </dl>
  );
}

import { useRef, type KeyboardEvent } from 'react';

/**
 * State filter pills (UI-SPEC §6.1, ADR-0184). The reference filters a queue
 * with a row of pill buttons carrying live counts — not Carbon's
 * ContentSwitcher, whose selected segment is a solid dark block that reads as a
 * tab bar rather than a filter.
 *
 * Semantics: a labeled group of toggle buttons using `aria-pressed`, with
 * roving arrow-key navigation so the group behaves like one composite control.
 */
export interface FilterPillItem {
  key: string;
  label: string;
  /** Pre-formatted count (the caller localizes the number). */
  count?: string;
}

export function FilterPills({
  items,
  value,
  onChange,
  ariaLabel,
  idPrefix = 'pill',
}: {
  items: FilterPillItem[];
  /** Active pill key; `null`/empty means "all". */
  value: string | null;
  onChange: (key: string | null) => void;
  ariaLabel: string;
  idPrefix?: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const keys = ['ArrowRight', 'ArrowLeft', 'Home', 'End'];
    if (!keys.includes(event.key)) {
      return;
    }
    const buttons = Array.from(
      containerRef.current?.querySelectorAll<HTMLButtonElement>('button') ?? [],
    );
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (current === -1) {
      return;
    }
    event.preventDefault();
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? buttons.length - 1
          : event.key === 'ArrowRight'
            ? (current + 1) % buttons.length
            : (current - 1 + buttons.length) % buttons.length;
    buttons[next]?.focus();
  };

  return (
    <div className="melodee-pills" role="group" aria-label={ariaLabel} ref={containerRef} onKeyDown={onKeyDown}>
      {items.map((item) => {
        const isActive = item.key === (value ?? '');
        return (
          <button
            key={item.key || '__all__'}
            id={`${idPrefix}-${item.key || 'all'}`}
            type="button"
            className={`melodee-pill${isActive ? ' melodee-pill--active' : ''}`}
            aria-pressed={isActive}
            onClick={() => onChange(item.key === '' ? null : item.key)}
          >
            <span>{item.label}</span>
            {item.count ? <span className="melodee-pill__count">{item.count}</span> : null}
          </button>
        );
      })}
    </div>
  );
}

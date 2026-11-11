import { useRef, type KeyboardEvent, type ReactNode } from 'react';

/**
 * Underlined tab bar (UI-SPEC §6.1, §7; ADR-0184). Screens keep ownership of
 * the active tab in the URL, so this is a presented list of toggle buttons
 * rather than Carbon Tabs — Carbon's MobileTabs/Tabs pair renders its own
 * panels and fights URL-reflected state.
 *
 * Roving arrow-key focus with Home/End, `aria-selected` per tab.
 */
export interface TabNavItem {
  key: string;
  label: ReactNode;
}

export function TabNav({
  tabs,
  value,
  onChange,
  ariaLabel,
}: {
  tabs: TabNavItem[];
  value: string;
  onChange: (key: string) => void;
  ariaLabel: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowRight', 'ArrowLeft', 'Home', 'End'].includes(event.key)) {
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
    <div className="melodee-tabnav" role="tablist" aria-label={ariaLabel} ref={containerRef} onKeyDown={onKeyDown}>
      {tabs.map((tab) => {
        const active = tab.key === value;
        return (
          <button
            key={tab.key}
            type="button"
            role="tab"
            id={`tab-${tab.key}`}
            aria-selected={active}
            tabIndex={active ? 0 : -1}
            className={`melodee-tab${active ? ' melodee-tab--active' : ''}`}
            onClick={() => onChange(tab.key)}
          >
            {tab.label}
          </button>
        );
      })}
    </div>
  );
}

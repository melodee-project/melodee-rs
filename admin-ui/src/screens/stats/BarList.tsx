import { useTranslation } from 'react-i18next';

/**
 * Horizontal bar list (ADR-0184): a labelled magnitude list with the value and an
 * optional detail rendered as text, so the comparison survives without color and
 * without a chart library. Bars are proportional to the largest value in the list.
 */
export interface BarListItem {
  key: string;
  label: string;
  value: number;
  valueText: string;
  detail?: string;
}

export function BarList({
  items,
  ariaLabel,
}: {
  items: BarListItem[];
  ariaLabel: string;
}) {
  const { t } = useTranslation();
  if (items.length === 0) {
    return null;
  }
  const max = Math.max(...items.map((item) => item.value), 1);
  return (
    <ul className="melodee-bar-list" aria-label={ariaLabel}>
      {items.map((item) => (
        <li key={item.key} className="melodee-bar-list__row">
          <div className="melodee-bar-list__head">
            <span className="melodee-bar-list__label">{item.label}</span>
            <span className="melodee-bar-list__value">{item.valueText}</span>
          </div>
          <div
            className="melodee-bar-list__track"
            role="img"
            aria-label={t('stats.barLabel', { label: item.label, value: item.valueText })}
          >
            <span
              className="melodee-bar-list__fill"
              style={{ inlineSize: `${Math.max(2, Math.round((item.value / max) * 100))}%` }}
            />
          </div>
          {item.detail ? <p className="melodee-bar-list__detail">{item.detail}</p> : null}
        </li>
      ))}
    </ul>
  );
}

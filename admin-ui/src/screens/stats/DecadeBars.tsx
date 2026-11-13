import { useTranslation } from 'react-i18next';

import { formatNumber } from '../../i18n/format';

/**
 * Vertical bar chart for a single bucketed series (releases by decade).
 *
 * ADR-0184 keeps this surface library-free: the plot is a token-styled SVG with a
 * value axis, and the same numbers are exposed as a visually-hidden table so the
 * comparison is available to assistive technology.
 */

const CHART_HEIGHT = 150;
const BAR_GAP = 8;

export interface DecadeSeries {
  decade: number;
  releases: number;
}

export function DecadeBars({
  series,
  labelFor,
  ariaLabel,
}: {
  series: DecadeSeries[];
  labelFor: (decade: number) => string;
  ariaLabel: string;
}) {
  const { t } = useTranslation();
  if (series.length === 0) {
    return <p className="melodee-stats-empty">{t('stats.library.noYears')}</p>;
  }
  const max = Math.max(...series.map((entry) => entry.releases), 1);
  const width = Math.max(240, series.length * 56);
  const barWidth = (width - BAR_GAP * (series.length - 1)) / series.length;

  return (
    <div className="melodee-decade-chart">
      <div className="melodee-decade-chart__plot">
        <ul className="melodee-decade-chart__axis" aria-hidden>
          {[max, Math.round(max / 2), 0].map((tick) => (
            <li key={tick}>{formatNumber(tick)}</li>
          ))}
        </ul>
        <svg
          className="melodee-decade-chart__svg"
          viewBox={`0 0 ${width} ${CHART_HEIGHT}`}
          preserveAspectRatio="none"
          role="img"
          aria-label={ariaLabel}
        >
          {series.map((entry, index) => {
            const height = Math.max(2, Math.round((entry.releases / max) * CHART_HEIGHT));
            return (
              <rect
                key={entry.decade}
                className="melodee-decade-chart__bar"
                x={index * (barWidth + BAR_GAP)}
                y={CHART_HEIGHT - height}
                width={barWidth}
                height={height}
                rx={2}
              />
            );
          })}
        </svg>
      </div>
      <ul className="melodee-decade-chart__labels" aria-hidden>
        {series.map((entry) => (
          <li key={entry.decade} className="melodee-decade-chart__label">
            {labelFor(entry.decade)}
          </li>
        ))}
      </ul>
      <table className="cds--visually-hidden">
        <caption>{ariaLabel}</caption>
        <thead>
          <tr>
            <th scope="col">{t('stats.library.decade')}</th>
            <th scope="col">{t('stats.library.releases')}</th>
          </tr>
        </thead>
        <tbody>
          {series.map((entry) => (
            <tr key={entry.decade}>
              <td>{labelFor(entry.decade)}</td>
              <td>{formatNumber(entry.releases)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

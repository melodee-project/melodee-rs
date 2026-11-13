import { useTranslation } from 'react-i18next';

import { formatDate, formatNumber } from '../../i18n/format';

/**
 * Daily play-count line chart for the Stats page.
 *
 * ADR-0184 keeps this surface chart-library-free: the plot is a token-styled SVG
 * with a value axis and hour-free day labels underneath, plus the same series as a
 * visually-hidden table. Axis labels are HTML (the plot stretches to the card
 * width, which would distort text drawn inside the SVG).
 */

const CHART_HEIGHT = 180;
const CHART_WIDTH = 720;

export interface DaySeries {
  day: string;
  plays: number;
  seconds: number;
}

export function PlaysChart({
  series,
  title,
  playLabel,
}: {
  series: DaySeries[];
  title: string;
  playLabel: string;
}) {
  const { t } = useTranslation();
  if (series.length === 0) {
    return <p className="melodee-stats-empty">{t('stats.listening.noPlays')}</p>;
  }
  const max = Math.max(...series.map((entry) => entry.plays), 1);
  const step = series.length <= 1 ? CHART_WIDTH : CHART_WIDTH / (series.length - 1);
  const coordinates = series.map((entry, index) => ({
    x: index * step,
    y: CHART_HEIGHT - (entry.plays / max) * CHART_HEIGHT,
  }));
  const line = coordinates
    .map((point, index) => `${index === 0 ? 'M' : 'L'} ${point.x.toFixed(1)} ${point.y.toFixed(1)}`)
    .join(' ');
  const area = `${line} L ${CHART_WIDTH} ${CHART_HEIGHT} L 0 ${CHART_HEIGHT} Z`;
  const valueTicks = [max, Math.round(max / 2), 0];
  const labelStride = Math.max(1, Math.ceil(series.length / 5));

  return (
    <div className="melodee-plays-chart">
      <h3 className="melodee-stats-panel__title">{title}</h3>
      <div className="melodee-plays-chart__plot">
        <ul className="melodee-plays-chart__value-axis" aria-hidden>
          {valueTicks.map((tick) => (
            <li key={tick}>{formatNumber(tick)}</li>
          ))}
        </ul>
        <svg
          className="melodee-plays-chart__svg"
          viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
          preserveAspectRatio="none"
          role="img"
          aria-label={t('stats.listening.perDayChartLabel', { series: playLabel })}
        >
          <path className="melodee-plays-chart__area" d={area} />
          <path className="melodee-plays-chart__line" d={line} />
        </svg>
        <ul className="melodee-plays-chart__time-axis" aria-hidden>
          {series.map((entry, index) =>
            index % labelStride === 0 || index === series.length - 1 ? (
              <li key={entry.day}>{formatDate(`${entry.day}T00:00:00`)}</li>
            ) : null,
          )}
        </ul>
      </div>
      <table className="cds--visually-hidden">
        <caption>{title}</caption>
        <thead>
          <tr>
            <th scope="col">{t('stats.listening.day')}</th>
            <th scope="col">{t('stats.listening.plays')}</th>
            <th scope="col">{t('stats.listening.seconds')}</th>
          </tr>
        </thead>
        <tbody>
          {series.map((entry) => (
            <tr key={entry.day}>
              <td>{entry.day}</td>
              <td>{formatNumber(entry.plays)}</td>
              <td>{formatNumber(entry.seconds)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

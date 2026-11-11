import { useTranslation } from 'react-i18next';

import type { DashboardActivityBucket } from '../../api/types';
import { formatDateTime, formatNumber } from '../../i18n/format';
import { layoutActivitySeries } from './overview';

/**
 * Accessible two-series line chart (ADR-0184). No chart library is approved for
 * this surface, so this is a small token-styled SVG: the graphic is `role="img"`
 * with a localized label and a visible legend, and the same two series render as
 * a visually-hidden data table for screen readers (UI-SPEC §9).
 */

const CHART_WIDTH = 720;
const CHART_HEIGHT = 190;
const GRID_LINES = 4;

export function ActivityChart({
  releases,
  tracks,
}: {
  releases: DashboardActivityBucket[];
  tracks: DashboardActivityBucket[];
}) {
  const { t } = useTranslation();
  const layout = layoutActivitySeries(releases, tracks, CHART_WIDTH, CHART_HEIGHT);
  if (layout.hours.length === 0) {
    return null;
  }
  const trackCounts = new Map(tracks.map((bucket) => [bucket.hour, bucket.count]));
  return (
    <>
      <svg
        className="melodee-activity-chart"
        viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={t('overview.activity.chartLabel')}
      >
        {Array.from({ length: GRID_LINES }, (_unused, index) => {
          const y = (CHART_HEIGHT / GRID_LINES) * index + 0.5;
          return (
            <line
              key={`grid-${index}`}
              className="melodee-activity-chart__grid"
              x1={0}
              x2={CHART_WIDTH}
              y1={y}
              y2={y}
            />
          );
        })}
        {layout.releases.path ? (
          <path
            className="melodee-activity-chart__line melodee-activity-chart__line--releases"
            d={layout.releases.path}
          />
        ) : null}
        {layout.tracks.path ? (
          <path
            className="melodee-activity-chart__line melodee-activity-chart__line--tracks"
            d={layout.tracks.path}
          />
        ) : null}
        <line
          className="melodee-activity-chart__baseline"
          x1={0}
          x2={CHART_WIDTH}
          y1={CHART_HEIGHT - 0.5}
          y2={CHART_HEIGHT - 0.5}
        />
      </svg>
      <div className="melodee-activity-chart__legend">
        <span className="melodee-activity-chart__legend-item">
          <span aria-hidden className="melodee-activity-chart__swatch melodee-activity-chart__swatch--releases" />
          {t('overview.activity.series.releases')}
        </span>
        <span className="melodee-activity-chart__legend-item">
          <span aria-hidden className="melodee-activity-chart__swatch melodee-activity-chart__swatch--tracks" />
          {t('overview.activity.series.tracks')}
        </span>
      </div>
      <div className="cds--visually-hidden">
      <table>
        <caption>{t('overview.activity.tableCaption')}</caption>
        <thead>
          <tr>
            <th scope="col">{t('overview.activity.column.hour')}</th>
            <th scope="col">{t('overview.activity.series.releases')}</th>
            <th scope="col">{t('overview.activity.column.indexed')}</th>
          </tr>
        </thead>
        <tbody>
          {layout.hours.map((hour) => {
            const releaseBucket = releases.find((bucket) => bucket.hour === hour);
            return (
              <tr key={hour}>
                <td>{formatDateTime(hour)}</td>
                <td>{formatNumber(releaseBucket?.count ?? 0)}</td>
                <td>{formatNumber(trackCounts.get(hour) ?? 0)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      </div>
    </>
  );
}

import { useTranslation } from "react-i18next";

import type { DashboardActivityBucket } from "../../api/types";
import { formatDateTime, formatNumber } from "../../i18n/format";
import { layoutBars } from "./overview";

/**
 * Accessible completed-per-hour bar chart . No chart library is
 * approved for , so this is a small token-styled SVG: the graphic is
 * role="img" with a localized label, and the same series renders as a
 * visually-hidden data table for screen readers (UI-SPEC §9). Bars use the
 * brand/status tokens, never raw hex.
 */

const CHART_WIDTH = 720;
const CHART_HEIGHT = 160;

export function ActivityChart({
  buckets,
}: {
  buckets: DashboardActivityBucket[];
}) {
  const { t } = useTranslation();
  const bars = layoutBars(buckets, CHART_WIDTH, CHART_HEIGHT);
  if (bars.length === 0) {
    return null;
  }
  return (
    <>
      <svg
        className="melodee-activity-chart"
        viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
        role="img"
        aria-label={t("overview.activity.chartLabel")}
        preserveAspectRatio="none"
      >
        {bars.map((bar) => (
          <rect
            key={bar.key}
            x={bar.x}
            y={bar.y}
            width={bar.width}
            height={bar.height}
            className="melodee-activity-chart__bar"
          />
        ))}
        <line
          x1={0}
          x2={CHART_WIDTH}
          y1={CHART_HEIGHT - 0.5}
          y2={CHART_HEIGHT - 0.5}
          className="melodee-activity-chart__baseline"
        />
      </svg>
      <table className="cds--visually-hidden">
        <caption>{t("overview.activity.tableCaption")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("overview.activity.column.hour")}</th>
            <th scope="col">{t("overview.activity.column.completed")}</th>
          </tr>
        </thead>
        <tbody>
          {buckets.map((bucket) => (
            <tr key={bucket.hour}>
              <td>{formatDateTime(bucket.hour)}</td>
              <td>{formatNumber(bucket.count)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

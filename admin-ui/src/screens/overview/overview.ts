import type {
  DashboardActivityBucket,
  IngestionState,
  WorkItemStateCount,
} from '../../api/types';

/**
 * Pure helpers for the overview dashboard: work-item state grouping
 * and activity-chart geometry. Kept component-free so they unit-test without
 * a DOM.
 */

/** Inbound: discovered and settling, i.e. arrived but not yet in the pipeline. */
export const INBOUND_STATES: readonly IngestionState[] = ['discovered', 'settling'];

/** Actively moving through the pipeline (excludes terminal states). */
export const ACTIVE_STATES: readonly IngestionState[] = [
  'ready',
  'inspecting',
  'normalizing',
  'validating',
  'staged',
  'publishing',
  'published',
  'cataloging',
  'indexing',
];

/** Sums the reported counts for the given states. The counts endpoint omits
 *  empty states, so absence means zero. */
export function countStates(
  counts: readonly WorkItemStateCount[],
  states: readonly IngestionState[],
): number {
  return counts.reduce(
    (sum, entry) => (states.includes(entry.state) ? sum + entry.count : sum),
    0,
  );
}

/** Buckets sorted ascending by hour; the backend omits empty hours. */
export function sortedBuckets(
  buckets: readonly DashboardActivityBucket[],
): DashboardActivityBucket[] {
  return [...buckets].sort((a, b) => Date.parse(a.hour) - Date.parse(b.hour));
}

export interface ChartBar {
  /** Bucket hour (ISO), used as the React key. */
  key: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Vertical bar geometry for the completed-per-hour series. */
export function layoutBars(
  buckets: readonly DashboardActivityBucket[],
  width: number,
  height: number,
): ChartBar[] {
  const sorted = sortedBuckets(buckets);
  if (sorted.length === 0) {
    return [];
  }
  const max = Math.max(1, ...sorted.map((bucket) => bucket.count));
  const slot = width / sorted.length;
  const barWidth = Math.max(1, Math.floor(slot * 0.6));
  return sorted.map((bucket, index) => {
    const barHeight = Math.max(1, Math.round((bucket.count / max) * height));
    return {
      key: bucket.hour,
      x: Math.round(index * slot + (slot - barWidth) / 2),
      y: height - barHeight,
      width: barWidth,
      height: barHeight,
    };
  });
}

/** One point in a two-series activity line chart. */
export interface LinePoint {
  x: number;
  y: number;
}

export interface LineSeries {
  /** SVG path data (`M … L …`), empty when the series has no points. */
  path: string;
  points: LinePoint[];
  /** Highest value in this series (the legend/table report it). */
  peak: number;
}

export interface ActivityLineLayout {
  hours: string[];
  /** Shared y-axis maximum, so both series are comparable on one scale. */
  max: number;
  releases: LineSeries;
  tracks: LineSeries;
}

function toPath(points: LinePoint[]): string {
  if (points.length === 0) {
    return '';
  }
  return points
    .map((point, index) => `${index === 0 ? 'M' : 'L'} ${point.x.toFixed(2)} ${point.y.toFixed(2)}`)
    .join(' ');
}

/**
 * Two-series line geometry for the activity card (ADR-0184). The reference
 * plots Releases processed and Tracks indexed on one shared scale; the hour
 * axis is the union of both series so a missing bucket is an honest gap rather
 * than a shifted point.
 */
export function layoutActivitySeries(
  releases: readonly DashboardActivityBucket[],
  tracks: readonly DashboardActivityBucket[],
  width: number,
  height: number,
): ActivityLineLayout {
  const hours = [...new Set([...releases, ...tracks].map((bucket) => bucket.hour))].sort(
    (a, b) => Date.parse(a) - Date.parse(b),
  );
  const max = Math.max(1, ...releases.map((bucket) => bucket.count), ...tracks.map((bucket) => bucket.count));
  const indexOf = new Map(hours.map((hour, index) => [hour, index]));

  const build = (series: readonly DashboardActivityBucket[]): LineSeries => {
    const counts = new Map(series.map((bucket) => [bucket.hour, bucket.count]));
    const peak = series.reduce((most, bucket) => Math.max(most, bucket.count), 0);
    const points = hours
      .filter((hour) => counts.has(hour))
      .map((hour) => {
        const index = indexOf.get(hour) ?? 0;
        const x = hours.length <= 1 ? width / 2 : (index / (hours.length - 1)) * width;
        const value = counts.get(hour) ?? 0;
        return { x, y: height - (value / max) * height };
      });
    return { path: toPath(points), points, peak };
  };

  return { hours, max, releases: build(releases), tracks: build(tracks) };
}

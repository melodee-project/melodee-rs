import type {
  DashboardActivityBucket,
  IngestionState,
  WorkItemStateCount,
} from '../../api/types';

/**
 * Pure helpers for the overview dashboard (WP-14): work-item state grouping
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

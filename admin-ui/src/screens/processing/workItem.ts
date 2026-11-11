import type { IngestionState, WorkItem } from '../../api/types';

/**
 * Display name for a work item: the last path segment of its current (or
 * source) path. This is a display value only — the work item id remains the
 * identity; paths never determine semantic identity (SPEC).
 */
export function workItemDisplayName(item: WorkItem): string {
  const path = item.currentPath || item.sourcePath;
  const segments = path.split(/[\\/]/).filter((segment) => segment.length > 0);
  return segments[segments.length - 1] ?? path;
}

/**
 * Deterministic stage progress for a work item (ADR-0184, decision D5).
 *
 * The ingestion state machine is discrete, so the queue shows the item's
 * *position in the documented lifecycle* rather than an invented elapsed-time
 * percentage. The mapping is total over `IngestionState`; terminal outcomes
 * report their own value (100 for success, 100 for quarantined/failed because
 * processing stopped, 0 for ignored/deleted because nothing was produced).
 */
const STAGE_PROGRESS: Record<IngestionState, number> = {
  discovered: 5,
  settling: 10,
  ready: 20,
  inspecting: 30,
  normalizing: 40,
  validating: 50,
  needs_review: 60,
  staged: 70,
  publishing: 80,
  published: 85,
  cataloging: 90,
  cataloged: 95,
  indexing: 98,
  indexed: 100,
  failed: 100,
  quarantined: 100,
  ignored: 0,
  deleted: 0,
};

/** Stage progress (0–100) for a work-item state. */
export function stageProgress(state: IngestionState): number {
  return STAGE_PROGRESS[state] ?? 0;
}

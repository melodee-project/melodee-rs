import type { WorkItem } from '../../api/types';

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

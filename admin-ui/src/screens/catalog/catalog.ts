import { useCallback, useEffect, useRef, useState } from 'react';

import { ApiError } from '../../api/client';
import type {
  CatalogAuditEvent,
  CatalogReleaseStatus,
  PolicyDisposition,
  ReleaseWorkflowState,
  SortParam,
} from '../../api/types';
import type { AuditEvent } from '../../components/AuditTimeline';
import type { StatusKind } from '../../components/StatusTag';

/**
 * Shared helpers for the catalog browsing + detail screens.
 * Pure mappings and the detail fetch hook; no rendering.
 */

/** Maps a catalog Artist/Release status onto the §5 status semantics. */
export function catalogStatusKind(status: string): StatusKind {
  switch (status) {
    case 'verified':
      return 'success';
    case 'active':
      return 'info';
    case 'merged':
    case 'deprecated':
      return 'neutral';
    default:
      return 'neutral';
  }
}

export interface ReleaseStatusContext {
  status: CatalogReleaseStatus;
  workflowState: ReleaseWorkflowState | null;
  policyDisposition: PolicyDisposition | null;
  policyEvidenceSummary: string | null;
}

/**
 * Converts the catalog visibility projection into operator language. A
 * withheld Release is described by its actionable workflow/policy state,
 * rather than the internal `deprecated` storage value.
 */
export function releaseStatusPresentation(release: ReleaseStatusContext): {
  kind: StatusKind;
  labelKey: string;
  description?: string;
} {
  const description = release.policyEvidenceSummary ?? undefined;
  switch (release.workflowState) {
    case 'needs_review':
      return { kind: 'warning', labelKey: 'status.needsReview', description };
    case 'quarantined':
      return { kind: 'error', labelKey: 'status.quarantined', description };
    case 'ignored':
      return { kind: 'neutral', labelKey: 'status.ignored', description };
    case 'deleted':
      return { kind: 'neutral', labelKey: 'queue.state.deleted', description };
    default:
      return {
        kind: catalogStatusKind(release.status),
        labelKey: `catalog.status.${release.status}`,
        description,
      };
  }
}

/** Parses `sort=key:asc|desc` against the backend whitelist for the list. */
export function parseCatalogSort(
  raw: string | null,
  whitelist: ReadonlySet<string>,
): SortParam | undefined {
  if (!raw) {
    return undefined;
  }
  const [key, direction] = raw.split(':');
  if (!key || (direction !== 'asc' && direction !== 'desc') || !whitelist.has(key)) {
    return undefined;
  }
  return `${key}:${direction}` as SortParam;
}

export function parsePageParam(raw: string | null): number {
  return Math.max(1, Number.parseInt(raw ?? '1', 10) || 1);
}

export function parsePageSizeParam(raw: string | null): number {
  return Math.max(1, Number.parseInt(raw ?? '50', 10) || 50);
}

export function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

/** Maps a catalog audit event onto the shared audit-timeline shape. */
export function toCatalogAuditEvent(event: CatalogAuditEvent): AuditEvent {
  return {
    id: event.eventId,
    occurredAt: event.createdAt,
    action: event.eventType,
    actor: event.actor,
    reason: event.reason,
  };
}

/** Ordinal display "1.7" when disc/track numbers exist, else the ordinal. */
export function trackOrdinalLabel(
  ordinal: number,
  discNumber: number | null,
  trackNumber: number | null,
  format: (value: number) => string,
): string {
  if (discNumber != null && trackNumber != null) {
    return `${format(discNumber)}.${format(trackNumber)}`;
  }
  return format(ordinal);
}

/**
 * Detail fetch with cancellation: the last error replaces the payload so a
 * failed id shows the retryable error instead of a stale entity.
 */
export function useCatalogDetail<T>(
  fetcher: (id: string, signal?: AbortSignal) => Promise<T>,
  id: string,
) {
  const [detail, setDetail] = useState<T | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const request = useRef<AbortController | null>(null);

  const fetchDetail = useCallback(
    () => {
      request.current?.abort();
      const controller = new AbortController();
      request.current = controller;
      return fetcher(id, controller.signal)
        .then((result) => {
          if (!controller.signal.aborted) {
            setDetail(result);
            setError(null);
            setLoading(false);
          }
        })
        .catch((cause: unknown) => {
          if (!controller.signal.aborted) {
            setError(toApiError(cause));
            setLoading(false);
          }
        });
    },
    [fetcher, id],
  );

  useEffect(() => {
    void fetchDetail();
    return () => request.current?.abort();
  }, [fetchDetail]);

  const retry = useCallback(() => {
    setLoading(detail === null);
    void fetchDetail();
  }, [detail, fetchDetail]);

  return { detail, error, loading, retry };
}

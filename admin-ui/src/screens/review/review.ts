import type {
  CatalogComparison,
  IngestionState,
  NoveltyTrack,
  ReviewDetail,
  ReviewManifestSummary,
  WorkItemEvent,
} from "../../api/types";
import type { AuditEvent } from "../../components/AuditTimeline";
import type { StatusKind } from "../../components/StatusTag";

/**
 * Shared helpers for the Release Review screens (queue + workspace).
 * Pure mappings only; no I/O.
 */

/** Review-able states, mirroring REVIEW_STATES in admin_review.rs. */
export const REVIEW_STATES: IngestionState[] = [
  "needs_review",
  "failed",
  "quarantined",
];

/** Decision actions in their canonical presentation order. */
export const REVIEW_ACTIONS = [
  "accept",
  "quarantine",
  "ignore",
  "delete",
] as const;
export type ReviewAction = (typeof REVIEW_ACTIONS)[number];

/** Maps a per-Track novelty category onto the §5 status semantics. */
export function noveltyKind(category: string): StatusKind {
  switch (category) {
    case "exact_duplicate":
    case "known_audio_identity":
      return "success";
    case "new_track":
      return "info";
    case "alternate_recording":
      return "provisional";
    case "uncertain":
    default:
      return "warning";
  }
}

/** Localized label key for a novelty category (falls back to uncertain). */
export function noveltyLabelKey(category: string): string {
  return `review.novelty.${category}`;
}

/**
 * Maps the durable policy disposition onto §5 status semantics for the
 * Policy Match recommendation tag. Disposition is not classification.
 */
export function dispositionKind(disposition: string): StatusKind {
  switch (disposition) {
    case "accept":
      return "success";
    case "review":
      return "warning";
    case "quarantine":
      return "warning";
    case "ignore":
      return "neutral";
    case "delete":
      return "error";
    default:
      return "neutral";
  }
}

/**
 * The action the policy decision recommends, when it is one of the four
 * operator actions. `delete` is never rendered as the primary/recommended
 * choice (PRD-UI-008) even when policy recommends it — the operator must
 * pick the destructive action deliberately.
 */
export function recommendedAction(detail: ReviewDetail | null): ReviewAction {
  const disposition = detail?.decision?.disposition;
  if (
    disposition === "accept" ||
    disposition === "quarantine" ||
    disposition === "ignore"
  ) {
    return disposition;
  }
  return "accept";
}

/** Maps a work-item event onto the shared audit-timeline shape. */
export function toAuditEvent(event: WorkItemEvent): AuditEvent {
  const transition =
    event.fromState && event.toState
      ? `${event.fromState} → ${event.toState}`
      : null;
  return {
    id: String(event.id),
    occurredAt: event.createdAt,
    action: transition ? `${event.eventType}: ${transition}` : event.eventType,
    actor: event.workerId,
    reason: event.reasonCode,
  };
}

/**
 * Per-track novelty lookup for the Track Breakdown result pill.
 *
 * The catalog comparison rows are keyed by catalog TrackId while manifest
 * tracks carry only their manifest index — there is no wire identity join.
 * Cataloging inserts tracks sequentially in manifest order and novelty rows
 * come back ordered by their v7 (time-ordered) track ids, so when the row
 * counts match exactly, position is a faithful correspondence. When counts
 * differ (partial cataloging) the column is omitted entirely rather than
 * showing a possibly misattributed result.
 */
export function noveltyByIndex(
  manifest: ReviewManifestSummary | null,
  comparison: CatalogComparison | null,
): Map<number, NoveltyTrack> {
  const result = new Map<number, NoveltyTrack>();
  if (
    !manifest ||
    !comparison ||
    comparison.tracks.length !== manifest.tracks.length
  ) {
    return result;
  }
  manifest.tracks.forEach((track, position) => {
    const novelty = comparison.tracks[position];
    if (novelty) {
      result.set(track.index, novelty);
    }
  });
  return result;
}

/** Total duration across manifest tracks, or null when nothing is known. */
export function totalDurationMs(
  manifest: ReviewManifestSummary | null,
): number | null {
  if (!manifest) {
    return null;
  }
  let total = 0;
  let any = false;
  for (const track of manifest.tracks) {
    if (track.durationMs != null) {
      total += track.durationMs;
      any = true;
    }
  }
  return any ? total : null;
}

/**
 * Display title for the review workspace header: the observed Release
 * artist and title from the manifest (display values, never identity),
 * falling back to the work-item path segment.
 */
export function releaseDisplayTitle(
  detail: ReviewDetail,
  fallback: string,
): string {
  const title = detail.manifest?.releaseTitle ?? fallback;
  const artists = detail.manifest?.releaseArtists ?? [];
  return artists.length > 0 ? `${artists.join(", ")} — ${title}` : title;
}

/** Release display name for the queue row (manifest-free listing). */
export function queueRowDisplayName(item: {
  currentPath: string;
  sourcePath: string;
}): string {
  const path = item.currentPath || item.sourcePath;
  const segments = path.split(/[\\/]/).filter((segment) => segment.length > 0);
  return segments[segments.length - 1] ?? path;
}

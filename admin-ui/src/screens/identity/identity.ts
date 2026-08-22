import type {
  IdentityCandidate,
  IdentityDecisionEntry,
  MatchEvidence,
  ObservationDetail,
  SplitAssignmentKind,
  UnresolvedObservation,
} from '../../api/types';
import type { AuditEvent } from '../../components/AuditTimeline';
import type { Evidence } from '../../components/EvidenceItem';
import type { StatusKind } from '../../components/StatusTag';

/**
 * Shared helpers for the Artist Identity Review screen. Pure mappings
 * only; no I/O. Product language (AGENTS.md): an observed string is never an
 * Artist identity — it resolves to zero, one, or multiple Melodee-owned
 * Artist identities, so helpers speak of observations and candidates.
 */

/** Decision actions in their canonical presentation order. */
export const IDENTITY_ACTIONS = ['confirm', 'create-new', 'leave-provisional', 'never-match'] as const;
export type IdentityAction = (typeof IDENTITY_ACTIONS)[number];

/** Split assignment kinds accepted by the backend (SplitAssignment). */
export const SPLIT_ASSIGNMENT_KINDS: SplitAssignmentKind[] = [
  'releaseArtist',
  'trackArtist',
  'contribution',
  'alias',
  'externalIdentity',
];

/** Public-id (UUID) shape required by the merge/split forms. */
const UUID_PATTERN = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

export function isPublicId(value: string): boolean {
  return UUID_PATTERN.test(value.trim());
}

/** Confidence basis points (out of 10_000) as a whole percentage. */
export function confidencePercent(basisPoints: number): number {
  return Math.round(basisPoints / 100);
}

/**
 * Artist status onto §5 semantics. Provisional artists are identity work in
 * progress (purple); merged/deprecated artists cannot participate.
 */
export function artistStatusKind(status: string): StatusKind {
  switch (status) {
    case 'active':
      return 'success';
    case 'provisional':
      return 'provisional';
    case 'merged':
    case 'deprecated':
      return 'neutral';
    default:
      return 'neutral';
  }
}

/** Source context line for a queue row: Release title plus Track when scoped. */
export function observationContext(observation: UnresolvedObservation): string {
  return observation.trackTitle
    ? `${observation.releaseTitle} · ${observation.trackTitle}`
    : observation.releaseTitle;
}

/** Evidence payload as a compact single-line fact string. */
export function evidenceFact(evidence: MatchEvidence): string {
  if (evidence.payload === null || evidence.payload === undefined) {
    return evidence.source;
  }
  if (typeof evidence.payload === 'string') {
    return evidence.payload;
  }
  try {
    return JSON.stringify(evidence.payload);
  } catch {
    return evidence.source;
  }
}

/** Maps one persisted evidence fact onto the shared evidence row shape. */
export function toEvidence(evidence: MatchEvidence): Evidence {
  return {
    source: `${evidence.source} · ${evidence.kind}`,
    observation: evidenceFact(evidence),
    confidence: evidence.confidence / 10_000,
  };
}

export interface EvidenceCount {
  /** Raw evidence kind from the DTO (localization key suffix). */
  kind: string;
  count: number;
  /** Facts whose polarity is negative (do-not-match evidence). */
  negative: number;
}

/**
 * Evidence breakdown counts across the observation's candidates, grouped by
 * the persisted evidence kind (alias / year / release / external / …) with
 * negative-polarity facts counted separately. Kinds come from the DTO, so
 * unknown kinds pass through rather than being dropped.
 */
export function evidenceCounts(detail: ObservationDetail | null): EvidenceCount[] {
  if (!detail) {
    return [];
  }
  const byKind = new Map<string, EvidenceCount>();
  const allEvidence = detail.candidates.flatMap((candidate) => candidate.evidence);
  for (const evidence of allEvidence) {
    const entry = byKind.get(evidence.kind) ?? { kind: evidence.kind, count: 0, negative: 0 };
    entry.count += 1;
    if (evidence.polarity === 'negative') {
      entry.negative += 1;
    }
    byKind.set(evidence.kind, entry);
  }
  return [...byKind.values()].sort((a, b) => a.kind.localeCompare(b.kind));
}

/** Maps an identity audit-feed entry onto the shared audit-timeline shape. */
export function toAuditEvent(entry: IdentityDecisionEntry): AuditEvent {
  return {
    id: entry.eventId,
    occurredAt: entry.createdAt,
    action: `${entry.eventType} (${entry.entityType})`,
    actor: entry.actor,
    reason: entry.reason || null,
  };
}

/** The candidate a Confirm/Never-Match action binds to, if any. */
export function selectedCandidate(
  detail: ObservationDetail | null,
  candidateId: string | null,
): IdentityCandidate | null {
  if (!detail || !candidateId) {
    return null;
  }
  return detail.candidates.find((candidate) => candidate.artistId === candidateId) ?? null;
}

/**
 * The queue item to select after the current one resolves: the next row in
 * the current page ordering, else the previous row, else nothing. Keeps the
 * operator's position stable as the queue refetches (UI-SPEC §7).
 */
export function nextSelection(
  items: UnresolvedObservation[],
  resolvedId: string,
): string | null {
  const index = items.findIndex((item) => item.id === resolvedId);
  if (index === -1) {
    return null;
  }
  return items[index + 1]?.id ?? items[index - 1]?.id ?? null;
}

/**
 * Module-scope flash-notice slot.
 *
 * The admin router remounts route content on URL changes (observed
 * app-wide, e.g. the processing queue). Advancing the queue selection after
 * a successful resolution changes the URL, so a success notice set in
 * component state would be discarded by the remount. Arming the notice here
 * lets the remounted screen pick it up exactly once; when no remount occurs
 * (unit tests), the consume effect simply clears the slot.
 */
export interface FlashNotice {
  kind: 'success' | 'error' | 'info' | 'warning';
  text: string;
}

let flashNotice: (FlashNotice & { armedAt: number }) | null = null;

/** Flash notices go stale: a remount more than this long after arming is
 *  unrelated to the mutation that armed the notice. */
const FLASH_TTL_MS = 5_000;

export function armFlashNotice(notice: FlashNotice): void {
  flashNotice = { ...notice, armedAt: Date.now() };
}

/** Reads and clears the armed flash notice (consume-once semantics). */
export function takeFlashNotice(): FlashNotice | null {
  const notice = flashNotice;
  flashNotice = null;
  if (!notice || Date.now() - notice.armedAt > FLASH_TTL_MS) {
    return null;
  }
  return notice;
}

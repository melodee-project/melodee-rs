import type { IngestionPhase, IngestionState, WorkItemStateCount } from '../../api/types';
import type { StatusKind } from '../../components/StatusTag';

/**
 * Operator-facing phases (ADR-0185).
 *
 * The domain state machine keeps fourteen states because each is a claim
 * partition with its own fencing and evidence. Operators do not need that
 * granularity: they ask "is this importing, being checked, publishing, in the
 * catalog, searchable, parked, or done?". This module is the single client-side
 * projection used by the queue pills, row labels, and the inspector, and it
 * mirrors `IngestionPhase::states()` so a phase always selects the same states
 * the API selects.
 */
export const PHASE_STATES: Record<IngestionPhase, readonly IngestionState[]> = {
  inbound: ['discovered', 'settling', 'ready'],
  processing: ['inspecting', 'normalizing', 'validating'],
  publishing: ['staged', 'publishing', 'published'],
  catalog: ['cataloging', 'cataloged'],
  // Live to clients. Entering `indexing` is the policy accept commit, which sets
  // the Release status to `active`; every client route filters on that status,
  // so the Release is playable here. `indexed` only adds the search projection.
  active: ['indexing', 'indexed'],
  review: ['needs_review', 'quarantined'],
  disposition: ['ignored', 'deleted'],
  failed: ['failed'],
};

/** Lifecycle order for filter pills and summary rows. */
export const PHASE_ORDER: readonly IngestionPhase[] = [
  'inbound',
  'processing',
  'publishing',
  'catalog',
  'active',
  'review',
  'failed',
  'disposition',
];

/** Fallback projection for a payload that predates the API `phase` field. */
export function phaseForState(state: string): IngestionPhase {
  for (const [phase, states] of Object.entries(PHASE_STATES) as [
    IngestionPhase,
    readonly IngestionState[],
  ][]) {
    if ((states as readonly string[]).includes(state)) {
      return phase;
    }
  }
  return 'processing';
}

/**
 * Per-phase counts from the per-state counts endpoint. Phases without items are
 * omitted so the pills never show a zero they cannot drill into.
 */
export function countByPhase(
  counts: readonly WorkItemStateCount[],
): { phase: IngestionPhase; count: number }[] {
  const totals = new Map<IngestionPhase, number>();
  for (const entry of counts) {
    const phase = phaseForState(entry.state);
    totals.set(phase, (totals.get(phase) ?? 0) + entry.count);
  }
  return PHASE_ORDER.flatMap((phase) => {
    const count = totals.get(phase) ?? 0;
    return count > 0 ? [{ phase, count }] : [];
  });
}

/** Status semantics for a phase chip (UI-SPEC §5: never colour-only). */
export function phaseStatusKind(phase: IngestionPhase): StatusKind {
  switch (phase) {
    case 'active':
      return 'success';
    case 'processing':
    case 'publishing':
    case 'catalog':
      return 'info';
    case 'review':
      return 'warning';
    case 'failed':
      return 'error';
    case 'inbound':
    case 'disposition':
      return 'neutral';
  }
}

/**
 * The single answer to "can a client listen to this yet?".
 *
 * `clientVisible` comes from the API (authoritative Release status); the state
 * projection is the fallback for payloads that predate it. The search phase is
 * deliberately *not* consulted: the projection never gates playback.
 */
export function isClientReady(item: {
  clientVisible?: boolean;
  state?: string;
}): boolean {
  if (typeof item.clientVisible === 'boolean') {
    return item.clientVisible;
  }
  const state = item.state ?? '';
  return state === 'indexing' || state === 'indexed';
}

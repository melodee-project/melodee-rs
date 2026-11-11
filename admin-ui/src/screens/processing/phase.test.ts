import { describe, expect, it } from 'vitest';

import type { IngestionPhase, IngestionState, WorkItemStateCount } from '../../api/types';
import { countByPhase, PHASE_ORDER, PHASE_STATES, phaseForState } from './phase';

const ALL_STATES: IngestionState[] = [
  'discovered',
  'settling',
  'ready',
  'inspecting',
  'normalizing',
  'validating',
  'needs_review',
  'staged',
  'publishing',
  'published',
  'cataloging',
  'cataloged',
  'indexing',
  'indexed',
  'failed',
  'quarantined',
  'ignored',
  'deleted',
];

describe('phase projection', () => {
  it('maps every state to exactly one phase (mirrors the domain partition)', () => {
    const seen = new Map<string, IngestionPhase>();
    for (const [phase, states] of Object.entries(PHASE_STATES) as [
      IngestionPhase,
      readonly IngestionState[],
    ][]) {
      for (const state of states) {
        expect(seen.has(state)).toBe(false);
        seen.set(state, phase);
        expect(phaseForState(state)).toBe(phase);
      }
    }
    expect(seen.size).toBe(ALL_STATES.length);
    for (const state of ALL_STATES) {
      expect(seen.has(state)).toBe(true);
    }
  });

  it('orders phases in lifecycle order', () => {
    expect(PHASE_ORDER).toEqual([
      'inbound',
      'processing',
      'publishing',
      'catalog',
      'search',
      'review',
      'failed',
      'disposition',
    ]);
  });

  it('aggregates per-state counts into phases and omits empty phases', () => {
    const counts: WorkItemStateCount[] = [
      { state: 'discovered', count: 3 },
      { state: 'settling', count: 1 },
      { state: 'ready', count: 2 },
      { state: 'normalizing', count: 4 },
      { state: 'failed', count: 2 },
    ];
    expect(countByPhase(counts)).toEqual([
      { phase: 'inbound', count: 6 },
      { phase: 'processing', count: 4 },
      { phase: 'failed', count: 2 },
    ]);
    expect(countByPhase([])).toEqual([]);
  });

  it('falls back to a phase for an unknown state rather than dropping the row', () => {
    expect(phaseForState('some_future_state')).toBe('processing');
  });
});

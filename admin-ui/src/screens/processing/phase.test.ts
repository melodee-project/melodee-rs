import { describe, expect, it } from 'vitest';

import type { IngestionPhase, IngestionState, WorkItemStateCount } from '../../api/types';
import {
  countByPhase,
  isClientReady,
  PHASE_ORDER,
  PHASE_STATES,
  phaseForState,
  phaseStatusKind,
} from './phase';

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
      'active',
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

  it('marks the last unavailable and first available states explicitly', () => {
    // cataloged = imported but not yet accepted by policy; indexing = the accept
    // commit, which is when clients can play it.
    expect(phaseForState('cataloged')).toBe('catalog');
    expect(phaseForState('indexing')).toBe('active');
    expect(phaseForState('indexed')).toBe('active');
    expect(phaseStatusKind('active')).toBe('success');
    expect(phaseStatusKind('failed')).toBe('error');
    expect(phaseStatusKind('inbound')).toBe('neutral');
  });

  it('answers "can a client play this?" from the API flag, not the search phase', () => {
    // Authoritative flag wins.
    expect(isClientReady({ clientVisible: true, state: 'cataloged' })).toBe(true);
    expect(isClientReady({ clientVisible: false, state: 'indexing' })).toBe(false);
    // Fallback projection for payloads without the flag.
    expect(isClientReady({ state: 'indexing' })).toBe(true);
    expect(isClientReady({ state: 'indexed' })).toBe(true);
    expect(isClientReady({ state: 'cataloged' })).toBe(false);
    expect(isClientReady({ state: 'cataloging' })).toBe(false);
  });
});

import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import type { SystemHealth } from '../api/types';
import { SystemHealthPanel } from './SystemHealthPanel';

function makeHealth(overrides: Partial<SystemHealth> = {}): SystemHealth {
  return {
    status: 'degraded',
    postgresql: { status: 'ok', detail: 'postgresql answered a bounded probe' },
    postgresqlPool: {
      maxConnections: 32,
      openConnections: 2,
      idleConnections: 1,
      inUseConnections: 1,
      utilizationPercent: 3.125,
    },
    tantivy: { status: 'disabled', detail: 'search worker disabled' },
    storage: { status: 'degraded', detail: 'inbound path not writable' },
    ffmpeg: { status: 'ok', detail: 'ffmpeg 8.1' },
    podcastStorage: { status: 'disabled', detail: 'podcast worker disabled' },
    disk: { status: 'ok', detail: 'tightest of 2 root(s): storage has 512000 MiB free (50%)' },
    workers: { status: 'ok', detail: 'no stale leases; oldest ready item 0s; 0 ready / 0 claimed' },
    apiSurfaces: [{ surface: 'native', status: 'serving' }],
    resourceBounds: [],
    ...overrides,
  };
}

describe('SystemHealthPanel', () => {
  it('renders every subsystem verdict as label + detail, never color-only', () => {
    render(<SystemHealthPanel health={makeHealth()} title="System health" />);
    expect(screen.getByText('PostgreSQL')).toBeInTheDocument();
    expect(screen.getByText('Tantivy search index')).toBeInTheDocument();
    expect(screen.getByText('Storage')).toBeInTheDocument();
    expect(screen.getByText('FFmpeg')).toBeInTheDocument();
    expect(screen.getByText('Podcast storage')).toBeInTheDocument();
    expect(screen.getByText('Disk free space')).toBeInTheDocument();
    expect(screen.getByText('Background workers')).toBeInTheDocument();
    // Degraded and disabled are distinct, textual verdicts.
    expect(screen.getByText('Degraded')).toBeInTheDocument();
    // Tantivy and podcast storage are both disabled in this fixture.
    expect(screen.getAllByText('Disabled')).toHaveLength(2);
    expect(screen.getByText('inbound path not writable')).toBeInTheDocument();
    expect(screen.getByText('search worker disabled')).toBeInTheDocument();
  });

  it('renders API surfaces with localized names', () => {
    render(<SystemHealthPanel health={makeHealth()} title="System health" />);
    expect(screen.getByText('Melodee API v1')).toBeInTheDocument();
  });
});

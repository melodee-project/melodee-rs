import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import type { SystemHealth } from '../api/types';
import { SystemHealthPanel } from './SystemHealthPanel';

function makeHealth(overrides: Partial<SystemHealth> = {}): SystemHealth {
  return {
    status: 'degraded',
    postgresql: { status: 'ok', detail: 'postgresql answered a bounded probe' },
    tantivy: { status: 'disabled', detail: 'search worker disabled' },
    storage: { status: 'degraded', detail: 'inbound path not writable' },
    ffmpeg: { status: 'ok', detail: 'ffmpeg 8.1' },
    apiSurfaces: [{ surface: 'native', status: 'serving' }],
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
    // Degraded and disabled are distinct, textual verdicts.
    expect(screen.getByText('Degraded')).toBeInTheDocument();
    expect(screen.getByText('Disabled')).toBeInTheDocument();
    expect(screen.getByText('inbound path not writable')).toBeInTheDocument();
    expect(screen.getByText('search worker disabled')).toBeInTheDocument();
  });

  it('renders API surfaces with localized names', () => {
    render(<SystemHealthPanel health={makeHealth()} title="System health" />);
    expect(screen.getByText('Melodee API v1')).toBeInTheDocument();
  });
});

import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { ProcessingStateTag } from './ProcessingStateTag';

describe('ProcessingStateTag', () => {
  it('keeps quarantine distinct from failed and ignored', () => {
    const { container: failed } = render(<ProcessingStateTag state="failed" />);
    const { container: quarantined } = render(<ProcessingStateTag state="quarantined" />);
    const { container: ignored } = render(<ProcessingStateTag state="ignored" />);
    expect(failed.querySelector('.melodee-chip--error')).not.toBeNull();
    expect(quarantined.querySelector('.melodee-chip--error')).toBeNull();
    expect(ignored.querySelector('.melodee-chip--error')).toBeNull();
    // The raw server state remains the visible label in all three.
    expect(screen.getByText('quarantined')).toBeInTheDocument();
  });

  it('maps lifecycle states onto the six-way semantics', () => {
    render(<ProcessingStateTag state="cataloged" />);
    expect(screen.getByTitle('Complete, healthy, or accepted')).toBeInTheDocument();
  });

  it('falls back to neutral for unknown states', () => {
    render(<ProcessingStateTag state="mystery_state" />);
    expect(screen.getByTitle('Disabled or ignored')).toBeInTheDocument();
  });
});

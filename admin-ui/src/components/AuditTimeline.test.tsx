import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { AuditTimeline } from './AuditTimeline';

describe('AuditTimeline', () => {
  it('renders an empty state without events', () => {
    render(<AuditTimeline events={[]} />);
    expect(screen.getByText('No audit events recorded')).toBeInTheDocument();
  });

  it('renders events with actor and reason', () => {
    render(
      <AuditTimeline
        events={[
          {
            id: 'e1',
            occurredAt: '2026-08-14T12:00:00Z',
            action: 'state change',
            actor: 'admin',
            reason: 'operator decision',
          },
        ]}
      />,
    );
    expect(screen.getByText('state change')).toBeInTheDocument();
    expect(screen.getByText('admin')).toBeInTheDocument();
    expect(screen.getByText('operator decision')).toBeInTheDocument();
  });
});

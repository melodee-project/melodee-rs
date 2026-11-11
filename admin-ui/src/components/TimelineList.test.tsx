import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { TimelineList } from './TimelineList';

describe('TimelineList', () => {
  it('renders each event with title, detail, time, and toned marker', () => {
    const { container } = render(
      <TimelineList
        events={[
          { id: '1', title: 'Discovered', time: '1h ago', tone: 'success' },
          { id: '2', title: 'Review pending', detail: 'Awaiting operator', time: 'Now', tone: 'warning' },
        ]}
      />,
    );
    expect(screen.getByText('Discovered')).toBeInTheDocument();
    expect(screen.getByText('Awaiting operator')).toBeInTheDocument();
    expect(screen.getByText('Now')).toBeInTheDocument();
    expect(container.querySelectorAll('.melodee-timeline__item')).toHaveLength(2);
    expect(container.querySelector('.melodee-timeline__marker--success')).not.toBeNull();
    expect(container.querySelector('.melodee-timeline__marker--warning')).not.toBeNull();
  });
});

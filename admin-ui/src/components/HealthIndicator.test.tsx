import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { HealthIndicator } from './HealthIndicator';

describe('HealthIndicator', () => {
  it('renders localized labels for each health state', () => {
    render(
      <>
        <HealthIndicator state="online" />
        <HealthIndicator state="offline" />
      </>,
    );
    expect(screen.getByText('Server online')).toBeInTheDocument();
    expect(screen.getByText('Server offline')).toBeInTheDocument();
  });

  it('maps offline to the error semantic with icon and description', () => {
    const { container } = render(<HealthIndicator state="offline" />);
    expect(container.querySelector('svg')).not.toBeNull();
    expect(screen.getByText('Server offline').closest('[title]')).not.toBeNull();
  });
});

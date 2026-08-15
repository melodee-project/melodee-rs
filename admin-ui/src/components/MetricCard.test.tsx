import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { MetricCard } from './MetricCard';

describe('MetricCard', () => {
  it('renders label and value', () => {
    render(<MetricCard label="Releases" value="1,234" />);
    expect(screen.getByText('Releases')).toBeInTheDocument();
    expect(screen.getByText('1,234')).toBeInTheDocument();
  });

  it('renders a status tag when a status kind is given', () => {
    render(<MetricCard label="Queue" value="42" status="warning" />);
    expect(screen.getByText('Needs attention')).toBeInTheDocument();
  });
});

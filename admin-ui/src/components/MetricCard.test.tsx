import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { MetricCard } from './MetricCard';

describe('MetricCard', () => {
  it('renders label and value', () => {
    render(<MetricCard label="Releases" value="1,234" />);
    expect(screen.getByText('Releases')).toBeInTheDocument();
    expect(screen.getByText('1,234')).toBeInTheDocument();
  });

  it('renders a status chip when a status kind is given', () => {
    render(<MetricCard label="Queue" value="42" status="warning" />);
    expect(screen.getByText('Needs attention')).toBeInTheDocument();
  });

  it('renders a toned delta line under the numeral', () => {
    const { container } = render(
      <MetricCard label="Inbound" value="214" delta={{ text: '+12 today' }} />,
    );
    expect(screen.getByText('+12 today')).toBeInTheDocument();
    expect(container.querySelector('.melodee-metric-card__delta--warning')).toBeNull();

    const { container: warning } = render(
      <MetricCard label="Failed" value="3" delta={{ text: 'needs attention', tone: 'warning' }} />,
    );
    expect(warning.querySelector('.melodee-metric-card__delta--warning')).not.toBeNull();
  });

  it('prefers the delta line over a static hint', () => {
    render(<MetricCard label="Processing" value="47" delta={{ text: '8 active now' }} hint="ignored" />);
    expect(screen.getByText('8 active now')).toBeInTheDocument();
    expect(screen.queryByText('ignored')).not.toBeInTheDocument();
  });
});

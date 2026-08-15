import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { EvidenceItem } from './EvidenceItem';

describe('EvidenceItem', () => {
  it('shows the observation alongside the resolved value', () => {
    render(
      <EvidenceItem
        evidence={{ source: 'file tags', observation: 'radio head', resolved: 'Radiohead', confidence: 0.92 }}
      />,
    );
    expect(screen.getByLabelText('Observation')).toHaveTextContent('radio head');
    expect(screen.getByLabelText('Resolved value')).toHaveTextContent('Radiohead');
    expect(screen.getByText(/92%/)).toBeInTheDocument();
  });

  it('does not repeat the resolved value when it matches the observation', () => {
    render(<EvidenceItem evidence={{ source: 'filename', observation: 'OK Computer' }} />);
    expect(screen.getAllByText('OK Computer')).toHaveLength(1);
  });
});

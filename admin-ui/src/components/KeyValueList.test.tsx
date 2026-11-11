import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { KeyValueList } from './KeyValueList';

describe('KeyValueList', () => {
  it('renders a definition list of terms and values', () => {
    render(
      <KeyValueList
        ariaLabel="Release information"
        items={[
          { term: 'Release type', value: 'Studio' },
          { term: 'Label', value: 'Columbia', detail: 'Australia' },
        ]}
      />,
    );
    expect(screen.getByLabelText('Release information')).toBeInTheDocument();
    expect(screen.getByText('Release type').tagName).toBe('DT');
    expect(screen.getByText('Studio').tagName).toBe('DD');
    expect(screen.getByText('Australia')).toBeInTheDocument();
  });

  it('keeps rendering when a value is absent', () => {
    render(<KeyValueList items={[{ term: 'Barcode', value: '—' }]} />);
    expect(screen.getByText('—')).toBeInTheDocument();
  });
});

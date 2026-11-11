import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Surface } from './Surface';

describe('Surface', () => {
  it('renders a titled card labelled by its heading', () => {
    render(
      <Surface title="System health" subtitle="All subsystems" actions={<button>Retry</button>}>
        body
      </Surface>,
    );
    const heading = screen.getByRole('heading', { name: 'System health', level: 2 });
    expect(heading).toBeInTheDocument();
    expect(screen.getByText('All subsystems')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
    expect(screen.getByText('body')).toBeInTheDocument();
  });

  it('supports heading levels and unpadded table cards', () => {
    const { container } = render(
      <Surface title="Worker pools" headingLevel={3} variant="flush">
        rows
      </Surface>,
    );
    expect(screen.getByRole('heading', { level: 3, name: 'Worker pools' })).toBeInTheDocument();
    expect(container.querySelector('.melodee-surface--flush')).not.toBeNull();
  });

  it('falls back to an aria-label when there is no visible title', () => {
    render(<Surface ariaLabel="Release inspector">detail</Surface>);
    expect(screen.getByLabelText('Release inspector')).toHaveTextContent('detail');
  });
});

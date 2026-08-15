import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { InspectorPanel } from './InspectorPanel';

describe('InspectorPanel', () => {
  it('renders nothing when closed', () => {
    const { container } = render(
      <InspectorPanel title="Work item" open={false} onClose={() => undefined}>
        body
      </InspectorPanel>,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('renders a labeled complementary region and closes via the button', () => {
    const onClose = vi.fn();
    render(
      <InspectorPanel title="Work item" open onClose={onClose}>
        body content
      </InspectorPanel>,
    );
    expect(screen.getByRole('complementary', { name: 'Inspector' })).toHaveTextContent(
      'body content',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Close inspector' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

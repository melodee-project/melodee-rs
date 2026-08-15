import type { ReactNode, SyntheticEvent } from 'react';

/**
 * Interactive cell content inside an activatable table row: clicks and keys
 * on a button/link must not also trigger row activation (double action).
 */
export function CellInteractive({ children }: { children: ReactNode }) {
  const stop = (event: SyntheticEvent) => event.stopPropagation();
  return (
    <span onClick={stop} onKeyDown={stop}>
      {children}
    </span>
  );
}

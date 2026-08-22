import { Component, type ReactNode } from 'react';

import { ErrorState } from './ErrorState';

interface RouteErrorBoundaryProps {
  children: ReactNode;
  /** Change of key resets the boundary (navigation recovers the screen). */
  resetKey: string;
}

interface RouteErrorBoundaryState {
  hasError: boolean;
}

/**
 * Render-error boundary for the routed content region. A screen that throws
 * during render must not take down the shell (or the browser paints a blank
 * page); the fallback is the localized, themed `unexpected` error state.
 * React has no hook-based boundary, so this stays a class component.
 */
export class RouteErrorBoundary extends Component<
  RouteErrorBoundaryProps,
  RouteErrorBoundaryState
> {
  override state: RouteErrorBoundaryState = { hasError: false };

  static getDerivedStateFromError(): RouteErrorBoundaryState {
    return { hasError: true };
  }

  override componentDidCatch(error: unknown): void {
    // The only diagnostics channel available at this layer; the server has
    // its own correlation for API failures.
    console.error('admin-ui route render failed', error);
  }

  override componentDidUpdate(previous: RouteErrorBoundaryProps): void {
    if (previous.resetKey !== this.props.resetKey && this.state.hasError) {
      this.setState({ hasError: false });
    }
  }

  override render(): ReactNode {
    if (this.state.hasError) {
      return <ErrorState kind="unexpected" />;
    }
    return this.props.children;
  }
}

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ThemeProvider, useThemeState } from './ThemeProvider';
import { clearThemePackStyles } from './packs';

/**
 * Provider-level theme-pack behavior (PRD-THEME-004): the selected pack is
 * applied through the data attribute and, on failure, the provider restores
 * the Melodee default instead of leaving the UI half-themed. State is
 * observed through the DOM (the probe renders it) rather than by poking the
 * context object mid-render.
 */

function Probe() {
  const state = useThemeState();
  return (
    <div>
      <span data-testid="pack">{state.themePack}</span>
      <span data-testid="error">{state.themePackError ?? 'none'}</span>
      <button type="button" onClick={() => state.setThemePack('neon')}>
        select-neon
      </button>
    </div>
  );
}

function renderProvider() {
  return render(
    <ThemeProvider>
      <Probe />
    </ThemeProvider>,
  );
}

function stubCss(css: string, status = 200) {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(css, { status })));
}

describe('ThemeProvider theme-pack axis', () => {
  beforeEach(() => {
    window.localStorage.clear();
    clearThemePackStyles();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    clearThemePackStyles();
    window.localStorage.clear();
  });

  it('applies the persisted pack and reports no error', async () => {
    window.localStorage.setItem('melodee.theme-pack', 'neon');
    stubCss(':root { --melodee-brand-accent: #ff00ff; }');

    renderProvider();

    await waitFor(() => {
      expect(document.querySelector('#melodee-theme-pack-style')?.textContent).toContain(
        '--melodee-brand-accent: #ff00ff;',
      );
    });
    expect(screen.getByTestId('pack')).toHaveTextContent('neon');
    expect(screen.getByTestId('error')).toHaveTextContent('none');
  });

  it('falls back to the Melodee default when the pack stylesheet is missing', async () => {
    window.localStorage.setItem('melodee.theme-pack', 'gone');
    stubCss('not found', 404);

    renderProvider();

    await waitFor(() => {
      expect(screen.getByTestId('pack')).toHaveTextContent('default');
    });
    expect(screen.getByTestId('error')).toHaveTextContent('fetch');
    expect(window.localStorage.getItem('melodee.theme-pack')).toBe('default');
    expect(document.querySelector('#melodee-theme-pack-style')).toBeNull();
  });

  it('falls back to the Melodee default when the pack defines no tokens', async () => {
    window.localStorage.setItem('melodee.theme-pack', 'legacy-only');
    stubCss('.legacy-only { color: red; }');

    renderProvider();

    await waitFor(() => {
      expect(screen.getByTestId('pack')).toHaveTextContent('default');
    });
    expect(screen.getByTestId('error')).toHaveTextContent('noTokens');
  });

  it('clears a previous error when a new pack is selected', async () => {
    window.localStorage.setItem('melodee.theme-pack', 'gone');
    stubCss('nope', 404);
    renderProvider();
    await waitFor(() => expect(screen.getByTestId('error')).toHaveTextContent('fetch'));

    stubCss(':root { --melodee-brand-accent: #00ff00; }');
    fireEvent.click(screen.getByRole('button', { name: 'select-neon' }));

    await waitFor(() => expect(screen.getByTestId('error')).toHaveTextContent('none'));
    expect(screen.getByTestId('pack')).toHaveTextContent('neon');
    expect(window.localStorage.getItem('melodee.theme-pack')).toBe('neon');
  });
});

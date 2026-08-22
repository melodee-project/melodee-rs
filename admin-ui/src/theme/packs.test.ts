import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { applyThemePack, clearThemePackStyles } from './packs';

/**
 * Theme-pack application (PRD-THEME-004): the token layer is updated on
 * success and restored to the Melodee default on every failure mode.
 */

const STYLE_SELECTOR = '#melodee-theme-pack-style';

function stubThemeCss(body: string | { status: number }) {
  const fetchMock = vi.fn();
  if (typeof body === 'string') {
    fetchMock.mockResolvedValue(
      new Response(body, { status: 200, headers: { 'content-type': 'text/css' } }),
    );
  } else {
    fetchMock.mockResolvedValue(new Response('not found', { status: body.status }));
  }
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('applyThemePack', () => {
  beforeEach(() => {
    clearThemePackStyles();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    clearThemePackStyles();
  });

  it('clears injected styles for the built-in default pack without fetching', async () => {
    const fetchMock = stubThemeCss('');
    document.head.append(Object.assign(document.createElement('style'), { id: 'melodee-theme-pack-style' }));

    const result = await applyThemePack('default');

    expect(result).toEqual({ ok: true });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(document.querySelector(STYLE_SELECTOR)).toBeNull();
  });

  it('injects the pack tokens scoped to its data attribute', async () => {
    const fetchMock = stubThemeCss(
      ':root { --melodee-brand-accent: #ff00ff; --melodee-status-success: #00ff00; }\n' +
        '.legacy-selector { color: red; }',
    );

    const result = await applyThemePack('neon.nights');

    expect(result).toEqual({ ok: true });
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/v1/themes/neon.nights/file/theme.css',
      expect.objectContaining({ credentials: 'include' }),
    );
    const style = document.querySelector(STYLE_SELECTOR);
    expect(style).not.toBeNull();
    expect(style?.textContent).toContain('[data-melodee-theme-pack="neon.nights"]');
    expect(style?.textContent).toContain('--melodee-brand-accent: #ff00ff;');
    expect(style?.textContent).toContain('--melodee-status-success: #00ff00;');
    // Legacy component selectors are never injected — token layer only.
    expect(style?.textContent).not.toContain('.legacy-selector');
  });

  it('replaces previously injected styles when switching packs', async () => {
    stubThemeCss(':root { --melodee-brand-accent: #111111; }');
    await applyThemePack('first');
    stubThemeCss(':root { --melodee-brand-accent: #222222; }');
    await applyThemePack('second');

    const styles = document.querySelectorAll(STYLE_SELECTOR);
    expect(styles).toHaveLength(1);
    expect(styles[0]?.textContent).toContain('[data-melodee-theme-pack="second"]');
    expect(styles[0]?.textContent).toContain('--melodee-brand-accent: #222222;');
  });

  it('fails with reason fetch on a non-OK response and clears the layer', async () => {
    stubThemeCss(':root { --melodee-brand-accent: #111111; }');
    await applyThemePack('first');
    stubThemeCss({ status: 404 });

    const result = await applyThemePack('missing');

    expect(result).toEqual({ ok: false, reason: 'fetch' });
    expect(document.querySelector(STYLE_SELECTOR)).toBeNull();
  });

  it('fails with reason fetch when the request rejects', async () => {
    const fetchMock = vi.fn().mockRejectedValue(new Error('network down'));
    vi.stubGlobal('fetch', fetchMock);

    const result = await applyThemePack('neon');

    expect(result).toEqual({ ok: false, reason: 'fetch' });
  });

  it('fails with reason noTokens when the stylesheet defines no --melodee-* tokens', async () => {
    stubThemeCss('.legacy-only { color: red; }');

    const result = await applyThemePack('legacy-only');

    expect(result).toEqual({ ok: false, reason: 'noTokens' });
    expect(document.querySelector(STYLE_SELECTOR)).toBeNull();
  });

  it('rejects unsafe pack ids without a request', async () => {
    const fetchMock = stubThemeCss('');

    const result = await applyThemePack('../etc/passwd');

    expect(result).toEqual({ ok: false, reason: 'invalidId' });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

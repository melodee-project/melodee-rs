import { themePackAssetUrl } from '../api/themes';

/**
 * Theme-pack application (PRD-THEME-004, UI-SPEC §1–§2).
 *
 * The server stores a theme pack as a ZIP of assets served per-endpoint
 * (`GET /api/v1/themes/{id}/file/theme.css`). The honest, least-invasive
 * mapping onto the admin SPA token layer: fetch the pack's `theme.css`,
 * extract its `--melodee-*` custom-property declarations, and inject them
 * into a single managed <style> element scoped to the pack's
 * `data-melodee-theme-pack` attribute.
 *
 * Why extraction instead of loading the stylesheet verbatim: UI-SPEC §2
 * forbids CSS outside the token layer, and legacy packs (written for the
 * Blazor UI) carry component selectors that either no-op against the React
 * DOM or would illegitimately override Carbon component CSS. Only the
 * `--melodee-*` tokens are meaningful to this SPA.
 *
 * Documented limitations of this mapping:
 *  - token values apply flat to both appearances; a legacy pack that
 *    distinguished per-mode values (media-query blocks) gets its last
 *    declaration of each token, not per-mode switching;
 *  - fonts, images, and other assets shipped in the pack are not applied.
 *
 * Any failure (network, non-OK status, or a stylesheet defining zero
 * --melodee-* tokens) is reported so the caller can fall back to the
 * built-in default pack — the UI never stays half-themed.
 */

export const DEFAULT_THEME_PACK = 'default';

const STYLE_ELEMENT_ID = 'melodee-theme-pack-style';

/** Mirrors the server's valid_theme_id rule (discovery.rs). */
const SAFE_PACK_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

/** Matches `--melodee-name: value` declarations terminated by `;` or `}`. */
const TOKEN_DECLARATION = /(--melodee-[a-z0-9-]+)\s*:\s*([^;{}]+?)\s*(?:;|\})/gi;

export type ThemePackApplyFailure = 'invalidId' | 'fetch' | 'noTokens';

export type ThemePackApplyResult =
  | { ok: true }
  | { ok: false; reason: ThemePackApplyFailure };

/** Removes the managed pack style element, restoring tokens.css defaults. */
export function clearThemePackStyles(): void {
  document.getElementById(STYLE_ELEMENT_ID)?.remove();
}

function injectThemePackStyles(packId: string, tokens: Map<string, string>): void {
  const declarations = [...tokens.entries()]
    .map(([name, value]) => `  ${name}: ${value};`)
    .join('\n');
  // `:root` prefix lifts specificity (0,1,1) above the `.cds--g10`/`.cds--g100`
  // defaults (0,1,0) in tokens.css regardless of stylesheet order.
  const css = `:root [data-melodee-theme-pack="${packId}"] {\n${declarations}\n}`;
  let element = document.getElementById(STYLE_ELEMENT_ID);
  if (!element) {
    element = document.createElement('style');
    element.id = STYLE_ELEMENT_ID;
    document.head.append(element);
  }
  element.textContent = css;
}

/**
 * Fetches and applies a server theme pack. `default` (the built-in pack)
 * simply clears any injected styles and always succeeds.
 */
export async function applyThemePack(packId: string): Promise<ThemePackApplyResult> {
  if (packId === DEFAULT_THEME_PACK) {
    clearThemePackStyles();
    return { ok: true };
  }
  if (!SAFE_PACK_ID.test(packId)) {
    clearThemePackStyles();
    return { ok: false, reason: 'invalidId' };
  }
  let css: string;
  try {
    const response = await fetch(themePackAssetUrl(packId, 'theme.css'), {
      credentials: 'include',
    });
    if (!response.ok) {
      clearThemePackStyles();
      return { ok: false, reason: 'fetch' };
    }
    css = await response.text();
  } catch {
    clearThemePackStyles();
    return { ok: false, reason: 'fetch' };
  }
  const tokens = new Map<string, string>();
  for (const match of css.matchAll(TOKEN_DECLARATION)) {
    const name = match[1];
    const value = match[2];
    if (name && value) {
      tokens.set(name, value);
    }
  }
  if (tokens.size === 0) {
    clearThemePackStyles();
    return { ok: false, reason: 'noTokens' };
  }
  injectThemePackStyles(packId, tokens);
  return { ok: true };
}

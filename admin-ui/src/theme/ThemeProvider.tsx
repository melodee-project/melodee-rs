import { Theme } from '@carbon/react';
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import {
  DEFAULT_THEME_PACK,
  applyThemePack,
  type ThemePackApplyFailure,
} from './packs';

/**
 * Appearance and theme-pack state (PRD-THEME-001..004, UI-SPEC §1).
 *
 * Two independent axes:
 *  - appearance: system | light | dark, mapped onto Carbon g10 (light) and
 *    g100 (dark). `system` tracks prefers-color-scheme live.
 *  - theme pack: a named set of --melodee-* token values applied via the
 *    data-melodee-theme-pack attribute. The built-in "default" pack ships in
 *    tokens.css; server-listed packs (PRD-THEME-004) are fetched from the
 *    native theme endpoints and applied by./packs.ts. A pack that fails to
 *    load (or defines no --melodee-* tokens) falls back to the default pack
 *    and reports `themePackError` so the settings UI can explain it.
 *
 * The appearance choice persists in localStorage. High-contrast and
 * reduced-motion media queries are handled by tokens.css and are never
 * overridden.
 */

export type Appearance = 'system' | 'light' | 'dark';

export interface ThemeState {
  appearance: Appearance;
  /** Resolved Carbon theme actually applied. */
  carbonTheme: 'g10' | 'g100';
  themePack: string;
  /** Set when the requested pack failed to apply and the default was restored. */
  themePackError: ThemePackApplyFailure | null;
  setAppearance: (appearance: Appearance) => void;
  setThemePack: (pack: string) => void;
}

const STORAGE_KEY = 'melodee.appearance';
const THEME_PACK_KEY = 'melodee.theme-pack';

const ThemeStateContext = createContext<ThemeState | null>(null);

function readStoredAppearance(): Appearance {
  try {
    const value = window.localStorage.getItem(STORAGE_KEY);
    if (value === 'light' || value === 'dark' || value === 'system') {
      return value;
    }
  } catch {
    // Storage may be unavailable (private mode); fall through to system.
  }
  return 'system';
}

function clearStoredThemePack(): void {
  try {
    window.localStorage.setItem(THEME_PACK_KEY, DEFAULT_THEME_PACK);
  } catch {
    // Storage unavailable; the in-memory fallback still applies.
  }
}

function systemPrefersDark(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  );
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [appearance, setAppearanceState] = useState<Appearance>(readStoredAppearance);
  const [themePack, setThemePackState] = useState<string>(() => {
    try {
      return window.localStorage.getItem(THEME_PACK_KEY) ?? DEFAULT_THEME_PACK;
    } catch {
      return DEFAULT_THEME_PACK;
    }
  });
  const [themePackError, setThemePackError] = useState<ThemePackApplyFailure | null>(null);
  const [prefersDark, setPrefersDark] = useState<boolean>(systemPrefersDark);

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') {
      return;
    }
    const query = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = (event: MediaQueryListEvent) => setPrefersDark(event.matches);
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);

  const setAppearance = useCallback((next: Appearance) => {
    setAppearanceState(next);
    try {
      window.localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Non-persisted appearance still applies for the session.
    }
  }, []);

  const setThemePack = useCallback((next: string) => {
    setThemePackState(next);
    setThemePackError(null);
    try {
      window.localStorage.setItem(THEME_PACK_KEY, next);
    } catch {
      // Non-persisted theme pack still applies for the session.
    }
  }, []);

  // Apply the selected pack's token layer; on failure restore the built-in
  // default (never leave the UI half-themed) and surface why.
  useEffect(() => {
    let cancelled = false;
    void applyThemePack(themePack).then((result) => {
      if (cancelled || result.ok) {
        return;
      }
      clearStoredThemePack();
      setThemePackState(DEFAULT_THEME_PACK);
      setThemePackError(result.reason);
    });
    return () => {
      cancelled = true;
    };
  }, [themePack]);

  const dark = appearance === 'dark' || (appearance === 'system' && prefersDark);
  const carbonTheme = dark ? 'g100' : 'g10';

  const value = useMemo<ThemeState>(
    () => ({ appearance, carbonTheme, themePack, themePackError, setAppearance, setThemePack }),
    [appearance, carbonTheme, themePack, themePackError, setAppearance, setThemePack],
  );

  return (
    <ThemeStateContext.Provider value={value}>
      <Theme
        theme={carbonTheme}
        data-melodee-theme-pack={themePack}
        style={{ minBlockSize: '100vh', background: 'var(--cds-background)' }}
      >
        {children}
      </Theme>
    </ThemeStateContext.Provider>
  );
}

// The context hook is intentionally colocated with its provider (one import
// for consumers, hook is the only non-component export). See AuthContext.tsx.
// eslint-disable-next-line react-refresh/only-export-components
export function useThemeState(): ThemeState {
  const context = useContext(ThemeStateContext);
  if (!context) {
    throw new Error('useThemeState must be used inside <ThemeProvider>');
  }
  return context;
}

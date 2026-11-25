import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import { AUTH_EXPIRED_EVENT, ApiError, apiFetch } from '../api/client';
import { getSystemInfo } from '../api/admin';

/**
 * Cookie-session auth state (ADR-0139).
 *
 * The SPA signs in through the frozen native-v1 cookie endpoints and holds no
 * token in web storage. Session truth comes from the server: the boot probe
 * calls an admin-gated endpoint and reads the gate's answer:
 *   200 → authenticated administrator
 *   403 → check review access for editors, otherwise show access denied
 *   401 → anonymous / expired session
 * The 401 path also fires from any later API call via `melodee:auth-expired`.
 */

export type SessionState =
  | { kind: 'loading' }
  | { kind: 'anonymous' }
  | { kind: 'forbidden' }
  | { kind: 'admin' | 'editor'; username: string | null };

export interface AuthContextValue {
  session: SessionState;
  signIn: (identifier: string, password: string) => Promise<void>;
  signOut: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

// Exact frozen native-v1 cookie auth contract (apps/melodee-server/src/native/auth.rs).
const SIGN_IN_PATH = '/api/v1/auth/cookie/sign-in';
const SIGN_OUT_PATH = '/api/v1/auth/cookie/sign-out';

interface MessageBody {
  message: string;
}

const USERNAME_STORAGE_KEY = 'melodee.username';

function readStoredUsername(): string | null {
  try {
    return window.sessionStorage.getItem(USERNAME_STORAGE_KEY);
  } catch {
    return null;
  }
}

async function probeSession(): Promise<'admin' | 'editor'> {
  try {
    await getSystemInfo({ suppressAuthEvent: true });
    return 'admin';
  } catch (error) {
    if (!(error instanceof ApiError) || error.status !== 403) throw error;
    const access = await apiFetch<{ role: 'administrator' | 'editor' }>('/api/admin/v1/review/access', {
      suppressAuthEvent: true,
    });
    return access.role === 'administrator' ? 'admin' : 'editor';
  }
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<SessionState>({ kind: 'loading' });

  // Boot probe: subscription-style promise callbacks so state changes happen
  // asynchronously, never synchronously inside the effect body.
  useEffect(() => {
    let cancelled = false;
    probeSession()
      .then((kind) => {
        if (!cancelled) {
          setSession({ kind, username: readStoredUsername() });
        }
      })
      .catch((error: unknown) => {
        if (cancelled) {
          return;
        }
        if (error instanceof ApiError && error.status === 403) {
          setSession({ kind: 'forbidden' });
        } else {
          setSession({ kind: 'anonymous' });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const onExpired = () => setSession({ kind: 'anonymous' });
    window.addEventListener(AUTH_EXPIRED_EVENT, onExpired);
    return () => window.removeEventListener(AUTH_EXPIRED_EVENT, onExpired);
  }, []);

  const signIn = useCallback(async (identifier: string, password: string) => {
    // LoginRequest { userName?, email?, password } — camelCase, exactly as
    // the frozen native-v1 contract declares.
    const body = identifier.includes('@')
      ? { email: identifier, password }
      : { userName: identifier, password };
    await apiFetch<MessageBody>(SIGN_IN_PATH, {
      method: 'POST',
      body,
      suppressAuthEvent: true,
    });
    try {
      window.sessionStorage.setItem(USERNAME_STORAGE_KEY, identifier);
    } catch {
      // Display-only convenience; the session itself is the cookie.
    }
    // The probe re-reads the gate so a non-admin account lands on the
    // access-denied state instead of the shell.
    try {
      const kind = await probeSession();
      setSession({ kind, username: identifier });
    } catch (error) {
      if (error instanceof ApiError && error.status === 403) {
        setSession({ kind: 'forbidden' });
      } else {
        setSession({ kind: 'anonymous' });
      }
    }
  }, []);

  const signOut = useCallback(async () => {
    try {
      await apiFetch<MessageBody>(SIGN_OUT_PATH, { method: 'POST', suppressAuthEvent: true });
    } finally {
      try {
        window.sessionStorage.removeItem(USERNAME_STORAGE_KEY);
      } catch {
        // Nothing else to clean up; the server already expired the cookie.
      }
      setSession({ kind: 'anonymous' });
    }
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({ session, signIn, signOut }),
    [session, signIn, signOut],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

/**
 * Returns the current operator role for display gating. Outside an auth
 * provider (unit tests) it defaults to administrator; the server still
 * enforces every mutation.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function useSessionRole(): 'admin' | 'editor' {
  const context = useContext(AuthContext);
  return context?.session.kind === 'editor' ? 'editor' : 'admin';
}

// The context hook is intentionally colocated with its provider: consumers
// import one module, and the hook is the only non-component export here. The
// react-refresh rule is an HMR fast-refresh optimization, not a correctness
// requirement, so the warning is suppressed for this export.
// eslint-disable-next-line react-refresh/only-export-components
export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used inside <AuthProvider>');
  }
  return context;
}

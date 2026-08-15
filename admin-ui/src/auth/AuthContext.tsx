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
 *   403 → authenticated but not an administrator (access-denied state)
 *   401 → anonymous / expired session
 * The 401 path also fires from any later API call via `melodee:auth-expired`.
 */

export type SessionState =
  | { kind: 'loading' }
  | { kind: 'anonymous' }
  | { kind: 'forbidden' }
  | { kind: 'admin'; username: string | null };

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

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<SessionState>({ kind: 'loading' });

  // Boot probe: subscription-style promise callbacks so state changes happen
  // asynchronously, never synchronously inside the effect body.
  useEffect(() => {
    let cancelled = false;
    getSystemInfo({ suppressAuthEvent: true })
      .then(() => {
        if (!cancelled) {
          setSession({ kind: 'admin', username: readStoredUsername() });
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
      await getSystemInfo({ suppressAuthEvent: true });
      setSession({ kind: 'admin', username: identifier });
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

export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used inside <AuthProvider>');
  }
  return context;
}

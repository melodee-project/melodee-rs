import { expect, type APIRequestContext, type BrowserContext } from '@playwright/test';

/**
 * Shared live-session helper.
 *
 * The live suites need an authenticated Admin UI. Driving the login *form* in
 * every test is slow and rate-limit fragile, so this signs in once through the
 * native cookie endpoint and installs the session cookie into the browser
 * context. `LoginScreen` itself is covered by the live smoke suite, which still
 * uses the form.
 */

export const LIVE_URL = process.env.MELODEE_LIVE_URL ?? 'http://127.0.0.1:8080/admin';
export const LIVE_API = process.env.MELODEE_LIVE_API_URL ?? 'http://127.0.0.1:8080';
export const LIVE_USERNAME = process.env.MELODEE_LIVE_USERNAME ?? '';
export const LIVE_PASSWORD = process.env.MELODEE_LIVE_PASSWORD ?? '';
export const HAVE_CREDENTIALS = LIVE_USERNAME !== '' && LIVE_PASSWORD !== '';

/**
 * Ensures the test session is authenticated and returns its cookie header.
 *
 * The browser context already carries the session when the `live-setup` project
 * ran (see playwright.live.config.ts), so this reads the persisted state instead
 * of signing in again — the API rate-limits sign-in and repeated logins produce
 * `429`. It falls back to a real sign-in when no state file exists (running a
 * single spec directly).
 */
export async function signIn(
  context: BrowserContext,
  request: APIRequestContext,
): Promise<string> {
  const statePath = process.env.MELODEE_LIVE_STATE ?? '/tmp/melodee-live-auth-state.json';
  try {
    const { readFileSync } = await import('node:fs');
    const state = JSON.parse(readFileSync(statePath, 'utf8')) as {
      cookies: { name: string; value: string }[];
    };
    if (state.cookies.length > 0) {
      return state.cookies.map((cookie) => `${cookie.name}=${cookie.value}`).join('; ');
    }
  } catch {
    // No persisted state: fall through to a direct sign-in.
  }
  const response = await request.post(`${LIVE_API}/api/v1/auth/cookie/sign-in`, {
    data: { userName: LIVE_USERNAME, password: LIVE_PASSWORD },
  });
  expect(response.status(), 'admin sign-in').toBe(200);

  const headers = response.headersArray();
  type Cookie = Parameters<BrowserContext['addCookies']>[0][number];
  const cookies: Cookie[] = [];
  for (const header of headers) {
    if (header.name.toLowerCase() !== 'set-cookie') {
      continue;
    }
    const [pair] = header.value.split(';');
    const index = pair?.indexOf('=') ?? -1;
    if (!pair || index <= 0) {
      continue;
    }
    cookies.push({
      name: pair.slice(0, index).trim(),
      value: pair.slice(index + 1).trim(),
      domain: new URL(LIVE_API).hostname,
      path: '/',
      httpOnly: true,
      secure: false,
      sameSite: 'Lax',
    });
  }
  expect(cookies.length, 'session cookie issued').toBeGreaterThan(0);
  await context.addCookies(cookies);
  // Return the cookie pair: API assertions use the standalone request context
  // with this header, which is deterministic regardless of origin handling.
  return cookies.map((cookie) => `${cookie.name}=${cookie.value}`).join('; ');
}

/** GET with an explicit session cookie (the standalone request context). */
export async function apiGet<T>(
  request: APIRequestContext,
  cookie: string,
  path: string,
): Promise<T> {
  const response = await request.get(path, { headers: { Cookie: cookie } });
  if (!response.ok()) {
    throw new Error(`GET ${path} -> ${response.status()}`);
  }
  return (await response.json()) as T;
}

/** PUT with an explicit session cookie. */
export async function apiPut(
  request: APIRequestContext,
  cookie: string,
  path: string,
  body: unknown,
): Promise<number> {
  const response = await request.put(path, { headers: { Cookie: cookie }, data: body });
  return response.status();
}

/** Same-origin GET returning parsed JSON (carries the browser session). */
export async function apiJson<T>(page: import('@playwright/test').Page, path: string): Promise<T> {
  return page.evaluate(async (url) => {
    const response = await fetch(url, { credentials: 'same-origin' });
    if (!response.ok) {
      throw new Error(`GET ${url} -> ${response.status}`);
    }
    return (await response.json()) as unknown;
  }, path) as Promise<T>;
}

/** Same-origin request returning the status code (carries the browser session). */
export async function apiSend(
  page: import('@playwright/test').Page,
  path: string,
  method: 'PUT' | 'POST' | 'DELETE',
  body?: unknown,
): Promise<number> {
  return page.evaluate(
    async ({ url, verb, payload }) => {
      const response = await fetch(url, {
        method: verb,
        credentials: 'same-origin',
        headers: payload === null ? undefined : { 'Content-Type': 'application/json' },
        body: payload === null ? undefined : JSON.stringify(payload),
      });
      return response.status;
    },
    { url: path, verb: method, payload: body ?? null },
  );
}

import type { ApiErrorBody } from './types';

/**
 * Same-origin fetch wrapper for the admin API.
 *
 * The SPA authenticates through the server-issued cookie session (ADR-0139):
 * every request sends `credentials: 'include'` and no token is ever held in
 * web storage. Error responses use the stable envelope
 * `{code, message, correlationId}`; they surface here as a typed ApiError.
 * A 401 dispatches `melodee:auth-expired`, which the auth provider handles by
 * routing to the login screen.
 */

export const AUTH_EXPIRED_EVENT = 'melodee:auth-expired';

export class ApiError extends Error {
  readonly code: string;
  readonly correlationId: string | null;
  readonly status: number;

  constructor(status: number, code: string, message: string, correlationId: string | null) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.correlationId = correlationId;
  }
}

function isErrorBody(value: unknown): value is ApiErrorBody {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as ApiErrorBody).code === 'string' &&
    typeof (value as ApiErrorBody).message === 'string'
  );
}

async function parseError(response: Response): Promise<ApiError> {
  let body: unknown = null;
  try {
    body = await response.json();
  } catch {
    // Non-JSON error bodies (proxies, infra) degrade to status-only errors.
  }
  if (isErrorBody(body)) {
    return new ApiError(response.status, body.code, body.message, body.correlationId ?? null);
  }
  return new ApiError(response.status, 'http_error', `HTTP ${response.status}`, null);
}

export interface ApiRequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE';
  body?: unknown;
  /** Cancels the underlying HTTP request when a screen is superseded or unmounted. */
  signal?: AbortSignal;
  /** Raw non-JSON body (e.g. image bytes) with its wire Content-Type. */
  rawBody?: { data: BodyInit; contentType: string };
  /** When true, a 401 does not dispatch the auth-expired event (used by the
   *  session probe itself, which handles 401 locally). */
  suppressAuthEvent?: boolean;
}

export async function apiFetch<T>(path: string, options: ApiRequestOptions = {}): Promise<T> {
  const response = await fetch(path, {
    method: options.method ?? 'GET',
    credentials: 'include',
    signal: options.signal,
    headers:
      options.rawBody !== undefined
        ? { 'content-type': options.rawBody.contentType }
        : options.body !== undefined
          ? { 'content-type': 'application/json' }
          : undefined,
    body:
      options.rawBody !== undefined
        ? options.rawBody.data
        : options.body !== undefined
          ? JSON.stringify(options.body)
          : undefined,
  });
  if (response.status === 401 && !options.suppressAuthEvent) {
    window.dispatchEvent(new CustomEvent(AUTH_EXPIRED_EVENT));
  }
  if (!response.ok) {
    throw await parseError(response);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

/** Builds the documented limit/offset/sort query string (sort = key:asc|desc). */
export function buildQuery(params: Record<string, unknown>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== '' && value !== null) {
      search.set(key, String(value));
    }
  }
  const rendered = search.toString();
  return rendered ? `?${rendered}` : '';
}

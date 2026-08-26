import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError, apiFetch, AUTH_EXPIRED_EVENT, buildQuery } from './client';

function stubFetch(status: number, body: unknown) {
  const fetchMock = vi.fn().mockResolvedValue(
    new Response(body === null ? null : JSON.stringify(body), {
      status,
      headers: { 'content-type': 'application/json' },
    }),
  );
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('apiFetch', () => {
  it('sends credentials with every request (cookie session)', async () => {
    const fetchMock = stubFetch(200, { service: 'melodee' });
    await apiFetch('/api/admin/v1/system/info');
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/admin/v1/system/info',
      expect.objectContaining({ credentials: 'include' }),
    );
  });

  it('forwards cancellation to the underlying HTTP request', async () => {
    const fetchMock = stubFetch(200, { service: 'melodee' });
    const controller = new AbortController();
    await apiFetch('/api/admin/v1/system/info', { signal: controller.signal });
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/admin/v1/system/info',
      expect.objectContaining({ signal: controller.signal }),
    );
  });

  it('unwraps the error envelope into a typed ApiError', async () => {
    stubFetch(404, { code: 'not_found', message: 'work item not found', correlationId: 'abc' });
    const error: unknown = await apiFetch('/api/admin/v1/ingest/work-items/x').catch(
      (cause: unknown) => cause,
    );
    expect(error).toBeInstanceOf(ApiError);
    const apiError = error as ApiError;
    expect(apiError.code).toBe('not_found');
    expect(apiError.correlationId).toBe('abc');
    expect(apiError.status).toBe(404);
  });

  it('dispatches the auth-expired event on 401', async () => {
    stubFetch(401, { code: 'unauthenticated', message: 'unauthenticated', correlationId: 'x' });
    const listener = vi.fn();
    window.addEventListener(AUTH_EXPIRED_EVENT, listener);
    await apiFetch('/api/admin/v1/system/info').catch(() => undefined);
    window.removeEventListener(AUTH_EXPIRED_EVENT, listener);
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('suppresses the auth-expired event when asked (session probe)', async () => {
    stubFetch(401, { code: 'unauthenticated', message: 'x', correlationId: 'x' });
    const listener = vi.fn();
    window.addEventListener(AUTH_EXPIRED_EVENT, listener);
    await apiFetch('/probe', { suppressAuthEvent: true }).catch(() => undefined);
    window.removeEventListener(AUTH_EXPIRED_EVENT, listener);
    expect(listener).not.toHaveBeenCalled();
  });

  it('returns undefined for 204 responses', async () => {
    stubFetch(204, null);
    await expect(apiFetch<void>('/delete')).resolves.toBeUndefined();
  });
});

describe('buildQuery', () => {
  it('renders limit/offset/sort and skips empty values', () => {
    expect(buildQuery({ limit: 50, offset: 0, sort: 'addedAt:desc' })).toBe(
      '?limit=50&offset=0&sort=addedAt%3Adesc',
    );
    expect(buildQuery({})).toBe('');
    expect(buildQuery({ limit: 10, state: undefined })).toBe('?limit=10');
  });
});

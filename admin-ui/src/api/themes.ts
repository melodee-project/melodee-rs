import { ApiError, apiFetch } from './client';
import type { ThemeImportResult, ThemePackInfo } from './types';

/**
 * Client for the native-v1 theme-pack endpoints (PRD-THEME-004;
 * apps/melodee-server/src/native/discovery.rs). These are native-surface
 * routes, not `/api/admin/v1`:
 *
 *  - `GET /api/v1/themes` and `GET /api/v1/themes/{id}/file/{path}` are
 *    public (no auth), so they work pre-login and never dispatch the
 *    auth-expired event;
 *  - `POST /api/v1/themes/rescan`, `POST /api/v1/themes/import`,
 *    `GET /api/v1/themes/{id}/export`, and `DELETE /api/v1/themes/{id}` sit
 *    on the cookie-protected router gated by the legacy default cookie
 *    scheme and require an administrator principal — exactly the session
 *    the admin SPA holds (ADR-0139), so they are consumed as-is.
 *
 * Error bodies on the native surface do not use the admin `{code,message,
 * correlationId}` envelope (they are legacy shapes), so failures surface as
 * status-only ApiErrors; the UI maps them to localized fallback text.
 */

const NATIVE = '/api/v1';

/** Lists every theme pack the server knows about (built-in first). */
export function listThemePacks(): Promise<ThemePackInfo[]> {
  return apiFetch<ThemePackInfo[]>(`${NATIVE}/themes`);
}

/** Administrator-only: rescans the server theme directory. */
export function rescanThemePacks(): Promise<ThemePackInfo[]> {
  return apiFetch<ThemePackInfo[]>(`${NATIVE}/themes/rescan`, { method: 'POST' });
}

/** Administrator-only: deletes a non-built-in pack (server enforces it). */
export function deleteThemePack(id: string): Promise<void> {
  return apiFetch<void>(`${NATIVE}/themes/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

/** Administrator-only export: the pack ZIP as a same-origin download URL. */
export function themePackExportUrl(id: string): string {
  return `${NATIVE}/themes/${encodeURIComponent(id)}/export`;
}

/** Public asset URL inside a pack (e.g. `theme.css`, preview images). */
export function themePackAssetUrl(id: string, path: string): string {
  const encodedPath = path
    .split('/')
    .map((part) => encodeURIComponent(part))
    .join('/');
  return `${NATIVE}/themes/${encodeURIComponent(id)}/file/${encodedPath}`;
}

/**
 * Administrator-only import: uploads a theme ZIP as multipart field `file`.
 * Not routed through apiFetch, which is JSON-only; the same cookie
 * credentials and 401 behavior apply.
 */
export async function importThemePack(file: File): Promise<ThemeImportResult> {
  const body = new FormData();
  body.append('file', file, file.name);
  const response = await fetch(`${NATIVE}/themes/import`, {
    method: 'POST',
    credentials: 'include',
    body,
  });
  if (!response.ok) {
    let message = `HTTP ${response.status}`;
    try {
      const parsed: unknown = await response.json();
      if (
        typeof parsed === 'object' &&
        parsed !== null &&
        typeof (parsed as { error?: unknown }).error === 'string'
      ) {
        message = (parsed as { error: string }).error;
      }
    } catch {
      // Non-JSON error body; keep the status-only message.
    }
    throw new ApiError(response.status, 'theme_import_failed', message, null);
  }
  return (await response.json()) as ThemeImportResult;
}

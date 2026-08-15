import { expect, test, type Page } from '@playwright/test';

/**
 * Settings e2e (WP-19): the configuration browser with redacted secrets,
 * and the import flow — upload, preview=true diff, explicit apply with
 * preview=false. The admin API is mocked; no live server.
 */

const EXPORT_DOC = {
  format: 'melodee-config-export/v1',
  warning: 'Secrets are REDACTED and must be re-entered after import.',
  exportedAt: '2026-08-14T12:00:00Z',
  settings: {
    'server.port': 8080,
    'smtp.password': '__MELODEE_SECRET_REDACTED__',
  },
};

interface Captured {
  previewCalls: string[];
  applied: unknown;
}

async function mockAdminApi(page: Page): Promise<Captured> {
  const captured: Captured = { previewCalls: [], applied: null };
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (pathname === '/api/admin/v1/system/info') {
      await json({ service: 'melodee', version: 'test', status: 'ok' });
      return;
    }
    if (pathname === '/api/admin/v1/config/export') {
      await json(EXPORT_DOC);
      return;
    }
    if (pathname === '/api/admin/v1/config/import') {
      const preview = url.searchParams.get('preview') ?? 'true';
      if (preview === 'true') {
        captured.previewCalls.push(url.search);
        await json({
          applied: false,
          added: ['server.workers'],
          changed: ['server.port'],
          unchanged: [],
          secretsReentered: [],
          secretsSkipped: ['smtp.password'],
          unknown: [],
          warnings: [],
        });
      } else {
        captured.applied = route.request().postDataJSON();
        await json({
          applied: true,
          added: ['server.workers'],
          changed: ['server.port'],
          unchanged: [],
          secretsReentered: [],
          secretsSkipped: ['smtp.password'],
          unknown: [],
          warnings: [],
        });
      }
      return;
    }
    if (pathname === '/api/admin/v1/onboarding/status') {
      await json({
        completed: true,
        bypassed: false,
        canComplete: true,
        completedBy: 'admin',
        checks: [],
      });
      return;
    }
    await json({ code: 'not_found', message: 'not found', correlationId: null }, 404);
  });
  return captured;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('configuration browser groups settings and redacts secrets', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('system/settings');

  await expect(page.getByText('server.port')).toBeVisible();
  await expect(page.getByText('8080')).toBeVisible();
  // The secret renders as a redacted tag, never as an editable value.
  await expect(page.getByText('smtp.password')).toBeVisible();
  await expect(page.getByText('Redacted').first()).toBeVisible();
  await expect(page.getByText('Settings are read-only here')).toBeVisible();
});

test('import flow previews the diff then applies explicitly', async ({ page }) => {
  const captured = await mockAdminApi(page);
  await page.goto('system/settings');
  await expect(page.getByText('server.port')).toBeVisible();

  const upload = {
    format: 'melodee-config-export/v1',
    settings: { 'server.port': 9090, 'server.workers': 4 },
  };
  // The Theme tab adds a .zip uploader in its (hidden) panel; the
  // configuration import input is the JSON one.
  await page.locator('input[type="file"][accept*="json"]').setInputFiles({
    name: 'config.json',
    mimeType: 'application/json',
    buffer: Buffer.from(JSON.stringify(upload)),
  });

  // Preview request and the rendered diff.
  await expect.poll(() => captured.previewCalls.length).toBe(1);
  expect(captured.previewCalls[0]).toContain('preview=true');
  await expect(page.getByText('Import preview: config.json')).toBeVisible();
  await expect(page.getByText('server.workers')).toBeVisible();
  await expect(
    page.getByText(/These secret keys carry the redaction marker/),
  ).toBeVisible();

  // Explicit apply through the confirmation dialog.
  await page.getByRole('button', { name: 'Apply import' }).click();
  const dialog = page.getByRole('dialog', { name: 'Apply configuration import' });
  await dialog.getByRole('button', { name: 'Apply import' }).click();
  await expect.poll(() => captured.applied).toEqual(upload);
  await expect(page.getByText('Configuration import applied.')).toBeVisible();
});

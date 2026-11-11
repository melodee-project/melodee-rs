import { expect, test, type Page } from '@playwright/test';

/**
 * Onboarding wizard e2e (PRD-SETUP-001/002): fail-closed completion
 * gating, the 409 re-evaluation surface, the completed/bypassed redirect,
 * and the ar-SA dir=rtl smoke. The admin API is mocked.
 */

function check(name: string, overrides: Record<string, unknown> = {}) {
  return {
    name,
    required: true,
    passed: true,
    detail: `${name} ok`,
    remediation: null,
    ...overrides,
  };
}

const ALL_PASSING = [
  check('database'),
  check('storage_paths'),
  check('media_tools'),
  check('search_index', { required: false }),
  check('admin_user'),
  check('email', { required: false }),
];

async function mockAdminApi(
  page: Page,
  status: Record<string, unknown>,
  options: { completeConflict?: boolean } = {},
): Promise<{ completeCalls: number[] }> {
  const state = { completeCalls: [] as number[] };
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const method = route.request().method();
    const json = (body: unknown, statusCode = 200) =>
      route.fulfill({
        status: statusCode,
        contentType: 'application/json',
        body: JSON.stringify(body),
      });
    if (pathname === '/api/admin/v1/system/info') {
      await json({ service: 'melodee', version: 'test', status: 'ok' });
      return;
    }
    if (pathname === '/api/admin/v1/onboarding/status') {
      await json(status);
      return;
    }
    if (pathname === '/api/admin/v1/onboarding/complete' && method === 'POST') {
      state.completeCalls.push(1);
      if (options.completeConflict) {
        await json(
          {
            code: 'onboarding_checks_failed',
            message: 'required preflight checks are failing',
            correlationId: null,
            checks: status.checks,
          },
          409,
        );
      } else {
        await json({ ...status, completed: true, completedBy: 'admin' });
      }
      return;
    }
    await json({ code: 'not_found', message: 'not found', correlationId: null }, 404);
  });
  return state;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('fail-closed: a failing required check disables completion and shows remediation', async ({
  page,
}) => {
  const state = await mockAdminApi(page, {
    completed: false,
    bypassed: false,
    canComplete: false,
    completedBy: null,
    checks: ALL_PASSING.map((entry) =>
      entry.name === 'storage_paths'
        ? {
            ...entry,
            passed: false,
            detail: 'inbound path not writable',
            remediation: 'Fix permissions on the inbound library path.',
          }
        : entry,
    ),
  });
  await page.goto('onboarding');

  await expect(page.getByText('inbound path not writable')).toBeVisible();
  await expect(
    page.getByText('Fix permissions on the inbound library path.'),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Complete setup' })).toBeDisabled();
  // Re-run refetches the status without completing.
  await page.getByRole('button', { name: 'Re-run checks' }).click();
  await expect.poll(() => state.completeCalls.length).toBe(0);
});

test('completion posts and redirects home', async ({ page }) => {
  const state = await mockAdminApi(page, {
    completed: false,
    bypassed: false,
    canComplete: true,
    completedBy: null,
    checks: ALL_PASSING,
  });
  await page.goto('onboarding');

  await page.getByRole('button', { name: 'Complete setup' }).click();
  await expect.poll(() => state.completeCalls.length).toBe(1);
  await expect(page).toHaveURL(/\/admin$/);
});

test('a 409 from completion surfaces the fail-closed conflict message', async ({ page }) => {
  await mockAdminApi(
    page,
    {
      completed: false,
      bypassed: false,
      canComplete: true,
      completedBy: null,
      checks: ALL_PASSING,
    },
    { completeConflict: true },
  );
  await page.goto('onboarding');

  await page.getByRole('button', { name: 'Complete setup' }).click();
  await expect(page.getByText(/at least one required check is still failing/)).toBeVisible();
  await expect(page).toHaveURL(/\/onboarding/);
});

test('already-completed setup redirects to the overview', async ({ page }) => {
  await mockAdminApi(page, {
    completed: true,
    bypassed: false,
    canComplete: true,
    completedBy: 'admin',
    checks: ALL_PASSING,
  });
  await page.goto('onboarding');
  await expect(page).toHaveURL(/\/admin$/);
});

test('rtl smoke: ar-SA renders the wizard with dir=rtl', async ({ page }) => {
  await mockAdminApi(page, {
    completed: false,
    bypassed: false,
    canComplete: false,
    completedBy: null,
    checks: ALL_PASSING,
  });
  await page.goto('onboarding');
  await expect(page.getByText('Database').first()).toBeVisible();

  await page.getByRole('button', { name: /^Language:/ }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
});

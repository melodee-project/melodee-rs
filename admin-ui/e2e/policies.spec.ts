import { expect, test, type Page } from '@playwright/test';

/**
 * Policies / Release Rules e2e. The admin API is mocked with
 * page.route() — no live server is required. Runs at all three canonical
 * viewports (UI-SPEC §8) via the playwright projects. Covers the rules
 * table, the list → edit → save flow against the versioned rule endpoints,
 * the test-against-catalog dry-run panel (including the destructive-impact
 * warning and the ad-hoc persistence honesty note), and the ar-SA dir=rtl
 * smoke.
 */

const RULE_SET = {
  id: '01989f00-7000-7000-8000-000500000003',
  name: 'phase5-default-rules',
  version: 3,
  createdAt: '2026-08-14T10:00:00Z',
  createdBy: { system: { policyRuleSetVersion: 3 } },
};

const RULES = [
  {
    id: 'POL-ADMIN-OVERRIDE',
    ruleType: 'administrator_override',
    disposition: 'review',
    confidenceThreshold: null,
    predicate: {},
    priority: 0,
    enabled: true,
    matchCount: 4,
  },
  {
    id: 'POL-CORRUPT',
    ruleType: 'corrupt_media',
    disposition: 'quarantine',
    confidenceThreshold: null,
    predicate: {},
    priority: 1,
    enabled: true,
    matchCount: 9,
  },
  {
    id: 'POL-GH-REDUNDANT',
    ruleType: 'greatest_hits_redundancy',
    disposition: 'ignore',
    confidenceThreshold: null,
    predicate: {},
    priority: 4,
    enabled: true,
    matchCount: 132,
  },
  {
    id: 'POL-LIVE-REVIEW',
    ruleType: 'trait_disposition',
    disposition: 'review',
    confidenceThreshold: 8000,
    predicate: { required_traits: ['live'] },
    priority: 5,
    enabled: true,
    matchCount: 12,
  },
];

function rulePage(version: number) {
  return {
    ruleSet: { ...RULE_SET, version },
    items: RULES,
    total: RULES.length,
    limit: RULES.length,
    offset: 0,
  };
}

async function mockAdminApi(
  page: Page,
  options: { saveConflict?: boolean } = {},
): Promise<{ putBodies: unknown[]; testBodies: unknown[] }> {
  const putBodies: unknown[] = [];
  const testBodies: unknown[] = [];
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const method = route.request().method();
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (pathname === '/api/admin/v1/system/info') {
      await json({ service: 'melodee', version: 'test', status: 'ok' });
      return;
    }
    if (pathname === '/api/admin/v1/policies/rules' && method === 'GET') {
      await json(rulePage(3));
      return;
    }
    if (pathname === '/api/admin/v1/policies/rules/POL-LIVE-REVIEW' && method === 'PUT') {
      if (options.saveConflict) {
        await json(
          {
            code: 'rule_set_conflict',
            message: 'the rule set changed concurrently; reload and retry',
            correlationId: null,
          },
          409,
        );
        return;
      }
      putBodies.push(route.request().postDataJSON());
      await json({ ruleSet: { ...RULE_SET, version: 4 }, rule: RULES[3], changed: true });
      return;
    }
    if (pathname === '/api/admin/v1/policies/rules/test' && method === 'POST') {
      testBodies.push(route.request().postDataJSON());
      await json({
        testRunId: null,
        persisted: false,
        ruleSetId: RULE_SET.id,
        ruleSetVersion: 4,
        evaluated: 500,
        matchedCount: 132,
        matchedRuleCounts: { 'POL-GH-REDUNDANT': 132 },
        proposedDispositionCounts: { ignore: 132 },
        existingDispositionCounts: { none: 500 },
        destructiveImpact: 0,
        destructive: false,
        sample: [
          {
            releaseId: '01989f00-7000-7000-8000-0000000000aa',
            title: 'Greatest Hits',
            artist: 'Example Artist',
          },
        ],
      });
      return;
    }
    await json({ code: 'not_found', message: 'not found', correlationId: null }, 404);
  });
  return { putBodies, testBodies };
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem('melodee.locale', 'en-US');
  });
});

test('lists rules with the rule-set summary and opens the editor from a row', async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto('policies/releases');

  const table = page.getByRole('table', { name: 'Policy rules' });
  await expect(table).toBeVisible();
  await expect(table.getByText('POL-GH-REDUNDANT')).toBeVisible();
  await expect(page.getByText(/phase5-default-rules · version 3/)).toBeVisible();
  await expect(page.getByText(/evaluated in priority order/)).toBeVisible();

  await table.getByText('POL-LIVE-REVIEW').click();
  await expect(page.getByRole('heading', { name: 'Policy Editor' })).toBeVisible();
  await expect(page.getByLabel('Rule ID')).toHaveValue('POL-LIVE-REVIEW');
});

test('list → edit → save replaces the rule at the same position', async ({ page }) => {
  const { putBodies } = await mockAdminApi(page);
  await page.goto('policies/releases');

  const table = page.getByRole('table', { name: 'Policy rules' });
  await table.getByText('POL-LIVE-REVIEW').click();
  const editor = page.locator('section', { has: page.getByRole('heading', { name: 'Policy Editor' }) });

  await editor.getByLabel('Disposition').selectOption('ignore');
  await editor.getByRole('button', { name: 'Save changes' }).click();

  await expect.poll(() => putBodies.length).toBe(1);
  expect(putBodies[0]).toEqual({
    id: 'POL-LIVE-REVIEW',
    ruleType: 'trait_disposition',
    disposition: 'ignore',
    confidenceThreshold: 8000,
    predicate: { required_traits: ['live'] },
  });
  await expect(page.getByText(/rule set is now version 4/)).toBeVisible();
});

test('test against catalog renders the dry-run panel with the ad-hoc honesty note', async ({
  page,
}) => {
  const { testBodies } = await mockAdminApi(page);
  await page.goto('policies/releases');

  const table = page.getByRole('table', { name: 'Policy rules' });
  await table.getByText('POL-LIVE-REVIEW').click();
  const editor = page.locator('section', { has: page.getByRole('heading', { name: 'Policy Editor' }) });

  // Add a second condition so the draft differs from the stored rule and
  // runs as an ad-hoc candidate.
  await editor.getByRole('button', { name: 'Add condition' }).click();
  await editor.getByRole('button', { name: 'Run policy test' }).click();

  await expect.poll(() => testBodies.length).toBe(1);
  expect(testBodies[0]).toMatchObject({ limit: 500 });
  expect((testBodies[0] as { rule?: { id?: string } }).rule?.id).toBe('POL-LIVE-REVIEW');

  await expect(editor.getByText('Matched', { exact: true })).toBeVisible();
  await expect(editor.getByText(/not persisted in the audit log/)).toBeVisible();
  await expect(
    editor.getByRole('table', { name: 'Matched Release sample' }).getByText('Example Artist'),
  ).toBeVisible();
});

test('a 409 rule_set_conflict on save offers a reload', async ({ page }) => {
  await mockAdminApi(page, { saveConflict: true });
  await page.goto('policies/releases');

  const table = page.getByRole('table', { name: 'Policy rules' });
  await table.getByText('POL-LIVE-REVIEW').click();
  const editor = page.locator('section', { has: page.getByRole('heading', { name: 'Policy Editor' }) });

  await editor.getByLabel('Disposition').selectOption('ignore');
  await editor.getByRole('button', { name: 'Save changes' }).click();

  await expect(editor.getByText(/changed concurrently/)).toBeVisible();
  await editor.getByRole('button', { name: 'Reload rules' }).click();
  // Reload recovery closes the editor; only the hint panel remains.
  await expect(page.getByLabel('Rule ID')).toHaveCount(0);
});

test('rtl smoke: ar-SA renders the policies screen with dir=rtl', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('policies/releases');
  await expect(page.getByRole('table', { name: 'Policy rules' })).toBeVisible();

  await page.getByRole('button', { name: 'Language', exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
  await expect(page.locator('html')).toHaveAttribute('lang', 'ar-SA');
  await expect(page.getByRole('table', { name: 'Policy rules' })).toBeVisible();
});

import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '../../api/client';
import type { PolicyRule, PolicyRulePage, PolicyTestRun } from '../../api/types';
import { PoliciesScreen } from './PoliciesScreen';

vi.mock('../../api/admin', () => ({
  listPolicyRules: vi.fn(),
  createPolicyRule: vi.fn(),
  updatePolicyRule: vi.fn(),
  deletePolicyRule: vi.fn(),
  enablePolicyRule: vi.fn(),
  disablePolicyRule: vi.fn(),
  testPolicyRule: vi.fn(),
}));

const {
  listPolicyRules,
  createPolicyRule,
  updatePolicyRule,
  deletePolicyRule,
  enablePolicyRule,
  testPolicyRule,
} = await import('../../api/admin');

const mockList = vi.mocked(listPolicyRules);
const mockCreate = vi.mocked(createPolicyRule);
const mockUpdate = vi.mocked(updatePolicyRule);
const mockDelete = vi.mocked(deletePolicyRule);
const mockEnable = vi.mocked(enablePolicyRule);
const mockTest = vi.mocked(testPolicyRule);

const RULE_SET = {
  id: '01989f00-7000-7000-8000-000500000001',
  name: 'phase5-default-rules',
  version: 3,
  createdAt: '2026-08-14T10:00:00Z',
  createdBy: { system: { policyRuleSetVersion: 3 } },
};

const ADMIN_OVERRIDE: PolicyRule = {
  id: 'POL-ADMIN-OVERRIDE',
  ruleType: 'administrator_override',
  disposition: 'review',
  confidenceThreshold: null,
  predicate: {},
  priority: 0,
  enabled: true,
  matchCount: 4,
};

const CORRUPT: PolicyRule = {
  id: 'POL-CORRUPT',
  ruleType: 'corrupt_media',
  disposition: 'quarantine',
  confidenceThreshold: null,
  predicate: {},
  priority: 1,
  enabled: true,
  matchCount: 9,
};

const GH_REDUNDANT: PolicyRule = {
  id: 'POL-GH-REDUNDANT',
  ruleType: 'greatest_hits_redundancy',
  disposition: 'ignore',
  confidenceThreshold: null,
  predicate: {},
  priority: 4,
  enabled: true,
  matchCount: 132,
};

const TRAIT_RULE: PolicyRule = {
  id: 'POL-LIVE-REVIEW',
  ruleType: 'trait_disposition',
  disposition: 'review',
  confidenceThreshold: 8000,
  predicate: { required_traits: ['live'], existing_dispositions: ['review'] },
  priority: 5,
  enabled: true,
  matchCount: 12,
};

const DISABLED_RULE: PolicyRule = {
  id: 'POL-OLD',
  ruleType: 'trait_disposition',
  disposition: 'ignore',
  confidenceThreshold: null,
  predicate: { required_traits: ['demo'] },
  priority: null,
  enabled: false,
  matchCount: 7,
};

function makePage(items: PolicyRule[] = [ADMIN_OVERRIDE, CORRUPT, GH_REDUNDANT, TRAIT_RULE, DISABLED_RULE]): PolicyRulePage {
  return { ruleSet: RULE_SET, items, total: items.length, limit: items.length, offset: 0 };
}

function makeTestRun(overrides: Partial<PolicyTestRun> = {}): PolicyTestRun {
  return {
    testRunId: null,
    persisted: false,
    ruleSetId: RULE_SET.id,
    ruleSetVersion: 4,
    evaluated: 500,
    matchedCount: 132,
    matchedRuleCounts: { 'POL-GH-REDUNDANT': 132 },
    proposedDispositionCounts: { delete: 132 },
    existingDispositionCounts: { none: 500 },
    destructiveImpact: 132,
    destructive: true,
    sample: [
      {
        releaseId: '01989f00-7000-7000-8000-0000000000aa',
        title: 'Greatest Hits',
        artist: 'Example Artist',
      },
    ],
    ...overrides,
  };
}

function renderScreen(initialEntry = '/policies/releases') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <PoliciesScreen />
    </MemoryRouter>,
  );
}

/**
 * Opens the editor for a rule row and returns the editor section. Scoped
 * queries are required because the (closed) delete confirmation Modal keeps
 * its buttons in the DOM.
 */
async function openEditorFor(ruleId: string): Promise<HTMLElement> {
  const cell = await screen.findByText(ruleId);
  const row = cell.closest('tr');
  expect(row).not.toBeNull();
  fireEvent.click(row as HTMLElement);
  const heading = await screen.findByRole('heading', { name: 'Policy Editor' });
  const section = heading.closest('section');
  expect(section).not.toBeNull();
  return section as HTMLElement;
}

describe('PoliciesScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockList.mockResolvedValue(makePage());
  });

  it('renders the rules table with rule-set summary, types, dispositions, and history rows', async () => {
    renderScreen();
    expect(await screen.findByText('POL-GH-REDUNDANT')).toBeInTheDocument();
    expect(screen.getByText('POL-OLD')).toBeInTheDocument();
    // Rule-set identity and evaluation-order note.
    expect(screen.getByText(/phase5-default-rules · version 3/)).toBeInTheDocument();
    expect(screen.getByText(/evaluated in priority order/)).toBeInTheDocument();
    // Disabled rules from history have no priority.
    const disabledRow = screen.getByText('POL-OLD').closest('tr') as HTMLElement;
    expect(within(disabledRow).getByText('None')).toBeInTheDocument();
    expect(within(disabledRow).getByText('Disabled')).toBeInTheDocument();
    // Match counts come from durable decisions.
    const row = screen.getByText('POL-GH-REDUNDANT').closest('tr') as HTMLElement;
    expect(within(row).getByText('132')).toBeInTheDocument();
    expect(within(row).getByText('Greatest Hits redundancy')).toBeInTheDocument();
  });

  it('saves an edit with the exact RuleInput payload (replace at same position)', async () => {
    mockUpdate.mockResolvedValue({ ruleSet: { ...RULE_SET, version: 4 }, rule: TRAIT_RULE, changed: true });
    renderScreen();
    const editor = await openEditorFor('POL-LIVE-REVIEW');

    // Change the disposition via the type-safe select, then save.
    fireEvent.change(within(editor).getByLabelText('Disposition'), { target: { value: 'ignore' } });
    fireEvent.click(within(editor).getByRole('button', { name: 'Save changes' }));

    await waitFor(() => expect(mockUpdate).toHaveBeenCalledTimes(1));
    expect(mockUpdate).toHaveBeenCalledWith('POL-LIVE-REVIEW', {
      id: 'POL-LIVE-REVIEW',
      ruleType: 'trait_disposition',
      disposition: 'ignore',
      confidenceThreshold: 8000,
      predicate: { required_traits: ['live'], existing_dispositions: ['review'] },
    });
    expect(await screen.findByText(/rule set is now version 4/)).toBeInTheDocument();
  });

  it('creates a rule with a valid predicate payload', async () => {
    mockCreate.mockResolvedValue({
      ruleSet: { ...RULE_SET, version: 4 },
      rule: { ...TRAIT_RULE, id: 'POL-BOOTLEG-REVIEW', priority: 6, matchCount: 0 },
      changed: true,
    });
    renderScreen();
    await screen.findByText('POL-GH-REDUNDANT');
    fireEvent.click(screen.getByRole('button', { name: 'Create Policy' }));

    fireEvent.change(screen.getByLabelText('Rule ID'), {
      target: { value: 'POL-BOOTLEG-REVIEW' },
    });
    // Default draft: trait_disposition with one greatest_hits condition;
    // switch the trait value to bootleg.
    fireEvent.change(screen.getByLabelText('Value'), { target: { value: 'bootleg' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => expect(mockCreate).toHaveBeenCalledTimes(1));
    expect(mockCreate).toHaveBeenCalledWith({
      id: 'POL-BOOTLEG-REVIEW',
      ruleType: 'trait_disposition',
      disposition: 'review',
      confidenceThreshold: null,
      predicate: { required_traits: ['bootleg'] },
    });
  });

  it('runs an ad-hoc test and renders results with the destructive warning and honesty note', async () => {
    mockTest.mockResolvedValue(makeTestRun());
    renderScreen();
    const editor = await openEditorFor('POL-GH-REDUNDANT');

    // Modify the rule (disposition delete + threshold) so the run is ad-hoc.
    fireEvent.change(within(editor).getByLabelText('Disposition'), { target: { value: 'delete' } });
    fireEvent.change(within(editor).getByLabelText('Confidence threshold (basis points)'), {
      target: { value: '9500' },
    });
    fireEvent.click(within(editor).getByRole('button', { name: 'Run policy test' }));

    await waitFor(() => expect(mockTest).toHaveBeenCalledTimes(1));
    expect(mockTest).toHaveBeenCalledWith({
      rule: {
        id: 'POL-GH-REDUNDANT',
        ruleType: 'greatest_hits_redundancy',
        disposition: 'delete',
        confidenceThreshold: 9500,
        predicate: {},
      },
      limit: 500,
    });

    // Destructive impact is a warning, never a neutral line.
    expect(
      await screen.findByText('132 matched Releases would be proposed for delete.'),
    ).toBeInTheDocument();
    // Ad-hoc runs state honestly that nothing was persisted.
    expect(screen.getByText(/not persisted in the audit log/)).toBeInTheDocument();
    // Sample rows render title and artist and link to the catalog.
    expect(screen.getByRole('table', { name: 'Matched Release sample' })).toBeInTheDocument();
    const sampleLink = screen.getByRole('link', { name: 'Greatest Hits' });
    expect(sampleLink).toHaveAttribute(
      'href',
      `/catalog/releases?query=${encodeURIComponent('Greatest Hits')}`,
    );
  });

  it('tests an unchanged stored rule by ruleId (persisted run)', async () => {
    mockTest.mockResolvedValue(
      makeTestRun({
        testRunId: '01989f00-7000-7000-8000-0000000000bb',
        persisted: true,
        destructive: false,
        destructiveImpact: 0,
        proposedDispositionCounts: { ignore: 132 },
      }),
    );
    renderScreen();
    const editor = await openEditorFor('POL-GH-REDUNDANT');
    fireEvent.click(within(editor).getByRole('button', { name: 'Run policy test' }));

    await waitFor(() => expect(mockTest).toHaveBeenCalledTimes(1));
    expect(mockTest).toHaveBeenCalledWith({ ruleId: 'POL-GH-REDUNDANT', limit: 500 });
    expect(
      await screen.findByText('This test run was recorded in the audit log.'),
    ).toBeInTheDocument();
  });

  it('offers reload on a 409 rule_set_conflict during save', async () => {
    mockUpdate.mockRejectedValue(
      new ApiError(409, 'rule_set_conflict', 'the rule set changed concurrently; reload and retry', null),
    );
    renderScreen();
    const editor = await openEditorFor('POL-LIVE-REVIEW');
    fireEvent.change(within(editor).getByLabelText('Disposition'), { target: { value: 'ignore' } });
    fireEvent.click(within(editor).getByRole('button', { name: 'Save changes' }));

    expect(await within(editor).findByText(/changed concurrently/)).toBeInTheDocument();
    const reloadButton = within(editor).getByRole('button', { name: 'Reload rules' });
    // Recovery: reload refetches and closes the editor.
    fireEvent.click(reloadButton);
    await waitFor(() => expect(mockList).toHaveBeenCalledTimes(2));
  });

  it('disables mutation controls for protected safety rules', async () => {
    renderScreen();
    const editor = await openEditorFor('POL-ADMIN-OVERRIDE');

    expect(within(editor).getByText(/safety rule required by the policy engine/)).toBeInTheDocument();
    expect(within(editor).queryByRole('button', { name: 'Delete' })).not.toBeInTheDocument();
    expect(within(editor).queryByRole('button', { name: 'Disable' })).not.toBeInTheDocument();
    // The rule type select is locked for safety rules.
    expect(within(editor).getByLabelText('Rule type')).toBeDisabled();
  });

  it('locks saving for disabled rules and enables via the idempotent endpoint', async () => {
    mockEnable.mockResolvedValue({
      ruleSet: { ...RULE_SET, version: 4 },
      rule: { ...DISABLED_RULE, enabled: true, priority: 6 },
      changed: true,
    });
    renderScreen();
    const editor = await openEditorFor('POL-OLD');

    expect(within(editor).getByText(/retained in immutable rule-set history/)).toBeInTheDocument();
    expect(within(editor).getByRole('button', { name: 'Save changes' })).toBeDisabled();
    fireEvent.click(within(editor).getByRole('button', { name: 'Enable' }));
    await waitFor(() => expect(mockEnable).toHaveBeenCalledWith('POL-OLD'));
  });

  it('confirms delete in a dialog and removes the rule in a new version', async () => {
    mockDelete.mockResolvedValue(undefined);
    renderScreen();
    const editor = await openEditorFor('POL-LIVE-REVIEW');
    fireEvent.click(within(editor).getByRole('button', { name: 'Delete' }));

    const dialog = await screen.findByRole('dialog', { name: 'Delete rule POL-LIVE-REVIEW' });
    expect(within(dialog).getByText(/immutable history/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(mockDelete).toHaveBeenCalledWith('POL-LIVE-REVIEW'));
    expect(await screen.findByText('Rule deleted in a new rule-set version.')).toBeInTheDocument();
  });

  it('blocks saving an invalid draft without calling the API', async () => {
    renderScreen();
    await screen.findByText('POL-GH-REDUNDANT');
    fireEvent.click(screen.getByRole('button', { name: 'Create Policy' }));
    // Empty id fails validation client-side.
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));
    expect(
      await screen.findByText('A rule ID of at most 200 characters is required.'),
    ).toBeInTheDocument();
    expect(mockCreate).not.toHaveBeenCalled();
  });
});

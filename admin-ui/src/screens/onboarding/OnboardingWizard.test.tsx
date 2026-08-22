import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '../../api/client';
import type { OnboardingCheck, OnboardingStatus } from '../../api/types';
import { OnboardingWizard } from './OnboardingWizard';

vi.mock('../../api/admin', () => ({
  getOnboardingStatus: vi.fn(),
  completeOnboarding: vi.fn(),
}));

const { getOnboardingStatus, completeOnboarding } = await import('../../api/admin');
const mockGetStatus = vi.mocked(getOnboardingStatus);
const mockComplete = vi.mocked(completeOnboarding);

function makeCheck(name: string, overrides: Partial<OnboardingCheck> = {}): OnboardingCheck {
  return {
    name,
    required: true,
    passed: true,
    detail: `${name} ok`,
    remediation: null,
    ...overrides,
  };
}

const ALL_PASSING: OnboardingCheck[] = [
  makeCheck('database'),
  makeCheck('storage_paths'),
  makeCheck('media_tools'),
  makeCheck('search_index', { required: false }),
  makeCheck('admin_user'),
  makeCheck('email', { required: false }),
];

function makeStatus(overrides: Partial<OnboardingStatus> = {}): OnboardingStatus {
  return {
    completed: false,
    bypassed: false,
    canComplete: true,
    completedBy: null,
    checks: ALL_PASSING,
    ...overrides,
  };
}

function renderWizard() {
  return render(
    <MemoryRouter initialEntries={['/onboarding']}>
      <Routes>
        <Route path="/onboarding" element={<OnboardingWizard />} />
        <Route path="/" element={<output data-testid="home">home</output>} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('OnboardingWizard', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders the six checks with status, detail, and remediation', async () => {
    mockGetStatus.mockResolvedValue(
      makeStatus({
        canComplete: false,
        checks: ALL_PASSING.map((check) =>
          check.name === 'storage_paths'
            ? {
                ...check,
                passed: false,
                detail: 'inbound path not writable',
                remediation: 'Fix permissions on the inbound library path.',
              }
            : check,
        ),
      }),
    );
    renderWizard();
    // Each check name appears in both the stepper and the detail list.
    expect((await screen.findAllByText('Database')).length).toBeGreaterThan(0);
    expect(screen.getAllByText('Storage paths').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Media tools').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Search index').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Administrator account').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Email delivery').length).toBeGreaterThan(0);
    expect(screen.getByText('inbound path not writable')).toBeInTheDocument();
    expect(
      screen.getByText(/Fix permissions on the inbound library path\./),
    ).toBeInTheDocument();
  });

  it('is fail-closed: the complete button is disabled until canComplete', async () => {
    mockGetStatus.mockResolvedValue(makeStatus({ canComplete: false }));
    renderWizard();
    expect(
      await screen.findByRole('button', { name: 'Complete setup' }),
    ).toBeDisabled();
    expect(mockComplete).not.toHaveBeenCalled();
  });

  it('completes when canComplete and redirects home', async () => {
    mockGetStatus.mockResolvedValue(makeStatus());
    mockComplete.mockResolvedValue(makeStatus({ completed: true, completedBy: 'admin' }));
    renderWizard();
    fireEvent.click(await screen.findByRole('button', { name: 'Complete setup' }));
    await waitFor(() => expect(mockComplete).toHaveBeenCalledTimes(1));
    expect(await screen.findByTestId('home')).toBeInTheDocument();
  });

  it('surfaces the 409 onboarding_checks_failed envelope and refetches status', async () => {
    mockGetStatus.mockResolvedValue(makeStatus());
    mockComplete.mockRejectedValue(
      new ApiError(409, 'onboarding_checks_failed', 'required preflight checks are failing', null),
    );
    renderWizard();
    fireEvent.click(await screen.findByRole('button', { name: 'Complete setup' }));
    expect(await screen.findByText(/at least one required check is still failing/))
      .toBeInTheDocument();
    await waitFor(() => expect(mockGetStatus).toHaveBeenCalledTimes(2));
  });

  it('redirects home when setup is already completed', async () => {
    mockGetStatus.mockResolvedValue(makeStatus({ completed: true }));
    renderWizard();
    expect(await screen.findByTestId('home')).toBeInTheDocument();
    expect(mockComplete).not.toHaveBeenCalled();
  });

  it('redirects home when setup was bypassed by migration heuristics', async () => {
    mockGetStatus.mockResolvedValue(makeStatus({ bypassed: true }));
    renderWizard();
    expect(await screen.findByTestId('home')).toBeInTheDocument();
  });

  it('re-runs the checks on demand', async () => {
    mockGetStatus.mockResolvedValue(makeStatus({ canComplete: false }));
    renderWizard();
    fireEvent.click(await screen.findByRole('button', { name: 'Re-run checks' }));
    await waitFor(() => expect(mockGetStatus).toHaveBeenCalledTimes(2));
  });
});

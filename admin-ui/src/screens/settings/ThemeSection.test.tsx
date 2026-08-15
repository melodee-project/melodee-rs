import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { ThemePackInfo } from '../../api/types';
import { ThemeProvider } from '../../theme/ThemeProvider';
import { clearThemePackStyles } from '../../theme/packs';
import { ThemeSection } from './ThemeSection';

vi.mock('../../api/themes', () => ({
  listThemePacks: vi.fn(),
  rescanThemePacks: vi.fn(),
  deleteThemePack: vi.fn(),
  importThemePack: vi.fn(),
  themePackExportUrl: vi.fn((id: string) => `/api/v1/themes/${id}/export`),
  themePackAssetUrl: vi.fn((id: string, path: string) => `/api/v1/themes/${id}/file/${path}`),
}));

const themes = await import('../../api/themes');
const mockList = vi.mocked(themes.listThemePacks);
const mockRescan = vi.mocked(themes.rescanThemePacks);
const mockDelete = vi.mocked(themes.deleteThemePack);

const BUILT_IN: ThemePackInfo = {
  id: 'server-default',
  name: 'Server Built-in',
  author: 'Melodee',
  version: '1.0.0',
  description: null,
  isBuiltIn: true,
  previewImage: null,
  hasWarnings: false,
  warningDetails: null,
};

const NEON: ThemePackInfo = {
  id: 'neon',
  name: 'Neon Nights',
  author: 'Example Author',
  version: '2.1.0',
  description: 'High-saturation accent pack',
  isBuiltIn: false,
  previewImage: null,
  hasWarnings: false,
  warningDetails: null,
};

function renderSection() {
  return render(
    <MemoryRouter>
      <ThemeProvider>
        <ThemeSection />
      </ThemeProvider>
    </MemoryRouter>,
  );
}

function stubThemeCss(css: string, status = 200) {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(css, { status })));
}

describe('ThemeSection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    clearThemePackStyles();
    mockList.mockResolvedValue([BUILT_IN, NEON]);
  });

  it('lists the server theme packs with the built-in default first', async () => {
    renderSection();

    // Carbon renders the group as a fieldset + legend (role "group").
    const group = await screen.findByRole('group', { name: 'Available theme packs' });
    expect(group).toBeInTheDocument();
    expect(screen.getByText('Melodee default')).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /Neon Nights/ })).toBeInTheDocument();
    expect(
      screen.getByText(/High-saturation accent pack/, { exact: false }),
    ).toBeInTheDocument();
    expect(screen.getByText('Server Built-in')).toBeInTheDocument();
    // The Melodee default is selected out of the box.
    expect(screen.getByRole('radio', { name: /Melodee default/ })).toBeChecked();
  });

  it('applies a pack onto the token layer and persists the choice', async () => {
    stubThemeCss(':root { --melodee-brand-accent: #ff00ff; }');
    renderSection();
    await screen.findByRole('radio', { name: /Neon Nights/ });

    fireEvent.click(screen.getByRole('radio', { name: /Neon Nights/ }));

    await waitFor(() => {
      expect(document.querySelector('#melodee-theme-pack-style')?.textContent).toContain(
        '--melodee-brand-accent: #ff00ff;',
      );
    });
    expect(window.localStorage.getItem('melodee.theme-pack')).toBe('neon');
  });

  it('explains the fallback when the selected pack cannot be applied', async () => {
    stubThemeCss('not found', 404);
    renderSection();
    await screen.findByRole('radio', { name: /Neon Nights/ });

    fireEvent.click(screen.getByRole('radio', { name: /Neon Nights/ }));

    expect(
      await screen.findByText('The theme pack could not be applied'),
    ).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /Melodee default/ })).toBeChecked();
  });

  it('shows a retryable error when the pack list fails to load', async () => {
    mockList.mockRejectedValue(new Error('offline'));
    renderSection();

    expect(await screen.findByText('Theme packs could not be loaded')).toBeInTheDocument();
    mockList.mockResolvedValue([NEON]);
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByRole('radio', { name: /Neon Nights/ })).toBeInTheDocument();
  });

  it('rescans the server theme directory on demand', async () => {
    mockRescan.mockResolvedValue([BUILT_IN, NEON]);
    renderSection();
    await screen.findByRole('radio', { name: /Neon Nights/ });

    fireEvent.click(screen.getByRole('button', { name: 'Rescan theme packs' }));

    await waitFor(() => expect(mockRescan).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('Theme packs rescanned.')).toBeInTheDocument();
  });

  it('deletes a non-built-in pack only after confirmation', async () => {
    mockDelete.mockResolvedValue(undefined);
    renderSection();
    await screen.findByRole('radio', { name: /Neon Nights/ });

    // Built-in packs never get a delete affordance (server rule mirrored).
    const rows = screen.getAllByRole('button', { name: 'Delete' });
    expect(rows).toHaveLength(1);

    fireEvent.click(rows[0]!);
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent('Neon Nights');
    fireEvent.click(screen.getByRole('button', { name: 'Delete theme pack' }));

    await waitFor(() => expect(mockDelete).toHaveBeenCalledWith('neon'));
    expect(await screen.findByText('Theme pack deleted.')).toBeInTheDocument();
  });

  it('offers export only as the documented same-origin download URL', async () => {
    renderSection();
    await screen.findByRole('radio', { name: /Neon Nights/ });

    const exportLink = screen.getByRole('link', { name: 'Export' });
    expect(exportLink).toHaveAttribute('href', '/api/v1/themes/neon/export');
  });
});

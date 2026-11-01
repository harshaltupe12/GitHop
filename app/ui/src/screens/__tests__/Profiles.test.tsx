import { screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../../lib/ipc';
import { setQueryData } from '../../lib/query';
import { STATE_KEY } from '../../lib/appState';
import { expectNoA11yViolations } from '../../test/a11y';
import { appState, office, personal, profileSet } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { ProfilesScreen } from '../Profiles';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

beforeEach(() => {
  setQueryData(STATE_KEY, appState());
});

describe('Profiles screen', () => {
  it('empty state: icon, copy and Add profile', async () => {
    const { container, user } = renderWith(
      <ProfilesScreen profiles={{ version: 1, default: null, profile: [] }} />,
    );
    expect(screen.getByRole('heading', { name: 'No profiles yet' })).toBeInTheDocument();
    expect(screen.getByText('Add your first identity to start.')).toBeInTheDocument();
    const adds = screen.getAllByRole('button', { name: 'Add profile' });
    expect(adds).toHaveLength(2);
    await expectNoA11yViolations(container);
    await user.click(adds[1]!);
    expect(screen.getByRole('heading', { level: 1, name: 'Add profile' })).toBeInTheDocument();
  });

  it('normal: default profile first, then alphabetical', async () => {
    const { container } = renderWith(<ProfilesScreen profiles={profileSet()} />);
    const cards = screen.getAllByRole('article');
    expect(cards.map((c) => within(c).getByRole('heading').textContent)).toEqual([
      'Personal',
      'Office',
    ]);
    expect(within(cards[0]!).getByText('Default')).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('Make default from the overflow menu calls set_default and toasts', async () => {
    const { user } = renderWith(<ProfilesScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'More actions for Office' }));
    await user.click(screen.getByRole('menuitem', { name: 'Make default' }));
    expect(m.setDefault).toHaveBeenCalledWith('office');
    expect(await screen.findByText('Default profile: Office')).toBeInTheDocument();
  });

  it('Test connection: denied shows a warning toast', async () => {
    m.testConnection.mockResolvedValueOnce({ kind: 'denied' });
    const { user } = renderWith(<ProfilesScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'More actions for Office' }));
    await user.click(screen.getByRole('menuitem', { name: 'Test connection' }));
    expect(m.testConnection).toHaveBeenCalledWith('office');
    expect(await screen.findByText(/GitHub didn’t accept this key/)).toBeInTheDocument();
  });

  it('error: set_default failure shows an error toast', async () => {
    m.setDefault.mockRejectedValueOnce({
      code: 'locked',
      message: 'Profiles are being edited elsewhere.',
      issues: [],
    });
    const { user } = renderWith(<ProfilesScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'More actions for Office' }));
    await user.click(screen.getByRole('menuitem', { name: 'Make default' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Profiles are being edited elsewhere.',
    );
  });

  it('Delete needs the typed label; default needs a new default', async () => {
    m.deleteProfile.mockResolvedValueOnce(profileSet({ default: 'office', profile: [office] }));
    const { user } = renderWith(<ProfilesScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'More actions for Personal' }));
    await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
    const dlg = screen.getByRole('dialog', { name: 'Delete Personal?' });
    const confirm = within(dlg).getByRole('button', { name: 'Delete profile' });
    expect(confirm).toBeDisabled();
    expect(within(dlg).getByRole('combobox', { name: 'New default profile' })).toHaveValue(
      'office',
    );
    await user.type(
      within(dlg).getByRole('textbox', { name: 'Type Personal to confirm' }),
      'Personal',
    );
    expect(confirm).toBeEnabled();
    await expectNoA11yViolations();
    await user.click(confirm);
    await waitFor(() => expect(m.deleteProfile).toHaveBeenCalledWith('personal', 'office'));
  });

  it('Ctrl+N intent opens the add form', () => {
    renderWith(<ProfilesScreen profiles={profileSet()} />, { intent: { kind: 'new-profile' } });
    expect(screen.getByRole('heading', { level: 1, name: 'Add profile' })).toBeInTheDocument();
  });

  it('Edit opens the form with the profile', async () => {
    const { user } = renderWith(<ProfilesScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'Edit Personal' }));
    expect(screen.getByRole('heading', { level: 1, name: 'Edit Personal' })).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Email' })).toHaveValue(personal.email);
  });
});

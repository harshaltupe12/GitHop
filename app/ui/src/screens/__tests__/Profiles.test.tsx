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

  describe('Test connection result on the card', () => {
    const testFrom = async (user: ReturnType<typeof renderWith>['user'], label: string) => {
      await user.click(screen.getByRole('button', { name: `More actions for ${label}` }));
      await user.click(screen.getByRole('menuitem', { name: 'Test connection' }));
    };
    const card = (label: string) => screen.getByRole('article', { name: label });

    it('busy, then success: inline check + account + just now, and a toast', async () => {
      let done: (v: { kind: 'connected'; user: string }) => void = () => undefined;
      m.testConnection.mockImplementationOnce(() => new Promise((r) => (done = r)));
      const { user, container } = renderWith(<ProfilesScreen profiles={profileSet()} />);
      await testFrom(user, 'Office');
      expect(m.testConnection).toHaveBeenCalledWith('office');
      expect(within(card('Office')).getByRole('status')).toHaveTextContent('Testing…');
      done({ kind: 'connected', user: 'office-org' });
      expect(
        await within(card('Office')).findByText('Connected to GitHub as office-org'),
      ).toBeInTheDocument();
      expect(within(card('Office')).getByText(/just now/)).toBeInTheDocument();
      expect(await screen.findByText('Connected to GitHub as office-org.')).toBeInTheDocument();
      // Matches the profile's owner: no wrong-account warning.
      expect(screen.queryByText(/This key belongs to/)).toBeNull();
      await expectNoA11yViolations(container);
    });

    it('connected as an account the profile doesn’t own: warns', async () => {
      m.testConnection.mockResolvedValueOnce({ kind: 'connected', user: 'jane-acme' });
      const { user } = renderWith(<ProfilesScreen profiles={profileSet()} />);
      await testFrom(user, 'Personal');
      expect(
        await within(card('Personal')).findByText(
          'This key belongs to jane-acme, but this profile owns jane-personal.',
        ),
      ).toBeInTheDocument();
    });

    it('denied: what to do next, Copy public key and Open GitHub SSH settings', async () => {
      m.testConnection.mockResolvedValueOnce({ kind: 'denied' });
      m.readPublicKey.mockResolvedValueOnce('ssh-ed25519 AAAA office\n');
      const { user, container } = renderWith(<ProfilesScreen profiles={profileSet()} />);
      await testFrom(user, 'Office');
      const c = card('Office');
      expect(await within(c).findByText('GitHub didn’t accept this key')).toBeInTheDocument();
      expect(
        within(c).getByText(
          'Add it to the GitHub account that owns this profile’s repos (office-org), then test again.',
        ),
      ).toBeInTheDocument();
      await user.click(within(c).getByRole('button', { name: 'Copy public key' }));
      expect(m.readPublicKey).toHaveBeenCalledWith('office');
      expect(m.copyText).toHaveBeenCalledWith('ssh-ed25519 AAAA office');
      await user.click(within(c).getByRole('button', { name: 'Open GitHub SSH settings' }));
      expect(m.openExternal).toHaveBeenCalledWith('ssh-keys');
      await expectNoA11yViolations(container);
    });

    it('failed: the reason and Try again', async () => {
      m.testConnection.mockClear();
      m.testConnection
        .mockResolvedValueOnce({ kind: 'failed', reason: 'ssh wasn’t found' })
        .mockResolvedValueOnce({ kind: 'connected', user: 'office-org' });
      const { user } = renderWith(<ProfilesScreen profiles={profileSet()} />);
      await testFrom(user, 'Office');
      const c = card('Office');
      expect(await within(c).findByRole('alert')).toHaveTextContent(
        'Couldn’t test the connection: ssh wasn’t found',
      );
      await user.click(within(c).getByRole('button', { name: 'Try again' }));
      expect(m.testConnection).toHaveBeenCalledTimes(2);
      expect(await within(c).findByText('Connected to GitHub as office-org')).toBeInTheDocument();
    });
  });

  it('card warning from the core: Add owner opens Edit with Owns focused', async () => {
    const msg =
      '"Office" has no GitHub owners or folders, so GitHop can\'t tell which repos are its.';
    m.validateProfile.mockImplementation((p) =>
      Promise.resolve(
        p.id === 'office'
          ? [{ profile_id: 'office', field: 'owners', severity: 'error', message: msg }]
          : [],
      ),
    );
    const ps = profileSet({ profile: [{ ...office, owners: [], folders: [] }, personal] });
    const { user } = renderWith(<ProfilesScreen profiles={ps} />);
    const c = screen.getByRole('article', { name: 'Office' });
    expect(await within(c).findByText(msg)).toBeInTheDocument();
    expect(within(screen.getByRole('article', { name: 'Personal' })).queryByText(msg)).toBeNull();
    await user.click(within(c).getByRole('button', { name: 'Add owner to Office' }));
    expect(screen.getByRole('heading', { level: 1, name: 'Edit Office' })).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'GitHub owners' })).toHaveFocus();
    // The core's message is right under Owns, live from validate_profile.
    expect(
      await within(screen.getByRole('region', { name: 'Owns' })).findByText(msg),
    ).toBeInTheDocument();
    m.validateProfile.mockReset();
    m.validateProfile.mockResolvedValue([]);
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

import { screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import * as ipc from '../../lib/ipc';
import { expectNoA11yViolations } from '../../test/a11y';
import { office, personal, profileSet } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { ProfileForm } from '../ProfileForm';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

describe('ProfileForm', () => {
  it('add: sections, id follows label, next free colour, sticky Save/Cancel', async () => {
    const onSaved = vi.fn();
    const onCancel = vi.fn();
    const { container, user } = renderWith(
      <ProfileForm profiles={profileSet()} onSaved={onSaved} onCancel={onCancel} />,
    );
    for (const h of ['Identity', 'Colour', 'Owns', 'SSH key'])
      expect(screen.getByRole('heading', { name: h })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Danger zone' })).toBeNull();
    // blue + violet are used → pink is next
    expect(screen.getByRole('radio', { name: 'pink' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('radio', { name: 'blue, used by office' })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await user.type(screen.getByRole('textbox', { name: 'Label' }), 'Free Lance');
    expect(screen.getByRole('textbox', { name: 'Profile ID' })).toHaveValue('free-lance');
    await waitFor(() =>
      expect(
        screen.getByText(/Creates C:\/Users\/harshal\/\.ssh\/githop_free-lance/),
      ).toBeInTheDocument(),
    );
    await expectNoA11yViolations(container);
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onCancel).toHaveBeenCalled();
  });

  it('noreply helper fills the GitHub noreply format from the first owner', async () => {
    const { user } = renderWith(<ProfileForm profiles={profileSet()} onSaved={vi.fn()} />);
    await user.type(screen.getByRole('textbox', { name: 'GitHub owners' }), 'harshal{Enter}');
    await user.click(screen.getByRole('button', { name: 'Use noreply' }));
    expect(screen.getByRole('textbox', { name: 'Email' })).toHaveValue(
      'ID+harshal@users.noreply.github.com',
    );
    expect(screen.getByText(/Replace ID and the user name/)).toBeInTheDocument();
  });

  it('noreply placeholder ID+ blocks save until replaced', async () => {
    const { user } = renderWith(<ProfileForm profiles={profileSet()} onSaved={vi.fn()} />);
    await user.type(screen.getByRole('textbox', { name: 'Label' }), 'Free');
    await user.click(screen.getByRole('button', { name: 'Use noreply' }));
    await user.click(screen.getByRole('button', { name: 'Save profile' }));
    const email = screen.getByRole('textbox', { name: 'Email' });
    await waitFor(() => expect(email).toHaveAttribute('aria-invalid', 'true'));
    expect(email).toHaveAccessibleDescription(/Replace ID with your GitHub ID number/);
    expect(email).toHaveFocus();
    expect(m.saveProfile).not.toHaveBeenCalled();
    await user.clear(email);
    await user.type(email, '987+free@users.noreply.github.com');
    await waitFor(() => expect(email).not.toHaveAttribute('aria-invalid', 'true'));
  });

  it('focuses the form heading on mount (screen change)', async () => {
    renderWith(<ProfileForm profiles={profileSet()} onSaved={vi.fn()} />);
    expect(screen.getByRole('heading', { level: 1, name: 'Add profile' })).toHaveFocus();
  });

  it('inline validation from validate_profile maps issues to fields and blocks save', async () => {
    m.validateProfile.mockResolvedValue([
      {
        profile_id: 'x',
        field: 'email',
        severity: 'error',
        message: 'Email already used by Office',
      },
      {
        profile_id: 'x',
        field: 'owners',
        severity: 'error',
        message: 'Owner office-org already belongs to Office',
      },
    ]);
    const { user } = renderWith(<ProfileForm profiles={profileSet()} onSaved={vi.fn()} />);
    await user.type(screen.getByRole('textbox', { name: 'Label' }), 'X');
    await user.click(screen.getByRole('button', { name: 'Save profile' }));
    const email = screen.getByRole('textbox', { name: 'Email' });
    await waitFor(() => expect(email).toHaveAttribute('aria-invalid', 'true'));
    expect(email).toHaveAccessibleDescription(/Email already used by Office/);
    expect(screen.getByText('Owner office-org already belongs to Office')).toBeInTheDocument();
    expect(screen.getByText('Fix the highlighted fields to save.')).toBeInTheDocument();
    expect(email).toHaveFocus();
    expect(m.saveProfile).not.toHaveBeenCalled();
    m.validateProfile.mockResolvedValue([]);
  });

  it('save: calls save_profile with the generated key path and reports the set', async () => {
    const saved = profileSet();
    m.saveProfile.mockResolvedValueOnce(saved);
    const onSaved = vi.fn();
    const { user } = renderWith(<ProfileForm profiles={profileSet()} onSaved={onSaved} />);
    await user.type(screen.getByRole('textbox', { name: 'Label' }), 'Freelance');
    await user.type(screen.getByRole('textbox', { name: 'Display name' }), 'Harshal F');
    await user.type(screen.getByRole('textbox', { name: 'Email' }), 'h@freelance.dev');
    await waitFor(() => expect(m.suggestKeyPath).toHaveBeenCalledWith('freelance'));
    await user.click(screen.getByRole('button', { name: 'Save profile' }));
    await waitFor(() => expect(m.saveProfile).toHaveBeenCalled());
    const [p, original] = m.saveProfile.mock.calls[0]!;
    expect(p).toMatchObject({
      id: 'freelance',
      label: 'Freelance',
      email: 'h@freelance.dev',
      color: 'pink',
      ssh_key: 'C:/Users/harshal/.ssh/githop_freelance',
    });
    expect(original).toBeUndefined();
    expect(onSaved).toHaveBeenCalledWith(saved, expect.anything());
    // The key was never generated: the form said so, and the toast says so too.
    expect(
      screen.getByText(
        'No key yet. Click Generate key, or the profile is saved without a working SSH key.',
      ),
    ).toBeInTheDocument();
    expect(
      await screen.findByText(
        'Profile saved: Freelance. Its SSH key doesn’t exist yet: edit the profile and click Generate key.',
      ),
    ).toBeInTheDocument();
  });

  it('save error from backend (validation code) shows issues inline', async () => {
    m.saveProfile.mockRejectedValueOnce({
      code: 'validation',
      message: 'Fix these',
      issues: [
        { field: 'color', severity: 'error', message: 'Pick a colour not used by another profile' },
      ],
    });
    const { user } = renderWith(
      <ProfileForm profiles={profileSet()} original={office} onSaved={vi.fn()} />,
    );
    await user.click(screen.getByRole('button', { name: 'Save profile' }));
    expect(
      await screen.findByText('Pick a colour not used by another profile'),
    ).toBeInTheDocument();
  });

  it('save error (other code) shows the message', async () => {
    m.saveProfile.mockRejectedValueOnce({
      code: 'io',
      message: 'Couldn’t write profiles.toml.',
      issues: [],
    });
    const { user } = renderWith(
      <ProfileForm profiles={profileSet()} original={office} onSaved={vi.fn()} />,
    );
    await user.click(screen.getByRole('button', { name: 'Save profile' }));
    expect(await screen.findByText('Couldn’t write profiles.toml.')).toBeInTheDocument();
  });

  it('generate key: passphrase note (console), never asks passphrase in UI', async () => {
    const { user } = renderWith(<ProfileForm profiles={profileSet()} onSaved={vi.fn()} />);
    expect(
      screen.getByText(/A console window opens so you can type a passphrase/),
    ).toBeInTheDocument();
    expect(document.querySelector('input[type="password"]')).toBeNull();
    await user.type(screen.getByRole('textbox', { name: 'Label' }), 'Work');
    await user.click(screen.getByRole('button', { name: 'Generate key' }));
    await waitFor(() => expect(m.generateKey).toHaveBeenCalledWith('work', false));
    expect(await screen.findByRole('textbox', { name: 'Public key' })).toHaveValue(
      'ssh-ed25519 AAAAC3Nza githop',
    );
    expect(screen.getByRole('button', { name: 'Test connection' })).toBeDisabled();
    expect(screen.queryByText(/No key yet/)).toBeNull();
    expect(screen.getByText('Save the profile first to test the connection.')).toBeInTheDocument();
  });

  it('edit: key panel with copy, GitHub settings, test connection; danger zone', async () => {
    m.readPublicKey.mockResolvedValueOnce('ssh-ed25519 AAAA personal');
    const onDelete = vi.fn();
    const { container, user } = renderWith(
      <ProfileForm
        profiles={profileSet()}
        original={personal}
        onSaved={vi.fn()}
        onDelete={onDelete}
        onCancel={vi.fn()}
      />,
    );
    expect(await screen.findByRole('textbox', { name: 'Public key' })).toHaveValue(
      'ssh-ed25519 AAAA personal',
    );
    await user.click(screen.getByRole('button', { name: 'Copy public key' }));
    expect(m.copyText).toHaveBeenCalledWith('ssh-ed25519 AAAA personal');
    await user.click(screen.getByRole('button', { name: 'Open GitHub SSH settings' }));
    expect(m.openExternal).toHaveBeenCalledWith('ssh-keys');
    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(m.testConnection).toHaveBeenCalledWith('personal');
    expect(await screen.findByText('Connected to GitHub as harshal.')).toBeInTheDocument();
    const danger = screen.getByRole('region', { name: 'Danger zone' });
    await user.click(within(danger).getByRole('button', { name: 'Delete profile' }));
    expect(onDelete).toHaveBeenCalled();
    await expectNoA11yViolations(container);
  });
});

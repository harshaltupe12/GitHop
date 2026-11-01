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

describe('ProfileForm required fields', () => {
  it('marks Label, Display name, Email, SSH key and (with 2+ profiles) Owns', async () => {
    const { container } = renderWith(
      <ProfileForm profiles={profileSet()} onSaved={vi.fn()} onCancel={vi.fn()} />,
    );
    for (const name of ['Label', 'Display name', 'Email'])
      expect(screen.getByRole('textbox', { name })).toHaveAttribute('aria-required', 'true');
    // Not required: Profile ID (follows the label) and the owners input alone.
    expect(screen.getByRole('textbox', { name: 'Profile ID' })).not.toHaveAttribute(
      'aria-required',
    );
    expect(screen.getByRole('textbox', { name: 'GitHub owners' })).not.toHaveAttribute(
      'aria-required',
    );
    // The * is visible (with its legend) but not part of the names.
    expect(screen.getByText('Required')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'SSH key' }).textContent).toBe('SSH key*');
    const owns = screen.getByRole('region', { name: 'Owns' });
    expect(owns).toHaveAccessibleDescription('Add at least one GitHub owner or folder.');
    expect(within(owns).getByRole('heading', { name: 'Owns' }).textContent).toBe('Owns*');
    await expectNoA11yViolations(container);
  });

  it('a single profile: Owns isn’t required', () => {
    renderWith(<ProfileForm profiles={profileSet({ profile: [] })} onSaved={vi.fn()} />);
    expect(screen.queryByText('Add at least one GitHub owner or folder.')).toBeNull();
    expect(screen.getByRole('heading', { name: 'Owns' }).textContent).toBe('Owns');
  });

  it('errors show on blur, and Save focuses the first invalid field', async () => {
    m.validateProfile.mockImplementation(() =>
      Promise.resolve([
        {
          profile_id: '',
          field: 'name',
          severity: 'error',
          message: 'Add the name to use on commits.',
        },
        {
          profile_id: '',
          field: 'label',
          severity: 'error',
          message: 'Add a label for this profile, like Office.',
        },
      ]),
    );
    const { user } = renderWith(<ProfileForm profiles={profileSet()} onSaved={vi.fn()} />);
    const label = screen.getByRole('textbox', { name: 'Label' });
    await user.click(label);
    await waitFor(() => expect(m.validateProfile).toHaveBeenCalled());
    expect(screen.queryByText('Add a label for this profile, like Office.')).toBeNull();
    await user.tab(); // blur
    expect(
      await screen.findByText('Add a label for this profile, like Office.'),
    ).toBeInTheDocument();
    expect(label).toHaveAttribute('aria-invalid', 'true');
    m.saveProfile.mockClear();
    const save = screen.getByRole('button', { name: 'Save profile' });
    expect(save).toBeEnabled();
    await user.click(save);
    // Label comes before Display name on screen.
    await waitFor(() => expect(label).toHaveFocus());
    expect(screen.getByText('Add the name to use on commits.')).toBeInTheDocument();
    expect(m.saveProfile).not.toHaveBeenCalled();
    m.validateProfile.mockReset();
    m.validateProfile.mockResolvedValue([]);
  });

  it('adding a second profile while the first has no owner: fix it in place or Edit it', async () => {
    const officeU = { ...office, owners: [], folders: [] };
    const onEditProfile = vi.fn();
    const { user, container } = renderWith(
      <ProfileForm
        profiles={profileSet({ default: 'office', profile: [officeU] })}
        onSaved={vi.fn()}
        onEditProfile={onEditProfile}
      />,
    );
    expect(screen.getByText('Office needs a GitHub owner or folder too.')).toBeInTheDocument();
    await expectNoA11yViolations(container);
    // Save goes to the other profile's fix first.
    m.saveProfile.mockClear();
    await user.click(screen.getByRole('button', { name: 'Save profile' }));
    await waitFor(() =>
      expect(screen.getByRole('textbox', { name: 'GitHub owner for Office' })).toHaveFocus(),
    );
    expect(m.saveProfile).not.toHaveBeenCalled();
    // Empty owner: says what to enter.
    await user.click(screen.getByRole('button', { name: 'Add to Office' }));
    expect(
      await screen.findByText('Enter a GitHub owner, for example your company’s org.'),
    ).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Edit Office' }));
    expect(onEditProfile).toHaveBeenCalledWith('office');
  });

  it('a save refused because of another profile names it, with Edit', async () => {
    const onEditProfile = vi.fn();
    m.saveProfile.mockRejectedValueOnce({
      code: 'validation',
      message: 'The profiles aren’t valid.',
      issues: [
        {
          profile_id: 'office',
          field: 'email',
          severity: 'error',
          message: 'Profile "Office" has an email git can’t use.',
        },
      ],
    });
    const { user } = renderWith(
      <ProfileForm
        profiles={profileSet()}
        original={personal}
        onSaved={vi.fn()}
        onEditProfile={onEditProfile}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Save profile' }));
    expect(
      await screen.findByText('Another profile needs a fix before this one can be saved:'),
    ).toBeInTheDocument();
    expect(screen.getByText('Profile "Office" has an email git can’t use.')).toBeInTheDocument();
    // Not shown under this profile's own Email field.
    expect(screen.getByRole('textbox', { name: 'Email' })).not.toHaveAttribute('aria-invalid');
    await user.click(screen.getByRole('button', { name: 'Edit Office' }));
    expect(onEditProfile).toHaveBeenCalledWith('office');
  });
});

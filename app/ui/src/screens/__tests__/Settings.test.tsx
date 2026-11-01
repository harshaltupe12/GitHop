import { screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { STATE_KEY } from '../../lib/appState';
import * as ipc from '../../lib/ipc';
import { getQueryData, setQueryData } from '../../lib/query';
import type { AppState } from '../../lib/types';
import { expectNoA11yViolations } from '../../test/a11y';
import { appState, personal, settings } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { SettingsScreen } from '../Settings';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

beforeEach(() => setQueryData(STATE_KEY, appState()));

describe('Settings screen', () => {
  it('normal: every row from Design §8.5', async () => {
    const { container } = renderWith(<SettingsScreen state={appState()} />);
    for (const h of [
      'When identity is wrong',
      'Activity log',
      'Start with Windows',
      'Move to another PC',
      'Updates',
      'Diagnostics',
      'About',
      'Remove everything',
    ])
      expect(screen.getByRole('heading', { name: h })).toBeInTheDocument();
    expect(
      screen.getByText('GitHop never connects to the internet. No telemetry, no accounts.'),
    ).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: 'Block' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('switch', { name: 'Activity log' })).toHaveAttribute(
      'aria-checked',
      'false',
    );
    expect(await screen.findByText('GitHop 0.1.0')).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('enforce + logging + autostart call update_settings', async () => {
    m.updateSettings.mockResolvedValueOnce(settings({ enforce: 'warn' }));
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('radio', { name: 'Warn' }));
    expect(m.updateSettings).toHaveBeenCalledWith({ enforce: 'warn' });
    await waitFor(() => expect(getQueryData<AppState>(STATE_KEY)?.settings.enforce).toBe('warn'));
    m.updateSettings.mockResolvedValueOnce(settings({ logging: true }));
    await user.click(screen.getByRole('switch', { name: 'Activity log' }));
    expect(m.updateSettings).toHaveBeenLastCalledWith({ logging: true });
    m.updateSettings.mockResolvedValueOnce(settings());
    await user.click(screen.getByRole('switch', { name: 'Start with Windows' }));
    await waitFor(() => expect(m.updateSettings).toHaveBeenLastCalledWith({ autostart: false }));
  });

  it('error: update failure shows an error toast', async () => {
    m.updateSettings.mockRejectedValueOnce({
      code: 'locked',
      message: 'Settings are locked.',
      issues: [],
    });
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('switch', { name: 'Activity log' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Settings are locked.');
  });

  it('Check for updates opens the releases page via open_external', async () => {
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('button', { name: 'Check for updates' }));
    expect(m.openExternal).toHaveBeenCalledWith('releases');
  });

  it('doctor dialog shows checks as icon + text', async () => {
    m.doctor.mockResolvedValueOnce({
      checks: [
        { id: 'hooks', status: 'ok', title: 'Hooks installed', detail: '' },
        {
          id: 'ssh',
          status: 'warning',
          title: 'SSH config',
          detail: 'Host * sets IdentityFile',
          fix: 'githop doctor --fix',
        },
      ],
    });
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('button', { name: 'Run doctor' }));
    const dlg = await screen.findByRole('dialog', { name: 'Doctor' });
    expect(await within(dlg).findByText('Hooks installed')).toBeInTheDocument();
    expect(within(dlg).getByText('Warning')).toBeInTheDocument();
    expect(within(dlg).getByText('githop doctor --fix')).toBeInTheDocument();
    await expectNoA11yViolations();
  });

  it('about dialog shows licences from app_info', async () => {
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('button', { name: 'Licences' }));
    const dlg = await screen.findByRole('dialog', { name: 'About GitHop' });
    expect(within(dlg).getByText(/SIL OFL/)).toBeInTheDocument();
  });

  it('wipe: typed WIPE, delete keys unchecked by default', async () => {
    m.wipe.mockResolvedValueOnce({
      steps: [{ step: 'Restored git config', ok: true, detail: '' }],
    });
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('button', { name: 'Wipe GitHop' }));
    const dlg = screen.getByRole('dialog', { name: 'Wipe GitHop?' });
    const keys = within(dlg).getByRole('checkbox', {
      name: 'Also delete SSH keys created by GitHop',
    });
    expect(keys).not.toBeChecked();
    const go = within(dlg).getByRole('button', { name: 'Wipe GitHop' });
    expect(go).toBeDisabled();
    await user.type(within(dlg).getByRole('textbox', { name: 'Type WIPE to confirm' }), 'WIPE');
    await expectNoA11yViolations();
    await user.click(go);
    expect(m.wipe).toHaveBeenCalledWith({
      unpin_repos: true,
      delete_keys: false,
      keep_data: false,
    });
    expect(await screen.findByRole('dialog', { name: 'Wipe finished' })).toHaveTextContent(
      'Restored git config',
    );
  });

  it('export: password ≥ 12 + match + file, strength hint', async () => {
    m.pickSavePath.mockResolvedValueOnce('C:\\Users\\h\\githop.githop');
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('button', { name: 'Export' }));
    const dlg = screen.getByRole('dialog', { name: 'Export profiles' });
    const go = within(dlg).getByRole('button', { name: 'Export' });
    expect(go).toBeDisabled();
    await user.click(within(dlg).getByRole('button', { name: 'Choose…' }));
    await user.type(within(dlg).getByLabelText('Password'), 'short');
    expect(within(dlg).getByText('Use at least 12 characters.')).toBeInTheDocument();
    expect(within(dlg).getByText(/Password strength: Weak/)).toBeInTheDocument();
    await user.clear(within(dlg).getByLabelText('Password'));
    await user.type(within(dlg).getByLabelText('Password'), 'Correct-Horse-42!');
    await user.type(within(dlg).getByLabelText('Confirm password'), 'Correct-Horse-42!');
    expect(within(dlg).getByText(/Password strength: Strong/)).toBeInTheDocument();
    expect(go).toBeEnabled();
    await expectNoA11yViolations();
    await user.click(go);
    expect(m.exportProfiles).toHaveBeenCalledWith(
      'C:/Users/h/githop.githop',
      'Correct-Horse-42!',
      false,
    );
  });

  it('import: preview, conflict resolution, apply', async () => {
    m.pickFile.mockResolvedValueOnce('C:/x.githop');
    m.importPreview.mockResolvedValueOnce({
      profiles: [personal],
      conflicts: [{ profile_id: 'personal', kind: 'email', existing_id: 'personal' }],
      keys: [],
      include_settings: false,
    });
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('button', { name: 'Import' }));
    const dlg = screen.getByRole('dialog', { name: 'Import profiles' });
    await user.click(within(dlg).getByRole('button', { name: 'Choose…' }));
    await user.type(within(dlg).getByLabelText('Password'), 'Correct-Horse-42!');
    await user.click(within(dlg).getByRole('button', { name: 'Continue' }));
    expect(await within(dlg).findByText('personal: same email as Personal')).toBeInTheDocument();
    await user.selectOptions(within(dlg).getByRole('combobox', { name: 'What to do' }), 'rename');
    const newId = within(dlg).getByRole('textbox', { name: 'New profile ID' });
    await user.clear(newId);
    await user.type(newId, 'personal-2');
    await expectNoA11yViolations();
    await user.click(within(dlg).getByRole('button', { name: 'Import' }));
    expect(m.importApply).toHaveBeenCalledWith('C:/x.githop', 'Correct-Horse-42!', [
      { profile_id: 'personal', action: 'rename', new_id: 'personal-2' },
    ]);
  });

  it('strict mode note', () => {
    renderWith(
      <SettingsScreen state={appState({ settings: settings({ enforce: 'block-strict' }) })} />,
    );
    expect(screen.getByText(/Strict mode is on/)).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: 'Block' })).toHaveAttribute('aria-checked', 'true');
  });
});

// ADR 0006: HTTPS → SSH on the Repos screen, the policy in Settings, and the
// key panel's "Remember my passphrase".
import { screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { PassphraseStatus } from '../../components/PassphraseStatus';
import { reposStore, STATE_KEY } from '../../lib/appState';
import * as ipc from '../../lib/ipc';
import { getQueryData, setQueryData } from '../../lib/query';
import type { AppState, RepoRow } from '../../lib/types';
import { expectNoA11yViolations } from '../../test/a11y';
import { appState, httpsRemote, office, profileSet, repo, settings } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { ProfileForm } from '../ProfileForm';
import { repoStatus, ReposScreen } from '../Repos';
import { SettingsScreen } from '../Settings';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

const https = (name: string, owner = 'harshal-personal'): RepoRow =>
  repo(`C:/Personal/${name}`, {}, { https_remotes: [httpsRemote(owner, name)] });
const switchedRow = (r: RepoRow): RepoRow => ({
  ...r,
  https_remotes: [],
  ssh_switched: ['origin'],
  switched: ['origin'],
});

function show(rows: RepoRow[]) {
  reposStore.set((s) => ({
    ...s,
    repos: rows,
    scanned: true,
    autoScanned: true,
    folders: ['C:/Personal'],
  }));
}

beforeEach(() => {
  vi.clearAllMocks();
  setQueryData(STATE_KEY, appState());
});

describe('HTTPS remotes on the Repos screen', () => {
  it('pinned repo over GitHub HTTPS: warning pill (icon + text) and Switch to SSH', async () => {
    const r = https('GitHop');
    expect(repoStatus(r, profileSet())).toMatchObject({
      kind: 'warning',
      text: 'HTTPS — signs in with Windows’ saved account',
      action: 'ssh',
    });
    show([r]);
    m.remoteSsh.mockResolvedValueOnce(switchedRow(r));
    m.remoteUndo.mockResolvedValueOnce({ ...r });
    const { container, user } = renderWith(<ReposScreen profiles={profileSet()} />);
    expect(screen.getByText('HTTPS — signs in with Windows’ saved account')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Why GitHop needs SSH' })).toBeInTheDocument();
    // One HTTPS repo: the row action is enough, no bulk button.
    expect(screen.queryByRole('button', { name: 'Switch all to SSH' })).toBeNull();
    await expectNoA11yViolations(container);

    await user.click(screen.getByRole('button', { name: 'Switch to SSH: GitHop' }));
    expect(m.remoteSsh).toHaveBeenCalledWith('C:/Personal/GitHop');
    expect(
      await screen.findByText('GitHop switched to SSH. Pushes now use Personal’s key.'),
    ).toBeInTheDocument();
    expect(screen.getByText('Matches')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Undo' }));
    expect(m.remoteUndo).toHaveBeenCalledWith('C:/Personal/GitHop');
    expect(await screen.findByText('GitHop uses its HTTPS address again.')).toBeInTheDocument();
  });

  it('Switch all to SSH switches every HTTPS repo with a profile', async () => {
    const a = https('blog');
    const b = https('site');
    const none = repo(
      'C:/Misc/x',
      { profile_id: null, source: null, pinned: null },
      {
        https_remotes: [httpsRemote('someone', 'x')],
      },
    );
    show([a, b, none]);
    m.remoteSshAll.mockResolvedValueOnce([switchedRow(a), switchedRow(b)]);
    const { user } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'Switch all to SSH' }));
    expect(m.remoteSshAll).toHaveBeenCalledWith(['C:/Personal/blog', 'C:/Personal/site']);
    expect(
      await screen.findByText('2 repos switched to SSH. Pushes now use each profile’s key.'),
    ).toBeInTheDocument();
  });

  it('policy ask: Pin offers to switch, with Always do this (sets policy auto)', async () => {
    const unpinned = repo(
      'C:/Office/api',
      { profile_id: 'office', source: 'owner', pinned: null },
      {
        https_remotes: [httpsRemote('office-org', 'api')],
      },
    );
    show([unpinned]);
    const pinned = {
      ...unpinned,
      resolution: { ...unpinned.resolution, source: 'pin' as const, pinned: 'office' },
    };
    m.pin.mockResolvedValueOnce(pinned);
    m.updateSettings.mockResolvedValueOnce(settings({ remote_https_policy: 'auto' }));
    m.remoteSsh.mockResolvedValueOnce(switchedRow(pinned));
    const { user } = renderWith(<ReposScreen profiles={profileSet()} httpsPolicy="ask" />);
    await user.click(screen.getByRole('button', { name: 'Pin api to a profile' }));
    await user.click(screen.getByRole('menuitem', { name: 'Pin to Office' }));
    const dialog = await screen.findByRole('dialog', { name: 'Switch to SSH?' });
    expect(within(dialog).getByText(httpsRemote('office-org', 'api').url)).toBeInTheDocument();
    expect(within(dialog).getByText('git@github.com:office-org/api.git')).toBeInTheDocument();
    expect(
      within(dialog).getByText('Only the remote address changes. You can undo it.'),
    ).toBeInTheDocument();
    await expectNoA11yViolations(dialog);
    await user.click(
      within(dialog).getByRole('checkbox', { name: 'Always do this when I pin a repo' }),
    );
    await user.click(within(dialog).getByRole('button', { name: 'Switch to SSH' }));
    await waitFor(() =>
      expect(m.updateSettings).toHaveBeenCalledWith({ remote_https_policy: 'auto' }),
    );
    expect(m.remoteSsh).toHaveBeenCalledWith('C:/Office/api');
    await waitFor(() =>
      expect(getQueryData<AppState>(STATE_KEY)?.settings.remote_https_policy).toBe('auto'),
    );
  });

  it('policy ask: Not now leaves the remote; policy auto: the pin toast says it switched', async () => {
    const unpinned = repo(
      'C:/Office/api',
      { profile_id: 'office', source: 'owner', pinned: null },
      {
        https_remotes: [httpsRemote('office-org', 'api')],
      },
    );
    show([unpinned]);
    const pinned = {
      ...unpinned,
      resolution: { ...unpinned.resolution, source: 'pin' as const, pinned: 'office' },
    };
    m.pin.mockResolvedValueOnce(pinned);
    const { user, unmount } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'Pin api to a profile' }));
    await user.click(screen.getByRole('menuitem', { name: 'Pin to Office' }));
    const dialog = await screen.findByRole('dialog', { name: 'Switch to SSH?' });
    await user.click(within(dialog).getByRole('button', { name: 'Not now' }));
    expect(m.remoteSsh).not.toHaveBeenCalled();
    unmount();

    show([unpinned]);
    m.pin.mockResolvedValueOnce(switchedRow(pinned));
    const auto = renderWith(<ReposScreen profiles={profileSet()} httpsPolicy="auto" />);
    await auto.user.click(screen.getByRole('button', { name: 'Pin api to a profile' }));
    await auto.user.click(screen.getByRole('menuitem', { name: 'Pin to Office' }));
    expect(
      await screen.findByText(
        `api pinned to ${office.label} and switched to SSH, so pushes use ${office.label}’s key.`,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByRole('dialog', { name: 'Switch to SSH?' })).toBeNull();
  });
});

describe('Settings: GitHub HTTPS remotes', () => {
  it('segmented Switch to SSH / Ask / Leave saves remote_https_policy', async () => {
    m.updateSettings.mockResolvedValueOnce(settings({ remote_https_policy: 'never' }));
    const { container, user } = renderWith(<SettingsScreen state={appState()} />);
    const group = screen.getByRole('radiogroup', { name: 'GitHub HTTPS remotes' });
    expect(within(group).getByRole('radio', { name: 'Ask' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await user.click(within(group).getByRole('radio', { name: 'Leave' }));
    expect(m.updateSettings).toHaveBeenCalledWith({ remote_https_policy: 'never' });
    await waitFor(() =>
      expect(getQueryData<AppState>(STATE_KEY)?.settings.remote_https_policy).toBe('never'),
    );
    await expectNoA11yViolations(container);
  });
});

describe('Remember my passphrase', () => {
  it('agent running, key not added: status + Remember my passphrase', async () => {
    m.passphraseState.mockResolvedValueOnce('asks-each-time');
    m.rememberPassphrase.mockResolvedValueOnce('remembered');
    const { container, user } = renderWith(
      <PassphraseStatus profileId="personal" keyPath="C:/k/githop_personal" />,
    );
    expect(await screen.findByText('Asks for your passphrase on each push')).toBeInTheDocument();
    expect(
      screen.getByText('A console window opens. Type your passphrase there; GitHop never sees it.'),
    ).toBeInTheDocument();
    await expectNoA11yViolations(container);
    await user.click(screen.getByRole('button', { name: 'Remember my passphrase' }));
    expect(m.rememberPassphrase).toHaveBeenCalledWith('personal');
    expect(m.turnOnAgent).not.toHaveBeenCalled();
    expect(await screen.findByText('Passphrase remembered by Windows')).toBeInTheDocument();
    expect(
      screen.getByText('Passphrase remembered. Pushes won’t ask for it again.'),
    ).toBeInTheDocument();
  });

  it('agent off: Turn on and remember (UAC); declined prompt is explained', async () => {
    m.passphraseState.mockResolvedValue('agent-off');
    m.turnOnAgent.mockResolvedValueOnce('cancelled');
    const { user } = renderWith(
      <PassphraseStatus profileId="office" keyPath="C:/k/githop_office" />,
    );
    expect(
      await screen.findByText(
        'Windows’ ssh-agent, which remembers passphrases, is turned off. Turning it on needs one admin prompt.',
      ),
    ).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Turn on and remember' }));
    expect(m.turnOnAgent).toHaveBeenCalledWith('office');
    expect(
      await screen.findByText(
        'Windows didn’t turn on ssh-agent because the admin prompt was declined.',
      ),
    ).toBeInTheDocument();
    m.passphraseState.mockReset();
  });

  it('no passphrase: one muted line, no button; unknown: nothing', async () => {
    m.passphraseState.mockResolvedValueOnce('no-passphrase');
    const { unmount } = renderWith(<PassphraseStatus profileId="a" keyPath="C:/k/a" />);
    expect(
      await screen.findByText('This key has no passphrase, so nothing needs remembering.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button')).toBeNull();
    unmount();
    m.passphraseState.mockResolvedValueOnce('unknown');
    const { container } = renderWith(<PassphraseStatus profileId="a" keyPath="C:/k/a" />);
    await waitFor(() => expect(m.passphraseState).toHaveBeenCalledTimes(2));
    expect(container.textContent).toBe('');
  });

  it('shows in the edit form’s key panel', async () => {
    m.readPublicKey.mockResolvedValue('ssh-ed25519 AAAA office');
    m.passphraseState.mockResolvedValueOnce('remembered');
    renderWith(<ProfileForm profiles={profileSet()} original={office} onSaved={vi.fn()} />);
    expect(await screen.findByText('Passphrase remembered by Windows')).toBeInTheDocument();
    expect(m.passphraseState).toHaveBeenCalledWith('office');
    m.readPublicKey.mockReset();
  });
});

import { screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { reposStore } from '../../lib/appState';
import * as ipc from '../../lib/ipc';
import { KEYS } from '../../lib/storage';
import type { ScanProgress } from '../../lib/types';
import { expectNoA11yViolations } from '../../test/a11y';
import { plan, profileSet, repo } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { repoStatus, ReposScreen } from '../Repos';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

const repos = [
  repo('C:/Personal/blog'),
  repo('C:/Office/api', { profile_id: 'office', source: 'owner', pinned: null }),
  repo('C:/Personal/app', { warnings: [{ kind: 'local-hooks-path', path: '.husky' }] }),
  repo('C:/Misc/notes', { profile_id: null, source: null, pinned: null, remote: null }),
];

describe('repoStatus', () => {
  const ps = profileSet();
  it('maps every Design §8.3 status', () => {
    expect(repoStatus(repos[0]!, 0, ps)).toMatchObject({ kind: 'ok', text: 'Matches' });
    expect(repoStatus(repos[0]!, 2, ps)).toMatchObject({
      kind: 'blocked',
      text: '2 wrong commits',
      action: 'fix',
    });
    expect(repoStatus(repos[2]!, 0, ps)).toMatchObject({
      kind: 'warning',
      text: 'Husky hooks',
      action: 'husky',
    });
    expect(repoStatus(repos[1]!, 0, ps)).toMatchObject({
      kind: 'warning',
      text: 'Unassigned',
      action: 'pin',
    });
    expect(repoStatus(repo('C:/x', { remote: null }), 0, ps)).toMatchObject({ text: 'No remote' });
  });
});

describe('Repos screen', () => {
  it('not scanned: empty state with Scan folder', async () => {
    const { container } = renderWith(<ReposScreen profiles={profileSet()} />);
    expect(
      screen.getByText('Scan a folder to see which identity each repo uses.'),
    ).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Scan folder' })).toHaveLength(2);
    await expectNoA11yViolations(container);
  });

  it('scan: native picker, progress, table with pills, remembers folders', async () => {
    m.pickFolder.mockResolvedValueOnce('C:\\Personal');
    let progress: ((p: ScanProgress) => void) | undefined;
    m.onScanProgress.mockImplementation((cb) => {
      progress = cb;
      return Promise.resolve(() => undefined);
    });
    let finish: (v: typeof repos) => void = () => undefined;
    m.scan.mockImplementationOnce(() => new Promise((r) => (finish = r)));
    m.fixPlan.mockImplementation((root: string) =>
      Promise.resolve(
        plan({ repo: root, offenders: root.endsWith('blog') ? plan().offenders : [] }),
      ),
    );

    const { container, user } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getAllByRole('button', { name: 'Scan folder' })[0]!);
    await waitFor(() => expect(m.scan).toHaveBeenCalledWith('C:/Personal'));
    progress?.({ folder: 'C:/Personal', found: 12, done: 3 });
    expect(await screen.findByText('Scanning C:/Personal… 12 repos found')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
    finish(repos);

    const table = await screen.findByRole('table', { name: 'Repos' });
    expect(within(table).getAllByRole('row')).toHaveLength(5);
    expect(await within(table).findByText('2 wrong commits')).toBeInTheDocument();
    expect(within(table).getByText('Husky hooks')).toBeInTheDocument();
    expect(within(table).getAllByText('Unassigned')).toHaveLength(2);
    expect(screen.getByText('Scanned C:/Personal · 4 repos')).toBeInTheDocument();
    expect(JSON.parse(window.localStorage.getItem(KEYS.scanFolders)!)).toEqual(['C:/Personal']);
    expect(screen.getByRole('button', { name: 'Pin all' })).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('Pin all pins resolvable repos; Fix navigates; How to fix shows the snippet', async () => {
    reposStore.set((s) => ({
      ...s,
      repos,
      scanned: true,
      autoScanned: true,
      folders: ['C:/'],
      offenders: { 'C:/Personal/blog': 2 },
    }));
    m.pinAll.mockResolvedValueOnce([
      repo('C:/Office/api', { profile_id: 'office', source: 'pin', pinned: 'office' }),
    ]);
    const { user, nav } = renderWith(<ReposScreen profiles={profileSet()} />);

    await user.click(screen.getByRole('button', { name: 'Pin all' }));
    expect(m.pinAll).toHaveBeenCalledWith(['C:/Office/api']);
    expect(await screen.findByText('1 repo pinned')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Fix' }));
    expect(nav.go).toHaveBeenCalledWith('fix', { kind: 'fix-repo', repo: 'C:/Personal/blog' });

    await user.click(screen.getByRole('button', { name: 'How to fix' }));
    const dlg = await screen.findByRole('dialog', { name: 'Husky hooks' });
    expect(await within(dlg).findByText('githop check commit || exit $?')).toBeInTheDocument();
    await user.click(within(dlg).getByRole('button', { name: 'Copy line' }));
    expect(m.copyText).toHaveBeenCalledWith('githop check commit || exit $?');
    await expectNoA11yViolations();
  });

  it('Pin on an unassigned repo opens a profile menu', async () => {
    reposStore.set((s) => ({
      ...s,
      repos: [repos[3]!],
      scanned: true,
      autoScanned: true,
      folders: ['C:/Misc'],
    }));
    m.pin.mockResolvedValueOnce(repo('C:/Misc/notes', { profile_id: 'office', source: 'pin' }));
    const { user } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getByRole('button', { name: 'Pin notes to a profile' }));
    await user.click(screen.getByRole('menuitem', { name: 'Pin to Office' }));
    expect(m.pin).toHaveBeenCalledWith('C:/Misc/notes', 'office');
    expect(await screen.findByText('notes pinned to Office')).toBeInTheDocument();
  });

  it('error: scan failure shows a blocked banner with retry', async () => {
    m.pickFolder.mockResolvedValueOnce('C:/Nope');
    m.scan.mockRejectedValueOnce({ code: 'io', message: 'Couldn’t read C:/Nope.', issues: [] });
    const { user, container } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getAllByRole('button', { name: 'Scan folder' })[0]!);
    expect(await screen.findByRole('alert')).toHaveTextContent('Couldn’t read C:/Nope.');
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('remembered folders are rescanned on first visit', async () => {
    window.localStorage.setItem(KEYS.scanFolders, JSON.stringify(['C:/Office']));
    m.scan.mockResolvedValueOnce([repos[1]!]);
    renderWith(<ReposScreen profiles={profileSet()} />);
    await waitFor(() => expect(m.scan).toHaveBeenCalledWith('C:/Office'));
    expect(await screen.findByText('Scanned C:/Office · 1 repo')).toBeInTheDocument();
  });

  it('scanned folder with no repos', async () => {
    reposStore.set((s) => ({ ...s, scanned: true, autoScanned: true, folders: ['C:/Empty'] }));
    renderWith(<ReposScreen profiles={profileSet()} />);
    expect(screen.getByText('No git repos found in this folder.')).toBeInTheDocument();
  });
});

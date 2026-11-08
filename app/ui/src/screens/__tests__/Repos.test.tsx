import { screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { reposStore } from '../../lib/appState';
import * as ipc from '../../lib/ipc';
import { KEYS } from '../../lib/storage';
import type { ScanProgress } from '../../lib/types';
import { expectNoA11yViolations } from '../../test/a11y';
import { office, personal, profileSet, repo } from '../../test/fixtures';
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

// The backend counts unpushed wrong commits per row (RepoRow.unpushed_offenders).
const withWrong = repos.map((r) => (r.root.endsWith('blog') ? { ...r, unpushed_offenders: 2 } : r));

describe('repoStatus', () => {
  const ps = profileSet();
  it('maps every Design §8.3 status', () => {
    expect(repoStatus(repos[0]!, ps)).toMatchObject({ kind: 'ok', text: 'Matches' });
    expect(repoStatus({ ...repos[0]!, unpushed_offenders: 2 }, ps)).toMatchObject({
      kind: 'blocked',
      text: '2 wrong commits',
      action: 'fix',
    });
    expect(repoStatus(repos[2]!, ps)).toMatchObject({
      kind: 'warning',
      text: 'Husky hooks',
      action: 'husky',
    });
    // An owner rule picked it but it isn't pinned yet.
    expect(repoStatus(repos[1]!, ps)).toMatchObject({
      kind: 'warning',
      text: 'Not pinned',
      action: 'pin',
    });
    // No profile at all.
    expect(repoStatus(repos[3]!, ps)).toMatchObject({ kind: 'warning', text: 'Unassigned' });
    // Only the default profile, while other profiles exist.
    const def = repo('C:/x', { source: 'default', pinned: null });
    expect(repoStatus(def, ps)).toMatchObject({
      kind: 'warning',
      text: 'No owner match',
      action: 'pin',
    });
    // The default is the only profile: nothing else could match.
    expect(repoStatus(def, profileSet({ profile: [personal] }))).toMatchObject({
      text: 'Not pinned',
    });
    expect(repoStatus(repo('C:/x', { remote: null }), ps)).toMatchObject({ text: 'No remote' });
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

    const { container, user } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getAllByRole('button', { name: 'Scan folder' })[0]!);
    await waitFor(() => expect(m.scan).toHaveBeenCalledWith('C:/Personal'));
    progress?.({ folder: 'C:/Personal', found: 12, done: 3 });
    expect(await screen.findByText('Scanning C:/Personal… 12 repos found')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
    finish(withWrong);

    const table = await screen.findByRole('table', { name: 'Repos' });
    expect(within(table).getAllByRole('row')).toHaveLength(5);
    expect(await within(table).findByText('2 wrong commits')).toBeInTheDocument();
    expect(within(table).getByText('Husky hooks')).toBeInTheDocument();
    expect(within(table).getAllByText('Unassigned')).toHaveLength(1);
    expect(within(table).getAllByText('Not pinned')).toHaveLength(1);
    expect(screen.getByText('Scanned C:/Personal · 4 repos')).toBeInTheDocument();
    expect(JSON.parse(window.localStorage.getItem(KEYS.scanFolders)!)).toEqual(['C:/Personal']);
    expect(screen.getByRole('button', { name: 'Pin all' })).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('Pin all pins resolvable repos; Fix navigates; How to fix shows the snippet', async () => {
    reposStore.set((s) => ({
      ...s,
      repos: withWrong,
      scanned: true,
      autoScanned: true,
      folders: ['C:/'],
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

  it('Cancel stops the backend scan too (cancel_scan)', async () => {
    m.pickFolder.mockResolvedValueOnce('C:/Big');
    let finish: (v: typeof repos) => void = () => undefined;
    m.scan.mockImplementationOnce(() => new Promise((r) => (finish = r)));
    const { user } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getAllByRole('button', { name: 'Scan folder' })[0]!);
    await user.click(await screen.findByRole('button', { name: 'Cancel' }));
    expect(m.cancelScan).toHaveBeenCalledTimes(1);
    expect(await screen.findByText('Scan cancelled.')).toBeInTheDocument();
    // Late results of the cancelled scan are dropped.
    finish(repos);
    await waitFor(() => expect(screen.queryByRole('table')).toBeNull());
  });

  it('wrong-commit pill comes from unpushed_offenders, no fix_plan per repo', async () => {
    m.pickFolder.mockResolvedValueOnce('C:/Personal');
    m.scan.mockResolvedValueOnce(withWrong);
    m.fixPlan.mockClear();
    const { user } = renderWith(<ReposScreen profiles={profileSet()} />);
    await user.click(screen.getAllByRole('button', { name: 'Scan folder' })[0]!);
    const table = await screen.findByRole('table', { name: 'Repos' });
    expect(within(table).getByText('2 wrong commits')).toBeInTheDocument();
    expect(m.fixPlan).not.toHaveBeenCalled();
    // Table stacks rows at large text sizes instead of scrolling sideways (CSS container query).
    expect(table.parentElement).toHaveClass('gh-table');
    expect(
      within(table)
        .getAllByRole('cell')
        .some((c) => c.getAttribute('data-align') === 'right'),
    ).toBe(true);
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

// The owner's real setup: Office has no owners, Personal is the default, so every
// office repo (owners acme-labs, jane-acme) silently used Personal.
describe('Owners found panel', () => {
  const officeU = { ...office, email: 'jane@acmelabs.com', owners: [], folders: [] };
  const ps = profileSet({ profile: [officeU, personal] });
  const remote = (owner: string, root: string) => ({
    host: 'github.com',
    owner,
    repo: root.split('/').pop() ?? 'r',
  });
  const def = (root: string, owner: string) =>
    repo(root, { source: 'default', pinned: null, remote: remote(owner, root) });
  const byOwner = (root: string, owner: string) =>
    repo(root, {
      profile_id: 'office',
      source: 'owner',
      pinned: null,
      remote: remote(owner, root),
    });
  const rows = [
    def('C:/Work/api', 'acme-labs'),
    def('C:/Work/web', 'acme-labs'),
    def('C:/Work/tool', 'jane-acme'),
    repo('C:/Personal/blog'),
  ];
  const seed = () =>
    reposStore.set((s) => ({
      ...s,
      repos: rows,
      scanned: true,
      autoScanned: true,
      folders: ['C:/'],
    }));
  const panel = () =>
    screen.getByRole('region', { name: 'Choose a profile for these GitHub owners' });

  it('groups default-only repos by owner, suggests Office, rows say why', async () => {
    seed();
    const { container } = renderWith(<ReposScreen profiles={ps} />);
    const p = panel();
    expect(
      within(p).getByText(
        '3 repos matched no owner rule, so they use your default profile (Personal). Add each owner to the profile it belongs to.',
      ),
    ).toBeInTheDocument();
    const items = within(within(p).getByRole('list', { name: 'GitHub owners without a profile' }))
      .getAllByRole('listitem')
      .map((li) => li.textContent);
    expect(items[0]).toContain('acme-labs');
    expect(items[0]).toContain('2 repos');
    expect(items[1]).toContain('jane-acme');
    expect(items[1]).toContain('1 repo');
    // Suggested from Office's email domain (acmelabs.com), never applied on its own.
    const pick = within(p).getByRole('combobox', { name: 'Profile for acme-labs' });
    expect(pick).toHaveValue('office');
    expect(within(pick).getByRole('option', { name: 'Office (suggested)' })).toBeInTheDocument();
    expect(m.saveProfile).not.toHaveBeenCalled();

    const table = screen.getByRole('table', { name: 'Repos' });
    expect(within(table).getAllByText('No owner match')).toHaveLength(3);
    expect(within(table).getAllByText('default (no rule matched)')).toHaveLength(3);
    expect(
      within(table).getByRole('button', { name: 'Why api uses the default profile' }),
    ).toHaveAccessibleDescription(
      'No profile owns acme-labs, and no folder rule matched, so this repo uses your default profile (Personal). Add acme-labs to the profile it belongs to, or pin the repo.',
    );
    // Default-only repos are never bulk-pinned (that would make the guess permanent).
    expect(screen.queryByRole('button', { name: 'Pin all' })).toBeNull();
    await expectNoA11yViolations(container);
  });

  it('Add to profile saves the owner, rescans, then offers Pin all', async () => {
    seed();
    m.saveProfile.mockClear();
    m.listProfiles.mockResolvedValueOnce(ps);
    const saved = profileSet({ profile: [{ ...officeU, owners: ['acme-labs'] }, personal] });
    m.saveProfile.mockResolvedValueOnce(saved);
    m.scan.mockResolvedValueOnce([
      byOwner('C:/Work/api', 'acme-labs'),
      byOwner('C:/Work/web', 'acme-labs'),
      rows[2]!,
      rows[3]!,
    ]);
    m.pinAll.mockResolvedValueOnce([]);
    const { user, container } = renderWith(<ReposScreen profiles={ps} />);
    await user.click(within(panel()).getByRole('button', { name: 'Add to profile: acme-labs' }));
    await waitFor(() =>
      expect(m.saveProfile).toHaveBeenCalledWith({ ...officeU, owners: ['acme-labs'] }, 'office'),
    );
    expect(m.listProfiles).toHaveBeenCalled();
    expect(await screen.findByText('acme-labs added to Office.')).toBeInTheDocument();
    await waitFor(() => expect(m.scan).toHaveBeenCalledWith('C:/'));
    const p = panel();
    expect(
      within(p).getByText('acme-labs now belongs to Office. Its repos use Office from now on.'),
    ).toBeInTheDocument();
    expect(within(p).queryByRole('combobox', { name: 'Profile for acme-labs' })).toBeNull();
    expect(
      within(p).getByRole('combobox', { name: 'Profile for jane-acme' }),
    ).toBeInTheDocument();
    const pinAll = await within(p).findByRole('button', { name: 'Pin all' });
    await expectNoA11yViolations(container);
    await user.click(pinAll);
    expect(m.pinAll).toHaveBeenCalledWith(['C:/Work/api', 'C:/Work/web']);
  });

  it('an owner another profile has: shows the core’s message', async () => {
    seed();
    m.listProfiles.mockResolvedValueOnce(ps);
    m.saveProfile.mockRejectedValueOnce({
      code: 'validation',
      message: 'This profile has problems.',
      issues: [
        {
          profile_id: 'office',
          field: 'owners',
          severity: 'error',
          message: 'Owner jane-acme already belongs to Personal.',
        },
      ],
    });
    const { user } = renderWith(<ReposScreen profiles={ps} />);
    await user.click(
      within(panel()).getByRole('button', { name: 'Add to profile: jane-acme' }),
    );
    expect(await within(panel()).findByRole('alert')).toHaveTextContent(
      'Owner jane-acme already belongs to Personal.',
    );
    expect(
      within(panel()).getByRole('combobox', { name: 'Profile for jane-acme' }),
    ).toHaveAttribute('aria-invalid', 'true');
  });

  it('no suggestion: nothing pre-selected until you choose', async () => {
    reposStore.set((s) => ({
      ...s,
      repos: [def('C:/oss/react', 'facebook')],
      scanned: true,
      autoScanned: true,
      folders: ['C:/oss'],
    }));
    const { user } = renderWith(<ReposScreen profiles={ps} />);
    const add = within(panel()).getByRole('button', { name: 'Add to profile: facebook' });
    expect(add).toBeDisabled();
    await user.selectOptions(
      within(panel()).getByRole('combobox', { name: 'Profile for facebook' }),
      'personal',
    );
    expect(add).toBeEnabled();
  });

  it('hidden when every owner has a profile', () => {
    reposStore.set((s) => ({
      ...s,
      repos: [def('C:/Work/api', 'acme-labs')],
      scanned: true,
      autoScanned: true,
      folders: ['C:/'],
    }));
    renderWith(
      <ReposScreen
        profiles={profileSet({ profile: [{ ...officeU, owners: ['acme-labs'] }, personal] })}
      />,
    );
    expect(
      screen.queryByRole('region', { name: 'Choose a profile for these GitHub owners' }),
    ).toBeNull();
  });
});

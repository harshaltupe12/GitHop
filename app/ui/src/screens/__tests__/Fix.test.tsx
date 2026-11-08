import { screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { reposStore } from '../../lib/appState';
import * as ipc from '../../lib/ipc';
import { expectNoA11yViolations } from '../../test/a11y';
import { offender, plan, profileSet, repo } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { FixScreen } from '../Fix';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

const target = { kind: 'fix-repo' as const, repo: 'C:/Personal/blog' };

describe('Fix screen', () => {
  it('nothing to fix: check icon + copy', async () => {
    const { container } = renderWith(<FixScreen profiles={profileSet()} />);
    expect(
      screen.getByText('Nothing to fix. All commits match their profiles.'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Choose repo folder…' })).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('picker lists scanned repos that have wrong commits', async () => {
    reposStore.set((s) => ({
      ...s,
      repos: [repo('C:/Personal/blog', {}, { unpushed_offenders: 2 }), repo('C:/Personal/ok')],
    }));
    m.fixPlan.mockResolvedValueOnce(plan());
    const { user, container } = renderWith(<FixScreen profiles={profileSet()} />);
    expect(screen.getByRole('heading', { name: 'Choose a repo to fix' })).toBeInTheDocument();
    expect(screen.queryByText('ok')).toBeNull();
    await expectNoA11yViolations(container);
    await user.click(screen.getByRole('button', { name: /blog/ }));
    await waitFor(() => expect(m.fixPlan).toHaveBeenCalledWith('C:/Personal/blog'));
  });

  it('plan: blocked banner, table, reassurance, Fix → success + Undo', async () => {
    m.fixPlan
      .mockResolvedValueOnce(plan())
      .mockResolvedValueOnce(plan({ offenders: [] }))
      .mockResolvedValue(plan());
    const { user, container } = renderWith(<FixScreen profiles={profileSet()} />, {
      intent: target,
    });
    const blocked = await screen.findByRole('alert');
    expect(blocked).toHaveTextContent(
      '2 unpushed commits use the Office email in a Personal repo. Push is blocked.',
    );
    const table = screen.getByRole('table', { name: 'Commits to fix' });
    expect(within(table).getByText('a1b2c3d')).toBeInTheDocument();
    expect(within(table).getAllByText('jane@company.com')[0]).toHaveClass('text-blocked-text');
    expect(
      screen.getByText(
        'Only unpushed commits are changed. A backup is saved first, so you can undo.',
      ),
    ).toBeInTheDocument();
    await expectNoA11yViolations(container);

    await user.click(screen.getByRole('button', { name: 'Fix with Personal identity' }));
    expect(m.fixApply).toHaveBeenCalledWith('C:/Personal/blog');
    expect(await screen.findByText('2 commits fixed. You can push now.')).toBeInTheDocument();
    expect(screen.getByText('Backup saved as refs/githop/backup/1.')).toBeInTheDocument();

    const undo = screen.getAllByRole('button', { name: 'Undo' })[0]!;
    await user.click(undo);
    expect(m.fixUndo).toHaveBeenCalledWith('C:/Personal/blog', 'refs/githop/backup/1');
    expect(
      await screen.findByText('Fix undone. Your commits are back as they were.'),
    ).toBeInTheDocument();
  });

  it('pushed commits: warning, no Fix button', async () => {
    m.fixPlan.mockResolvedValueOnce(
      plan({ offenders: [], pushed_offenders: [offender('ffff000011', 'Old')] }),
    );
    const { container } = renderWith(<FixScreen profiles={profileSet()} />, { intent: target });
    expect(
      await screen.findByText(/1 commit is already pushed, so GitHop can’t fix it automatically/),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Fix with/ })).toBeNull();
    await expectNoA11yViolations(container);
  });

  it('pushed + unpushed: both banners, split tables, Fix changes only unpushed rows', async () => {
    reposStore.set((s) => ({
      ...s,
      repos: [repo('C:/Personal/blog', {}, { unpushed_offenders: 9 })],
    }));
    m.fixPlan.mockResolvedValueOnce(
      plan({ pushed_offenders: [offender('ffff000011', 'Old post')], signed: 1, rewrite_count: 3 }),
    );
    const { user, container } = renderWith(<FixScreen profiles={profileSet()} />, {
      intent: target,
    });
    // Blocked banner for the fixable ones is kept…
    expect(await screen.findByRole('alert')).toHaveTextContent(
      '2 unpushed commits use the Office email in a Personal repo. Push is blocked.',
    );
    // …and the pushed warning (glossary: no "rewrite") with a docs link.
    const warn = screen.getByText(/1 commit is already pushed/);
    expect(warn).toHaveTextContent('Changing a pushed commit changes history');
    expect(warn.textContent).not.toMatch(/rewrite/i);
    await user.click(screen.getByRole('button', { name: 'How to fix by hand' }));
    expect(m.openExternal).toHaveBeenCalledWith('docs');

    const fixable = screen.getByRole('table', { name: 'Commits to fix' });
    const kept = screen.getByRole('table', { name: 'Pushed commits, not changed' });
    expect(within(fixable).getAllByRole('row')).toHaveLength(3);
    expect(within(kept).getAllByRole('row')).toHaveLength(2);
    expect(within(kept).getByText('ffff000')).toBeInTheDocument();
    expect(within(fixable).queryByText('ffff000')).toBeNull();
    expect(
      screen.getByRole('heading', { name: '2 unpushed commits: Fix changes these' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('heading', { name: '1 pushed commit: not changed' }),
    ).toBeInTheDocument();

    // Signed commits and new ids are spelled out before the user clicks Fix.
    expect(
      screen.getByText(
        '1 signed commit loses its signature. Sign again after the fix if you need signed commits.',
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText('3 commits get new ids, including any later unpushed commits.'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Fix with Personal identity' })).toBeInTheDocument();
    // The Repos pill now counts only what Fix can change.
    expect(reposStore.get().repos[0]!.unpushed_offenders).toBe(2);
    await expectNoA11yViolations(container);
  });

  it('diverged after fix: no "pull --rebase" advice', async () => {
    m.fixPlan.mockResolvedValueOnce(plan()).mockResolvedValue(plan({ offenders: [] }));
    m.fixApply.mockResolvedValueOnce({
      backup_ref: 'refs/githop/backup/2',
      rewritten: 2,
      undo_hint: '',
      fast_forward: false,
    });
    const { user } = renderWith(<FixScreen profiles={profileSet()} />, { intent: target });
    await user.click(await screen.findByRole('button', { name: 'Fix with Personal identity' }));
    const done = await screen.findByText(/2 commits fixed. Your branch no longer lines up/);
    expect(done.textContent).not.toMatch(/pull --rebase/);
  });

  it('plan error with git output: friendly text, raw output behind Show details', async () => {
    m.fixPlan.mockRejectedValueOnce({ code: 'git', message: 'fatal: bad object HEAD', issues: [] });
    const { user } = renderWith(<FixScreen profiles={profileSet()} />, { intent: target });
    expect(await screen.findByRole('alert')).toHaveTextContent('Git reported a problem');
    expect(screen.getByText('fatal: bad object HEAD')).not.toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Show details' }));
    expect(screen.getByText('fatal: bad object HEAD')).toBeVisible();
  });

  it('blocked reason (dirty tree): warning, no Fix button', async () => {
    m.fixPlan.mockResolvedValueOnce(plan({ blocked_reason: 'you have uncommitted changes.' }));
    renderWith(<FixScreen profiles={profileSet()} />, { intent: target });
    expect(
      await screen.findByText(/can’t fix this repo right now: you have uncommitted changes/),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Fix with/ })).toBeNull();
  });

  it('error: plan failure shows a blocked banner with retry', async () => {
    m.fixPlan.mockRejectedValueOnce({
      code: 'not_a_repo',
      message: 'This folder isn’t a git repo.',
      issues: [],
    });
    const { user } = renderWith(<FixScreen profiles={profileSet()} />, { intent: target });
    expect(await screen.findByRole('alert')).toHaveTextContent('This folder isn’t a git repo.');
    m.fixPlan.mockResolvedValueOnce(plan({ offenders: [] }));
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(
      await screen.findByText('Nothing to fix. All commits match their profiles.'),
    ).toBeInTheDocument();
  });
});

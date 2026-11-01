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
      repos: [repo('C:/Personal/blog'), repo('C:/Personal/ok')],
      offenders: { 'C:/Personal/blog': 2, 'C:/Personal/ok': 0 },
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
    expect(within(table).getAllByText('harshal@company.com')[0]).toHaveClass('text-blocked-text');
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

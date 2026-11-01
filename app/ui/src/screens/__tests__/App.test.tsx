import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { App } from '../../App';
import * as ipc from '../../lib/ipc';
import { KEYS } from '../../lib/storage';
import { expectNoA11yViolations } from '../../test/a11y';
import { appState, emptyState, hooks, profileSet } from '../../test/fixtures';
import { TrayMenu } from '../../tray/TrayMenu';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

describe('App shell', () => {
  it('loading then main layout with sidebar, default panel and Profiles', async () => {
    const { container } = render(<App />);
    expect(screen.getByRole('status')).toHaveTextContent('Loading…');
    expect(await screen.findByRole('navigation', { name: 'Main' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Profiles' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    expect(screen.getByRole('button', { name: /Default profile: Personal/ })).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 1, name: 'Profiles' })).toBeInTheDocument();
    await expectNoA11yViolations(container);
  });

  it('Ctrl+1..4 switch screens; arrow keys move in the sidebar', async () => {
    render(<App />);
    await screen.findByRole('navigation', { name: 'Main' });
    const user = userEvent.setup();
    await user.keyboard('{Control>}2{/Control}');
    expect(screen.getByRole('heading', { level: 1, name: 'Repos' })).toBeInTheDocument();
    await user.keyboard('{Control>}3{/Control}');
    expect(screen.getByRole('heading', { level: 1, name: 'Fix commits' })).toBeInTheDocument();
    await user.keyboard('{Control>}4{/Control}');
    expect(screen.getByRole('heading', { level: 1, name: 'Settings' })).toBeInTheDocument();
    await user.keyboard('{Control>}n{/Control}');
    expect(screen.getByRole('heading', { level: 1, name: 'Add profile' })).toBeInTheDocument();
    screen.getByRole('button', { name: 'Profiles' }).focus();
    await user.keyboard('{ArrowDown}');
    expect(screen.getByRole('button', { name: 'Repos' })).toHaveFocus();
  });

  it('Ctrl+F opens the folder picker on Repos', async () => {
    render(<App />);
    await screen.findByRole('navigation', { name: 'Main' });
    await userEvent.keyboard('{Control>}f{/Control}');
    expect(screen.getByRole('heading', { level: 1, name: 'Repos' })).toBeInTheDocument();
    await waitFor(() => expect(m.pickFolder).toHaveBeenCalled());
  });

  it('hooks missing: warning banner with Turn on', async () => {
    m.getState.mockResolvedValue(
      appState({ hooks: hooks({ installed: false, points_to_us: false }) }),
    );
    window.localStorage.setItem(KEYS.onboardingDone, 'true');
    render(<App />);
    expect(
      await screen.findByText('Protection is off — git hooks aren’t installed.'),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Turn on' }));
    expect(m.installHooks).toHaveBeenCalled();
    m.getState.mockResolvedValue(appState());
  });

  it('config read error: blocked banner + Restore last backup', async () => {
    m.getState.mockRejectedValueOnce({
      code: 'io',
      message: 'profiles.toml line 3: expected "="',
      issues: [],
    });
    const { container } = render(<App />);
    expect(await screen.findByText('Couldn’t read profiles.toml.')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('profiles.toml line 3');
    await expectNoA11yViolations(container);
    await userEvent.click(screen.getByRole('button', { name: 'Restore last backup' }));
    expect(m.restoreLastBackup).toHaveBeenCalled();
  });

  it('no profiles: onboarding is shown', async () => {
    m.getState.mockResolvedValueOnce(emptyState());
    render(<App />);
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'One PC. Every git identity. No mix-ups.',
      }),
    ).toBeInTheDocument();
  });

  it('refetches on profiles://changed', async () => {
    let fire: () => void = () => undefined;
    m.onProfilesChanged.mockImplementation((cb) => {
      fire = cb;
      return Promise.resolve(() => undefined);
    });
    render(<App />);
    await screen.findByRole('navigation', { name: 'Main' });
    const n = m.getState.mock.calls.length;
    fire();
    await waitFor(() => expect(m.getState.mock.calls.length).toBeGreaterThan(n));
  });
});

describe('Tray menu', () => {
  it('radio list of profiles; click sets default and hides', async () => {
    const { container } = render(<TrayMenu />);
    const current = await screen.findByRole('menuitemradio', { name: /Personal/ });
    expect(current).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('menuitemradio', { name: /Office/ })).toHaveAttribute(
      'aria-checked',
      'false',
    );
    expect(screen.getByText('company.com')).toBeInTheDocument();
    await expectNoA11yViolations(container);
    await userEvent.click(screen.getByRole('menuitemradio', { name: /Office/ }));
    expect(m.setDefault).toHaveBeenCalledWith('office');
    await waitFor(() => expect(m.hideCurrentWindow).toHaveBeenCalled());
  });

  it('Open GitHop and Quit; Esc hides', async () => {
    render(<TrayMenu />);
    await screen.findByRole('menuitemradio', { name: /Personal/ });
    await userEvent.click(screen.getByRole('menuitem', { name: 'Open GitHop' }));
    expect(m.showMainWindow).toHaveBeenCalled();
    await userEvent.click(screen.getByRole('menuitem', { name: 'Quit' }));
    expect(m.quitApp).toHaveBeenCalled();
    await userEvent.keyboard('{Escape}');
    expect(m.hideCurrentWindow).toHaveBeenCalled();
  });

  it('empty and error states', async () => {
    m.listProfiles.mockResolvedValueOnce(profileSet({ default: null, profile: [] }));
    const { unmount } = render(<TrayMenu />);
    expect(await screen.findByText('No profiles yet')).toBeInTheDocument();
    unmount();
  });
});

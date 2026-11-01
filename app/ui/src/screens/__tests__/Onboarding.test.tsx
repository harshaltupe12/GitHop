import { screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { STATE_KEY } from '../../lib/appState';
import * as ipc from '../../lib/ipc';
import { setQueryData } from '../../lib/query';
import { KEYS } from '../../lib/storage';
import { expectNoA11yViolations } from '../../test/a11y';
import { appState, emptyState, office, profileSet } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { Onboarding } from '../Onboarding';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());
const m = vi.mocked(ipc);

beforeEach(() => setQueryData(STATE_KEY, emptyState()));

describe('Onboarding', () => {
  it('walks welcome → safety → first profile with the detected identity', async () => {
    const { user, container } = renderWith(<Onboarding state={emptyState()} onFinish={vi.fn()} />);
    expect(
      screen.getByRole('heading', { level: 1, name: 'One PC. Every git identity. No mix-ups.' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('list', { name: 'Step 1 of 6' })).toBeInTheDocument();
    expect(screen.getByText(/GitHop never connects to the internet/)).toBeInTheDocument();
    await expectNoA11yViolations(container);

    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { level: 1, name: 'Safety first' })).toBeInTheDocument();
    expect(screen.getByText('harshal@company.com')).toBeInTheDocument();
    expect(JSON.parse(window.localStorage.getItem(KEYS.onboardingStep)!)).toBe(1);

    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(
      screen.getByRole('heading', { level: 1, name: 'Your first profile' }),
    ).toBeInTheDocument();
    expect(
      screen.getByText('Looks like your Office identity. Check the details below.'),
    ).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Label' })).toHaveValue('Office');
    expect(screen.getByRole('textbox', { name: 'Email' })).toHaveValue('harshal@company.com');
    await expectNoA11yViolations(container);

    m.saveProfile.mockResolvedValueOnce(profileSet({ default: null, profile: [office] }));
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await waitFor(() => expect(m.saveProfile).toHaveBeenCalled());
    await waitFor(() => expect(m.setDefault).toHaveBeenCalledWith('office'));
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Add another' }),
    ).toBeInTheDocument();
  });

  it('resumes at a stored step; Back works', async () => {
    window.localStorage.setItem(KEYS.onboardingStep, '1');
    const { user } = renderWith(<Onboarding state={emptyState()} onFinish={vi.fn()} />);
    expect(screen.getByRole('heading', { level: 1, name: 'Safety first' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(
      screen.getByRole('heading', { level: 1, name: 'One PC. Every git identity. No mix-ups.' }),
    ).toBeInTheDocument();
  });

  it('protection step: notes existing global hooks, installs, then done', async () => {
    const st = appState({
      onboarding: {
        has_profiles: true,
        hooks_installed: false,
        global_identity: null,
        step_hint: '',
        backup_dir: '',
      },
    });
    st.hooks = {
      ...st.hooks,
      installed: false,
      points_to_us: false,
      global_hooks_path: 'C:/hooks',
    };
    const onFinish = vi.fn();
    const { user, container } = renderWith(<Onboarding state={st} onFinish={onFinish} />);
    expect(
      screen.getByRole('heading', { level: 1, name: 'Turn on protection' }),
    ).toBeInTheDocument();
    expect(
      screen.getByText('You already have global hooks in C:/hooks. We’ll keep running them.'),
    ).toBeInTheDocument();
    await expectNoA11yViolations(container);
    await user.click(screen.getByRole('button', { name: 'Turn on protection' }));
    expect(m.installHooks).toHaveBeenCalled();
    expect(
      await screen.findByRole('heading', { level: 1, name: 'GitHop is in your tray.' }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Open GitHop' }));
    expect(onFinish).toHaveBeenCalled();
    expect(window.localStorage.getItem(KEYS.onboardingDone)).toBe('true');
  });

  it('error: install failure shows a blocked banner', async () => {
    m.installHooks.mockRejectedValueOnce({
      code: 'io',
      message: 'Couldn’t write the hooks folder.',
      issues: [],
    });
    const st = appState({
      onboarding: {
        has_profiles: true,
        hooks_installed: false,
        global_identity: null,
        step_hint: '',
        backup_dir: '',
      },
    });
    const { user } = renderWith(<Onboarding state={st} onFinish={vi.fn()} />);
    await user.click(screen.getByRole('button', { name: 'Turn on protection' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Couldn’t write the hooks folder.');
  });

  it('add another: Skip moves on', async () => {
    window.localStorage.setItem(KEYS.onboardingStep, '3');
    const st = appState({
      onboarding: {
        has_profiles: true,
        hooks_installed: false,
        global_identity: null,
        step_hint: '',
        backup_dir: '',
      },
    });
    const { user } = renderWith(<Onboarding state={st} onFinish={vi.fn()} />);
    expect(screen.getByRole('heading', { level: 1, name: 'Add another' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Add another profile' }));
    expect(screen.getByRole('textbox', { name: 'Label' })).toHaveValue('Personal');
    // Focus moves into the new form (its heading), not left on <body>.
    expect(screen.getByRole('heading', { level: 2, name: 'Personal' })).toHaveFocus();
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    // …and back to the step heading when the form closes.
    expect(screen.getByRole('heading', { level: 1, name: 'Add another' })).toHaveFocus();
    await user.click(screen.getByRole('button', { name: 'Skip' }));
    expect(
      screen.getByRole('heading', { level: 1, name: 'Turn on protection' }),
    ).toBeInTheDocument();
  });

  it('safety step shows the real backup folder from the backend (GITHOP_HOME aware)', async () => {
    window.localStorage.setItem(KEYS.onboardingStep, '1');
    const st = emptyState();
    st.onboarding.backup_dir = 'D:/portable/githop/backups';
    renderWith(<Onboarding state={st} onFinish={vi.fn()} />);
    expect(screen.getByText('D:/portable/githop/backups')).toBeInTheDocument();
    expect(screen.queryByText(/%APPDATA%/)).toBeNull();
  });
});

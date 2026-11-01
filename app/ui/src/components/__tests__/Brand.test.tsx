import { render, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../../App';
import { STATE_KEY } from '../../lib/appState';
import { setQueryData } from '../../lib/query';
import { Onboarding } from '../../screens/Onboarding';
import { SettingsScreen } from '../../screens/Settings';
import { expectNoA11yViolations } from '../../test/a11y';
import { appState, emptyState } from '../../test/fixtures';
import { renderWith } from '../../test/render';
import { TrayMenu } from '../../tray/TrayMenu';
import { BrandLockup, BrandMark, BrandTile } from '../Brand';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());

const imgs = (root: HTMLElement) => Array.from(root.querySelectorAll('img'));
const src = (el: Element | null | undefined) => el?.getAttribute('src') ?? '';

describe('Brand components', () => {
  it('tile: favicon geometry at ≤32 px, full icon above; decorative by default', async () => {
    const { container } = render(
      <>
        <BrandTile size={16} />
        <BrandTile size={32} />
        <BrandTile size={96} />
      </>,
    );
    const [s16, s32, s96] = imgs(container);
    expect(src(s16)).toMatch(/githop-favicon\.svg/);
    expect(src(s32)).toMatch(/githop-favicon\.svg/);
    expect(src(s96)).toMatch(/githop-icon-light\.svg/);
    for (const i of [s16, s32, s96]) {
      expect(i).toHaveAttribute('alt', '');
      expect(i).toHaveAttribute('aria-hidden', 'true');
    }
    // Intrinsic box reserved (no layout shift) and sized in rem like icons.
    expect(s96).toHaveAttribute('width', '96');
    expect(s96).toHaveAttribute('height', '96');
    expect(s96).toHaveStyle({ width: `${96 / 14}rem` });
    expect(screen.queryByRole('img')).toBeNull();
    await expectNoA11yViolations(container);
  });

  it('tile can name the app', () => {
    render(<BrandTile size={48} decorative={false} />);
    expect(screen.getByRole('img', { name: 'GitHop' })).not.toHaveAttribute('aria-hidden');
  });

  it('lockup: alt "GitHop", whole-pixel width, small geometry ≤32, reverse in dark contrast themes', async () => {
    const { container } = render(
      <>
        <BrandLockup height={32} />
        <BrandLockup height={48} />
      </>,
    );
    const [small, large] = screen.getAllByRole('img', { name: 'GitHop' });
    expect(src(small)).toMatch(/githop-lockup-horizontal-small\.svg/);
    expect(small).toHaveAttribute('width', '98');
    expect(src(large)).toMatch(/githop-lockup-horizontal\.svg/);
    expect(large).toHaveAttribute('width', '147');
    const sources = Array.from(container.querySelectorAll('picture > source'));
    expect(sources).toHaveLength(2);
    for (const s of sources) {
      expect(s).toHaveAttribute(
        'media',
        '(forced-colors: active) and (prefers-color-scheme: dark)',
      );
      expect(s.getAttribute('srcset')).toMatch(/reverse/);
    }
    await expectNoA11yViolations(container);
  });

  it('mark: colour / mono variants, tile fallback at ≤32 px', () => {
    const { container } = render(
      <>
        <BrandMark size={40} />
        <BrandMark size={40} tone="mono" />
        <BrandMark size={24} tone="mono" />
      </>,
    );
    const [color, mono, tiny] = imgs(container);
    expect(src(color)).toMatch(/githop-mark-color\.svg/);
    expect(src(mono)).toMatch(/githop-mark-mono-ink\.svg/);
    expect(src(tiny)).toMatch(/githop-favicon\.svg/);
    expect(color).toHaveAttribute('alt', '');
  });
});

describe('Brand placements', () => {
  beforeEach(() => setQueryData(STATE_KEY, appState()));

  it('sidebar header: lockup named "GitHop", outside the nav', async () => {
    const { container } = render(<App />);
    const nav = await screen.findByRole('navigation', { name: 'Main' });
    const logo = screen.getByRole('img', { name: 'GitHop' });
    expect(src(logo)).toMatch(/lockup-horizontal-small/);
    expect(container.querySelector('aside')).toContainElement(logo);
    expect(nav).not.toContainElement(logo);
    await expectNoA11yViolations(container);
  });

  it('onboarding welcome: 96 px decorative tile, only on the first step', async () => {
    setQueryData(STATE_KEY, emptyState());
    const { container, user } = renderWith(<Onboarding state={emptyState()} onFinish={vi.fn()} />);
    const [tile] = imgs(container);
    expect(src(tile)).toMatch(/githop-icon-light\.svg/);
    expect(tile).toHaveAttribute('width', '96');
    expect(tile).toHaveAttribute('alt', '');
    expect(tile).toHaveAttribute('aria-hidden', 'true');
    await expectNoA11yViolations(container);
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(imgs(container)).toHaveLength(0);
  });

  it('about dialog: decorative lockup next to the version', async () => {
    const { user } = renderWith(<SettingsScreen state={appState()} />);
    await user.click(screen.getByRole('button', { name: 'Licences' }));
    const dlg = await screen.findByRole('dialog', { name: 'About GitHop' });
    const [lockup] = imgs(dlg);
    expect(src(lockup)).toMatch(/githop-lockup-horizontal\.svg/);
    expect(lockup).toHaveAttribute('height', '48');
    expect(lockup).toHaveAttribute('alt', '');
    expect(within(dlg).queryByRole('img')).toBeNull();
    expect(await within(dlg).findByText('Version 0.1.0')).toBeInTheDocument();
    await expectNoA11yViolations(dlg);
  });

  it('tray menu header: 16 px favicon tile, hidden from the menu', async () => {
    const { container } = render(<TrayMenu />);
    await screen.findByRole('menuitemradio', { name: /Personal/ });
    const [tile] = imgs(container);
    expect(src(tile)).toMatch(/githop-favicon\.svg/);
    expect(tile).toHaveAttribute('width', '16');
    expect(tile.closest('[aria-hidden="true"]')).not.toBeNull();
    await expectNoA11yViolations(container);
  });
});

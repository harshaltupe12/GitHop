import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { expectNoA11yViolations } from '../../test/a11y';
import { Avatar } from '../Avatar';
import { Banner } from '../Banner';
import { Button } from '../Button';
import { Chip } from '../Chip';
import { Icons } from '../icons';
import { Pill } from '../Pill';
import { Tooltip } from '../Tooltip';

describe('Button', () => {
  it.each(['primary', 'secondary', 'danger', 'ghost'] as const)(
    '%s renders and clicks',
    async (v) => {
      const onClick = vi.fn();
      const { container } = render(
        <Button variant={v} icon={Icons.add} onClick={onClick}>
          Add profile
        </Button>,
      );
      const b = screen.getByRole('button', { name: 'Add profile' });
      await userEvent.click(b);
      expect(onClick).toHaveBeenCalledOnce();
      expect(b.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
      await expectNoA11yViolations(container);
    },
  );

  it('disabled state does not fire', async () => {
    const onClick = vi.fn();
    render(
      <Button disabled onClick={onClick}>
        Save
      </Button>,
    );
    const b = screen.getByRole('button', { name: 'Save' });
    expect(b).toBeDisabled();
    expect(b.className).toContain('disabled:text-ink-disabled');
    await userEvent.click(b);
    expect(onClick).not.toHaveBeenCalled();
  });

  it('loading keeps the label, shows a spinner and disables', () => {
    render(
      <Button variant="primary" icon={Icons.add} loading>
        Saving…
      </Button>,
    );
    const b = screen.getByRole('button', { name: 'Saving…' });
    expect(b).toBeDisabled();
    expect(b).toHaveAttribute('aria-busy', 'true');
    expect(b.querySelector('.spin')).not.toBeNull();
    expect(b.querySelector('svg')).toBeNull();
  });

  it('icon-only button has an accessible name and a tooltip', async () => {
    vi.useFakeTimers();
    const { container } = render(
      <Button variant="ghost" icon={Icons.copy} label="Copy public key" />,
    );
    const b = screen.getByRole('button', { name: 'Copy public key' });
    act(() => b.focus());
    act(() => {
      vi.advanceTimersByTime(600);
    });
    expect(screen.getByRole('tooltip')).toHaveTextContent('Copy public key');
    expect(screen.getByRole('tooltip')).toBeVisible();
    vi.useRealTimers();
    await expectNoA11yViolations(container);
  });

  it('compact size for tables', () => {
    render(<Button compact>Pin</Button>);
    expect(screen.getByRole('button', { name: 'Pin' }).className).toContain('min-h-7');
  });
});

describe('Tooltip', () => {
  it('appears after a 500 ms delay and hides on Esc and blur', () => {
    vi.useFakeTimers();
    render(
      <Tooltip content="Used by Office" describe>
        <button type="button">Blue</button>
      </Tooltip>,
    );
    const b = screen.getByRole('button', { name: 'Blue' });
    act(() => b.focus());
    act(() => {
      vi.advanceTimersByTime(300);
    });
    expect(screen.getByRole('tooltip', { hidden: true })).not.toBeVisible();
    act(() => {
      vi.advanceTimersByTime(300);
    });
    expect(screen.getByRole('tooltip')).toBeVisible();
    expect(b).toHaveAttribute('aria-describedby', screen.getByRole('tooltip').id);
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });
    expect(screen.getByRole('tooltip', { hidden: true })).not.toBeVisible();
    vi.useRealTimers();
  });
});

describe('Avatar', () => {
  it('names the profile and colour, shows the initial', async () => {
    const { container } = render(<Avatar label="Personal" color="violet" />);
    const a = screen.getByRole('img', { name: 'Personal, violet' });
    expect(a).toHaveTextContent('P');
    expect(a.className).toContain('bg-profile-violet');
    await expectNoA11yViolations(container);
  });

  it.each([22, 30, 40] as const)('size %i', (size) => {
    render(<Avatar label="Office" color="blue" size={size} />);
    expect(screen.getByRole('img', { name: 'Office, blue' })).toBeInTheDocument();
  });

  it('decorative avatar is hidden from assistive tech', () => {
    const { container } = render(<Avatar label="Office" color="blue" decorative />);
    expect(screen.queryByRole('img')).toBeNull();
    expect(container.firstChild).toHaveAttribute('aria-hidden', 'true');
  });
});

describe('Pill', () => {
  it.each([
    ['ok', 'Matches', 'bg-aqua-tint'],
    ['blocked', '2 wrong commits', 'bg-blocked-tint'],
    ['warning', 'Husky hooks', 'bg-warning-tint'],
    ['neutral', 'Pinned', 'bg-bg'],
  ] as const)('%s pill = icon + text', async (kind, text, cls) => {
    const { container } = render(<Pill kind={kind}>{text}</Pill>);
    const pill = screen.getByText(text);
    expect(pill.className).toContain(cls);
    expect(pill.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
    await expectNoA11yViolations(container);
  });
});

describe('Chip', () => {
  it('mono chip with leading icon', () => {
    render(<Chip icon={Icons.key}>githop_personal</Chip>);
    const c = screen.getByText('githop_personal').parentElement!;
    expect(c.className).toContain('font-mono');
    expect(c.querySelector('svg')).not.toBeNull();
  });

  it('removable chip', async () => {
    const onRemove = vi.fn();
    const { container } = render(<Chip onRemove={onRemove}>office-org</Chip>);
    await userEvent.click(screen.getByRole('button', { name: 'Remove office-org' }));
    expect(onRemove).toHaveBeenCalled();
    await expectNoA11yViolations(container);
  });

  it('plain text chip', () => {
    render(<Chip mono={false}>Folder</Chip>);
    expect(screen.getByText('Folder').parentElement!.className).toContain('text-small');
  });
});

describe('Banner', () => {
  it.each([
    ['success', 'status', 'bg-aqua-tint'],
    ['blocked', 'alert', 'bg-blocked-tint'],
    ['warning', 'status', 'bg-warning-tint'],
    ['info', 'status', 'bg-bg'],
    ['privacy', 'status', 'bg-aqua-tint'],
  ] as const)('%s banner', async (kind, role, cls) => {
    const { container } = render(<Banner kind={kind}>Message</Banner>);
    const el = screen.getByRole(role);
    expect(el.className).toContain(cls);
    expect(el.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
    await expectNoA11yViolations(container);
  });

  it('optional action link', async () => {
    const onClick = vi.fn();
    render(
      <Banner kind="warning" action={{ label: 'Turn on', onClick }}>
        Protection is off — git hooks aren’t installed.
      </Banner>,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Turn on' }));
    expect(onClick).toHaveBeenCalled();
  });

  it('static banner has no live role', () => {
    render(
      <Banner kind="info" live={false}>
        Quiet
      </Banner>,
    );
    expect(screen.queryByRole('status')).toBeNull();
  });
});

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useEffect } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from '../../App';
import { Dialog } from '../../components/Dialog';
import { Pill } from '../../components/Pill';
import { Segmented } from '../../components/Segmented';
import { Table } from '../../components/Table';
import { ToastProvider, useToast } from '../../components/Toast';
import { Tooltip } from '../../components/Tooltip';
import { motionEnabled } from '../motion';

vi.mock('../ipc', async () => (await import('../../test/ipcMock')).createIpcMock());

/** jsdom has no matchMedia (= the instant path). Tests opt into motion with this. */
function stubMotion(reduce: boolean) {
  window.matchMedia = vi.fn((q: string) => ({
    matches: reduce && q.includes('prefers-reduced-motion: reduce'),
    media: q,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

/**
 * jsdom does no layout. Give `.gh-nav` buttons and segmented radios a fake
 * vertical / horizontal position by index so the sliding indicator can measure.
 */
function stubLayout() {
  const index = (el: HTMLElement, sel: string) =>
    Array.from(el.ownerDocument.querySelectorAll(sel)).indexOf(el);
  const props: Record<string, (el: HTMLElement) => number> = {
    offsetWidth: (el) => (el.matches('.gh-nav') ? 168 : el.matches('.gh-seg > button') ? 60 : 0),
    offsetHeight: (el) => (el.matches('.gh-nav') ? 36 : el.matches('.gh-seg > button') ? 28 : 0),
    offsetTop: (el) => (el.matches('.gh-nav') ? index(el, '.gh-nav') * 38 : 2),
    offsetLeft: (el) =>
      el.matches('.gh-seg > button') ? 2 + index(el, '.gh-seg > button') * 60 : 0,
  };
  const restore: Array<() => void> = [];
  for (const [name, get] of Object.entries(props)) {
    const orig = Object.getOwnPropertyDescriptor(HTMLElement.prototype, name);
    Object.defineProperty(HTMLElement.prototype, name, {
      configurable: true,
      get(this: HTMLElement) {
        return get(this);
      },
    });
    restore.push(() => orig && Object.defineProperty(HTMLElement.prototype, name, orig));
  }
  return () => restore.forEach((r) => r());
}

afterEach(() => {
  delete (window as { matchMedia?: unknown }).matchMedia;
  vi.useRealTimers();
});

describe('motionEnabled', () => {
  it('is off without matchMedia and under reduced motion, on otherwise', () => {
    expect(motionEnabled()).toBe(false);
    stubMotion(true);
    expect(motionEnabled()).toBe(false);
    stubMotion(false);
    expect(motionEnabled()).toBe(true);
  });
});

describe('Dialog exit', () => {
  const ui = (open: boolean) => (
    <Dialog open={open} title="Wipe GitHop?" onClose={() => undefined}>
      Body text
    </Dialog>
  );

  it('stays mounted, marked closed, until its exit animation ends', () => {
    stubMotion(false);
    const { rerender } = render(ui(true));
    const dialog = screen.getByRole('dialog');
    const scrim = dialog.parentElement as HTMLElement;
    expect(scrim).toHaveAttribute('data-state', 'open');
    // The enter animation ending must not unmount it.
    fireEvent.animationEnd(scrim);
    expect(screen.getByRole('dialog')).toBeInTheDocument();

    rerender(ui(false));
    expect(scrim).toHaveAttribute('data-state', 'closed');
    expect(scrim).toHaveAttribute('aria-hidden', 'true');
    expect(scrim).toHaveTextContent('Body text');
    // A bubbling animationend from inside (e.g. a spinner) is ignored.
    fireEvent.animationEnd(dialog);
    expect(scrim).toBeInTheDocument();
    fireEvent.animationEnd(scrim);
    expect(scrim).not.toBeInTheDocument();
  });

  it('keeps showing its last content while closing', () => {
    stubMotion(false);
    const { rerender } = render(
      <Dialog open title="Husky hooks" onClose={() => undefined}>
        snippet A
      </Dialog>,
    );
    rerender(
      <Dialog open={false} title="" onClose={() => undefined}>
        {null}
      </Dialog>,
    );
    expect(screen.getByText('snippet A')).toBeInTheDocument();
    expect(screen.getByText('Husky hooks')).toBeInTheDocument();
  });

  it('unmounts after a safety timeout if animationend never comes', () => {
    stubMotion(false);
    vi.useFakeTimers();
    const { rerender } = render(ui(true));
    rerender(ui(false));
    expect(screen.queryByText('Body text')).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(screen.queryByText('Body text')).not.toBeInTheDocument();
  });

  it('reduced motion: unmounts at once', () => {
    stubMotion(true);
    const { rerender } = render(ui(true));
    rerender(ui(false));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('reopening while closing cancels the exit', () => {
    stubMotion(false);
    const { rerender } = render(ui(true));
    rerender(ui(false));
    rerender(ui(true));
    const scrim = screen.getByRole('dialog').parentElement as HTMLElement;
    expect(scrim).toHaveAttribute('data-state', 'open');
    expect(scrim).not.toHaveAttribute('aria-hidden');
  });
});

describe('Toast exit', () => {
  function Fire() {
    const toast = useToast();
    useEffect(() => toast({ message: 'Default profile: Personal' }), [toast]);
    return null;
  }

  it('dismiss plays the exit, then removes the toast', async () => {
    stubMotion(false);
    render(
      <ToastProvider>
        <Fire />
      </ToastProvider>,
    );
    const toast = await screen.findByText('Default profile: Personal');
    const shell = toast.closest('.gh-toast') as HTMLElement;
    expect(shell).toHaveAttribute('data-state', 'open');
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(shell).toHaveAttribute('data-state', 'closed');
    fireEvent.animationEnd(shell);
    expect(shell).not.toBeInTheDocument();
  });

  it('same enter/exit inside an open dialog’s toast slot', async () => {
    stubMotion(false);
    render(
      <ToastProvider>
        <Dialog open title="Export" onClose={() => undefined}>
          body
        </Dialog>
        <Fire />
      </ToastProvider>,
    );
    const toast = await screen.findByText('Default profile: Personal');
    const shell = toast.closest('.gh-toast') as HTMLElement;
    expect(shell.closest('[data-toast-slot]')).not.toBeNull();
    expect(shell).toHaveAttribute('data-state', 'open');
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(shell).toHaveAttribute('data-state', 'closed');
    fireEvent.animationEnd(shell);
    expect(screen.queryByText('Default profile: Personal')).not.toBeInTheDocument();
  });

  it('without motion: removed at once', async () => {
    render(
      <ToastProvider>
        <Fire />
      </ToastProvider>,
    );
    await screen.findByText('Default profile: Personal');
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByText('Default profile: Personal')).not.toBeInTheDocument();
  });
});

describe('Tooltip skip-delay group', () => {
  it('first waits 500 ms and animates; a neighbour right after opens instantly', () => {
    vi.useFakeTimers();
    render(
      <>
        <Tooltip content="Copy">
          <button type="button">a</button>
        </Tooltip>
        <Tooltip content="Edit">
          <button type="button">b</button>
        </Tooltip>
      </>,
    );
    const [a, b] = screen.getAllByRole('button');
    fireEvent.mouseEnter(a!.parentElement as HTMLElement);
    expect(screen.queryByRole('tooltip')).toBeNull();
    act(() => {
      vi.advanceTimersByTime(500);
    });
    const first = screen.getByRole('tooltip');
    expect(first).toHaveTextContent('Copy');
    expect(first).not.toHaveAttribute('data-instant');
    fireEvent.mouseLeave(a!.parentElement as HTMLElement);
    fireEvent.mouseEnter(b!.parentElement as HTMLElement);
    const second = screen.getByRole('tooltip');
    expect(second).toHaveTextContent('Edit');
    expect(second).toHaveAttribute('data-instant');
    // After a pause, the delay comes back.
    fireEvent.mouseLeave(b!.parentElement as HTMLElement);
    act(() => {
      vi.advanceTimersByTime(400);
    });
    fireEvent.mouseEnter(a!.parentElement as HTMLElement);
    expect(screen.queryByRole('tooltip')).toBeNull();
  });
});

describe('Arrivals and changes', () => {
  const cols = [{ key: 'n', header: 'Name', render: (r: string) => r }];

  it('table rows: none animate on mount; only rows added later fade in', () => {
    const { rerender } = render(
      <Table label="Repos" columns={cols} rows={['a', 'b']} rowKey={(r) => r} />,
    );
    const rowsOf = () => screen.getAllByRole('row').slice(1);
    expect(rowsOf().some((r) => r.classList.contains('appear'))).toBe(false);
    rerender(<Table label="Repos" columns={cols} rows={['a', 'b', 'c']} rowKey={(r) => r} />);
    expect(rowsOf().map((r) => r.classList.contains('appear'))).toEqual([false, false, true]);
  });

  it('table rows: a large batch arriving at once does not animate', () => {
    const many = Array.from({ length: 40 }, (_, i) => `r${i}`);
    const { rerender } = render(
      <Table label="Repos" columns={cols} rows={['a']} rowKey={(r) => r} />,
    );
    rerender(<Table label="Repos" columns={cols} rows={['a', ...many]} rowKey={(r) => r} />);
    expect(document.querySelectorAll('tr.appear')).toHaveLength(0);
  });

  it('pill cross-fades only when its status changes', () => {
    const { rerender, container } = render(<Pill kind="warning">Unassigned</Pill>);
    expect(container.querySelector('.gh-pill')).not.toHaveClass('appear');
    rerender(<Pill kind="ok">Matches</Pill>);
    expect(container.querySelector('.gh-pill')).toHaveClass('appear');
  });
});

describe('Sliding indicators', () => {
  it('segmented thumb measures the selected segment and follows the value', async () => {
    const restore = stubLayout();
    try {
      const opts = [
        { value: 'block', label: 'Block' },
        { value: 'warn', label: 'Warn' },
      ];
      const { rerender, container } = render(
        <Segmented
          label="When identity is wrong"
          value="block"
          onChange={() => undefined}
          options={opts}
        />,
      );
      const group = screen.getByRole('radiogroup');
      expect(group).toHaveAttribute('data-indicator', 'ready');
      const thumb = container.querySelector<HTMLElement>('.gh-indicator')!;
      expect(thumb.style.transform).toBe('translate(2px, 2px)');
      expect(thumb.style.width).toBe('60px');
      // First placement never animates; transitions switch on a frame later.
      expect(thumb).not.toHaveAttribute('data-animate');
      await waitFor(() => expect(thumb).toHaveAttribute('data-animate'));
      rerender(
        <Segmented
          label="When identity is wrong"
          value="warn"
          onChange={() => undefined}
          options={opts}
        />,
      );
      expect(thumb.style.transform).toBe('translate(62px, 2px)');
    } finally {
      restore();
    }
  });

  it('sidebar indicator glides to the selected screen', async () => {
    const restore = stubLayout();
    try {
      const { container } = render(<App />);
      const nav = await screen.findByRole('navigation', { name: 'Main' });
      const bar = container.querySelector<HTMLElement>('nav .gh-indicator')!;
      expect(nav).toHaveAttribute('data-indicator', 'ready');
      expect(bar.style.transform).toBe('translate(0px, 0px)');
      expect(bar.style.height).toBe('36px');
      await userEvent.click(screen.getByRole('button', { name: 'Fix commits' }));
      expect(bar.style.transform).toBe('translate(0px, 76px)');
      await userEvent.keyboard('{Control>}4{/Control}');
      expect(bar.style.transform).toBe('translate(0px, 114px)');
    } finally {
      restore();
    }
  });

  it('without layout (no size) the selected item keeps its own background', () => {
    render(
      <Segmented
        label="x"
        value="a"
        onChange={() => undefined}
        options={[
          { value: 'a', label: 'A' },
          { value: 'b', label: 'B' },
        ]}
      />,
    );
    expect(screen.getByRole('radiogroup')).not.toHaveAttribute('data-indicator');
    expect(document.querySelector('.gh-indicator')).toBeNull();
  });
});

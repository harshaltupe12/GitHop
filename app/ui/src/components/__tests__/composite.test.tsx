import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import * as ipc from '../../lib/ipc';
import { expectNoA11yViolations } from '../../test/a11y';
import { office, personal, profileSet } from '../../test/fixtures';
import { DefaultPanel } from '../DefaultPanel';
import { ConfirmDialog, Dialog } from '../Dialog';
import { ErrorText } from '../ErrorText';
import { FolderField } from '../FolderField';
import { Icons } from '../icons';
import { MenuButton } from '../Menu';
import { NavItem } from '../NavItem';
import { ProfileCard } from '../ProfileCard';
import { Table } from '../Table';
import { ToastProvider, useToast } from '../Toast';

vi.mock('../../lib/ipc', async () => (await import('../../test/ipcMock')).createIpcMock());

describe('NavItem', () => {
  it('selected uses aria-current, filled icon and aqua tint', async () => {
    const onSelect = vi.fn();
    const { container } = render(
      <nav aria-label="Main">
        <NavItem
          label="Profiles"
          icon={Icons.profiles}
          selectedIcon={Icons.profilesSelected}
          selected
          onSelect={onSelect}
        />
        <NavItem
          label="Repos"
          icon={Icons.repos}
          selectedIcon={Icons.reposSelected}
          selected={false}
          onSelect={onSelect}
        />
      </nav>,
    );
    const p = screen.getByRole('button', { name: 'Profiles' });
    expect(p).toHaveAttribute('aria-current', 'page');
    expect(p.className).toContain('bg-aqua-tint');
    expect(screen.getByRole('button', { name: 'Repos' })).not.toHaveAttribute('aria-current');
    await userEvent.click(screen.getByRole('button', { name: 'Repos' }));
    expect(onSelect).toHaveBeenCalled();
    await expectNoA11yViolations(container);
  });
});

describe('MenuButton', () => {
  it('opens, arrow keys move, Enter selects, Esc closes and refocuses', async () => {
    const a = vi.fn();
    const b = vi.fn();
    render(
      <MenuButton
        label="More actions for Office"
        iconOnly
        trigger="…"
        items={[
          { key: 'a', label: 'Make default', onSelect: a },
          { key: 'b', label: 'Delete', danger: true, onSelect: b },
        ]}
      />,
    );
    const trigger = screen.getByRole('button', { name: 'More actions for Office' });
    await userEvent.click(trigger);
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
    await waitFor(() =>
      expect(screen.getByRole('menuitem', { name: 'Make default' })).toHaveFocus(),
    );
    await expectNoA11yViolations();
    await userEvent.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Delete' })).toHaveFocus();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('menu')).toBeNull();
    expect(trigger).toHaveFocus();
    await userEvent.click(trigger);
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));
    expect(b).toHaveBeenCalled();
    expect(a).not.toHaveBeenCalled();
  });
});

describe('DefaultPanel', () => {
  it('shows the default and changes it from a radio menu', async () => {
    const onChange = vi.fn();
    const { container } = render(<DefaultPanel profiles={profileSet()} onChange={onChange} />);
    const trigger = screen.getByRole('button', {
      name: 'Default Personal, change default profile',
    });
    expect(trigger).toHaveTextContent('Default');
    expect(trigger).toHaveTextContent('Personal');
    // WCAG 2.5.3 label-in-name: the accessible name starts with the visible text.
    // Visible text is "Default" + "Personal"; the name starts with exactly that.
    expect(trigger.getAttribute('aria-label')!.startsWith('Default Personal')).toBe(true);
    await expectNoA11yViolations(container);
    await userEvent.click(trigger);
    expect(screen.getByRole('menuitemradio', { name: 'Personal' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Office' }));
    expect(onChange).toHaveBeenCalledWith('office');
  });

  it('no default set', () => {
    render(<DefaultPanel profiles={profileSet({ default: null })} onChange={() => undefined} />);
    expect(screen.getByRole('button', { name: /None set/ })).toBeInTheDocument();
  });
});

describe('ProfileCard', () => {
  const handlers = () => ({
    onEdit: vi.fn(),
    onMakeDefault: vi.fn(),
    onTest: vi.fn(),
    onDelete: vi.fn(),
  });

  it('default card: aqua border, Default pill, chips, Edit', async () => {
    const h = handlers();
    const { container } = render(<ProfileCard profile={personal} isDefault {...h} />);
    const card = screen.getByRole('article', { name: 'Personal' });
    expect(card.className).toContain('border-aqua');
    expect(screen.getByText('Default')).toBeInTheDocument();
    expect(screen.getByText(personal.email)).toHaveClass('font-mono');
    expect(screen.getByText('github.com/harshal-personal')).toBeInTheDocument();
    expect(screen.getByText('githop_personal')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Edit Personal' }));
    expect(h.onEdit).toHaveBeenCalled();
    await expectNoA11yViolations(container);
  });

  it('non-default card overflow menu: Make default, Test connection, Delete', async () => {
    const h = handlers();
    render(<ProfileCard profile={office} isDefault={false} {...h} />);
    expect(screen.queryByText('Default')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'More actions for Office' }));
    expect(screen.getAllByRole('menuitem').map((m) => m.textContent)).toEqual([
      'Make default',
      'Test connection',
      'Delete',
    ]);
    await userEvent.click(screen.getByRole('menuitem', { name: 'Make default' }));
    expect(h.onMakeDefault).toHaveBeenCalled();
  });

  it('testing state', () => {
    render(<ProfileCard profile={office} isDefault={false} testing {...handlers()} />);
    expect(screen.getByRole('status')).toHaveTextContent('Testing connection…');
  });
});

describe('Table', () => {
  it('semantic table with headers', async () => {
    const { container } = render(
      <Table
        label="Repos"
        rowKey={(r: { n: string }) => r.n}
        rows={[{ n: 'blog' }, { n: 'site' }]}
        columns={[
          { key: 'n', header: 'Repo', render: (r) => r.n },
          { key: 'a', header: 'Action', align: 'right', render: () => 'Pin' },
        ]}
      />,
    );
    expect(screen.getByRole('table', { name: 'Repos' })).toBeInTheDocument();
    expect(screen.getAllByRole('columnheader')).toHaveLength(2);
    expect(screen.getAllByRole('row')).toHaveLength(3);
    await expectNoA11yViolations(container);
  });
});

describe('FolderField', () => {
  it('read-only path + Choose… uses the native picker', async () => {
    vi.mocked(ipc.pickFolder).mockResolvedValueOnce('C:\\Personal\\code');
    const onChange = vi.fn();
    const { container } = render(
      <>
        <label htmlFor="f">Folder</label>
        <FolderField
          value=""
          onChange={onChange}
          control={{ id: 'f' }}
          pickTitle="Choose a folder"
          emptyText="No folders"
        />
      </>,
    );
    expect(screen.getByLabelText('Folder')).toHaveAttribute('readonly');
    await userEvent.click(screen.getByRole('button', { name: 'Choose…' }));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith('C:/Personal/code'));
    expect(ipc.pickFolder).toHaveBeenCalledWith('Choose a folder');
    await expectNoA11yViolations(container);
  });

  it('file mode picks a file', async () => {
    vi.mocked(ipc.pickFile).mockResolvedValueOnce('C:/Users/h/.ssh/id_ed25519');
    const onChange = vi.fn();
    render(
      <FolderField
        mode="file"
        value=""
        onChange={onChange}
        control={{ id: 'k' }}
        pickTitle="Choose key"
      />,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Choose…' }));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith('C:/Users/h/.ssh/id_ed25519'));
  });
});

describe('Dialog', () => {
  it('labelled, traps focus, Esc closes', async () => {
    const onClose = vi.fn();
    render(
      <>
        <button type="button">Opener</button>
        <Dialog
          open
          title="Doctor"
          onClose={onClose}
          footer={
            <>
              <button type="button">First</button>
              <button type="button">Last</button>
            </>
          }
        >
          Body
        </Dialog>
      </>,
    );
    const dlg = screen.getByRole('dialog', { name: 'Doctor' });
    expect(dlg).toHaveAttribute('aria-modal', 'true');
    await waitFor(() => expect(screen.getByRole('button', { name: 'First' })).toHaveFocus());
    await userEvent.tab();
    expect(screen.getByRole('button', { name: 'Last' })).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByRole('button', { name: 'First' })).toHaveFocus();
    await expectNoA11yViolations();
    await userEvent.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalled();
  });

  it('closed dialog renders nothing', () => {
    render(
      <Dialog open={false} title="X" onClose={() => undefined}>
        Body
      </Dialog>,
    );
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('ConfirmDialog (typed confirmation)', () => {
  it('Danger button enabled only after typing the word; focus starts on Cancel', async () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Wipe GitHop?"
        word="WIPE"
        typeLabel="Type WIPE to confirm"
        confirmLabel="Wipe GitHop"
        onConfirm={onConfirm}
        onClose={() => undefined}
      >
        This restores your original git config.
      </ConfirmDialog>,
    );
    await waitFor(() => expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus());
    const go = screen.getByRole('button', { name: 'Wipe GitHop' });
    expect(go).toBeDisabled();
    await userEvent.type(screen.getByRole('textbox', { name: 'Type WIPE to confirm' }), 'wipe');
    expect(go).toBeDisabled();
    await userEvent.clear(screen.getByRole('textbox', { name: 'Type WIPE to confirm' }));
    await userEvent.type(screen.getByRole('textbox', { name: 'Type WIPE to confirm' }), 'WIPE');
    expect(go).toBeEnabled();
    await expectNoA11yViolations();
    await userEvent.click(go);
    expect(onConfirm).toHaveBeenCalled();
  });
});

function ToastButton({ kind }: { kind: 'success' | 'error' }) {
  const t = useToast();
  return (
    <button
      type="button"
      onClick={() =>
        t({
          kind,
          message: kind === 'error' ? 'Couldn’t save.' : '2 commits fixed.',
          action: { label: 'Undo', onClick: () => undefined },
        })
      }
    >
      Go
    </button>
  );
}

describe('Toast', () => {
  it('success auto-dismisses after 4 s; live region announces it', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    render(
      <ToastProvider>
        <ToastButton kind="success" />
      </ToastProvider>,
    );
    await user.click(screen.getByRole('button', { name: 'Go' }));
    const region = screen.getByRole('region', { name: 'Notifications' });
    expect(region).toHaveAttribute('aria-live', 'polite');
    expect(screen.getByRole('status')).toHaveTextContent('2 commits fixed.');
    expect(screen.getByRole('button', { name: 'Undo' })).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(4100);
    });
    expect(screen.queryByText('2 commits fixed.')).toBeNull();
    vi.useRealTimers();
  });

  it('errors stay until dismissed', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const { container } = render(
      <ToastProvider>
        <ToastButton kind="error" />
      </ToastProvider>,
    );
    await user.click(screen.getByRole('button', { name: 'Go' }));
    act(() => {
      vi.advanceTimersByTime(10000);
    });
    expect(screen.getByRole('alert')).toHaveTextContent('Couldn’t save.');
    vi.useRealTimers();
    await expectNoA11yViolations(container);
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('never covers an open dialog: toasts stack below the panel inside the dialog layer', async () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <ToastButton kind="error" />
          <button type="button" onClick={() => setOpen(true)}>
            Open
          </button>
          <Dialog open={open} title="Delete Office?" onClose={() => setOpen(false)}>
            Body
          </Dialog>
        </>
      );
    }
    const user = userEvent.setup();
    render(
      <ToastProvider>
        <Harness />
      </ToastProvider>,
    );
    await user.click(screen.getByRole('button', { name: 'Go' }));
    // No dialog: bottom-right fixed region.
    expect(screen.getByRole('region', { name: 'Notifications' })).toHaveClass('fixed');
    await user.click(screen.getByRole('button', { name: 'Open' }));
    const dlg = await screen.findByRole('dialog', { name: 'Delete Office?' });
    const toast = screen.getByRole('alert');
    const slot = toast.closest('[data-toast-slot]');
    expect(slot).not.toBeNull();
    // In the dialog's own layer (same overlay), after the panel, never inside or over it.
    expect(slot!.parentElement).toBe(dlg.parentElement);
    expect(dlg.contains(toast)).toBe(false);
    expect(dlg.compareDocumentPosition(slot!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole('region', { name: 'Notifications' })).not.toHaveClass('fixed');
    // Closing the dialog puts the toast back in the corner.
    await user.keyboard('{Escape}');
    await waitFor(() =>
      expect(screen.getByRole('region', { name: 'Notifications' })).toHaveClass('fixed'),
    );
    expect(screen.getByRole('alert')).toHaveTextContent('Couldn’t save.');
  });
});

describe('Dialog width', () => {
  it('standard dialogs use max-w-dialog (420 px), wide ones 40 rem', () => {
    const { unmount } = render(
      <Dialog open title="Standard" onClose={() => undefined}>
        x
      </Dialog>,
    );
    expect(screen.getByRole('dialog')).toHaveClass('w-full', 'max-w-dialog');
    unmount();
    render(
      <Dialog open wide title="Wide" onClose={() => undefined}>
        x
      </Dialog>,
    );
    expect(screen.getByRole('dialog')).toHaveClass('max-w-[40rem]');
  });
});

describe('ErrorText', () => {
  it('technical codes: calm sentence + keyboard disclosure; others: message as is', async () => {
    const { container, rerender } = render(
      <ErrorText error={{ code: 'io', message: 'TOML parse error at line 3', issues: [] }} />,
    );
    expect(screen.getByText(/couldn’t read or write a file/)).toBeInTheDocument();
    const btn = screen.getByRole('button', { name: 'Show details' });
    const panel = document.getElementById(btn.getAttribute('aria-controls')!)!;
    expect(panel).not.toBeVisible();
    btn.focus();
    await userEvent.keyboard(' ');
    expect(btn).toHaveAttribute('aria-expanded', 'true');
    expect(panel).toBeVisible();
    expect(panel).toHaveTextContent('TOML parse error at line 3');
    await expectNoA11yViolations(container);
    rerender(
      <ErrorText error={{ code: 'locked', message: 'Another GitHop is saving.', issues: [] }} />,
    );
    expect(screen.getByText('Another GitHop is saving.')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Show details' })).toBeNull();
  });
});

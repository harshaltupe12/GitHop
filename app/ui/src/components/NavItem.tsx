import { forwardRef, type KeyboardEventHandler } from 'react';
import { cx } from '../lib/cx';
import { Ico, type Icon } from './icons';

/** Design §7.2 — 36 px, radius-md, 20 px icon (Filled when selected). */
export const NavItem = forwardRef<
  HTMLButtonElement,
  {
    label: string;
    icon: Icon;
    selectedIcon: Icon;
    selected: boolean;
    onSelect: () => void;
    onKeyDown?: KeyboardEventHandler<HTMLButtonElement>;
    shortcut?: string;
    tabIndex?: number;
  }
>(function NavItem(
  { label, icon, selectedIcon, selected, onSelect, onKeyDown, shortcut, tabIndex },
  ref,
) {
  return (
    <button
      ref={ref}
      type="button"
      onClick={onSelect}
      onKeyDown={onKeyDown}
      tabIndex={tabIndex}
      aria-current={selected ? 'page' : undefined}
      aria-keyshortcuts={shortcut}
      className={cx(
        'gh-nav transition-ui flex min-h-9 w-full items-center gap-2 rounded-md px-3 py-2 text-left text-body',
        selected
          ? 'bg-aqua-tint font-medium text-aqua-ink'
          : 'text-ink-nav hover:bg-surface-hover active:bg-surface-pressed',
      )}
    >
      <Ico icon={selected ? selectedIcon : icon} size={20} />
      <span className="min-w-0 truncate">{label}</span>
    </button>
  );
});

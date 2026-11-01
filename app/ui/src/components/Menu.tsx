import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { cx } from '../lib/cx';
import { usePresence } from '../lib/motion';
import { Ico, Icons, type Icon } from './icons';
import { Tooltip } from './Tooltip';

export type MenuItem = {
  key: string;
  label: ReactNode;
  /** Accessible text when `label` is not plain text. */
  text?: string;
  icon?: Icon;
  leading?: ReactNode;
  onSelect: () => void;
  checked?: boolean;
  danger?: boolean;
  disabled?: boolean;
};

/**
 * Popover menu (used by the profile card overflow, the Default panel and Pin).
 * Arrow keys / Home / End move, Enter / Space choose, Esc closes and returns focus.
 */
export function MenuButton({
  items,
  label,
  trigger,
  triggerClassName,
  align = 'right',
  placement = 'bottom',
  radio = false,
  heading,
  iconOnly = false,
  compact = false,
}: {
  items: MenuItem[];
  /** Accessible name of the trigger. */
  label: string;
  trigger: ReactNode;
  triggerClassName?: string;
  align?: 'left' | 'right';
  placement?: 'bottom' | 'top';
  /** Items are a single choice (menuitemradio). */
  radio?: boolean;
  heading?: string;
  iconOnly?: boolean;
  compact?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const presence = usePresence(open);

  const itemEls = () =>
    Array.from(
      menuRef.current?.querySelectorAll<HTMLElement>(
        '[role^="menuitem"]:not([aria-disabled="true"])',
      ) ?? [],
    );

  const close = useCallback((refocus = true) => {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  }, []);

  useEffect(() => {
    if (!open) return;
    const els = itemEls();
    const checked = els.find((el) => el.getAttribute('aria-checked') === 'true');
    (checked ?? els[0])?.focus();
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) close(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open, close]);

  const onMenuKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const els = itemEls();
    const i = els.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      els[(i + 1) % els.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      els[(i - 1 + els.length) % els.length]?.focus();
    } else if (e.key === 'Home') {
      e.preventDefault();
      els[0]?.focus();
    } else if (e.key === 'End') {
      e.preventDefault();
      els[els.length - 1]?.focus();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === 'Tab') {
      close(false);
    }
  };

  const onTriggerKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      setOpen(true);
    }
  };

  const button = (
    <button
      ref={triggerRef}
      type="button"
      aria-haspopup="menu"
      aria-expanded={open}
      aria-controls={open ? menuId : undefined}
      aria-label={label}
      onClick={() => setOpen((o) => !o)}
      onKeyDown={onTriggerKey}
      className={cx(
        'press',
        iconOnly &&
          cx(
            'inline-flex items-center justify-center rounded-md text-ink-muted hover:bg-surface-hover active:bg-surface-pressed',
            compact ? 'h-7 w-7' : 'h-8 w-8',
          ),
        triggerClassName,
      )}
    >
      {trigger}
    </button>
  );

  return (
    <div ref={wrapRef} className="relative inline-flex">
      {iconOnly ? <Tooltip content={label}>{button}</Tooltip> : button}
      {presence.mounted ? (
        <div
          id={menuId}
          ref={menuRef}
          role="menu"
          aria-label={heading ?? label}
          aria-hidden={presence.exiting || undefined}
          tabIndex={-1}
          onKeyDown={onMenuKey}
          data-state={presence.state}
          onAnimationEnd={presence.onAnimationEnd}
          className={cx(
            'gh-menu gh-pop absolute z-40 min-w-[13rem] rounded-lg border border-line bg-surface p-1 shadow-float',
            align === 'right' ? 'right-0' : 'left-0',
            placement === 'bottom' ? 'top-full mt-1' : 'bottom-full mb-1',
            // Grow out of the corner that touches the trigger.
            placement === 'bottom'
              ? align === 'right'
                ? 'origin-top-right'
                : 'origin-top-left'
              : align === 'right'
                ? 'origin-bottom-right'
                : 'origin-bottom-left',
          )}
        >
          {heading ? (
            <div className="px-2 pb-1 pt-1.5 text-small text-ink-muted" aria-hidden="true">
              {heading}
            </div>
          ) : null}
          {items.map((it) => (
            <button
              type="button"
              key={it.key}
              role={radio ? 'menuitemradio' : 'menuitem'}
              aria-checked={radio ? !!it.checked : undefined}
              aria-disabled={it.disabled || undefined}
              aria-label={it.text}
              tabIndex={-1}
              onClick={() => {
                if (it.disabled) return;
                close();
                it.onSelect();
              }}
              className={cx(
                'transition-ui flex min-h-8 w-full cursor-default items-center gap-2 rounded-md px-2 text-left text-body outline-offset-0',
                it.disabled
                  ? 'text-ink-disabled'
                  : it.danger
                    ? 'text-blocked-text hover:bg-surface-hover'
                    : 'text-ink hover:bg-surface-hover',
                it.checked && 'bg-aqua-tint text-aqua-ink hover:bg-aqua-tint',
              )}
            >
              {it.leading ?? (it.icon ? <Ico icon={it.icon} /> : null)}
              <span className="min-w-0 flex-1 truncate">{it.label}</span>
              {radio && it.checked ? <Ico icon={Icons.ok} /> : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

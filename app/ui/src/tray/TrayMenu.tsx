import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { Avatar } from '../components/Avatar';
import { BrandTile } from '../components/Brand';
import { Ico, Icons } from '../components/icons';
import { cx } from '../lib/cx';
import { sortProfiles } from '../lib/format';
import * as ipc from '../lib/ipc';
import { invalidate, setQueryData, useQuery } from '../lib/query';
import type { ProfileSet } from '../lib/types';
import { S } from '../strings';

const KEY = 'tray-profiles';

/**
 * Design §7.14 — compact tray menu window (rendered when location.hash is
 * "#tray"). Click a profile = set default, then the window hides. Closes on
 * focus loss or Esc.
 */
export function TrayMenu() {
  const { data, error } = useQuery<ProfileSet>(KEY, ipc.listProfiles);
  const listRef = useRef<HTMLDivElement>(null);
  // The window is hidden, not closed: go transparent on blur so the next show
  // starts from nothing and rises in again (MOTION.md), instead of popping.
  const [phase, setPhase] = useState<'open' | 'hidden'>('open');

  useEffect(() => {
    const subs: Array<Promise<() => void>> = [];
    subs.push(ipc.onProfilesChanged(() => invalidate(KEY)).catch(() => () => undefined));
    subs.push(
      ipc.onTrayDefaultChanged((set) => setQueryData(KEY, set)).catch(() => () => undefined),
    );
    subs.push(
      ipc
        .onCurrentWindowBlur(() => {
          setPhase('hidden');
          void ipc.hideCurrentWindow();
        })
        .catch(() => () => undefined),
    );
    subs.push(ipc.onCurrentWindowFocus(() => setPhase('open')).catch(() => () => undefined));
    const onFocus = () => invalidate(KEY);
    window.addEventListener('focus', onFocus);
    return () => {
      window.removeEventListener('focus', onFocus);
      subs.forEach((p) => void p.then((u) => u()));
    };
  }, []);

  useEffect(() => {
    const el =
      listRef.current?.querySelector<HTMLElement>('[aria-checked="true"]') ??
      listRef.current?.querySelector<HTMLElement>('[role^="menuitem"]');
    el?.focus();
  }, [data]);

  const items = () =>
    Array.from(listRef.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]') ?? []);

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const els = items();
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
      setPhase('hidden');
      void ipc.hideCurrentWindow();
    }
  };

  const choose = async (id: string) => {
    try {
      const set = await ipc.setDefault(id);
      setQueryData(KEY, set);
    } finally {
      setPhase('hidden');
      void ipc.hideCurrentWindow();
    }
  };

  const list = data ? sortProfiles(data) : [];
  const itemCls =
    'transition-ui flex min-h-8 w-full cursor-default items-center gap-2 rounded-md px-2 text-left text-body text-ink outline-offset-0 hover:bg-surface-hover active:bg-surface-pressed';

  return (
    <div
      ref={listRef}
      role="menu"
      aria-label={S.tray.heading}
      tabIndex={-1}
      onKeyDown={onKey}
      data-state={phase}
      className="gh-menu gh-tray flex w-full flex-col rounded-lg border border-line bg-surface p-1 shadow-float"
    >
      <div
        className="flex items-center gap-2 px-2 pb-1 pt-1.5 text-small text-ink-muted"
        aria-hidden="true"
      >
        {/* Avatar-wide slot: the tile centres over the avatars, the text lines up with labels. */}
        <span className="flex w-[1.5714rem] shrink-0 justify-center">
          <BrandTile size={16} />
        </span>
        {S.tray.heading}
      </div>
      {error ? (
        <p className="px-2 py-1 text-small text-blocked-text">{error.message}</p>
      ) : list.length === 0 && data ? (
        <p className="px-2 py-1 text-small text-ink-muted">{S.tray.none}</p>
      ) : null}
      {list.map((p) => {
        const checked = p.id === data?.default;
        const domain = p.email.split('@')[1] ?? '';
        return (
          <button
            key={p.id}
            type="button"
            role="menuitemradio"
            aria-checked={checked}
            tabIndex={-1}
            onClick={() => void choose(p.id)}
            // Long labels truncate; the full label and email show on hover.
            title={p.email ? p.label + ' · ' + p.email : p.label}
            className={cx(itemCls, checked && 'bg-aqua-tint text-aqua-ink hover:bg-aqua-tint')}
          >
            <Avatar label={p.label} color={p.color} size={22} decorative />
            <span className="min-w-[5rem] flex-1 truncate">{p.label}</span>
            {checked ? (
              <Ico icon={Icons.ok} />
            ) : domain && !domain.endsWith('noreply.github.com') ? (
              <span className="min-w-0 max-w-[45%] shrink truncate font-mono text-mono-small text-ink-muted">
                {domain}
              </span>
            ) : null}
          </button>
        );
      })}
      <div role="separator" className="my-1 border-t border-line" />
      <button
        type="button"
        role="menuitem"
        tabIndex={-1}
        className={itemCls}
        onClick={() => void ipc.showMainWindow().catch(() => undefined)}
      >
        <Ico icon={Icons.window} />
        {S.tray.open}
      </button>
      <button
        type="button"
        role="menuitem"
        tabIndex={-1}
        className={itemCls}
        onClick={() => void ipc.quitApp().catch(() => undefined)}
      >
        <Ico icon={Icons.quit} />
        {S.tray.quit}
      </button>
    </div>
  );
}

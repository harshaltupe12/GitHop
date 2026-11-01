import type { ReactNode } from 'react';
import { cx } from '../lib/cx';
import { Ico, Icons, type Icon } from './icons';

export type BannerKind = 'success' | 'blocked' | 'warning' | 'info' | 'privacy';

const KIND: Record<BannerKind, { cls: string; icon: Icon; iconCls: string }> = {
  success: { cls: 'bg-aqua-tint border-aqua-border text-aqua-ink', icon: Icons.ok, iconCls: '' },
  blocked: {
    cls: 'bg-blocked-tint border-blocked-border text-blocked-ink',
    icon: Icons.blocked,
    iconCls: '',
  },
  warning: {
    cls: 'bg-warning-tint border-warning-border text-warning-ink',
    icon: Icons.warning,
    iconCls: '',
  },
  info: { cls: 'bg-bg border-line text-ink', icon: Icons.info, iconCls: 'text-ink-muted' },
  privacy: {
    cls: 'bg-aqua-tint border-aqua-border text-aqua-ink',
    icon: Icons.offline,
    iconCls: '',
  },
};

/** Design §7.8 — inline message with icon + text and an optional action link. */
export function Banner({
  kind,
  children,
  icon,
  action,
  live = true,
  className,
}: {
  kind: BannerKind;
  children: ReactNode;
  icon?: Icon;
  action?: { label: string; onClick: () => void; busy?: boolean };
  /** Announce to screen readers (alert for blocked, status otherwise). */
  live?: boolean;
  className?: string;
}) {
  const k = KIND[kind];
  const role = !live ? undefined : kind === 'blocked' ? 'alert' : 'status';
  return (
    <div
      role={role}
      className={cx(
        'gh-banner appear flex w-full flex-wrap items-start gap-x-3 gap-y-1 rounded-md border px-3 py-2.5 text-body',
        k.cls,
        className,
      )}
    >
      <div className="flex min-w-0 flex-1 items-start gap-2">
        <Ico icon={icon ?? k.icon} className={cx('mt-0.5', k.iconCls)} />
        <div className="min-w-0 flex-1">{children}</div>
      </div>
      {action ? (
        <button
          type="button"
          onClick={action.onClick}
          disabled={action.busy}
          className="ml-auto shrink-0 rounded-sm font-medium text-aqua-text underline-offset-2 hover:underline disabled:text-ink-disabled"
        >
          {action.label}
        </button>
      ) : null}
    </div>
  );
}

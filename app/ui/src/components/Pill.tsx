import type { ReactNode } from 'react';
import { cx } from '../lib/cx';
import { useChangeCount } from '../lib/motion';
import { Ico, Icons, type Icon } from './icons';

export type PillKind = 'ok' | 'blocked' | 'warning' | 'neutral';

const KIND: Record<PillKind, { cls: string; icon: Icon }> = {
  ok: { cls: 'bg-aqua-tint text-aqua-ink', icon: Icons.ok },
  blocked: { cls: 'bg-blocked-tint text-blocked-ink', icon: Icons.blocked },
  warning: { cls: 'bg-warning-tint text-warning-ink', icon: Icons.warning },
  neutral: { cls: 'bg-bg text-ink-muted', icon: Icons.pin },
};

/** Design §7.7 — status badge. Always icon + text (never colour alone). */
export function Pill({
  kind,
  children,
  icon,
  className,
}: {
  kind: PillKind;
  children: ReactNode;
  icon?: Icon;
  className?: string;
}) {
  const k = KIND[kind];
  // A status change (e.g. Unassigned → Pinned) cross-fades in; first render doesn't.
  const changes = useChangeCount(kind);
  return (
    <span
      key={changes}
      className={cx(
        'gh-pill inline-flex min-h-5 items-center gap-1 whitespace-nowrap rounded-full px-2 text-small',
        changes > 0 && 'appear',
        k.cls,
        className,
      )}
    >
      <Ico icon={icon ?? k.icon} />
      {children}
    </span>
  );
}

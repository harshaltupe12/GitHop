import type { ReactNode } from 'react';
import { cx } from '../lib/cx';
import { S } from '../strings';
import { Ico, Icons, type Icon } from './icons';

/** Design §7.6 — 22 px, radius-full, bg `bg`, 1 px border, mono-small (or small). */
export function Chip({
  children,
  icon,
  mono = true,
  title,
  onRemove,
  removeLabel,
  className,
}: {
  children: ReactNode;
  icon?: Icon;
  mono?: boolean;
  title?: string;
  onRemove?: () => void;
  removeLabel?: string;
  className?: string;
}) {
  return (
    <span
      title={title}
      className={cx(
        'gh-chip inline-flex min-h-[1.5714rem] max-w-full items-center gap-1 rounded-full border border-line bg-bg px-2 text-ink',
        mono ? 'font-mono text-mono-small' : 'text-small',
        className,
      )}
    >
      {icon ? <Ico icon={icon} size={12} className="text-ink-muted" /> : null}
      <span className="truncate">{children}</span>
      {onRemove ? (
        <button
          type="button"
          onClick={onRemove}
          aria-label={removeLabel ?? S.common.remove(String(children))}
          className="transition-ui -mr-1 inline-flex h-4 w-4 items-center justify-center rounded-full text-ink-muted hover:bg-surface-hover"
        >
          <Ico icon={Icons.dismissSmall} size={12} />
        </button>
      ) : null}
    </span>
  );
}

import type { ReactNode } from 'react';
import { cx } from '../lib/cx';
import { Ico, type Icon } from './icons';

/** Screen title row: `title` text + actions on the right (Design §4.5). */
export function ScreenHeader({
  title,
  actions,
  aside,
  id,
}: {
  title: string;
  actions?: ReactNode;
  aside?: ReactNode;
  id?: string;
}) {
  return (
    <header className="mb-4 flex flex-wrap items-center gap-x-4 gap-y-2">
      <h1 id={id} className="text-title text-ink">
        {title}
      </h1>
      {aside ? (
        <div className="min-w-0 truncate font-mono text-mono text-ink-muted">{aside}</div>
      ) : null}
      {actions ? <div className="ml-auto flex flex-wrap items-center gap-2">{actions}</div> : null}
    </header>
  );
}

/** Design §8.7 empty states. */
export function EmptyState({
  icon,
  iconClassName,
  title,
  body,
  action,
}: {
  icon: Icon;
  iconClassName?: string;
  title?: string;
  body: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center gap-3 rounded-lg border border-line bg-surface px-6 py-10 text-center">
      <Ico icon={icon} size={24} className={cx('text-ink-muted', iconClassName)} />
      {title ? <h2 className="text-heading text-ink">{title}</h2> : null}
      <p className="max-w-[28rem] text-body text-ink-muted">{body}</p>
      {action ? <div className="mt-1">{action}</div> : null}
    </div>
  );
}

/** A settings row card: label + helper on the left, control on the right (Design §8.5). */
export function SettingRow({
  title,
  titleId,
  helper,
  helperId,
  children,
}: {
  title: string;
  titleId: string;
  helper?: ReactNode;
  helperId?: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-labelledby={titleId}
      className="flex flex-wrap items-center gap-x-6 gap-y-3 rounded-lg border border-line bg-surface px-4 py-3"
    >
      <div className="min-w-[12rem] flex-1">
        <h2 id={titleId} className="text-body font-medium text-ink">
          {title}
        </h2>
        {helper ? (
          <p id={helperId} className="text-small text-ink-muted">
            {helper}
          </p>
        ) : null}
      </div>
      <div className="flex flex-wrap items-center gap-2">{children}</div>
    </section>
  );
}

export function Section({
  title,
  children,
  id,
}: {
  title: string;
  children: ReactNode;
  id: string;
}) {
  return (
    <section aria-labelledby={id} className="flex flex-col gap-4">
      <h2 id={id} className="text-heading text-ink">
        {title}
      </h2>
      {children}
    </section>
  );
}

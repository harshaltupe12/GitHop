import { useId, useState, type ReactNode } from 'react';
import { cx } from '../lib/cx';
import type { IpcError } from '../lib/types';
import { S } from '../strings';
import { Ico, Icons } from './icons';

/**
 * Codes whose message can carry raw parser, file-system or git output. They get
 * a plain Design §9 sentence, with the technical text behind "Show details".
 * Other codes (validation, locked, conflict, bad_input…) already carry a
 * user-facing sentence from the backend and are shown as they are.
 */
const TECHNICAL: Partial<Record<IpcError['code'], string>> = {
  io: S.errors.io,
  git: S.errors.git,
  internal: S.errors.internal,
};

/** Small keyboard-accessible disclosure for technical error text. */
export function Details({ children, className }: { children: ReactNode; className?: string }) {
  const id = useId();
  const [open, setOpen] = useState(false);
  return (
    <div className={cx('mt-1', className)}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((o) => !o)}
        className="inline-flex items-center gap-1 rounded-sm text-small font-medium underline-offset-2 hover:underline"
      >
        <Ico
          icon={Icons.chevronDown}
          size={12}
          className={cx('transition-ui', open ? 'rotate-180' : '')}
        />
        {open ? S.errors.hideDetails : S.errors.showDetails}
      </button>
      <div
        id={id}
        hidden={!open}
        className="mt-1 whitespace-pre-wrap break-words font-mono text-mono-small"
      >
        {children}
      </div>
    </div>
  );
}

/**
 * Body of an error banner: a calm summary (what happened → next step) and, for
 * technical errors, the raw message behind "Show details".
 */
export function ErrorText({ error, title }: { error: IpcError; title?: string }) {
  const summary = TECHNICAL[error.code];
  if (!summary)
    return (
      <>
        {title ? <span className="block font-medium">{title}</span> : null}
        <span className="block">{error.message}</span>
      </>
    );
  return (
    <>
      <span className="block font-medium">{title ?? summary}</span>
      {title ? <span className="block">{summary}</span> : null}
      {error.message ? <Details>{error.message}</Details> : null}
    </>
  );
}

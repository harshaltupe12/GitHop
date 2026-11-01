import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from 'react';
import { createPortal } from 'react-dom';
import { cx } from '../lib/cx';
import { S } from '../strings';
import { Button, type ButtonVariant } from './Button';
import { Field, TextInput } from './Field';

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * Design §7.13 — centred, 420 wide, radius-lg, shadow, padding 24. Esc closes,
 * focus is trapped, focus returns to the opener on close.
 */
export function Dialog({
  open,
  title,
  children,
  footer,
  onClose,
  initialFocus,
  wide = false,
}: {
  open: boolean;
  title: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  initialFocus?: RefObject<HTMLElement | null>;
  wide?: boolean;
}) {
  const titleId = useId();
  const bodyId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const opener = useRef<Element | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    opener.current = document.activeElement;
    const t = window.setTimeout(() => {
      const el =
        initialFocus?.current ??
        panel.current?.querySelector<HTMLElement>(FOCUSABLE) ??
        panel.current;
      el?.focus();
    }, 0);
    return () => {
      window.clearTimeout(t);
      const o = opener.current as HTMLElement | null;
      if (o && typeof o.focus === 'function' && document.contains(o)) o.focus();
    };
  }, [open, initialFocus]);

  if (!open) return null;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onCloseRef.current();
      return;
    }
    if (e.key !== 'Tab' || !panel.current) return;
    const els = Array.from(panel.current.querySelectorAll<HTMLElement>(FOCUSABLE));
    if (!els.length) return;
    const first = els[0]!;
    const last = els[els.length - 1]!;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-scrim p-4">
      {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- dialog owns Esc and the focus trap */}
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
        className={cx(
          'gh-dialog menu-in flex max-h-full w-full flex-col rounded-lg bg-surface p-6 shadow-float',
          wide ? 'max-w-[40rem]' : 'max-w-dialog',
        )}
      >
        <h2 id={titleId} className="text-heading text-ink">
          {title}
        </h2>
        <div id={bodyId} className="mt-3 min-h-0 overflow-y-auto text-body text-ink">
          {children}
        </div>
        {footer ? <div className="mt-6 flex flex-wrap justify-end gap-2">{footer}</div> : null}
      </div>
    </div>,
    document.body,
  );
}

/**
 * Destructive confirmation (Wipe, Delete profile): typing `word` enables the
 * Danger button. Initial focus is on Cancel (Design §7.13).
 */
export function ConfirmDialog({
  open,
  title,
  children,
  word,
  typeLabel,
  confirmLabel,
  onConfirm,
  onClose,
  busy = false,
  variant = 'danger',
  extra,
  confirmDisabled = false,
}: {
  open: boolean;
  title: ReactNode;
  children: ReactNode;
  /** Text to type; omit for a simple confirmation. */
  word?: string;
  typeLabel?: string;
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
  busy?: boolean;
  variant?: ButtonVariant;
  extra?: ReactNode;
  confirmDisabled?: boolean;
}) {
  const cancel = useRef<HTMLButtonElement>(null);
  const [typed, setTyped] = useState('');
  useEffect(() => {
    if (!open) setTyped('');
  }, [open]);
  const matches = !word || typed.trim() === word;
  return (
    <Dialog
      open={open}
      title={title}
      onClose={busy ? () => undefined : onClose}
      initialFocus={cancel}
      footer={
        <>
          <Button ref={cancel} onClick={onClose} disabled={busy}>
            {S.common.cancel}
          </Button>
          <Button
            variant={variant}
            onClick={onConfirm}
            disabled={!matches || confirmDisabled}
            loading={busy}
          >
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div>{children}</div>
        {extra}
        {word ? (
          <Field label={typeLabel ?? word}>
            {(p) => (
              <TextInput
                {...p}
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
                autoCapitalize="off"
              />
            )}
          </Field>
        ) : null}
      </div>
    </Dialog>
  );
}

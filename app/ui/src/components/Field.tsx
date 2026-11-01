import { forwardRef, useId, useState, type InputHTMLAttributes, type ReactNode } from 'react';
import { cx } from '../lib/cx';
import type { ValidationIssue } from '../lib/types';
import { S } from '../strings';
import { Ico, Icons } from './icons';

export type ControlProps = {
  id: string;
  'aria-describedby'?: string;
  'aria-invalid'?: boolean;
  'aria-required'?: boolean;
};

/**
 * Visible "required" mark after a label: an asterisk explained by the form's
 * "* Required" legend. Hidden from screen readers, which hear `aria-required`.
 */
export function RequiredMark() {
  return (
    <span aria-hidden="true" className="ml-0.5 font-normal text-ink-muted">
      *
    </span>
  );
}

/**
 * Design §7.10 — label above (`body-strong`), helper below (`small`, muted),
 * errors below (`small`, blocked-text, Warning icon). Warnings and info from
 * the core validator are shown the same way in their own tone.
 */
export function Field({
  label,
  helper,
  issues = [],
  children,
  className,
  labelAs = 'label',
  required = false,
}: {
  label: ReactNode;
  helper?: ReactNode;
  issues?: Pick<ValidationIssue, 'severity' | 'message'>[];
  children: (p: ControlProps) => ReactNode;
  className?: string;
  /** Use 'legend'-like text (no <label for>) when the control labels itself. */
  labelAs?: 'label' | 'div';
  /** Required field: `*` after the label and `aria-required` on the control. */
  required?: boolean;
}) {
  const id = useId();
  const helpId = `${id}-help`;
  const issueId = `${id}-issues`;
  const hasError = issues.some((i) => i.severity === 'error');
  const describedBy = [helper ? helpId : null, issues.length ? issueId : null]
    .filter(Boolean)
    .join(' ');
  const Label = labelAs;
  return (
    <div className={cx('flex flex-col gap-1', className)}>
      <Label
        {...(labelAs === 'label' ? { htmlFor: id } : { id: `${id}-label` })}
        className="text-body font-medium text-ink"
      >
        {label}
        {required ? <RequiredMark /> : null}
      </Label>
      {children({
        id,
        'aria-describedby': describedBy || undefined,
        'aria-invalid': hasError || undefined,
        'aria-required': required || undefined,
      })}
      {helper ? (
        <div id={helpId} className="text-small text-ink-muted">
          {helper}
        </div>
      ) : null}
      {issues.length ? (
        <ul id={issueId} className="flex flex-col gap-0.5">
          {issues.map((i, n) => (
            <li
              key={n}
              className={cx(
                'flex items-start gap-1 text-small',
                i.severity === 'error'
                  ? 'text-blocked-text'
                  : i.severity === 'warning'
                    ? 'text-warning-ink'
                    : 'text-ink-muted',
              )}
            >
              <Ico icon={i.severity === 'info' ? Icons.info : Icons.warning} className="mt-px" />
              <span>
                <span className="sr-only">
                  {i.severity === 'error' ? `${S.common.errorPrefix}: ` : ''}
                </span>
                {i.message}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

const INPUT =
  'gh-input transition-ui min-h-8 w-full rounded-md border bg-surface px-2.5 text-body text-ink placeholder:text-ink-muted disabled:bg-bg disabled:text-ink-disabled read-only:bg-bg';

export const TextInput = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement> & { mono?: boolean }
>(function TextInput({ mono, className, ...rest }, ref) {
  const invalid = rest['aria-invalid'] === true || rest['aria-invalid'] === 'true';
  return (
    <input
      ref={ref}
      type="text"
      spellCheck={false}
      autoComplete="off"
      className={cx(
        INPUT,
        invalid ? 'border-blocked-text' : 'border-line-input',
        mono && 'font-mono text-mono',
        className,
      )}
      {...rest}
    />
  );
});

/** Password input with show/hide (never sent anywhere but the local backend). */
export const PasswordInput = forwardRef<
  HTMLInputElement,
  Omit<InputHTMLAttributes<HTMLInputElement>, 'type'>
>(function PasswordInput({ className, ...rest }, ref) {
  const [shown, setShown] = useState(false);
  const invalid = rest['aria-invalid'] === true || rest['aria-invalid'] === 'true';
  return (
    <div
      className={cx(
        'gh-input flex min-h-8 items-center rounded-md border bg-surface pr-0.5',
        invalid ? 'border-blocked-text' : 'border-line-input',
        className,
      )}
    >
      <input
        ref={ref}
        type={shown ? 'text' : 'password'}
        autoComplete="new-password"
        spellCheck={false}
        className="min-w-0 flex-1 bg-transparent px-2.5 text-body text-ink"
        {...rest}
      />
      <button
        type="button"
        onClick={() => setShown((s) => !s)}
        aria-pressed={shown}
        aria-label={shown ? S.password.hide : S.password.show}
        className="transition-ui inline-flex h-7 w-7 items-center justify-center rounded-md text-ink-muted hover:bg-surface-hover"
      >
        <Ico icon={shown ? Icons.hide : Icons.show} />
      </button>
    </div>
  );
});

export function Checkbox({
  label,
  helper,
  checked,
  onChange,
  disabled,
}: {
  label: ReactNode;
  helper?: ReactNode;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div className="flex items-start gap-2">
      <input
        id={id}
        type="checkbox"
        className="mt-0.5 rounded-sm"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        aria-describedby={helper ? `${id}-help` : undefined}
      />
      <div className="flex flex-col">
        <label htmlFor={id} className="text-body text-ink">
          {label}
        </label>
        {helper ? (
          <span id={`${id}-help`} className="text-small text-ink-muted">
            {helper}
          </span>
        ) : null}
      </div>
    </div>
  );
}

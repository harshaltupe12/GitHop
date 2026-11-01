import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react';
import { cx } from '../lib/cx';
import { Ico, type Icon } from './icons';
import { Spinner } from './Spinner';
import { Tooltip } from './Tooltip';

export type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'ghost';

const VARIANT: Record<ButtonVariant, string> = {
  primary:
    'bg-aqua text-aqua-ink font-medium enabled:hover:bg-aqua-hover enabled:active:bg-aqua-strong enabled:active:text-white',
  secondary:
    'border border-line bg-surface text-ink enabled:hover:bg-surface-hover enabled:active:bg-surface-pressed',
  danger:
    'bg-blocked-tint text-blocked-ink enabled:hover:bg-blocked-hover enabled:active:bg-blocked-pressed',
  ghost:
    'bg-transparent text-ink-muted enabled:hover:bg-surface-hover enabled:active:bg-surface-pressed',
};

type Props = Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children'> & {
  variant?: ButtonVariant;
  /** 32 px default, 28 px inside tables (Design §7.1). */
  compact?: boolean;
  icon?: Icon;
  loading?: boolean;
  children?: ReactNode;
  /** Icon-only button: required accessible name, also shown as a tooltip. */
  label?: string;
};

/** Design §7.1. One primary per screen. */
export const Button = forwardRef<HTMLButtonElement, Props>(function Button(
  {
    variant = 'secondary',
    compact = false,
    icon,
    loading = false,
    disabled,
    children,
    label,
    className,
    type = 'button',
    ...rest
  },
  ref,
) {
  const iconOnly = !children && !!label;
  const btn = (
    <button
      ref={ref}
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      aria-label={iconOnly ? label : undefined}
      className={cx(
        'press inline-flex select-none items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-body',
        compact ? 'min-h-7' : 'min-h-8',
        iconOnly ? (compact ? 'w-7' : 'w-8') : 'px-3',
        VARIANT[variant],
        'disabled:cursor-not-allowed disabled:bg-bg disabled:text-ink-disabled',
        className,
      )}
      {...rest}
    >
      {/* The spinner takes the icon's 16 px slot (no width change) and fades in. */}
      {loading ? <Spinner /> : icon ? <Ico icon={icon} /> : null}
      {children}
    </button>
  );
  return iconOnly ? <Tooltip content={label}>{btn}</Tooltip> : btn;
});
